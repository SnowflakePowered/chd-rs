use crate::compression::{
    CodecImplementation, CompressionCodec, CompressionCodecType, DecompressResult,
};
use crate::header::CodecType;
#[cfg(feature = "encode")]
use crate::huffman::HuffmanEncoder;
use crate::huffman::{Huffman8BitDecoder, HuffmanDecoder};
use crate::{huffman, Error, Result};
use bitreader::BitReader;
#[cfg(feature = "encode")]
use bitstream_io::{BitWrite, BitWriter};
#[cfg(feature = "encode")]
use byteorder::ByteOrder;
use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use claxon::frame::FrameReader;

use arrayvec::ArrayVec;
use std::io::{Cursor, Read, Write};
use std::mem;
use std::ops::DerefMut;

// Length of the decompressed frame header.
const AVHU_HEADER_LEN: usize = 12;
// Length of the fixed compressed header, not including the audio stream lengths.
const AVHU_COMP_HEADER_LEN: usize = 10;
// Tree size to indicate FLAC compression for channel streams.
const AVHU_FLAC_TREESIZE: u16 = 0xffff;

#[inline(always)]
const fn code_to_rle_count(code: u32) -> u32 {
    match code {
        0 => 1,
        // Codes less than 0x107 have an RLE count of 8 + (code - 0x100)
        code if code <= 0x107 => 8 + (code - 0x100),
        // Codes greater than 0x107
        code => 16 << (code - 0x108),
    }
}

/// The largest RLE code whose count is less than or equal to the provided count.
#[cfg(feature = "encode")]
const fn rle_count_to_code(rle_count: u32) -> u32 {
    match rle_count {
        2048.. => 0x10f,
        1024.. => 0x10e,
        512.. => 0x10d,
        256.. => 0x10c,
        128.. => 0x10b,
        64.. => 0x10a,
        32.. => 0x109,
        16.. => 0x108,
        // Codes up to 0x107 have an RLE count of 8 + (code - 0x100)
        8.. => 0x100 + (rle_count - 8),
        // A zero delta that is not part of a run is encoded as itself.
        _ => 0,
    }
}

type DeltaRleHuffman<'a> = HuffmanDecoder<'a, { 256 + 16 }, 16, { huffman::lookup_len::<16>() }>;

#[cfg(feature = "encode")]
type DeltaRleHuffmanEncoder = HuffmanEncoder<'static, { 256 + 16 }, 16>;

/// Delta-RLE encoder for one component of the video data.
///
/// See [avhuff.cpp](https://github.com/mamedev/mame/blob/74b9fae5a3ca3c99828fe41da154e3c84951414e/src/lib/util/avhuff.cpp#L614).
#[cfg(feature = "encode")]
struct DeltaRleEncoder {
    huffman: DeltaRleHuffmanEncoder,
    // The code for each item of the component, or `None` if the item is part of a run.
    codes: Vec<Option<u16>>,
}

#[cfg(feature = "encode")]
impl DeltaRleEncoder {
    fn new() -> Self {
        Self {
            huffman: DeltaRleHuffmanEncoder::new(),
            codes: Vec::new(),
        }
    }

    /// Delta-RLE encode a component of the video data, and compute the Huffman tree for it.
    ///
    /// Each row has `items_per_row` items, `item_advance` bytes apart, starting
    /// at `offset` bytes from the start of the row.
    fn rle_and_histogram(
        &mut self,
        video: &[u8],
        offset: usize,
        items_per_row: usize,
        item_advance: usize,
        stride: usize,
    ) -> Result<()> {
        self.codes.clear();

        // The previous value carries over between rows.
        let mut prev = 0u8;
        for row in video.chunks_exact(stride) {
            let item = |index: usize| row[offset + index * item_advance];
            let mut index = 0;
            while index < items_per_row {
                let current = item(index);
                let delta = current.wrapping_sub(prev);
                prev = current;

                if delta != 0 {
                    self.codes.push(Some(delta as u16));
                    index += 1;
                    continue;
                }

                // Zero deltas scan forward for a count.
                let mut run = 1
                    + (index + 1..items_per_row)
                        .take_while(|&next| item(next) == prev)
                        .count();

                // If we hit the end of a row, maximize the count, since runs end
                // with the row anyways.
                if index + run >= items_per_row && run >= 8 {
                    run = 100000;
                }

                let code = rle_count_to_code(run as u32);
                let covered = (code_to_rle_count(code) as usize).min(items_per_row - index);
                self.codes.push(Some(code as u16));
                self.codes.extend(std::iter::repeat_n(None, covered - 1));
                index += covered;
            }
        }

        self.huffman
            .try_compute_tree(self.codes.iter().flatten().map(|&code| code as u32))
            .map_err(|_| Error::CompressionError)
    }

