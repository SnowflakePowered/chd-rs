use crate::compression::{
    CompressionCodec, CompressionCodecType, DecompressLength, InternalCodec,
};
use crate::error::{ChdError, Result};
use crate::header::CodecType;
use flate2::{Decompress, FlushDecompress};

/// Zlib Deflate codec
pub struct ZlibCodec {
    engine: Decompress,
}

impl InternalCodec for ZlibCodec {
    fn is_lossy(&self) -> bool {
        false
    }

    fn new(_: u32) -> Result<Self> {
        Ok(ZlibCodec {
            engine: Decompress::new(false),
        })
    }

    fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<DecompressLength> {
        self.engine.reset(false);
        let status = self
            .engine
            .decompress(input, output, FlushDecompress::Finish)
            .map_err(|_| ChdError::DecompressionError)?;

        if status == flate2::Status::BufError {
            return Err(ChdError::CompressionError);
        }

        let total_out = self.engine.total_out();
        if self.engine.total_out() != output.len() as u64 {
            return Err(ChdError::DecompressionError);
        }

        return Ok(DecompressLength::new(
            total_out as usize,
            self.engine.total_in() as usize,
        ));
    }
}

impl CompressionCodecType for ZlibCodec {
    fn codec_type(&self) -> CodecType {
        CodecType::Zlib
    }
}

impl CompressionCodec for ZlibCodec {}
