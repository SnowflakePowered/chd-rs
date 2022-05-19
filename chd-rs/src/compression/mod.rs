use crate::error::Result;
use crate::header::CodecType;
use std::ops::{Add, AddAssign};

mod avhuff;
mod cdrom;
mod ecc;
mod flac;
mod lzma;
mod none;
mod zlib;

#[cfg(feature = "flac_header")]
mod flac_header;
mod huff;

pub mod codecs {
    pub use crate::compression::avhuff::AVHuffCodec;
    pub use crate::compression::cdrom::CdLzmaCodec;
    pub use crate::compression::cdrom::CdZlibCodec;
    pub use crate::compression::flac::CdFlacCodec;
    pub use crate::compression::flac::RawFlacCodec;
    pub use crate::compression::huff::HuffmanCodec;
    pub use crate::compression::lzma::LzmaCodec;
    pub use crate::compression::none::NoneCodec;
    pub use crate::compression::zlib::ZlibCodec;
}

// unstable(trait_alias)
/// Marker trait for a codec that can be used to decompress a compressed hunk.
pub trait CompressionCodec: CodecImplementation + CompressionCodecType {}

/// Trait for a codec that implements a known CHD codec type.
pub trait CompressionCodecType {
    /// Returns the known [`CodecType`](crate::header::CodecType) that this
    /// codec implements.
    fn codec_type(&self) -> CodecType
    where
        Self: Sized;
}

/// Trait for a CHD decompression codec implementation.
pub trait CodecImplementation {
    /// Returns whethere is codec is lossy or not.
    fn is_lossy(&self) -> bool
    where
        Self: Sized;

    /// Creates a new instance of this codec for the provided hunk size.
    fn new(hunk_size: u32) -> Result<Self>
    where
        Self: Sized;

    /// Decompress compressed bytes from the input buffer into the
    /// output buffer.
    ///
    /// Usually the output buffer must have the exact
    /// length as `hunk_size`, but this may be dependent on the codec
    /// implementation.
    fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<DecompressResult>;
}

/// The result of a chunk decompression operation.
#[derive(Copy, Clone, Default)]
pub struct DecompressResult {
    bytes_out: usize,
    bytes_read: usize,
}

impl Add for DecompressResult {
    type Output = DecompressResult;

    fn add(self, rhs: Self) -> Self::Output {
        DecompressResult {
            bytes_out: self.total_out() + rhs.total_out(),
            bytes_read: self.total_in() + rhs.total_in(),
        }
    }
}

impl AddAssign for DecompressResult {
    fn add_assign(&mut self, rhs: Self) {
        self.bytes_read += rhs.bytes_read;
        self.bytes_out += rhs.bytes_out;
    }
}

impl DecompressResult {
    pub(crate) fn new(out: usize, read: usize) -> Self {
        DecompressResult {
            bytes_out: out,
            bytes_read: read,
        }
    }

    /// Returns the total number of decompressed bytes written to the output buffer.
    pub fn total_out(&self) -> usize {
        self.bytes_out
    }

    /// Returns the total number of bytes read from the compressed input buffer.
    pub fn total_in(&self) -> usize {
        self.bytes_read
    }
}
