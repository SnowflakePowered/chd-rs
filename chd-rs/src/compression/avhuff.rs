// Truly awful implementation of AVHuff as a direct translation from avhuff.cpp.
// Needs a lot of work and clean up to be more descriptive.
use std::io::Cursor;
use std::mem;
use bitreader::{BitReader, BitReaderError};
use byteorder::{BigEndian, WriteBytesExt};
use claxon::frame::FrameReader;
use crate::compression::{CompressionCodec, CompressionCodecType, DecompressLength, InternalCodec};
use crate::header::CodecType;
use crate::{ChdError, huffman};
use crate::huffman::{HuffmanDecoder, HuffmanError};

#[allow(unused)]
pub enum AVHuffError
{
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
        code if code <= 0x107 => {
            8 + (code - 0x100)
        }
        code => {
            16 << (code - 0x108)
        }
    }
}

type DeltaRleHuffman<'a> = HuffmanDecoder<'a, {256 + 16}, 16, {huffman::lookup_length::<16>()}>;

struct DeltaRleDecoder<'a> {
    huffman: DeltaRleHuffman<'a>,
    rle_count: u32,
    prev_data: u8
}

impl <'a> DeltaRleDecoder<'a> {
    pub fn new(huff: HuffmanDecoder<'a, {256 + 16}, 16, {huffman::lookup_length::<16>()}>) -> Self {
        Self {
            huffman: huff,
            rle_count: 0,
            prev_data: 0
        }
    }

    pub fn flush_rle(&mut self) {
        self.rle_count = 0;
    }

    #[inline(always)]
    pub fn decode_one(&mut self, reader: &mut BitReader<'a>) -> Result<u32, AVHuffError> {
        // I honestly don't know why avhuff.cpp widens u8 to u32, but
        // there's not much way to test this outside so I'm following it strictly.
        if self.rle_count != 0 {
            self.rle_count -= 1;
            return Ok(self.prev_data as u32);
        }

        let data = self.huffman.decode_one(reader)?;
        return if data < 0x100 {
            self.prev_data += data as u8;
            Ok(self.prev_data as u32)
        } else {
            self.rle_count = code_to_rle_count(data);
            self.rle_count -= 1;
            Ok(self.prev_data as u32)
        }
    }
}

pub struct AVHuffCodec {
    buffer: Vec<i32>,
}

impl CompressionCodec for AVHuffCodec {}

impl CompressionCodecType for AVHuffCodec {
    fn codec_type(&self) -> CodecType where Self: Sized {
        CodecType::AV
    }
}

impl InternalCodec for AVHuffCodec {
    fn is_lossy(&self) -> bool where Self: Sized {
        false
    }

    fn new(_hunk_bytes: u32) -> crate::Result<Self> {
        Ok(AVHuffCodec {
            buffer: Vec::new()
        })
    }

    fn decompress(&mut self, mut input: &[u8], mut output: &mut [u8]) -> crate::Result<DecompressLength> {
        // https://github.com/mamedev/mame/blob/master/src/lib/util/avhuff.cpp#L723
        if input.len() < 8 {
            return Err(ChdError::DecompressionError)
        }

        let mut total_bytes = 0;
        // todo: cursorize
        let meta_size: u32 = input[0] as u32;
        let channels: u32 = input[1] as u32;
        let samples: u32 = ((input[2] as u32) << 8) + input[3] as u32;
        let width: u32 = ((input[4] as u32) << 8) + input[5] as u32;
        let height: u32 = ((input[6] as u32) << 8) + input[7] as u32;

        if input.len() < (10 + 2 * channels) as usize {
            return Err(ChdError::DecompressionError);
        }

        let mut ch_comp_sizes = [0u16; 16];

        let mut total_size = 10 + 2 * channels;
        let tree_size: u32 = ((input[8] as u32) << 8) | input[9] as u32;

        if tree_size != 0xffff {
            total_size += tree_size;
        }
        for ch in 0..channels as usize {
            let ch_size = ((input[ch * 2 + 2] as u16) << 8) | input[ch * 2 + 3] as u16;
            ch_comp_sizes[ch] = ch_size;
            total_size += ch_size as u32;
        }

        if total_size as usize >= input.len() {
            return Err(ChdError::DecompressionError);
        }

        // create header (todo: cursorize)
        output[0] = b'c';
        output[1] = b'h';
        output[2] = b'a';
        output[3] = b'v';
        output[4] = meta_size as u8;
        output[5] = channels as u8;
        output[6] = (samples >> 8) as u8;
        output[7] = samples as u8;
        output[8] = (width >> 8) as u8;
        output[9] = width as u8;
        output[10] = (height >> 8) as u8;
        output[11] = height as u8;

        output = &mut output[12..];

        let (meta, mut rest) = output.split_at_mut(meta_size as usize);
        // workaround for Option<&mut [u8]> not being Copy.
        let mut channel_slices: [Option<&mut [u8]>; 16] =  [None, None, None, None,
                                                      None, None, None, None,
                                                      None, None, None, None,
                                                      None, None, None, None];

        for channel in 0..channels as usize {
            let (ch_out, next) = rest.split_at_mut(2 * samples as usize);
            channel_slices[channel] = Some(ch_out);
            rest = next;
        }

        let video = rest;

        if meta_size > 0 {
            meta.copy_from_slice(&input[10 * 2 * channels as usize..meta_size as usize]);
            input = &input[10 * 2 * channels as usize + meta_size as usize..];
        }

        // todo: use DecompressLength
        if channels > 0 {
            // todo: bounds
            total_bytes += self.decode_audio(samples, &input, &mut channel_slices, 0, &ch_comp_sizes[..], tree_size)
                .map_err(|_| ChdError::DecompressionError)?;

            input = &input[tree_size as usize + ch_comp_sizes.iter().sum::<u16>() as usize..]
        }

        if width > 0 && height > 0 && video.len() != 0 {
            total_bytes += self.decode_video(width, height, &input, video, (width * 2) as usize, 0)
                .map_err(|_| ChdError::DecompressionError)?;
        }


        Ok(DecompressLength::new(meta_size as usize + total_bytes, input.len()))
    }
}

