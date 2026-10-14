//! On-disk layouts of CHD hunk maps for parsing with zerocopy.
//!
//! Refer to [chd.cpp](https://github.com/mamedev/mame/blob/74b9fae5a3ca3c99828fe41da154e3c84951414e/src/lib/util/chd.cpp)
//! for the layout of each version.
use super::{
    V1_MAP_ENTRY_SIZE, V3_MAP_ENTRY_SIZE, V5_COMPRESSED_MAP_ENTRY_SIZE,
    V5_UNCOMPRESSED_MAP_ENTRY_SIZE,
};
use std::mem::size_of;
use zerocopy::byteorder::{BigEndian, U16, U32, U64};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

type U16Be = U16<BigEndian>;
type U32Be = U32<BigEndian>;
type U64Be = U64<BigEndian>;

/// The size of the header of a compressed V5 map.
pub(super) const V5_COMPRESSED_MAP_HEADER_SIZE: usize = 16;

/// A V1-V2 map entry.
///
/// The length of the map entry is in the top 20 bits.
/// The offset in the low 44 bits.
#[derive(FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
pub(super) struct RawMapEntryV1(pub U64Be);

impl RawMapEntryV1 {
    pub fn length(&self) -> u32 {
        (self.0.get() >> 44) as u32
    }

    pub fn offset(&self) -> u64 {
        (self.0.get() << 20) >> 20
    }
}

/// A V3-V4 map entry.
#[derive(FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
pub(super) struct RawMapEntryV3 {
    pub offset: U64Be,
    pub crc: U32Be,
    // The length is 24 bits, with the low 16 bits stored first.
    pub length_lo: U16Be,
    pub length_hi: u8,
    pub flags: u8,
}

impl RawMapEntryV3 {
    pub fn length(&self) -> u32 {
        self.length_lo.get() as u32 | (self.length_hi as u32) << 16
    }
}

/// A V5 map entry for an uncompressed CHD, stored as the offset of the hunk in units of hunks.
#[derive(FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned, Clone, Copy)]
#[repr(C)]
pub(super) struct RawMapEntryV5Uncompressed(pub U32Be);

/// A V5 map entry for a compressed CHD, as stored once the compressed map is decoded.
///
/// The CRC of the compressed map covers these entries, with self and parent references
/// expanded to [`CompressionSelf`](super::CompressionTypeV5::CompressionSelf) and
/// [`CompressionParent`](super::CompressionTypeV5::CompressionParent).
#[derive(FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned, Clone, Copy)]
#[repr(C)]
pub(super) struct RawMapEntryV5 {
    pub compression: u8,
    length: [u8; 3],
    offset: [u8; 6],
    pub crc: U16Be,
}

impl RawMapEntryV5 {
    /// Create a map entry. The length must fit in 24 bits, and the offset must fit in 48 bits.
    pub fn new(compression: u8, length: u32, offset: u64, crc: u16) -> Self {
        let length = length.to_be_bytes();
        let offset = offset.to_be_bytes();
        RawMapEntryV5 {
            compression,
            length: [length[1], length[2], length[3]],
            offset: [
                offset[2], offset[3], offset[4], offset[5], offset[6], offset[7],
            ],
            crc: crc.into(),
        }
    }

    pub fn length(&self) -> u32 {
        let [a, b, c] = self.length;
        u32::from_be_bytes([0, a, b, c])
    }

    pub fn offset(&self) -> u64 {
        let [a, b, c, d, e, f] = self.offset;
        u64::from_be_bytes([0, 0, a, b, c, d, e, f])
    }
}

/// The header of a compressed V5 map.
#[derive(FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
pub(super) struct RawCompressedMapHeader {
    /// The length of the compressed map data following the header.
    pub length: U32Be,
    first_offset: [u8; 6],
    /// The CRC of the decoded map entries.
    pub crc: U16Be,
    pub length_bits: u8,
    pub self_bits: u8,
    pub parent_bits: u8,
    pub reserved: u8,
}

impl RawCompressedMapHeader {
    /// The offset of the first compressed or uncompressed hunk.
    pub fn first_offset(&self) -> u64 {
        let [a, b, c, d, e, f] = self.first_offset;
        u64::from_be_bytes([0, 0, a, b, c, d, e, f])
    }

    #[cfg(feature = "encode")]
    pub fn set_first_offset(&mut self, offset: u64) {
        let offset = offset.to_be_bytes();
        self.first_offset = [
            offset[2], offset[3], offset[4], offset[5], offset[6], offset[7],
        ];
    }
}

const _: () = assert!(size_of::<RawMapEntryV1>() == V1_MAP_ENTRY_SIZE);
const _: () = assert!(size_of::<RawMapEntryV3>() == V3_MAP_ENTRY_SIZE);
const _: () = assert!(size_of::<RawMapEntryV5Uncompressed>() == V5_UNCOMPRESSED_MAP_ENTRY_SIZE);
const _: () = assert!(size_of::<RawMapEntryV5>() == V5_COMPRESSED_MAP_ENTRY_SIZE);
const _: () = assert!(size_of::<RawCompressedMapHeader>() == V5_COMPRESSED_MAP_HEADER_SIZE);
