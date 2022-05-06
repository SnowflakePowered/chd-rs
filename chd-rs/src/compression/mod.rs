use std::ops::Add;
use crate::header::CodecType;
use crate::error::Result;

mod ecc;
mod none;
mod zlib;
mod lzma;
mod cdrom;
pub mod flac;
mod io;

pub mod codecs {
    pub use crate::compression::none::NoneCodec;
    pub use crate::compression::zlib::ZlibCodec;
    pub use crate::compression::cdrom::CdZlCodec;
    pub use crate::compression::cdrom::CdLzCodec;
    pub use crate::compression::flac::CdFlCodec;
}

// unstable(trait_alias)
/// Marker trait for a public user exposed codec that is used to decode CHDs.
pub trait CompressionCodec: InternalCodec + CompressionCodecType {}

/// Marker trait for a codec that decompresses in fixed sized chunks of compressed data.
pub trait BlockCodec: InternalCodec {}

/// A codec that has a externally known type.
pub trait CompressionCodecType {
    fn codec_type() -> CodecType;
}

/// A compression codec used to decompress.
pub trait InternalCodec {
    fn is_lossy() -> bool;
    fn new(hunk_bytes: u32) -> Result<Self> where Self: Sized;
    fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<DecompressLength>;
}

#[derive(Copy, Clone)]
pub struct DecompressLength {
    bytes_out: usize,
    bytes_read: usize
}

impl Add for DecompressLength {
    type Output = DecompressLength;

    fn add(self, rhs: Self) -> Self::Output {
        DecompressLength {
            bytes_out: self.total_out() + rhs.total_out(),
            bytes_read: self.total_in() + rhs.total_in()
        }
    }
}

impl DecompressLength {
    pub fn new(out: usize, read: usize) -> Self {
        DecompressLength {
            bytes_out: out,
            bytes_read: read
        }
    }

    pub fn total_out(&self) -> usize {
        self.bytes_out
    }

    pub fn total_in(&self) -> usize {
        self.bytes_read
    }
}