    /// Write the Huffman tree as an RLE encoded tree, padded to a byte boundary.
    fn export_tree<W: BitWrite>(&self, writer: &mut W) -> Result<()> {
        self.huffman
            .export_tree_rle(writer)
            .map_err(|_| Error::CompressionError)?;
        writer.byte_align().map_err(|_| Error::CompressionError)
    }

    /// Write the code for an item, if it is not part of a run.
    fn encode<W: BitWrite>(&self, writer: &mut W, code: Option<u16>) -> Result<()> {
        let Some(code) = code else { return Ok(()) };
        self.huffman
            .encode_one(writer, code as u32)
            .map_err(|_| Error::CompressionError)
    }
}

struct DeltaRleDecoder<'a> {
    // We need three of these and they're too big to store on the stack.
    huffman: Box<DeltaRleHuffman<'a>>,
    rle_count: u32,
    prev_data: u8,
}

impl<'a> DeltaRleDecoder<'a> {
    pub fn new(huff: DeltaRleHuffman<'a>) -> Self {
        Self {
            huffman: Box::new(huff),
            rle_count: 0,
            prev_data: 0,
        }
    }

    pub fn flush_rle(&mut self) {
        self.rle_count = 0;
    }

    #[inline(always)]
    pub fn decode_one(&mut self, reader: &mut BitReader<'a>) -> Result<u8> {
        // avhuff.cpp widens this to u32 but we can just keep it as u8
        if self.rle_count != 0 {
            self.rle_count -= 1;
            return Ok(self.prev_data);
        }

        let data = self.huffman.decode_one(reader)?;
        if data < 0x100 {
            self.prev_data = self.prev_data.wrapping_add(data as u8);
            Ok(self.prev_data)
        } else {
            self.rle_count = code_to_rle_count(data);
            self.rle_count -= 1;
            Ok(self.prev_data)
        }
    }
}

/// MAME AV Huffman (avhu) decompression codec.
///
/// ## Format Details
/// AV Huffman is based around a [delta-RLE encoding](https://github.com/mamedev/mame/blob/ee1e4f9683a4953cb9d88f9256017fcbc38e3144/src/lib/util/huffman.cpp#L68) of a Huffman tree with parameters
/// * `NUM_CODES`: 256 + 16
/// * `MAX_BITS`: 16
///
/// Audio data is typically 16-bit signed integer FLAC encoded as an array of separate streams where
/// one stream contains the data for one audio channel. Older formats include uncompressed 16-bit
/// PCM audio, or Huffman encoded audio. These formats are supported but not as well tested in
/// this implementation as FLAC.
///
/// Video data utilizes the above delta-RLE Huffman to compress losslessly, contained in a raw array
/// directly after the audio stream data.
///
/// Compressed hunks have the layout of a 10-byte header containing the various lengths of each section,
/// followed by the sizes of the compressed audio streams as 16-bit big endian integers, followed
/// by the compressed audio streams, then video huffman tables and the compressed video data.
///
/// For additional format details, see
/// [avhuff.cpp](https://github.com/mamedev/mame/blob/ad1f89e85cd747c48f13bb47973908ee127c9c6e/src/lib/util/avhuff.cpp).
///
/// ## Buffer Restrictions
/// Each compressed AVHU hunk decompresses to a hunk-sized chunk. The input buffer must have a valid
/// compressed header and include enough data to decompress in accordance with the header. The
/// input buffer must decompress into at most a hunk-sized chunk. If the input buffer does not have
/// enough data, the remainder of the output buffer will be zero-filled.
pub struct AVHuffCodec {
    buffer: Vec<i32>,
    #[cfg(feature = "encode")]
    y_context: DeltaRleEncoder,
    #[cfg(feature = "encode")]
    cb_context: DeltaRleEncoder,
    #[cfg(feature = "encode")]
    cr_context: DeltaRleEncoder,
}

