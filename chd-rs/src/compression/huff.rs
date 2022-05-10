use std::io::Write;
use crate::compression::{CompressionCodec, CompressionCodecType, DecompressLength, InternalCodec};
use crate::header::CodecType;
use crate::error::Result;

pub struct HuffmanCodec;
impl InternalCodec for HuffmanCodec {
    fn is_lossy(&self) -> bool {
        false
    }

    fn new(_: u32) -> Result<Self> {
        Ok(HuffmanCodec)
    }

    fn decompress(&mut self, input: &[u8], mut output: &mut [u8]) -> Result<DecompressLength> {
        Ok(DecompressLength::new(output.write(&input)?, input.len()))
    }
}

impl CompressionCodecType for HuffmanCodec {
    fn codec_type(&self) -> CodecType {
        CodecType::HuffV5
    }
}

impl CompressionCodec for HuffmanCodec {}
