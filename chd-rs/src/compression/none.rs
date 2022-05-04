use std::io::Write;
use crate::compression::{CompressionCodec, CompressionCodecType, InternalCodec};
use crate::header::CodecType;
use crate::error::Result;

pub struct NoneCodec;
impl InternalCodec for NoneCodec {
    fn is_lossy() -> bool {
        false
    }

    fn new(_: u32) -> Result<Self> {
        Ok(NoneCodec)
    }

    fn decompress(&mut self, input: &[u8], mut output: &mut [u8]) -> Result<u64> {
        Ok(output.write(input)? as u64)
    }
}

impl CompressionCodecType for NoneCodec {
    fn codec_type() -> CodecType {
        CodecType::None
    }
}

impl CompressionCodec for NoneCodec {}