impl CompressionCodec for AVHuffCodec {}

impl CompressionCodecType for AVHuffCodec {
    fn codec_type(&self) -> CodecType
    where
        Self: Sized,
    {
        CodecType::AVHuffV5
    }
}

fn avhuff_write_header(
    output: &mut [u8; AVHU_HEADER_LEN],
    meta_size: u8,
    channels: u8,
    samples: u16,
    width: u16,
    height: u16,
) -> Result<DecompressResult> {
    let mut output = &mut output[..];
    output.write_all(b"chav")?;
    output.write_u8(meta_size as u8)?;
    output.write_u8(channels as u8)?;
    output.write_u16::<BigEndian>(samples)?;
    output.write_u16::<BigEndian>(width)?;
    output.write_u16::<BigEndian>(height)?;
    Ok(DecompressResult::new(AVHU_HEADER_LEN, 0))
}

impl CodecImplementation for AVHuffCodec {
    fn new(_hunk_bytes: u32) -> Result<Self> {
        Ok(AVHuffCodec {
            buffer: Vec::new(),
            #[cfg(feature = "encode")]
            y_context: DeltaRleEncoder::new(),
            #[cfg(feature = "encode")]
            cb_context: DeltaRleEncoder::new(),
            #[cfg(feature = "encode")]
            cr_context: DeltaRleEncoder::new(),
        })
    }

    fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<DecompressResult> {
        // https://github.com/mamedev/mame/blob/master/src/lib/util/avhuff.cpp#L723
        if input.len() < 8 {
            return Err(Error::DecompressionError);
        }

        let mut input_cursor = Cursor::new(input);
        let meta_size = input_cursor.read_u8()?;
        let channels = input_cursor.read_u8()?;
        let samples = input_cursor.read_u16::<BigEndian>()?;
        let width = input_cursor.read_u16::<BigEndian>()?;
        let height = input_cursor.read_u16::<BigEndian>()?;

        // Each channel length entry is u16 = 2 bytes.
        if input.len() < AVHU_COMP_HEADER_LEN + 2 * channels as usize {
            return Err(Error::DecompressionError);
        }

        // Total expected length of input in bytes
        let mut total_in: usize = AVHU_COMP_HEADER_LEN + 2 * channels as usize;

        // If the tree size is 0xffff we are dealing with FLAC not Huffman.
        let tree_size = input_cursor.read_u16::<BigEndian>()?;
        if tree_size != AVHU_FLAC_TREESIZE {
            total_in += tree_size as usize;
        }

        // sizes of channels in compressed
        let mut channel_comp_len: ArrayVec<u16, 16> = ArrayVec::new();
        for _ in 0..channels as usize {
            let ch_size = input_cursor.read_u16::<BigEndian>()?;
            channel_comp_len.push(ch_size);
            total_in += ch_size as usize;
        }

        // Input length has to have enough data for all the channel data.
        if total_in >= input.len() {
            return Err(Error::DecompressionError);
        }

        // Write the MAME compressed AV header.
        let header_result = avhuff_write_header(
            <&mut [u8; AVHU_HEADER_LEN]>::try_from(&mut output[..AVHU_HEADER_LEN])?,
            meta_size,
            channels,
            samples,
            width,
            height,
        )?;

        // Slice the output into three sections, excluding header.
        // [metadata] [audio channels] [video]

        // Get the slice in the output that stores metadata.
        let (out_meta, mut out_rest) = output[AVHU_HEADER_LEN..].split_at_mut(meta_size as usize);

        // Get the slices that each audio channel will decompress into
        let mut channel_slices: ArrayVec<&mut [u8], 16> = ArrayVec::new();
        for _ in &channel_comp_len {
            let (out_channel, next) = out_rest.split_at_mut(2 * samples as usize);
            channel_slices.push(out_channel);
            out_rest = next;
        }

        // The remainder of the destination stores video data.
        let video = out_rest;

        // Should be a no-op if meta_size == 0
        input_cursor.read_exact(out_meta)?;

        // So far we have written HEADER_LEN and the metadata.
        let mut result = DecompressResult::new(
            header_result.total_out() + meta_size as usize,
            input_cursor.position() as usize,
        );
        if channels > 0 {
            // decode_audio returns the number of bytes read from the input buffer,
            // and the number of bytes written into the output buffer (channel_slices).
            // The number of bytes read is either the Huffman tree size (`tree_size`),
            // or the sum of the lengths of compressed channel data (`channel_comp_len`)
            //
            // In avhuff.cpp, the equivalent is done, with asserts to illustrate equivalence
            // of the number of bytes read with `tree_size` or the sum of `channel_comp_len`,
            // where `audio_res` is the unwrapped return value from `decode_audio`.
            // ```
            // if tree_size != 0xffff {
            //     assert_eq!(audio_res.total_in(), tree_size as usize);
            //     input = &input[tree_size as usize..];
            // } else {
            //     assert_eq!(audio_res.total_in(), channel_comp_len.iter().sum::<u16>() as usize);
            //     input = &input[channel_comp_len.iter().sum::<u16>() as usize..]
            // }
            //```
            result += self.decode_audio(
                samples,
                &input[result.total_in()..],
                &mut channel_slices,
                &channel_comp_len[..],
                tree_size,
            )?;
        }

        if width > 0 && height > 0 && !video.is_empty() {
            // avhuff.cpp always gives a videoxor of 0, so we don't have it here in this
            // implementation for clarity. The purpose of videoxor is to swap endianness
            // but we can use byteorder to enforce endianness here.
            result += self.decode_video(
                width,
                height,
                &input[result.total_in()..],
                video,
                (width * 2) as usize,
            )?;
        }

        Ok(result)
    }

