use crate::header::CodecType;
use crate::error::Result;

mod ecc;
mod none;
mod zlib;
mod lzma;
mod cdrom;
pub mod flac;


pub mod codecs {
    pub use crate::compression::none::NoneCodec;
    pub use crate::compression::zlib::ZlibCodec;
    pub use crate::compression::cdrom::CdZlCodec;
    pub use crate::compression::cdrom::CdLzCodec;
}

// unstable(trait_alias)
/// Marker trait for a public user exposed codec that is used to decode CHDs.
pub trait CompressionCodec: InternalCodec + CompressionCodecType {}

/// Marker trait for a codec that decompresses in chunks of compressed data.
/// May not be necessary, but need to see FLAC implementation.
pub trait BlockCodec: InternalCodec {}

/// A codec that has a externally known type.
pub trait CompressionCodecType {
    fn codec_type() -> CodecType;
}

/// A compression codec used to decompress.
pub trait InternalCodec {
    fn is_lossy() -> bool;
    fn new(hunk_bytes: u32) -> Result<Self> where Self: Sized;
    fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<u64>;
}

