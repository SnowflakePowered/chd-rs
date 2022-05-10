use crate::error::Result;
use crate::header::CodecType;
use std::ops::Add;

mod cdrom;
mod ecc;
mod flac;
mod lzma;
mod none;
mod zlib;

#[cfg(feature = "avhuff")]
mod avhuff;

#[cfg(feature = "flac_header")]
mod flac_header;
mod huff;

pub mod codecs {
    pub use crate::compression::cdrom::CdLzCodec;
    pub use crate::compression::cdrom::CdZlCodec;
    pub use crate::compression::flac::CdFlCodec;
    pub use crate::compression::none::NoneCodec;
    pub use crate::compression::zlib::ZlibCodec;
    pub use crate::compression::lzma::LzmaCodec;
    pub use crate::compression::flac::RawFlacCodec;
    pub use crate::compression::huff::HuffmanCodec;
    #[cfg(feature = "avhuff")]
    pub use crate::compression::avhuff::AVHuffCodec;
}

// unstable(trait_alias)
/// Marker trait for a public user exposed codec that is used to decode CHDs.
pub trait CompressionCodec: InternalCodec + CompressionCodecType {}

/// Marker trait for a codec that decompresses in fixed sized chunks of compressed data.
pub trait BlockCodec: InternalCodec {}

/// A codec that has a externally known type.
pub trait CompressionCodecType {
    fn codec_type(&self) -> CodecType
    where
        Self: Sized;
}

/// A compression codec used to decompress.
pub trait InternalCodec {
    fn is_lossy(&self) -> bool
    where
        Self: Sized;
    fn new(hunk_bytes: u32) -> Result<Self>
    where
        Self: Sized;
    fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<DecompressLength>;
}

#[derive(Copy, Clone, Default)]
pub struct DecompressLength {
    bytes_out: usize,
    bytes_read: usize,
}

impl Add for DecompressLength {
    type Output = DecompressLength;

    fn add(self, rhs: Self) -> Self::Output {
        DecompressLength {
            bytes_out: self.total_out() + rhs.total_out(),
            bytes_read: self.total_in() + rhs.total_in(),
        }
    }
}

impl DecompressLength {
    pub fn new(out: usize, read: usize) -> Self {
        DecompressLength {
            bytes_out: out,
            bytes_read: read,
        }
    }

    pub fn total_out(&self) -> usize {
        self.bytes_out
    }

    pub fn total_in(&self) -> usize {
        self.bytes_read
    }
}