    /// Compresses a raw A/V frame.
    ///
    /// Returns [`InvalidData`](Error::InvalidData) if the input is not a raw A/V frame
    /// padded with zeroes.
    #[cfg(feature = "encode")]
    fn compress(&mut self, input: &[u8], output: &mut [u8]) -> Result<usize> {
        let header = input.get(..AVHU_HEADER_LEN).ok_or(Error::InvalidData)?;
        if &header[..4] != b"chav" {
            return Err(Error::InvalidData);
        }
        let meta_size = header[4] as usize;
        let channels = header[5] as usize;
        let samples = BigEndian::read_u16(&header[6..8]) as usize;
        let width = BigEndian::read_u16(&header[8..10]) as usize;
        let height = BigEndian::read_u16(&header[10..12]) as usize;
        if channels > 16 {
            return Err(Error::InvalidData);
        }

        // Short frames must be padded with zeroes.
        let audio_len = 2 * channels * samples;
        let raw_len = AVHU_HEADER_LEN + meta_size + audio_len + 2 * width * height;
        let (raw, padding) = input.split_at_checked(raw_len).ok_or(Error::InvalidData)?;
        if padding.iter().any(|&byte| byte != 0) {
            return Err(Error::InvalidData);
        }
        let (metadata, rest) = raw[AVHU_HEADER_LEN..].split_at(meta_size);
        let (audio, video) = rest.split_at(audio_len);

        // Anything not smaller than the input is a failure, so don't bother writing more.
        let output_len = output.len().min(input.len());
        let header_len = AVHU_COMP_HEADER_LEN + 2 * channels;
        let (header, data) = output[..output_len]
            .split_at_mut_checked(header_len)
            .ok_or(Error::CompressionError)?;
        header[0] = meta_size as u8;
        header[1] = channels as u8;
        header[2..8].copy_from_slice(&raw[6..12]);
        let (tree_size, channel_sizes) = header[8..].split_at_mut(2);

        let (metadata_out, mut remaining) = data
            .split_at_mut_checked(meta_size)
            .ok_or(Error::CompressionError)?;
        metadata_out.copy_from_slice(metadata);

        if channels > 0 {
            // A tree size of 0xffff indicates FLAC compressed audio.
            tree_size.copy_from_slice(&AVHU_FLAC_TREESIZE.to_be_bytes());
            let audio_len = Self::encode_audio(audio, samples, channel_sizes, remaining)?;
            remaining = &mut remaining[audio_len..];
        } else {
            tree_size.fill(0);
        }

        if width > 0 && height > 0 {
            let video_len = self.encode_video(width, video, remaining)?;
            remaining = &mut remaining[video_len..];
        }

        let compressed_len = output_len - remaining.len();
        if compressed_len >= input.len() {
            return Err(Error::CompressionError);
        }
        Ok(compressed_len)
    }
}

