//! On-disk layouts of CHD metadata for parsing with zerocopy.
//!
//! Refer to [chd.cpp](https://github.com/mamedev/mame/blob/74b9fae5a3ca3c99828fe41da154e3c84951414e/src/lib/util/chd.cpp#L1724-L1728)
//! for the layout of a metadata entry.
use super::METADATA_HEADER_SIZE;
use std::mem::size_of;
use zerocopy::byteorder::{BigEndian, U32, U64};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

/// The header of a metadata entry, which is followed by the data of the entry.
#[derive(FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
pub(crate) struct RawMetadataHeader {
    pub tag: U32<BigEndian>,
    pub flags: u8,
    length: [u8; 3],
    /// The offset of the next metadata entry, or 0 if this is the last entry.
    pub next: U64<BigEndian>,
}

impl RawMetadataHeader {
    /// Create a metadata header. The length must fit in 24 bits.
    #[cfg(feature = "encode")]
    pub fn new(tag: u32, flags: u8, length: u32, next: u64) -> Self {
        let [_, a, b, c] = length.to_be_bytes();
        RawMetadataHeader {
            tag: tag.into(),
            flags,
            length: [a, b, c],
            next: next.into(),
        }
    }

    pub fn length(&self) -> u32 {
        let [a, b, c] = self.length;
        u32::from_be_bytes([0, a, b, c])
    }
}

const _: () = assert!(size_of::<RawMetadataHeader>() == METADATA_HEADER_SIZE);
