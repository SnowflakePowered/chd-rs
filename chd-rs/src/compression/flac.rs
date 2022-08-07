use std::io::Cursor;
use std::marker::PhantomData;
use std::mem;

use byteorder::{BigEndian, ByteOrder, LittleEndian, WriteBytesExt};
use claxon::frame::FrameReader;

use crate::cdrom::{CD_FRAME_SIZE, CD_MAX_SECTOR_DATA, CD_MAX_SUBCODE_DATA};
use crate::compression::zlib::ZlibCodec;
use crate::compression::{
    CodecImplementation, CompressionCodec, CompressionCodecType, DecompressResult,
};
use crate::error::{ChdError, Result};
use crate::header::CodecType;

/// Generic block decoder for FLAC.
///
/// Defaults assume 2 channel interleaved FLAC.
/// The byte order determines the endianness of the output data.
struct FlacCodec<T: ByteOrder, const CHANNELS: usize = 2> {
    buffer: Vec<i32>,
    _byteorder: PhantomData<T>,
}

impl<T: ByteOrder, const CHANNELS: usize> CodecImplementation for FlacCodec<T, CHANNELS> {
    fn new(hunk_bytes: u32) -> Result<Self>
    where
        Self: Sized,
    {
        if hunk_bytes % (CHANNELS * mem::size_of::<i16>()) as u32 != 0 {
            return Err(ChdError::CodecError);
        }

        Ok(FlacCodec {
            buffer: Vec::new(),
            _byteorder: PhantomData::default(),
        })
    }

    fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<DecompressResult> {
        let comp_buf = Cursor::new(input);

        // Number of samples to write to the buffer.
        let sample_len = output.len() / (CHANNELS * mem::size_of::<i16>());

        // We don't need to create a fake header since claxon will read raw FLAC frames just fine.
        // We just need to be careful not to read past the number of blocks in the input buffer.
        let mut frame_read = FrameReader::new(comp_buf);

        let mut cursor = Cursor::new(output);

        // Buffer to hold decompressed FLAC block data.
        let mut block_buf = mem::take(&mut self.buffer);

        // A little bit of a misnomer. 1 'sample' refers to a sample for all channels.
        let mut samples_written = 0;

        while samples_written < sample_len {
            // Loop through all blocks until we have enough samples written.
            match frame_read.read_next_or_eof(block_buf) {
                Ok(Some(block)) => {
                    // We assume 2 channels (by default), so we can use claxon's stereo_samples
                    // iterator for slightly better performance.
                    #[cfg(not(feature = "nonstandard_channel_count"))]
                    for (l, r) in block.stereo_samples() {
                        cursor.write_i16::<T>(l as i16)?;
                        cursor.write_i16::<T>(r as i16)?;
                        samples_written += 1;
                    }

                    // This is generic over number of assumed channels, but is broken effectively
                    // for any value other than 2.
                    // What we really want here is specialization for CHANNELS = 2 ...
                    #[cfg(feature = "nonstandard_channel_count")]
                    for sample in 0..block.len() / block.channels() {
                        for channel in 0..block.channels() {
                            let sample_data = block.sample(channel, sample) as u16;
                            cursor.write_i16::<T>(sample_data as i16)?;
                        }
                        samples_written += 1;
                    }

                    block_buf = block.into_buffer();
                }
                _ => {
                    // If frame_read dies our buffer just gets eaten. The Error return for a failed
                    // read does not expose the inner buffer.
                    return Err(ChdError::DecompressionError);
                }
            }
        }

        self.buffer = block_buf;
        let bytes_in = frame_read.into_inner().position();
        Ok(DecompressResult::new(
            samples_written * 4,
            bytes_in as usize,
        ))
    }
}

/// Raw FLAC (flac) decompression codec.
///
/// ## Format details
/// Raw FLAC expects the first byte as either 'L' (0x4C) or 'B' (0x42) to indicate the endianness
/// of the output data, followed by the compressed FLAC data.
///
/// FLAC compressed audio data is assumed to be 2-channel 16-bit signed integer PCM.
/// The audio data is decompressed in interleaved format, with the left channel first, then
/// the right channel for each sample, for 32 bits each sample.
///
/// ## Buffer Restrictions
/// Each compressed FLAC hunk decompresses to a hunk-sized chunk.
/// The input buffer must contain enough samples to fill the hunk-sized output buffer.
pub struct RawFlacCodec {
    be: FlacCodec<BigEndian>,
    le: FlacCodec<LittleEndian>,
}

impl CompressionCodec for RawFlacCodec {}

impl CompressionCodecType for RawFlacCodec {
    fn codec_type(&self) -> CodecType
    where
        Self: Sized,
    {
        CodecType::FlacV5
    }
}

impl CodecImplementation for RawFlacCodec {
    fn new(hunk_bytes: u32) -> Result<Self> {
        Ok(RawFlacCodec {
            be: FlacCodec::new(hunk_bytes)?,
            le: FlacCodec::new(hunk_bytes)?,
        })
    }

    fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<DecompressResult> {
        match input[0] {
            b'L' => self.le.decompress(&input[1..], output),
            b'B' => self.be.decompress(&input[1..], output),
            _ => Err(ChdError::DecompressionError),
        }
    }
}

/// CD-ROM wrapper decompression codec (cdfl) using the FLAC
/// for decompression of sector data and the [Deflate codec](crate::codecs::ZlibCodec) for
/// decompression of subcode data.
///
/// ## Format Details
/// FLAC compressed audio data is assumed to be 2-channel 16-bit signed integer PCM.
/// The audio data is decompressed in interleaved format, with the left channel first, then
/// the right channel for each sample, for 32 bits each sample.
///
/// CD-ROM wrapped FLAC is always written to the output stream in big-endian byte order.
///
/// CD-ROM compressed hunks have a layout with all compressed frame data in sequential order,
/// followed by compressed subcode data.
///
/// ```c
/// [Frame0, Frame1, ..., FrameN, Subcode0, Subcode1, ..., SubcodeN]
/// ```
/// Unlike CDLZ or CDZL, there is no header before the compressed data begins.
/// The length of the compressed data is determined by the number of 2448-sized frames
/// that can fit into the hunk-sized output buffer. Following the FLAC compressed blocks,
/// the subcode data is a single Deflate stream.
///
/// After decompression, the data is swizzled so that each frame is followed by its corresponding
/// subcode data.
/// ```c
/// [Frame0, Subcode0, Frame1, Subcode1, ..., FrameN, SubcodeN]
/// ```
/// FLAC compressed frames does not require manual reconstruction of the sync header or ECC bytes.
///
/// ## Buffer Restrictions
/// Each compressed CDFL hunk decompresses to a hunk-sized chunk. The hunk size must be a multiple
/// of 2448, the size of each CD frame. The input buffer must contain enough samples to fill
/// the number of CD sectors that can fit into the output buffer.
pub struct CdFlacCodec {
    // cdfl always writes in big endian.
    engine: FlacCodec<BigEndian>,
    sub_engine: ZlibCodec,
    buffer: Vec<u8>,
}

impl CompressionCodec for CdFlacCodec {}

impl CompressionCodecType for CdFlacCodec {
    fn codec_type(&self) -> CodecType {
        CodecType::FlacCdV5
    }
}

impl CodecImplementation for CdFlacCodec {
    fn new(hunk_size: u32) -> Result<Self>
    where
        Self: Sized,
    {
        if hunk_size % CD_FRAME_SIZE != 0 {
            return Err(ChdError::CodecError);
        }

        // The size of the FLAC data in each cdfl hunk, excluding the subcode data.
        let max_frames = hunk_size / CD_FRAME_SIZE;
        let flac_data_size = max_frames * CD_MAX_SECTOR_DATA;

        // neither FlacCodec nor ZlibCodec actually make use of hunk_size.
        Ok(CdFlacCodec {
            engine: FlacCodec::new(flac_data_size)?,
            sub_engine: ZlibCodec::new(hunk_size)?,
            buffer: vec![0u8; hunk_size as usize],
        })
    }

    fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<DecompressResult> {
        let total_frames = output.len() / CD_FRAME_SIZE as usize;
        let frame_res = self.engine.decompress(
            input,
            &mut self.buffer[..total_frames * CD_MAX_SECTOR_DATA as usize],
        )?;

        #[cfg(feature = "want_subcode")]
        let sub_res = self.sub_engine.decompress(
            &input[frame_res.total_in()..],
            &mut self.buffer[total_frames * CD_MAX_SECTOR_DATA as usize..]
                [..total_frames * CD_MAX_SUBCODE_DATA as usize],
        )?;

        #[cfg(not(feature = "want_subcode"))]
        let sub_res = DecompressResult::default();

        // Decompressed FLAC data has layout
        // [Frame0, Frame1, ..., FrameN, Subcode0, Subcode1, ..., SubcodeN]
        // We need to reassemble the data to be
        // [Frame0, Subcode0, Frame1, Subcode1, ..., FrameN, SubcodeN]

        // Reassemble frame data to expected layout.
        for (frame_num, chunk) in self.buffer[..total_frames * CD_MAX_SECTOR_DATA as usize]
            .chunks_exact(CD_MAX_SECTOR_DATA as usize)
            .enumerate()
        {
            output[frame_num * CD_FRAME_SIZE as usize..][..CD_MAX_SECTOR_DATA as usize]
                .copy_from_slice(chunk);
        }

        // Reassemble subcode data to expected layout.
        #[cfg(feature = "want_subcode")]
        for (frame_num, chunk) in self.buffer[total_frames * CD_MAX_SECTOR_DATA as usize..]
            .chunks_exact(CD_MAX_SUBCODE_DATA as usize)
            .enumerate()
        {
            output[frame_num * CD_FRAME_SIZE as usize + CD_MAX_SECTOR_DATA as usize..]
                [..CD_MAX_SUBCODE_DATA as usize]
                .copy_from_slice(chunk);
        }

        Ok(frame_res + sub_res)
    }
}
