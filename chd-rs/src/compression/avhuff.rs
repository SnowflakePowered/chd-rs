use crate::compression::{
    CodecImplementation, CompressionCodec, CompressionCodecType, DecompressResult,
};
use crate::header::CodecType;
use crate::huffman::{Huffman8BitDecoder, HuffmanDecoder};
use crate::{huffman, ChdError, Result};
use bitreader::BitReader;
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

type DeltaRleHuffman<'a> = HuffmanDecoder<'a, { 256 + 16 }, 16, { huffman::lookup_len::<16>() }>;

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
    fn is_lossy(&self) -> bool
    where
        Self: Sized,
    {
        false
    }

    fn new(_hunk_bytes: u32) -> Result<Self> {
        Ok(AVHuffCodec { buffer: Vec::new() })
    }

    fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<DecompressResult> {
        // https://github.com/mamedev/mame/blob/master/src/lib/util/avhuff.cpp#L723
        if input.len() < 8 {
            return Err(ChdError::DecompressionError);
        }

        let mut input_cursor = Cursor::new(input);
        let meta_size = input_cursor.read_u8()?;
        let channels = input_cursor.read_u8()?;
        let samples = input_cursor.read_u16::<BigEndian>()?;
        let width = input_cursor.read_u16::<BigEndian>()?;
        let height = input_cursor.read_u16::<BigEndian>()?;

        // Each channel length entry is u16 = 2 bytes.
        if input.len() < AVHU_COMP_HEADER_LEN + 2 * channels as usize {
            return Err(ChdError::DecompressionError);
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
            return Err(ChdError::DecompressionError);
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

        // So far we have written HEADER_LEN
        let mut result =
            DecompressResult::new(header_result.total_out(), input_cursor.position() as usize);
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

        if width > 0 && height > 0 && video.len() != 0 {
            // avhuff.cpp always gives a videoxor of 0, so we don't have it here in this
            // implementation for clarity. The purpose of videoxor is to swap endianness
            // but we can use byteorder to enforce endianness here.
            result += self
                .decode_video(
                    width,
                    height,
                    &input[result.total_in()..],
                    video,
                    (width * 2) as usize,
                )
                .map_err(|_| ChdError::DecompressionError)?;
        }

        Ok(result)
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
                                .map_err(|_| ChdError::DecompressionError)?;
                        }
                        block_buf = block.into_buffer();
                    }
                    _ => return Err(ChdError::DecompressionError),
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
                    return Err(ChdError::DecompressionError);
                }

                source = &source[tree_size as usize..];
                bytes_read += bit_reader.position() / 8;

                for (channel, channel_dest) in dest.iter_mut().enumerate() {
                    let size = ch_comp_sizes[channel];
                    let mut channel = channel_dest.deref_mut();

                    let mut prev_sample = 0;
                    let mut bit_reader = BitReader::new(&source);

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
            return Err(ChdError::UnsupportedFormat);
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
            return Err(ChdError::DecompressionError);
        }

        // If we don't fill the output buffer, fill the remainder with zeroes.
        output[bytes_written..].fill(0);

        Ok(DecompressResult::new(
            output.len(),
            1 + bit_reader.position() as usize / 8,
        ))
    }
}
