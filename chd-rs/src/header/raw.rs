//! On-disk layouts of CHD headers for parsing with zerocopy.
//!
//! Refer to [chd.h](https://github.com/mamedev/mame/blob/74b9fae5a3ca3c99828fe41da154e3c84951414e/src/lib/util/chd.h)
//! for the layout of each version.
use super::{
    CHD_V1_HEADER_SIZE, CHD_V2_HEADER_SIZE, CHD_V3_HEADER_SIZE, CHD_V4_HEADER_SIZE,
    CHD_V5_HEADER_SIZE, MD5_BYTES, SHA1_BYTES,
};
use std::mem::size_of;
use zerocopy::byteorder::{BigEndian, U32, U64};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

type U32Be = U32<BigEndian>;
type U64Be = U64<BigEndian>;

/// The fields common to the start of every header version.
#[derive(FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
pub(super) struct RawHeaderPrefix {
    pub magic: [u8; 8],
    pub length: U32Be,
    pub version: U32Be,
}

/// A V1 header. V2 headers extend it with the sector length.
#[derive(FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
pub(super) struct RawHeaderV1 {
    pub prefix: RawHeaderPrefix,
    pub flags: U32Be,
    pub compression: U32Be,
    pub hunk_size: U32Be,
    pub total_hunks: U32Be,
    pub cylinders: U32Be,
    pub heads: U32Be,
    pub sectors: U32Be,
    pub md5: [u8; MD5_BYTES],
    pub parent_md5: [u8; MD5_BYTES],
}

#[derive(FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
pub(super) struct RawHeaderV2 {
    pub v1: RawHeaderV1,
    pub sector_length: U32Be,
}

#[derive(FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
pub(super) struct RawHeaderV3 {
    pub prefix: RawHeaderPrefix,
    pub flags: U32Be,
    pub compression: U32Be,
    pub total_hunks: U32Be,
    pub logical_bytes: U64Be,
    pub meta_offset: U64Be,
    pub md5: [u8; MD5_BYTES],
    pub parent_md5: [u8; MD5_BYTES],
    pub hunk_bytes: U32Be,
    pub sha1: [u8; SHA1_BYTES],
    pub parent_sha1: [u8; SHA1_BYTES],
}

#[derive(FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
pub(super) struct RawHeaderV4 {
    pub prefix: RawHeaderPrefix,
    pub flags: U32Be,
    pub compression: U32Be,
    pub total_hunks: U32Be,
    pub logical_bytes: U64Be,
    pub meta_offset: U64Be,
    pub hunk_bytes: U32Be,
    pub sha1: [u8; SHA1_BYTES],
    pub parent_sha1: [u8; SHA1_BYTES],
    pub raw_sha1: [u8; SHA1_BYTES],
}

#[derive(FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
pub(super) struct RawHeaderV5 {
    pub prefix: RawHeaderPrefix,
    pub compression: [U32Be; 4],
    pub logical_bytes: U64Be,
    pub map_offset: U64Be,
    pub meta_offset: U64Be,
    pub hunk_bytes: U32Be,
    pub unit_bytes: U32Be,
    pub raw_sha1: [u8; SHA1_BYTES],
    pub sha1: [u8; SHA1_BYTES],
    pub parent_sha1: [u8; SHA1_BYTES],
}

const _: () = assert!(size_of::<RawHeaderV1>() == CHD_V1_HEADER_SIZE as usize);
const _: () = assert!(size_of::<RawHeaderV2>() == CHD_V2_HEADER_SIZE as usize);
const _: () = assert!(size_of::<RawHeaderV3>() == CHD_V3_HEADER_SIZE as usize);
const _: () = assert!(size_of::<RawHeaderV4>() == CHD_V4_HEADER_SIZE as usize);
const _: () = assert!(size_of::<RawHeaderV5>() == CHD_V5_HEADER_SIZE as usize);