#[cfg(feature = "encode")]
impl AVHuffCodec {
    /// Compress each audio channel as a separate mono FLAC stream with a single block.
    ///
    /// Returns the total length of the compressed audio.
    fn encode_audio(
        audio: &[u8],
        samples: usize,
        channel_sizes: &mut [u8],
        output: &mut [u8],
    ) -> Result<usize> {
        use flacenc::component::BitRepr;
        use flacenc::error::Verify;

        let mut config = flacenc::config::Encoder::default();
        config.block_size = samples;
        let config = config
            .into_verified()
            .map_err(|_| Error::CompressionError)?;

        let output_len = output.len();
        let mut remaining = output;
        for (channel, size) in audio
            .chunks_exact(2 * samples)
            .zip(channel_sizes.chunks_exact_mut(2))
        {
            // Audio samples are stored big endian.
            let channel_samples: Vec<i32> = channel
                .chunks_exact(2)
                .map(|sample| BigEndian::read_i16(sample) as i32)
                .collect();
            let source = flacenc::source::MemSource::from_samples(&channel_samples, 1, 16, 48000);
            let stream = flacenc::encode_with_fixed_block_size(&config, source, samples)
                .map_err(|_| Error::CompressionError)?;

            let mut sink = flacenc::bitsink::ByteSink::new();
            for frame in (0..stream.frame_count()).filter_map(|n| stream.frame(n)) {
                frame
                    .write(&mut sink)
                    .map_err(|_| Error::CompressionError)?;
            }
            let compressed = sink.as_slice();

            // MAME limits each stream to the size of the uncompressed samples.
            let compressed_len = u16::try_from(compressed.len())
                .ok()
                .filter(|&len| len as usize <= 2 * samples)
                .ok_or(Error::CompressionError)?;
            size.copy_from_slice(&compressed_len.to_be_bytes());

            let (stream_out, rest) = mem::take(&mut remaining)
                .split_at_mut_checked(compressed.len())
                .ok_or(Error::CompressionError)?;
            stream_out.copy_from_slice(compressed);
            remaining = rest;
        }
        Ok(output_len - remaining.len())
    }

    /// Losslessly compress the YUY2 video data with delta-RLE Huffman encoding.
    /// The video data is `width * 2` bytes for each row.
    ///
    /// Returns the length of the compressed video.
    fn encode_video(&mut self, width: usize, video: &[u8], output: &mut [u8]) -> Result<usize> {
        // Each pair of pixels is Y Cb Y Cr, so the video must have an even width.
        if !width.is_multiple_of(2) {
            return Err(Error::CompressionError);
        }
        let stride = 2 * width;

        self.y_context
            .rle_and_histogram(video, 0, width, 2, stride)?;
        self.cb_context
            .rle_and_histogram(video, 1, width / 2, 4, stride)?;
        self.cr_context
            .rle_and_histogram(video, 3, width / 2, 4, stride)?;

        let output_len = output.len();
        let mut remaining = output;
        let mut writer = BitWriter::endian(&mut remaining, bitstream_io::BigEndian);

        // The first byte is 0x80 to indicate lossless encoding.
        writer
            .write::<8, u8>(0x80)
            .map_err(|_| Error::CompressionError)?;
        self.y_context.export_tree(&mut writer)?;
        self.cb_context.export_tree(&mut writer)?;
        self.cr_context.export_tree(&mut writer)?;

        // Each pair of pixels is Y Cb Y Cr.
        let rows = self
            .y_context
            .codes
            .chunks_exact(width)
            .zip(self.cb_context.codes.chunks_exact(width / 2))
            .zip(self.cr_context.codes.chunks_exact(width / 2));
        for ((y_row, cb_row), cr_row) in rows {
            for ((y, &cb), &cr) in y_row.chunks_exact(2).zip(cb_row).zip(cr_row) {
                self.y_context.encode(&mut writer, y[0])?;
                self.cb_context.encode(&mut writer, cb)?;
                self.y_context.encode(&mut writer, y[1])?;
                self.cr_context.encode(&mut writer, cr)?;
            }
        }
        writer.byte_align().map_err(|_| Error::CompressionError)?;

        Ok(output_len - remaining.len())
    }
}

