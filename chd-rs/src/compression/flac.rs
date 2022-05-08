use std::io::{Cursor, Read};
use std::mem;

use byteorder::{BigEndian, LittleEndian, NativeEndian, WriteBytesExt};
use claxon::frame::FrameReader;
use claxon::input::BufferedReader;

use crate::cdrom::{CD_FRAME_SIZE, CD_MAX_SECTOR_DATA, CD_MAX_SUBCODE_DATA};
use crate::compression::zlib::ZlibCodec;
use crate::compression::{CompressionCodec, CompressionCodecType, DecompressLength, InternalCodec};
use crate::error::{ChdError, Result};
use crate::header::CodecType;

struct FlacCodec {
    buffer: Vec<i32>,
}

// #[cfg(target_endian = "big")]
// const IS_LITTLE_ENDIAN: bool = false;
//
// #[cfg(target_endian = "little")]
// const IS_LITTLE_ENDIAN: bool = true;

impl InternalCodec for FlacCodec {
    fn is_lossy(&self) -> bool {
        false
    }

    fn new(_: u32) -> Result<Self> {
        Ok(FlacCodec { buffer: Vec::new() })
    }

    /// Decompress FLAC data from raw input.
    ///
    /// FLAC data is assumed to be 2-channel interleaved 16-bit PCM. Thus the length of the output
    /// buffer must be a multiple of 4 to hold 2 bytes per sample, for 2 channels.
    ///
    /// The input buffer must also contain enough commpressed samples to fill the length of the
    /// output buffer.
    fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<DecompressLength> {
        // should do the equivalent of flac_decoder_decode_interleaved
        // https://github.com/rtissera/libchdr/blob/cdcb714235b9ff7d207b703260706a364282b063/src/libchdr_flac.c#L158
        let frames = output.len() / CD_MAX_SECTOR_DATA as usize;

        let num_samples = frames * CD_MAX_SECTOR_DATA as usize / 4;

        let flac_buf = Cursor::new(input);

        // We don't need to create a fake header since claxon will read raw FLAC frames just fine.
        let mut frame_read = FrameReader::new(flac_buf);
        let mut cursor = Cursor::new(output);

        let mut samples_written = 0;

        let mut buf = mem::take(&mut self.buffer);
        while samples_written < num_samples {
            match frame_read.read_next_or_eof(buf) {
                Ok(Some(block)) => {
                    for sample in 0..block.len() / block.channels() {
                        for channel in 0..block.channels() {
                            let sample_data = block.sample(channel, sample) as u16;
                            cursor.write_i16::<BigEndian>(sample_data as i16)?;
                        }
                        samples_written += 1;
                    }
                    buf = block.into_buffer();
                }
                _ => {
                    // if frame_read dies our buffer just gets eaten. The Error return for a failed
                    // read does not expose the inner buffer.
                    return Err(ChdError::DecompressionError);
                }
            }
        }
        self.buffer = buf;
        let bytes_in = frame_read.into_inner().position();
        Ok(DecompressLength::new(
            samples_written * 4,
            bytes_in as usize,
        ))
    }
}

pub struct CdFlCodec {
    engine: FlacCodec,
    sub_engine: ZlibCodec,
    buffer: Vec<u8>,
}

impl CompressionCodec for CdFlCodec {}

impl CompressionCodecType for CdFlCodec {
    fn codec_type(&self) -> CodecType {
        CodecType::FlacCdV5
    }
}

impl InternalCodec for CdFlCodec {
    fn is_lossy(&self) -> bool {
        false
    }

    fn new(hunk_size: u32) -> Result<Self>
    where
        Self: Sized,
    {
        if hunk_size % CD_FRAME_SIZE != 0 {
            return Err(ChdError::CodecError);
        }

        // neither FlacCodec nor ZlibCodec actually make use of hunk_size.
        Ok(CdFlCodec {
            engine: FlacCodec::new(hunk_size)?,
            sub_engine: ZlibCodec::new(hunk_size)?,
            buffer: vec![0u8; hunk_size as usize],
        })
    }

    fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<DecompressLength> {
        let frames = output.len() / CD_FRAME_SIZE as usize;

        let frame_res = self.engine.decompress(
            input,
            &mut self.buffer[..frames * CD_MAX_SECTOR_DATA as usize],
        )?;

        let sub_res = self.sub_engine.decompress(
            &input[frame_res.total_in()..],
            &mut self.buffer[frames * CD_MAX_SECTOR_DATA as usize..]
                [..frames * CD_MAX_SUBCODE_DATA as usize],
        )?;

        // reassemble the data into the buffer
        for frame_num in 0..frames {
            output[frame_num * CD_FRAME_SIZE as usize..][..CD_MAX_SECTOR_DATA as usize]
                .copy_from_slice(
                    &self.buffer[frame_num * CD_MAX_SECTOR_DATA as usize..]
                        [..CD_MAX_SECTOR_DATA as usize],
                );

            // WANT_SUBCODE
            output[frame_num * CD_FRAME_SIZE as usize + CD_MAX_SECTOR_DATA as usize..]
                [..CD_MAX_SUBCODE_DATA as usize]
                .copy_from_slice(
                    &self.buffer[frames * CD_MAX_SECTOR_DATA as usize
                        + frame_num * CD_MAX_SUBCODE_DATA as usize..]
                        [..CD_MAX_SUBCODE_DATA as usize],
                );
        }
        Ok(frame_res + sub_res)
    }
}
