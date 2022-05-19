/// AV Huffman (avhu) decompression codec.
///
/// ## Format Details
///
/// ## Buffer Restrictions
use crate::compression::{
    CodecImplementation, CompressionCodec, CompressionCodecType, DecompressResult,
};
use crate::header::CodecType;
use crate::huffman::{Huffman8BitDecoder, HuffmanDecoder, HuffmanError};
use crate::{huffman, ChdError, Result};
use bitreader::{BitReader, BitReaderError};
use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use claxon::frame::FrameReader;

use std::io::{Cursor, Read, Write};
use std::mem;
use std::ops::DerefMut;
use arrayvec::ArrayVec;

#[allow(unused)]
pub enum AVHuffError {
    None,
    InvalidData,
    VideoTooLarge,
    AudioTooLarge,
    MetadataError,
    OutOfMemory,
    CompressionError,
    TooManyChannels,
    InvalidConfiguration,
    InvalidParameter,
    BufferTooSmall,
}

impl From<HuffmanError> for AVHuffError {
    fn from(_: HuffmanError) -> Self {
        AVHuffError::InvalidData
    }
}

impl From<bitreader::BitReaderError> for AVHuffError {
    fn from(err: BitReaderError) -> Self {
        match err {
            BitReaderError::NotEnoughData {
                position: _,
                length: _,
                requested: _,
            } => AVHuffError::BufferTooSmall,
            BitReaderError::TooManyBitsForType {
                position: _,
                requested: _,
                allowed: _,
            } => AVHuffError::InvalidData,
        }
    }
}

