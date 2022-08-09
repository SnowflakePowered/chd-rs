use crate::compression::{
    CodecImplementation, CompressionCodec, CompressionCodecType, DecompressResult,
};
use crate::error::{Error, Result};
use crate::header::CodecType;
use flate2::{Decompress, FlushDecompress};

/// Deflate (zlib) decompression codec.
///
/// ## Format Details
/// CHD compresses Deflate hunks without a zlib header.
///
/// ## Buffer Restrictions
/// Each compressed Deflate hunk decompresses to a hunk-sized chunk.
/// The input buffer must contain exactly enough data to fill the output buffer
/// when decompressed.
pub struct ZlibCodec {
    engine: Decompress,
}

impl CodecImplementation for ZlibCodec {
    fn new(_: u32) -> Result<Self> {
        Ok(ZlibCodec {
            engine: Decompress::new(false),
        })
    }

    fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<DecompressResult> {
        self.engine.reset(false);
        let status = self
            .engine
            .decompress(input, output, FlushDecompress::Finish)
            .map_err(|_| Error::DecompressionError)?;

        if status == flate2::Status::BufError {
            return Err(Error::CompressionError);
        }

        let total_out = self.engine.total_out();
        if self.engine.total_out() != output.len() as u64 {
            return Err(Error::DecompressionError);
        }

        Ok(DecompressResult::new(
            total_out as usize,
            self.engine.total_in() as usize,
        ))
    }
}

impl CompressionCodecType for ZlibCodec {
    fn codec_type(&self) -> CodecType {
        CodecType::Zlib
    }
}

impl CompressionCodec for ZlibCodec {}
