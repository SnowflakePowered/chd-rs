use crate::compression::{
    CodecImplementation, CompressionCodec, CompressionCodecType, DecompressResult,
};
use crate::header::CodecType;
use crate::Error;
#[cfg(not(feature = "fast_zstd"))]
use std::io::Read;

/// Zstandard (zstd) decompression codec.
///
/// ## Format Details
/// CHD compresses Zstandard hunks with the streaming compressor.
///
/// ## Buffer Restrictions
/// Each compressed Zstandard hunk decompresses to a hunk-sized chunk.
/// The input buffer must contain exactly enough data to fill the output buffer
/// when decompressed.
#[cfg(not(feature = "fast_zstd"))]
pub struct ZstdCodec {
    engine: ruzstd::FrameDecoder,
    buffer: Vec<u8>,
}

/// Zstandard (zstd) decompression codec.
///
/// ## Format Details
/// CHD compresses Zstandard hunks with the streaming compressor.
///
/// ## Buffer Restrictions
/// Each compressed Zstandard hunk decompresses to a hunk-sized chunk.
/// The input buffer must contain exactly enough data to fill the output buffer
/// when decompressed.
#[cfg(feature = "fast_zstd")]
pub struct ZstdCodec {
    zstd_context: zstd::zstd_safe::DCtx<'static>
}

#[cfg(not(feature = "fast_zstd"))]
impl CodecImplementation for ZstdCodec {
    fn new(hunk_size: u32) -> crate::Result<Self>
    where
        Self: Sized,
    {
        Ok(Self {
            engine: ruzstd::FrameDecoder::new(),
            buffer: Vec::with_capacity(hunk_size as usize),
        })
    }

    fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> crate::Result<DecompressResult> {
        self.buffer.clear();
        let engine = std::mem::take(&mut self.engine);
        let mut engine = ruzstd::StreamingDecoder::new_with_decoder(input, engine)
            .map_err(|_| Error::CodecError)?;

        // If each chunk doesn't output to exactly the same then it's an error
        let bytes_read = engine
            .read_to_end(&mut self.buffer)
            .map_err(|_| Error::DecompressionError)?;

        let bytes_out = self.buffer.len();

        if bytes_out != output.len() {
            return Err(Error::DecompressionError);
        }

        output.clone_from_slice(&self.buffer);

        self.engine = engine.inner();
        Ok(DecompressResult {
            bytes_out: output.len(),
            bytes_read,
        })
    }
}

#[cfg(feature = "fast_zstd")]
impl CodecImplementation for ZstdCodec {
    fn new(_hunk_size: u32) -> crate::Result<Self>
    where
        Self: Sized,
    {
        Ok(Self {
            zstd_context: zstd::zstd_safe::DCtx::try_create().ok_or(crate::Error::CodecError)?
        })
    }

    fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> crate::Result<DecompressResult> {
        // If each chunk doesn't output to exactly the same then it's an error
        let bytes_out = self.zstd_context
            .decompress(output, input)
            .map_err(|_| Error::DecompressionError)?;

        if bytes_out != output.len() {
            return Err(Error::DecompressionError);
        }

        Ok(DecompressResult {
            bytes_out: output.len(),
            // ZSTD_decompress() takes the exact size of a number of frames, so it
            // should've returned an error if it hasn't used the entire input slice.
            bytes_read: input.len()
        })
    }
}

impl CompressionCodecType for ZstdCodec {
    fn codec_type(&self) -> CodecType
    where
        Self: Sized,
    {
        CodecType::ZstdV5
    }
}

impl CompressionCodec for ZstdCodec {}
