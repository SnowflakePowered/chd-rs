use crate::compression::{
    CodecImplementation, CompressionCodec, CompressionCodecType, DecompressResult,
};
use crate::error::{Error, Result};
use crate::header::CodecType;
use flate2::{Compress, Decompress, FlushDecompress};

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
    #[cfg(feature = "encode")]
    compressor: Compress,
}

impl ZlibCodec {
    /// Deflate the input into the output buffer, returning the number of compressed bytes.
    ///
    /// Unlike [`compress`](CodecImplementation::compress), the compressed data is allowed
    /// to be larger than the input as long as it fits into the output buffer.
    ///
    /// This is pulled out so cdfl can reuse the same deflate logic.
    #[cfg(feature = "encode")]
    pub(crate) fn deflate(&mut self, input: &[u8], output: &mut [u8]) -> Result<usize> {
        self.compressor.reset();
        let status = self
            .compressor
            .compress(input, output, flate2::FlushCompress::Finish);

        // Anything but the end of the stream means the output buffer was too small.
        if !matches!(status, Ok(flate2::Status::StreamEnd)) {
            // zlib-rs can panic on the next stream if a stream that ran out of output space
            // is reset, so start over with a new compressor instead.
            self.compressor = Compress::new(flate2::Compression::best(), false);
            return Err(Error::CompressionError);
        }
        Ok(self.compressor.total_out() as usize)
    }
}

impl CodecImplementation for ZlibCodec {
    fn new(_: u32) -> Result<Self> {
        Ok(ZlibCodec {
            engine: Decompress::new(false),
            #[cfg(feature = "encode")]
            compressor: Compress::new(flate2::Compression::best(), false),
        })
    }

    fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<DecompressResult> {
        self.engine.reset(false);
        let status = self
            .engine
            .decompress(input, output, FlushDecompress::Finish)
            .map_err(|_| Error::DecompressionError)?;

        if status == flate2::Status::BufError {
            return Err(Error::DecompressionError);
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

    #[cfg(feature = "encode")]
    fn compress(&mut self, input: &[u8], output: &mut [u8]) -> Result<usize> {
        // Anything not smaller than the input is a failure, so don't bother writing more.
        let output_len = output.len().min(input.len());
        let compressed_len = self.deflate(input, &mut output[..output_len])?;
        if compressed_len >= input.len() {
            return Err(Error::CompressionError);
        }
        Ok(compressed_len)
    }
}

impl CompressionCodecType for ZlibCodec {
    fn codec_type(&self) -> CodecType {
        CodecType::Zlib
    }
}

impl CompressionCodec for ZlibCodec {}
