// None (copy) codec
use crate::compression::{
    CodecImplementation, CompressionCodec, CompressionCodecType, DecompressResult,
};
use crate::error::Result;
use crate::header::CodecType;
use std::io::Write;

/// None/copy codec that does a byte-for-byte copy of the input buffer.
///
/// ## Buffer Restrictions
/// The input buffer must be exactly the same length as the output buffer.
pub struct NoneCodec;
impl CodecImplementation for NoneCodec {
    fn is_lossy(&self) -> bool {
        false
    }

    fn new(_: u32) -> Result<Self> {
        Ok(NoneCodec)
    }

    fn decompress(&mut self, input: &[u8], mut output: &mut [u8]) -> Result<DecompressResult> {
        Ok(DecompressResult::new(output.write(input)?, input.len()))
    }
}

impl CompressionCodecType for NoneCodec {
    fn codec_type(&self) -> CodecType {
        CodecType::None
    }
}

impl CompressionCodec for NoneCodec {}
