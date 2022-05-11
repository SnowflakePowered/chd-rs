// None (copy) codec
use crate::compression::{CompressionCodec, CompressionCodecType, DecompressLength, InternalCodec};
use crate::error::Result;
use crate::header::CodecType;
use std::io::Write;

pub struct NoneCodec;
impl InternalCodec for NoneCodec {
    fn is_lossy(&self) -> bool {
        false
    }

    fn new(_: u32) -> Result<Self> {
        Ok(NoneCodec)
    }

    fn decompress(&mut self, input: &[u8], mut output: &mut [u8]) -> Result<DecompressLength> {
        Ok(DecompressLength::new(output.write(&input)?, input.len()))
    }
}

impl CompressionCodecType for NoneCodec {
    fn codec_type(&self) -> CodecType {
        CodecType::None
    }
}

impl CompressionCodec for NoneCodec {}