impl AVHuffCodec {
    fn decode_audio_flac(
        &mut self,
        inputs: &ArrayVec<&[u8], 16>,
        outputs: &mut ArrayVec<&mut [u8], 16>,
    ) -> Result<DecompressResult> {
        let mut total_written = 0;
        let mut total_read = 0;

        for (channel_idx, channel_out) in outputs.iter_mut().map(|d| d.deref_mut()).enumerate() {
            // Buffer to store block data.
            let mut block_buf = mem::take(&mut self.buffer);

            // FLAC frame reader
            let mut frame_read = FrameReader::new(Cursor::new(inputs[channel_idx]));

            let out_len = channel_out.len();
            let mut channel_out = Cursor::new(channel_out);
            while channel_out.position() < out_len as u64 {
                match frame_read.read_next_or_eof(block_buf) {
                    Ok(Some(block)) => {
                        // Every channel is stored in separate FLAC streams in channel 0
                        for sample in block.channel(0) {
                            channel_out
                                .write_i16::<BigEndian>(*sample as i16)
                                .map_err(|_| Error::DecompressionError)?;
                        }
                        block_buf = block.into_buffer();
                    }
                    _ => return Err(Error::DecompressionError),
                }
            }
            total_read += frame_read.into_inner().position();
            total_written += channel_out.position();

            self.buffer = block_buf;
        }
        Ok(DecompressResult::new(
            total_written as usize,
            total_read as usize,
        ))
    }