impl AVHuffCodec {
    fn decode_audio(&mut self, samples: u32, input: &[u8], dest: &mut [Option<&mut [u8]>], xor: usize, ch_sizes: &[u16], tree_size: u32) -> Result<usize, AVHuffError> {
        match tree_size {
            0xffff => {
                let mut source = input;
                let mut total_bytes_written = 0;
                // I have no idea if this is correct.
                for (channel, channel_dest) in dest.iter_mut().enumerate() {
                    let size = ch_sizes[channel];
                    let mut buf = mem::take(&mut self.buffer);
                    match channel_dest.as_deref_mut() {
                        Some(channel) => {
                            let flac_buf = Cursor::new(&source[..size as usize]);
                            let mut frame_read = FrameReader::new(flac_buf);
                            let mut bytes_written = 0;
                            let len = channel.len();
                            let mut cursor = Cursor::new(channel);
                            while bytes_written < len {
                                match frame_read.read_next_or_eof(buf) {
                                    Ok(Some(block)) => {
                                        for (l, r) in block.stereo_samples() {
                                            cursor.write_i16::<BigEndian>(l as i16)
                                                .map_err(|_| AVHuffError::InvalidParameter)?;
                                            cursor.write_i16::<BigEndian>(r as i16)
                                                .map_err(|_| AVHuffError::InvalidParameter)?;
                                            bytes_written += 4;
                                        }
                                        buf = block.into_buffer();
                                    }
                                    _ => return Err(AVHuffError::InvalidData)
                                }
                            }
                            total_bytes_written += len;
                        }
                        None => ()
                    }
                    self.buffer = buf;
                    // increase slice..
                    source = &source[size as usize..]
                }
                Ok(total_bytes_written)
            }
            0 => {
                // no huffman length
                let mut source = input;
                let mut bytes_written = 0;
                for (channel, channel_dest) in dest.iter_mut().enumerate() {
                    let size = ch_sizes[channel];
                    let mut cur_source = source;

                    match channel_dest.as_deref_mut() {
                        Some(mut channel) => {
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
                        }
                        None => {}
                    }

                    source = &source[size as usize..]
                }
                Ok(bytes_written)
            }
            tree_size => {
                let mut source = input;
                let mut bytes_written = 0;
                let mut bit_reader = BitReader::new(&source[..tree_size as usize]);
                // todo: should be HuffmanCodec (huffman8bit)
                let mut hi_decoder = DeltaRleDecoder::new(DeltaRleHuffman::from_tree_rle(&mut bit_reader)?);
                bit_reader.align(1)?;
                let mut lo_decoder = DeltaRleDecoder::new(DeltaRleHuffman::from_tree_rle(&mut bit_reader)?);

                bit_reader.align(1)?;
                if bit_reader.remaining() != 0 {
                    return Err(AVHuffError::InvalidData)
                }

                source = &source[tree_size as usize..];

                for (channel, channel_dest) in dest.iter_mut().enumerate() {
                    let size = ch_sizes[channel];
                    match channel_dest.as_deref_mut() {
                        Some(mut channel) => {
                            let mut prev_sample = 0;
                            let mut bit_reader = BitReader::new(&source);

                            for _sample in 0..samples {
                                let mut delta: u16 = (hi_decoder.decode_one(&mut bit_reader)? << 8) as u16;
                                delta |= lo_decoder.decode_one(&mut bit_reader)? as u16;

                                let new_sample = prev_sample + delta;
                                prev_sample = new_sample;
                                // todo:  is this just write_be?
                                channel[0 ^ xor] = (new_sample >> 8) as u8;
                                channel[1 ^ xor] = new_sample as u8;
                                bytes_written += 2;
                                channel = &mut channel[2..]
                            }
                        }
                        None => {}
                    }

                    source = &source[size as usize..]
                }
                Ok(bytes_written)
            }
        }
    }

    fn decode_video(&self, width: u32, height: u32, input: &[u8], dest: &mut [u8], stride: usize, xor: usize) -> Result<usize, AVHuffError> {
        if input[0] & 0x80 == 0 {
            return Err(AVHuffError::InvalidData)
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
            return Err(AVHuffError::InvalidData)
        }
        Ok(bytes_written)
    }
}


