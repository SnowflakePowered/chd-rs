use crate::compression::{
    CodecImplementation, CompressionCodec, CompressionCodecType, DecompressResult,
};
use crate::header::CodecType;
use crate::Error;

/// Zstandard (zstd) decompression codec.
///
/// ## Format Details
/// CHD compresses Zstandard hunks with the streaming compressor.
///
/// ## Buffer Restrictions
/// Each compressed Zstandard hunk decompresses to a hunk-sized chunk.
/// The input buffer must contain exactly enough data to fill the output buffer
/// when decompressed.
///
/// ## Compression
/// Without `fast_zstd`, hunks are compressed with the pure Rust ruzstd encoder.
/// ruzstd only encodedes up to around Zstandard level 1. Use `fast_zstd` for chdman-equivalent compression.
#[cfg(not(feature = "fast_zstd"))]
pub struct ZstdCodec {
    decoder: ruzstd::decoding::FrameDecoder,
    // ruzstd panics if it can not write to the output, so compress into a buffer first.
    #[cfg(feature = "encode")]
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
    zstd_context: zstd_safe::DCtx<'static>,
    #[cfg(feature = "encode")]
    compressor: zstd_safe::CCtx<'static>,
}

#[cfg(not(feature = "fast_zstd"))]
impl CodecImplementation for ZstdCodec {
    fn new(_hunk_size: u32) -> crate::Result<Self>
    where
        Self: Sized,
    {
        Ok(Self {
            decoder: ruzstd::decoding::FrameDecoder::new(),
            #[cfg(feature = "encode")]
            buffer: Vec::new(),
        })
    }

    #[cfg(feature = "encode")]
    fn compress(&mut self, input: &[u8], output: &mut [u8]) -> crate::Result<usize> {
        use ruzstd::encoding::{compress, CompressionLevel};

        self.buffer.clear();
        compress(input, &mut self.buffer, CompressionLevel::Fastest);

        let compressed_len = self.buffer.len();
        if compressed_len >= input.len() {
            return Err(Error::CompressionError);
        }
        output
            .get_mut(..compressed_len)
            .ok_or(Error::CompressionError)?
            .copy_from_slice(&self.buffer);
        Ok(compressed_len)
    }

    fn decompress(
        &mut self,
        mut input: &[u8],
        output: &mut [u8],
    ) -> crate::Result<DecompressResult> {
        let bytes_out = self
            .decoder
            .decode_all(&mut input, output)
            .map_err(|e| Error::DecompressionError)?;

        // If each chunk doesn't output to exactly the same then it's an error
        if bytes_out != output.len() {
            return Err(Error::DecompressionError);
        }

        Ok(DecompressResult {
            bytes_out,
            // The "read" value returned by decode_from_to() would be incorrect here,
            // since reset() modifies the slice length.
            // bytes_read_from_source() appears to return the whole block length.
            bytes_read: self.decoder.bytes_read_from_source() as usize,
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
            zstd_context: zstd_safe::DCtx::try_create().ok_or(crate::Error::CodecError)?,
            #[cfg(feature = "encode")]
            compressor: zstd_safe::CCtx::try_create().ok_or(Error::CodecError)?,
        })
    }

    #[cfg(feature = "encode")]
    fn compress(&mut self, input: &[u8], output: &mut [u8]) -> crate::Result<usize> {
        // Anything not smaller than the input is a failure, so don't bother writing more.
        let output_len = output.len().min(input.len());
        let compressed_len = self
            .compressor
            .compress(&mut output[..output_len], input, zstd_safe::max_c_level())
            .map_err(|_| Error::CompressionError)?;
        if compressed_len >= input.len() {
            return Err(Error::CompressionError);
        }
        Ok(compressed_len)
    }

    fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> crate::Result<DecompressResult> {
        self.zstd_context
            .reset(zstd_safe::ResetDirective::SessionAndParameters)
            .map_err(|_| Error::DecompressionError)?;

        // If each chunk doesn't output to exactly the same then it's an error
        let bytes_out = self
            .zstd_context
            .decompress(output, input)
            .map_err(|_| Error::DecompressionError)?;

        if bytes_out != output.len() {
            return Err(Error::DecompressionError);
        }

        Ok(DecompressResult {
            bytes_out: output.len(),
            // ZSTD_decompress() takes the exact size of a number of frames, so it
            // should've returned an error if it hasn't used the entire input slice.
            bytes_read: input.len(),
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