    fn decode_audio(
        &mut self,
        samples: u16,
        mut input: &[u8],
        dest: &mut ArrayVec<&mut [u8], 16>,
        ch_comp_sizes: &[u16],
        tree_size: u16,
    ) -> Result<DecompressResult> {
        match tree_size {
            AVHU_FLAC_TREESIZE => {
                // Split input array into slices.
                let mut input_slices: ArrayVec<&[u8], 16> = ArrayVec::new();
                for size in ch_comp_sizes {
                    let (slice, rest) = input.split_at(*size as usize);
                    input_slices.push(slice);
                    input = rest;
                }
                self.decode_audio_flac(&input_slices, dest)
            }
            0 => {
                // Tree size of 0 indicates uncompressed data.
                let mut bytes_written = 0;
                for (channel, channel_dest) in dest.iter_mut().enumerate() {
                    let size = ch_comp_sizes[channel];
                    let mut channel_input = &input[..size as usize];
                    let mut channel = channel_dest.deref_mut();

                    let mut prev_sample = 0;
                    for _sample in 0..samples {
                        let delta = channel_input.read_u16::<BigEndian>()?;

                        let new_sample = prev_sample + delta;
                        prev_sample = new_sample;
                        channel.write_u16::<BigEndian>(new_sample)?;
                        bytes_written += 2;

                        // write_u16::<BigEndian> is equivalent to the following
                        // channel[0 ^ xor] = (new_sample >> 8) as u8;
                        // channel[1 ^ xor] = new_sample as u8;
                        // channel = &mut channel[2..]
                    }

                    // Advance the slice
                    input = &input[size as usize..]
                }
                Ok(DecompressResult::new(bytes_written, bytes_written))
            }
            tree_size => {
                let mut source = input;
                let mut bytes_written = 0;
                let mut bytes_read = 0;
                let mut bit_reader = BitReader::new(&source[..tree_size as usize]);

                let hi_decoder = Huffman8BitDecoder::from_tree_rle(&mut bit_reader)?;
                bit_reader.align(1)?;
                let lo_decoder = Huffman8BitDecoder::from_tree_rle(&mut bit_reader)?;

                bit_reader.align(1)?;
                if bit_reader.remaining() != 0 {
                    return Err(Error::DecompressionError);
                }

                source = &source[tree_size as usize..];
                bytes_read += bit_reader.position() / 8;

                for (channel, channel_dest) in dest.iter_mut().enumerate() {
                    let size = ch_comp_sizes[channel];
                    let mut channel = channel_dest.deref_mut();

                    let mut prev_sample = 0;
                    let mut bit_reader = BitReader::new(source);

                    for _sample in 0..samples {
                        let mut delta: u16 = (hi_decoder.decode_one(&mut bit_reader)? << 8) as u16;
                        delta |= lo_decoder.decode_one(&mut bit_reader)? as u16;

                        let new_sample = prev_sample + delta;
                        prev_sample = new_sample;

                        channel.write_u16::<BigEndian>(new_sample)?;
                        bytes_written += 2;

                        // write_u16::<BigEndian> is equivalent to the following
                        // channel[0 ^ xor] = (new_sample >> 8) as u8;
                        // channel[1 ^ xor] = new_sample as u8;
                        // channel = &mut channel[2..]
                    }

                    bytes_read += bit_reader.position() / 8;
                    source = &source[size as usize..]
                }
                Ok(DecompressResult::new(bytes_written, bytes_read as usize))
            }
        }
    }

    fn decode_video(
        &self,
        width: u16,
        height: u16,
        input: &[u8],
        output: &mut [u8],
        stride: usize,
    ) -> Result<DecompressResult> {
        if input[0] & 0x80 == 0 {
            // avhuff.cpp only supports lossless format.
            return Err(Error::UnsupportedFormat);
        }

        // Skip first byte that indicates lossless.
        let mut bit_reader = BitReader::new(&input[1..]);
        let mut y_context = DeltaRleDecoder::new(DeltaRleHuffman::from_tree_rle(&mut bit_reader)?);
        bit_reader.align(1)?;
        let mut cb_context = DeltaRleDecoder::new(DeltaRleHuffman::from_tree_rle(&mut bit_reader)?);
        bit_reader.align(1)?;
        let mut cr_context = DeltaRleDecoder::new(DeltaRleHuffman::from_tree_rle(&mut bit_reader)?);
        bit_reader.align(1)?;

        // The decoders here are one-shot and do not need to be reset.
        // Unfortunately because three of them are too big to fit onto one stack frame
        // we have to box the inner Huffman decoders.

        let mut bytes_written = 0;
        for dy in 0..height as usize {
            let mut row = &mut output[dy * stride..];
            for _dx in 0..(width / 2) as usize {
                // Reconstruct the frame from the delta-Huffman decoder.
                // The order here is big-endian.
                let pixel = u32::from_be_bytes([
                    y_context.decode_one(&mut bit_reader)?,
                    cb_context.decode_one(&mut bit_reader)?,
                    y_context.decode_one(&mut bit_reader)?,
                    cr_context.decode_one(&mut bit_reader)?,
                ]);
                // Write in big endian.
                row.write_u32::<BigEndian>(pixel)?;
                bytes_written += 4;
            }
            y_context.flush_rle();
            cb_context.flush_rle();
            cr_context.flush_rle();
        }

        bit_reader.align(1)?;
        if bit_reader.remaining() != 0 {
            return Err(Error::DecompressionError);
        }

        // If we don't fill the output buffer, fill the remainder with zeroes.
        output[bytes_written..].fill(0);

        Ok(DecompressResult::new(
            output.len(),
            1 + bit_reader.position() as usize / 8,
        ))
    }
}