#[inline(always)]
const fn code_to_rle_count(code: u32) -> u32 {
    match code {
        0 => 1,
        code if code <= 0x107 => 8 + (code - 0x100),
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

pub struct AVHuffCodec {
    buffer: Vec<i32>,
}

impl CompressionCodec for AVHuffCodec {}

impl CompressionCodecType for AVHuffCodec {
    fn codec_type(&self) -> CodecType
    where
        Self: Sized,
    {
        CodecType::AV
    }
}

fn avhuff_write_header(output: &mut [u8; 12], meta_size: u8, channels: u8, samples: u16, width: u16, height: u16) -> Result<DecompressResult> {
    let mut output = &mut output[..];
    output.write_all(b"chav")?;
    output.write_u8(meta_size as u8)?;
    output.write_u8(channels as u8)?;
    output.write_u16::<BigEndian>(samples)?;
    output.write_u16::<BigEndian>(width)?;
    output.write_u16::<BigEndian>(height)?;
    Ok(DecompressResult::new(12, 0))
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

    fn decompress(
        &mut self,
        mut input: &[u8],
        output: &mut [u8],
    ) -> Result<DecompressResult> {
        // https://github.com/mamedev/mame/blob/master/src/lib/util/avhuff.cpp#L723
        if input.len() < 8 {
            return Err(ChdError::DecompressionError);
        }

        output.fill(0);
        let mut total_written = 0;
        let mut total_read = 0;
        // todo: cursorize

        let mut input_cursor = Cursor::new(input);

        let meta_size = input_cursor.read_u8()?;
        let channels = input_cursor.read_u8()?;
        let samples = input_cursor.read_u16::<BigEndian>()?;
        let width = input_cursor.read_u16::<BigEndian>()?;
        let height = input_cursor.read_u16::<BigEndian>()?;

        if input.len() < (10 + 2 * channels) as usize {
            return Err(ChdError::DecompressionError);
        }

        // Total expected length of input in bytes
        let mut total_in: usize = 10 + 2 * channels as usize;

        // If the tree size is 0xffff we are dealing with FLAC not Huffman.
        let tree_size = input_cursor.read_u16::<BigEndian>()?;
        if tree_size != 0xffff {
            total_in += tree_size as usize;
        }

        // sizes of channels in compressed
        let mut channel_comp_len: ArrayVec<u16, 16> = ArrayVec::new();
        for _ in 0..channels as usize {
            let ch_size = input_cursor.read_u16::<BigEndian>()?;
            channel_comp_len.push(ch_size);
            total_in += ch_size as usize;
        }

        if total_in >= input.len() {
            return Err(ChdError::DecompressionError);
        }

        // Write the MAME compressed AV header.
        let header = avhuff_write_header(
            <&mut [u8; 12]>::try_from(&mut output[..12])?,
            meta_size, channels, samples, width, height)?;

        total_written += header.total_out();

        // Slice the output into three sections, excluding header.
        // [metadata] [audio channels] [video]

        // Get the slice in the output that stores metadata.
        let (out_meta, mut out_rest) = output[12..].split_at_mut(meta_size as usize);

        // Get the slices that each audio channel will decompress into
        let mut channel_slices: ArrayVec<&mut [u8], 16> = ArrayVec::new();
        for _ in &channel_comp_len {
            let (out_channel, next) = out_rest.split_at_mut(2 * samples as usize);
            channel_slices.push(out_channel);
            out_rest = next;
        }

        // The remainder stores video data.
        let video = out_rest;


        input = &input[10 + 2 * channels as usize..];
        total_read += 10 + 2 * channels as usize;

        // good up to here
        if meta_size > 0 {
            input_cursor.read_exact(out_meta)?;
            out_meta.copy_from_slice(&input[..meta_size as usize]);
            input = &input[meta_size as usize..];
            total_read += meta_size as usize;
        }

        let mut result = DecompressResult::new(total_written, total_read);
        // todo: use DecompressLength
        if channels > 0 {
            // todo: bounds
            let audio_res = self
                .decode_audio(
                    samples,
                    input,
                    &mut channel_slices,
                    0,
                    &channel_comp_len[..],
                    tree_size,
                )
                .map_err(|_| ChdError::DecompressionError)?;

            result += audio_res;
            if tree_size != 0xffff {
                assert_eq!(audio_res.bytes_read, tree_size as usize);
                input = &input[tree_size as usize..];
            } else {
                assert_eq!(audio_res.bytes_read, channel_comp_len.iter().sum::<u16>() as usize);
                input = &input[channel_comp_len.iter().sum::<u16>() as usize..]
            }
        }

        if width > 0 && height > 0 && video.len() != 0 {
            result += self
                .decode_video(width, height, &input, video, (width * 2) as usize, 0)
                .map_err(|_| ChdError::DecompressionError)?;
        }

        // bytes_out is wrong here. need to fill rest with zeroes.
        Ok(result)
    }
}

impl AVHuffCodec {

    fn decode_audio_flac(&mut self, inputs: &ArrayVec<& [u8], 16>, outputs: &mut ArrayVec<&mut [u8], 16>)
        -> Result<DecompressResult> {

        let mut total_written = 0;
        let mut total_read = 0;

        for (channel_idx, channel_out) in outputs.iter_mut()
                .map(|d| d.deref_mut()).enumerate() {
            let mut block_buf = mem::take(&mut self.buffer);
            let flac_buf = Cursor::new(inputs[channel_idx]);
            let mut frame_read = FrameReader::new(flac_buf);
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
            total_written += out_len;

            self.buffer = block_buf;
        }
        Ok(DecompressResult::new(total_written, total_read as usize))
    }

    fn decode_audio(
        &mut self,
        samples: u16,
        mut input: &[u8],
        dest: &mut ArrayVec<&mut [u8], 16>,
        xor: usize,
        ch_comp_sizes: &[u16],
        tree_size: u16,
    ) -> Result<DecompressResult> {
        match tree_size {
            0xffff => {
                let mut input_slices: ArrayVec<&[u8], 16> = ArrayVec::new();
                for size in ch_comp_sizes {
                    let (slice, rest) = input.split_at(*size as usize);
                    input_slices.push(slice);
                    input = rest;
                }
                self.decode_audio_flac(&input_slices, dest)
            }
            0 => {
                // no huffman length
                let mut source = input;
                let mut bytes_written = 0;
                for (channel, channel_dest) in dest.iter_mut().enumerate() {
                    let size = ch_comp_sizes[channel];
                    let mut cur_source = source;

                    let mut channel = channel_dest.deref_mut();

                    let mut prev_sample = 0;
                    for _sample in 0..samples {
                        let delta = (cur_source[0] as u16) << 8 | cur_source[1] as u16;
                        cur_source = &cur_source[2..];

                        let new_sample = prev_sample + delta;
                        prev_sample = new_sample;
                        // todo:  is this just write_be?
                        channel[0 ^ xor] = (new_sample >> 8) as u8;
                        channel[1 ^ xor] = new_sample as u8;
                        bytes_written += 2;
                        channel = &mut channel[2..]
                    }

                    source = &source[size as usize..]
                }
                Ok(DecompressResult::new(bytes_written, bytes_written))
            }
            tree_size => {
                let mut source = input;
                let mut bytes_written = 0;
                let mut bytes_read = 0;
                let mut bit_reader = BitReader::new(&source[..tree_size as usize]);
                // todo: should be HuffmanCodec (huffman8bit)
                let hi_decoder = Huffman8BitDecoder::from_tree_rle(&mut bit_reader)?;
                bit_reader.align(1)?;
                let lo_decoder = Huffman8BitDecoder::from_tree_rle(&mut bit_reader)?;

                bit_reader.align(1)?;
                if bit_reader.remaining() != 0 {
                    return Err(ChdError::DecompressionError);
                }

                source = &source[tree_size as usize..];
                bytes_read += tree_size;

                for (channel, channel_dest) in dest.iter_mut().enumerate() {
                    let size = ch_comp_sizes[channel];
                    let mut channel = channel_dest.deref_mut();

                    let mut prev_sample = 0;
                    let mut bit_reader = BitReader::new(&source);

                    for _sample in 0..samples {
                        let mut delta: u16 =
                            (hi_decoder.decode_one(&mut bit_reader)? << 8) as u16;
                        delta |= lo_decoder.decode_one(&mut bit_reader)? as u16;

                        let new_sample = prev_sample + delta;
                        prev_sample = new_sample;
                        // todo:  is this just write_be?
                        channel[0 ^ xor] = (new_sample >> 8) as u8;
                        channel[1 ^ xor] = new_sample as u8;
                        bytes_written += 2;
                        channel = &mut channel[2..]
                    }

                    source = &source[size as usize..]
                }
                Ok(DecompressResult::new(bytes_written, bytes_read as usize + bytes_written))
            }
        }
    }

    fn decode_video(
        &self,
        width: u16,
        height: u16,
        input: &[u8],
        dest: &mut [u8],
        stride: usize,
        xor: usize,
    ) -> Result<DecompressResult> {
        if input[0] & 0x80 == 0 {
            return Err(ChdError::UnsupportedFormat);
        }

        // decode losslessly

        // skip first byte
        let mut bit_reader = BitReader::new(&input[1..]);
        let mut y_context = DeltaRleDecoder::new(DeltaRleHuffman::from_tree_rle(&mut bit_reader)?);
        bit_reader.align(1)?;
        let mut cb_context = DeltaRleDecoder::new(DeltaRleHuffman::from_tree_rle(&mut bit_reader)?);
        bit_reader.align(1)?;
        let mut cr_context = DeltaRleDecoder::new(DeltaRleHuffman::from_tree_rle(&mut bit_reader)?);
        bit_reader.align(1)?;

        // No need to reset decoders because they are one-shot and zero-allocation.

        let mut bytes_written = 0;
        for dy in 0..height as usize {
            let mut row = &mut dest[dy * stride..];
            for _dx in 0..(width / 2) as usize {
                // todo: maybe use cursor instead of xor?
                row[0 ^ xor] = y_context.decode_one(&mut bit_reader)? as u8;
                row[1 ^ xor] = cb_context.decode_one(&mut bit_reader)? as u8;
                row[2 ^ xor] = y_context.decode_one(&mut bit_reader)? as u8;
                row[3 ^ xor] = cr_context.decode_one(&mut bit_reader)? as u8;
                row = &mut row[4..];
                bytes_written += 4;
            }
            y_context.flush_rle();
            cb_context.flush_rle();
            cr_context.flush_rle();
        }

        // unsure about this bounds check.
        bit_reader.align(1)?;
        if bit_reader.remaining() != 0 {
            return Err(ChdError::DecompressionError);
        }
        Ok(DecompressResult::new(bytes_written, bit_reader.position() as usize/8))
    }
}
