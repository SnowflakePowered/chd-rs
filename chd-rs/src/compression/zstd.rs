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
#[cfg(not(feature = "fast_zstd"))]
pub struct ZstdCodec {
    decoder: ruzstd::decoding::FrameDecoder,
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
}

#[cfg(not(feature = "fast_zstd"))]
impl CodecImplementation for ZstdCodec {
    fn new(_hunk_size: u32) -> crate::Result<Self>
    where
        Self: Sized,
    {
        Ok(Self {
            decoder: ruzstd::decoding::FrameDecoder::new(),
        })
    }

    fn decompress(
        &mut self,
        mut input: &[u8],
        output: &mut [u8],
    ) -> crate::Result<DecompressResult> {
        self.decoder
            .reset(&mut input)
            .map_err(|_| Error::DecompressionError)?;

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
        })
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
