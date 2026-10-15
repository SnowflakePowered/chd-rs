//! Types and methods relating to header data for a CHD file.
//!
//! CHD V1-5 all have different header formats that are supported by this library.
//! Common information can be accessed with the [`Header`](crate::header::Header) enum,
//! but if version-specific information is needed, all fields in each version header struct
//! can be accessed publicly.
//!
//! [`Header`](crate::header::Header) makes no ABI guarantees and is not ABI-compatible
//! with [`libchdr::chd_header`](https://github.com/rtissera/libchdr/blob/6eeb6abc4adc094d489c8ba8cafdcff9ff61251b/include/libchdr/chd.h#L302).
use crate::chdfile::Codecs;
use crate::compression::codecs::{
    AVHuffCodec, CdFlacCodec, CdLzmaCodec, CdZlibCodec, CdZstdCodec, HuffmanCodec, LzmaCodec,
    NoneCodec, RawFlacCodec, ZlibCodec, ZstdCodec,
};
use crate::compression::{CodecImplementation, CompressionCodec};
use crate::error::{Error, Result};
use crate::metadata::{KnownMetadata, MetadataRefs, MetadataTag};
use crate::{make_tag, map};
use arrayvec::ArrayVec;
use num_derive::FromPrimitive;
use num_traits::FromPrimitive;
use raw::{RawHeaderPrefix, RawHeaderV1, RawHeaderV2, RawHeaderV3, RawHeaderV4, RawHeaderV5};
use std::io::{Read, Seek, SeekFrom};
use text_io::try_scan;
use zerocopy::{FromBytes, Immutable, KnownLayout};

mod raw;

/// The types of compression codecs supported in a CHD file.
#[repr(u32)]
#[derive(FromPrimitive, Debug, Copy, Clone, PartialEq, Eq)]
pub enum CodecType {
    /// No compression.
    None = 0,
    /// V1-4 Zlib DEFLATE compression.
    Zlib = 1,
    /// V1-4 Zlib+ DEFLATE compression.
    ZlibPlus = 2,
    /// V1-4 AV Huffman compression.
    AV = 3,
    /// V5 Zlib DEFLATE compression.
    ZLibV5 = make_tag(b"zlib"),
    /// V5 CD Zlib DEFLATE compression (cdzl)
    ZLibCdV5 = make_tag(b"cdzl"),
    /// V5 CD LZMA compression (cdlz)
    LzmaCdV5 = make_tag(b"cdlz"),
    /// V5 CD FLAC compression (cdfl)
    FlacCdV5 = make_tag(b"cdfl"),
    /// V5 FLAC compression (flac)
    FlacV5 = make_tag(b"flac"),
    /// V5 LZMA compression (lzma)
    LzmaV5 = make_tag(b"lzma"),
    /// V5 AV/Huffman compression (avhu)
    AVHuffV5 = make_tag(b"avhu"),
    /// V5 Huffman compression (huff)
    HuffV5 = make_tag(b"huff"),
    /// V5 Zstandard compression (zstd)
    ZstdV5 = make_tag(b"zstd"),
    /// V5 Zstandard CD compression (cdzs)
    ZstdCdV5 = make_tag(b"cdzs"),
}

impl CodecType {
    /// Returns whether or not the codec type is a legacy (V1-4) codec or a V5 codec.
    pub const fn is_legacy(&self) -> bool {
        match self {
            CodecType::None => false,
            CodecType::Zlib | CodecType::ZlibPlus | CodecType::AV => true,
            _ => false,
        }
    }

    /// Initializes the codec for the provided hunk size.
    pub(crate) fn init(&self, hunk_size: u32) -> Result<Box<dyn CompressionCodec>> {
        match self {
            CodecType::None => {
                NoneCodec::new(hunk_size).map(|x| Box::new(x) as Box<dyn CompressionCodec>)
            }
            CodecType::Zlib | CodecType::ZlibPlus | CodecType::ZLibV5 => {
                ZlibCodec::new(hunk_size).map(|x| Box::new(x) as Box<dyn CompressionCodec>)
            }
            CodecType::ZLibCdV5 => {
                CdZlibCodec::new(hunk_size).map(|x| Box::new(x) as Box<dyn CompressionCodec>)
            }
            CodecType::LzmaCdV5 => {
                CdLzmaCodec::new(hunk_size).map(|x| Box::new(x) as Box<dyn CompressionCodec>)
            }
            CodecType::FlacCdV5 => {
                CdFlacCodec::new(hunk_size).map(|x| Box::new(x) as Box<dyn CompressionCodec>)
            }
            CodecType::LzmaV5 => {
                <LzmaCodec>::new(hunk_size).map(|x| Box::new(x) as Box<dyn CompressionCodec>)
            }
            CodecType::FlacV5 => {
                RawFlacCodec::new(hunk_size).map(|x| Box::new(x) as Box<dyn CompressionCodec>)
            }
            CodecType::HuffV5 => {
                HuffmanCodec::new(hunk_size).map(|x| Box::new(x) as Box<dyn CompressionCodec>)
            }
            CodecType::AV | CodecType::AVHuffV5 => {
                AVHuffCodec::new(hunk_size).map(|x| Box::new(x) as Box<dyn CompressionCodec>)
            }
            CodecType::ZstdV5 => {
                ZstdCodec::new(hunk_size).map(|x| Box::new(x) as Box<dyn CompressionCodec>)
            }
            CodecType::ZstdCdV5 => {
                CdZstdCodec::new(hunk_size).map(|x| Box::new(x) as Box<dyn CompressionCodec>)
            }
            #[allow(unreachable_patterns)]
            _ => Err(Error::UnsupportedFormat),
        }
    }
}

/// The CHD header version.
#[repr(u32)]
#[derive(Copy, Clone)]
pub enum Version {
    /// CHD version 1.
    ChdV1 = 1,
    /// CHD version 2.
    ChdV2 = 2,
    /// CHD version 3.
    ChdV3 = 3,
    /// CHD version 4.
    ChdV4 = 4,
    /// CHD version 5
    ChdV5 = 5,
}

/// A CHD V1/V2 header. V1 and V2 headers share a similar format with the only difference being
/// V1 having a fixed 512-byte sector length, and V2 having an arbitrary sector length.
///
/// While all members of this struct are public, prefer the [`Header`](crate::header::Header) API over the fields
/// of this struct.
#[derive(Clone)]
pub struct HeaderV1 {
    /// The CHD version (1, or 2).
    pub version: Version,
    /// The length of the header.
    pub length: u32,
    /// CHD file flags.
    pub flags: u32,
    /// The compression codec used in the CHD file. See [`CodecType`](crate::header::CodecType) for the
    /// valid codec types supported by this library.
    pub compression: u32,
    /// The size of each hunk in the CHD file in units of sector length.
    pub hunk_size: u32,
    /// The total number of hunks in the CHD file.
    pub total_hunks: u32,
    /// The number of cylinders on the hard disk.
    pub cylinders: u32,
    /// The number of sectors on the hard disk.
    pub sectors: u32,
    /// The number of heads on the hard disk.
    pub heads: u32,
    /// The size of each hunk in the CHD file in bytes.
    pub hunk_bytes: u32,
    /// The MD5 hash of the CHD file.
    pub md5: [u8; MD5_BYTES],
    /// The MD5 hash of the parent CHD file.
    pub parent_md5: [u8; MD5_BYTES],
    /// The size of each unit in the CHD file in bytes.
    pub unit_bytes: u32,
    /// The number of units in each hunk.
    pub unit_count: u64,
    /// The logical size of the compressed data in bytes.
    pub logical_bytes: u64,
    /// The length of each sector on the hard disk.
    /// For V1 CHD files, this will be 512. For V2,
    /// CHD files, this can be arbitrary.
    pub sector_length: u32,
}

/// A CHD V3 header.
///
/// While all members of this struct are public, prefer the [`Header`](crate::header::Header) API over the fields
/// of this struct.
#[derive(Clone)]
pub struct HeaderV3 {
    /// The CHD version (3).
    pub version: Version,
    /// The length of the header.
    pub length: u32,
    /// CHD file flags.
    pub flags: u32,
    /// The compression codec used in the CHD file. See [`CodecType`](crate::header::CodecType) for the
    /// valid codec types supported by this library.
    pub compression: u32,
    /// The size of each hunk of the CHD file in bytes.
    pub hunk_bytes: u32,
    /// The total number of hunks in the CHD file.
    pub total_hunks: u32,
    /// The logical size of the compressed data in bytes.
    pub logical_bytes: u64,
    /// The offset in the stream where the CHD metadata section begins.
    pub meta_offset: u64,
    /// The MD5 hash of the CHD file.
    pub md5: [u8; MD5_BYTES],
    /// The MD5 hash of the parent CHD file.
    pub parent_md5: [u8; MD5_BYTES],
    /// The SHA1 hash of the CHD file.
    pub sha1: [u8; SHA1_BYTES],
    /// The SHA1 hash of the parent CHD file.
    pub parent_sha1: [u8; SHA1_BYTES],
    /// The size of each unit in bytes.
    pub unit_bytes: u32,
    /// The number of units in each hunk.
    pub unit_count: u64,
}

/// A CHD V4 header. The major difference between a V3 header and V4 header is the absence of MD5
/// hash information in CHD V4.
///
/// While all members of this struct are public, prefer the [`Header`](crate::header::Header) API over the fields
/// of this struct.
#[derive(Clone)]
pub struct HeaderV4 {
    /// The CHD version (4).
    pub version: Version,
    /// The length of the header.
    pub length: u32,
    /// CHD file flags.
    pub flags: u32,
    /// The compression codec used in the CHD file. See [`CodecType`](crate::header::CodecType) for the
    /// valid codec types supported by this library.
    pub compression: u32,
    /// The total number of hunks in the CHD file.
    pub total_hunks: u32,
    /// The logical size of the compressed data in bytes.
    pub logical_bytes: u64,
    /// The offset in the stream where the CHD metadata section begins.
    pub meta_offset: u64,
    /// The size of each hunk in the CHD file in bytes.
    pub hunk_bytes: u32,
    /// The SHA1 hash of the CHD file.
    pub sha1: [u8; SHA1_BYTES],
    /// The SHA1 hash of the parent CHD file.
    pub parent_sha1: [u8; SHA1_BYTES],
    /// The SHA1 hash of the raw, uncompressed data.
    pub raw_sha1: [u8; SHA1_BYTES],
    /// The size of each unit in bytes.
    pub unit_bytes: u32,
    /// The number of units in each hunk.
    pub unit_count: u64,
}

/// A CHD V5 header.
///
/// While all members of this struct are public, prefer the [`Header`](crate::header::Header) API over the fields
/// of this struct.
#[derive(Clone)]
pub struct HeaderV5 {
    /// The CHD version (5).
    pub version: Version,
    /// The length of the header.
    pub length: u32,
    /// The compression codec used in the CHD file. CHD V5 supports up to 4 different codecs in a
    /// single CHD file.
    ///
    /// See [`CodecType`](crate::header::CodecType) for the
    /// valid codec types supported by this library.
    pub compression: [u32; 4],
    /// The logical size of the compressed data in bytes.
    pub logical_bytes: u64,
    /// The offset in the stream where the CHD map section begins.
    pub map_offset: u64,
    /// The offset in the stream where the CHD metadata section begins.
    pub meta_offset: u64,
    /// The size of each hunk in the CHD file in bytes.
    pub hunk_bytes: u32,
    /// The size of each unit in bytes.
    pub unit_bytes: u32,
    /// The SHA1 hash of the CHD file.
    pub sha1: [u8; SHA1_BYTES],
    /// The SHA1 hash of the parent CHD file.
    pub parent_sha1: [u8; SHA1_BYTES],
    /// The SHA1 hash of the raw, uncompressed data.
    pub raw_sha1: [u8; SHA1_BYTES],
    /// The number of units in each hunk.
    pub unit_count: u64,
    /// The total number of hunks in the CHD file.
    pub hunk_count: u32,
    /// The size of each map entry in bytes.
    pub map_entry_bytes: u32,
}

/// A CHD header of unspecified version.
#[derive(Clone)]
pub enum Header {
    /// A CHD V1 header.
    V1Header(HeaderV1),
    /// A CHD V2 header.
    V2Header(HeaderV1),
    /// A CHD V3 header.
    V3Header(HeaderV3),
    /// A CHD V4 header.
    V4Header(HeaderV4),
    /// A CHD V5 header.
    V5Header(HeaderV5),
}

const MD5_BYTES: usize = 16;
const SHA1_BYTES: usize = 20;

/// The CHD magic number.
pub const CHD_MAGIC: &str = "MComprHD";

const CHD_V1_HEADER_SIZE: u32 = 76;
const CHD_V2_HEADER_SIZE: u32 = 80;
const CHD_V3_HEADER_SIZE: u32 = 120;
const CHD_V4_HEADER_SIZE: u32 = 108;
const CHD_V5_HEADER_SIZE: u32 = 124;

const CHD_MAX_HEADER_SIZE: usize = CHD_V5_HEADER_SIZE as usize;
// pub const COOKIE_VALUE: u32 = 0xbaadf00d;

impl Header {
    /// Reads CHD header data from the provided stream.
    ///
    /// If the header is not valid, returns `Error::InvalidParameter`.
    /// If the header indicates an unsupported compression format, returns `Error::UnsupportedFormat`
    pub fn try_read_header<F: Read + Seek>(file: &mut F) -> Result<Header> {
        let header = read_header(file)?;
        if !header.validate() {
            return Err(Error::InvalidParameter);
        }
        if !header.validate_compression() {
            return Err(Error::UnsupportedFormat);
        }
        Ok(header)
    }

    /// Serialize the header into it's on-disk representation.
    ///
    /// The length of the serialized header depends on the header version.
    #[cfg(feature = "encode")]
    pub fn to_bytes(&self) -> ArrayVec<u8, CHD_MAX_HEADER_SIZE> {
        let mut bytes = ArrayVec::new();
        match self {
            Header::V1Header(c) | Header::V2Header(c) => bytes.extend(c.to_bytes()),
            Header::V3Header(c) => bytes.extend(c.to_bytes()),
            Header::V4Header(c) => bytes.extend(c.to_bytes()),
            Header::V5Header(c) => bytes.extend(c.to_bytes()),
        }
        bytes
    }

    /// Returns whether or not the CHD file is compressed.
    pub fn is_compressed(&self) -> bool {
        match self {
            Header::V1Header(c) => c.compression != CodecType::None as u32,
            Header::V2Header(c) => c.compression != CodecType::None as u32,
            Header::V3Header(c) => c.compression != CodecType::None as u32,
            Header::V4Header(c) => c.compression != CodecType::None as u32,
            Header::V5Header(c) => c.compression[0] != CodecType::None as u32,
        }
    }

    /// Returns the offset of the CHD metadata, if available.
    pub fn meta_offset(&self) -> Option<u64> {
        match self {
            Header::V1Header(_c) => None,
            Header::V2Header(_c) => None,
            Header::V3Header(c) => Some(c.meta_offset),
            Header::V4Header(c) => Some(c.meta_offset),
            Header::V5Header(c) => Some(c.meta_offset),
        }
    }

    /// Returns the flags of the CHD file, if available.
    pub fn flags(&self) -> Option<u32> {
        match self {
            Header::V1Header(c) => Some(c.flags),
            Header::V2Header(c) => Some(c.flags),
            Header::V3Header(c) => Some(c.flags),
            Header::V4Header(c) => Some(c.flags),
            Header::V5Header(_c) => None,
        }
    }

    /// Returns the total number of hunks in the CHD file.
    pub fn hunk_count(&self) -> u32 {
        match self {
            Header::V1Header(c) => c.total_hunks,
            Header::V2Header(c) => c.total_hunks,
            Header::V3Header(c) => c.total_hunks,
            Header::V4Header(c) => c.total_hunks,
            Header::V5Header(c) => c.hunk_count,
        }
    }

    /// Returns the size of each hunk in the CHD file in bytes.
    pub fn hunk_size(&self) -> u32 {
        match self {
            Header::V1Header(c) => c.hunk_bytes,
            Header::V2Header(c) => c.hunk_bytes,
            Header::V3Header(c) => c.hunk_bytes,
            Header::V4Header(c) => c.hunk_bytes,
            Header::V5Header(c) => c.hunk_bytes,
        }
    }

    /// Returns the logical size of the compressed data in bytes.
    pub fn logical_bytes(&self) -> u64 {
        match self {
            Header::V1Header(c) => c.logical_bytes,
            Header::V2Header(c) => c.logical_bytes,
            Header::V3Header(c) => c.logical_bytes,
            Header::V4Header(c) => c.logical_bytes,
            Header::V5Header(c) => c.logical_bytes,
        }
    }

    /// Returns the number of bytes per unit within each hunk.
    pub fn unit_bytes(&self) -> u32 {
        match self {
            Header::V1Header(c) => c.unit_bytes,
            Header::V2Header(c) => c.unit_bytes,
            Header::V3Header(c) => c.unit_bytes,
            Header::V4Header(c) => c.unit_bytes,
            Header::V5Header(c) => c.unit_bytes,
        }
    }

    /// Returns the number of units per hunk.
    pub fn unit_count(&self) -> u64 {
        match self {
            Header::V1Header(c) => c.unit_count,
            Header::V2Header(c) => c.unit_count,
            Header::V3Header(c) => c.unit_count,
            Header::V4Header(c) => c.unit_count,
            Header::V5Header(c) => c.unit_count,
        }
    }

    /// Returns whether or not this CHD file has a parent.
    pub fn has_parent(&self) -> bool {
        match self {
            Header::V5Header(c) => c.parent_sha1 != [0u8; SHA1_BYTES],
            _ => self
                .flags()
                .map(|f| (f & Flags::HasParent as u32) != 0)
                .unwrap_or(false),
        }
    }

    /// Returns the CHD header version.
    pub fn version(&self) -> Version {
        match self {
            Header::V1Header(c) => c.version,
            Header::V2Header(c) => c.version,
            Header::V3Header(c) => c.version,
            Header::V4Header(c) => c.version,
            Header::V5Header(c) => c.version,
        }
    }

    /// Returns the SHA1 of the CHD file if available.
    pub fn sha1(&self) -> Option<[u8; SHA1_BYTES]> {
        match self {
            Header::V3Header(c) => Some(c.sha1),
            Header::V4Header(c) => Some(c.sha1),
            Header::V5Header(c) => Some(c.sha1),
            _ => None,
        }
    }

    /// Returns the SHA1 of the parent of the CHD if available.
    pub fn parent_sha1(&self) -> Option<[u8; SHA1_BYTES]> {
        match self {
            Header::V3Header(c) => Some(c.parent_sha1),
            Header::V4Header(c) => Some(c.parent_sha1),
            Header::V5Header(c) => Some(c.parent_sha1),
            _ => None,
        }
    }

    /// Returns the raw (hunk data only) SHA1 of the CHD file if available.
    pub fn raw_sha1(&self) -> Option<[u8; SHA1_BYTES]> {
        match self {
            Header::V4Header(c) => Some(c.raw_sha1),
            Header::V5Header(c) => Some(c.raw_sha1),
            _ => None,
        }
    }

    /// Returns the MD5 of the CHD file if available.
    pub fn md5(&self) -> Option<[u8; MD5_BYTES]> {
        match self {
            Header::V1Header(c) => Some(c.md5),
            Header::V2Header(c) => Some(c.md5),
            Header::V3Header(c) => Some(c.md5),
            _ => None,
        }
    }

    /// Returns the MD5 of the parent CHD file if available.
    pub fn parent_md5(&self) -> Option<[u8; MD5_BYTES]> {
        match self {
            Header::V1Header(c) => Some(c.parent_md5),
            Header::V2Header(c) => Some(c.parent_md5),
            Header::V3Header(c) => Some(c.parent_md5),
            _ => None,
        }
    }

    /// Returns the length of the header.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> u32 {
        match self {
            Header::V1Header(c) => c.length,
            Header::V2Header(c) => c.length,
            Header::V3Header(c) => c.length,
            Header::V4Header(c) => c.length,
            Header::V5Header(c) => c.length,
        }
    }

    pub(crate) fn create_compression_codecs(&self) -> Result<Codecs> {
        match self {
            Header::V1Header(c) => CodecType::from_u32(c.compression)
                .map(|e| (e.init(self.hunk_size())))
                .ok_or(Error::UnsupportedFormat)?
                .map(|e| Codecs::Single(e)),
            Header::V2Header(c) => CodecType::from_u32(c.compression)
                .map(|e| e.init(self.hunk_size()))
                .ok_or(Error::UnsupportedFormat)?
                .map(|e| Codecs::Single(e)),
            Header::V3Header(c) => CodecType::from_u32(c.compression)
                .map(|e| e.init(self.hunk_size()))
                .ok_or(Error::UnsupportedFormat)?
                .map(|e| Codecs::Single(e)),
            Header::V4Header(c) => CodecType::from_u32(c.compression)
                .map(|e| e.init(self.hunk_size()))
                .ok_or(Error::UnsupportedFormat)?
                .map(|e| Codecs::Single(e)),
            Header::V5Header(c) => {
                let array = c
                    .compression
                    .map(CodecType::from_u32)
                    .map(|f| f.ok_or(Error::UnsupportedFormat))
                    .map(|f| f.and_then(|f| f.init(self.hunk_size())))
                    .into_iter()
                    .collect::<Result<ArrayVec<Box<dyn CompressionCodec>, 4>>>()?;
                Ok(Codecs::Four(
                    array.into_inner().map_err(|_| Error::InvalidFile)?,
                ))
            }
        }
    }

    /// Validate the header.
    fn validate(&self) -> bool {
        let length_valid = match self {
            Header::V1Header(c) => c.length == CHD_V1_HEADER_SIZE,
            Header::V2Header(c) => c.length == CHD_V2_HEADER_SIZE,
            Header::V3Header(c) => c.length == CHD_V3_HEADER_SIZE,
            Header::V4Header(c) => c.length == CHD_V4_HEADER_SIZE,
            Header::V5Header(c) => c.length == CHD_V5_HEADER_SIZE,
        };

        if !length_valid {
            return false;
        }

        // Do not validate V5 header
        if let Header::V5Header(_) = self {
            return true;
        }

        // Require valid flags
        if let Some(flags) = self.flags() {
            if flags & Flags::Undefined as u32 != 0 {
                return false;
            }
        }

        // require valid hunk size
        if self.hunk_size() == 0 || self.hunk_size() >= 65536 * 256 {
            return false;
        }

        // require valid hunk count
        if self.hunk_count() == 0 {
            return false;
        }

        // if we use a parent make sure we have valid md5
        let parent_ok = if self.has_parent() {
            match self {
                Header::V1Header(c) => c.parent_md5 != [0u8; MD5_BYTES],
                Header::V2Header(c) => c.parent_md5 != [0u8; MD5_BYTES],
                Header::V3Header(c) => {
                    c.parent_md5 != [0u8; MD5_BYTES] && c.parent_sha1 != [0u8; SHA1_BYTES]
                }
                Header::V4Header(c) => c.parent_sha1 != [0u8; SHA1_BYTES],
                Header::V5Header(_) => true,
            }
        } else {
            true
        };

        if !parent_ok {
            return false;
        }
        // obsolete field checks are done by type system
        true
    }

    /// Validate the compression types of the CHD file can be read.
    fn validate_compression(&self) -> bool {
        match self {
            Header::V1Header(c) => Header::validate_legacy_compression(c.compression),
            Header::V2Header(c) => Header::validate_legacy_compression(c.compression),
            Header::V3Header(c) => Header::validate_legacy_compression(c.compression),
            Header::V4Header(c) => Header::validate_legacy_compression(c.compression),
            Header::V5Header(c) => c
                .compression
                .map(Header::validate_v5_compression)
                .iter()
                .all(|&x| x),
        }
    }

    /// Validate compression for V1-4 CHD headers.
    fn validate_legacy_compression(value: u32) -> bool {
        CodecType::from_u32(value)
            .map(|e| e.is_legacy())
            .unwrap_or(false)
    }

    /// Validate compression for V5 CHD headers.
    fn validate_v5_compression(value: u32) -> bool {
        // v5 can not be legacy.
        CodecType::from_u32(value)
            .map(|e| !e.is_legacy())
            .unwrap_or(false)
    }
}

#[cfg(feature = "encode")]
impl RawHeaderPrefix {
    fn new(version: Version, length: u32) -> Self {
        let mut magic = [0u8; 8];
        magic.copy_from_slice(CHD_MAGIC.as_bytes());
        RawHeaderPrefix {
            magic,
            length: length.into(),
            version: (version as u32).into(),
        }
    }
}

#[cfg(feature = "encode")]
impl HeaderV1 {
    /// Serialize the header into it's on-disk representation.
    ///
    /// V2 headers are 4 bytes longer than V1 headers to store the sector length.
    pub fn to_bytes(&self) -> ArrayVec<u8, { CHD_V2_HEADER_SIZE as usize }> {
        use zerocopy::IntoBytes;

        let length = match self.version {
            Version::ChdV1 => CHD_V1_HEADER_SIZE,
            // `HeaderV1` is only used for V1 and V2 headers.
            _ => CHD_V2_HEADER_SIZE,
        };
        let v1 = RawHeaderV1 {
            prefix: RawHeaderPrefix::new(self.version, length),
            flags: self.flags.into(),
            compression: self.compression.into(),
            hunk_size: self.hunk_size.into(),
            total_hunks: self.total_hunks.into(),
            cylinders: self.cylinders.into(),
            heads: self.heads.into(),
            sectors: self.sectors.into(),
            md5: self.md5,
            parent_md5: self.parent_md5,
        };

        let mut bytes = ArrayVec::new();
        match self.version {
            Version::ChdV1 => bytes.extend(v1.as_bytes().iter().copied()),
            _ => {
                let v2 = RawHeaderV2 {
                    v1,
                    sector_length: self.sector_length.into(),
                };
                bytes.extend(v2.as_bytes().iter().copied())
            }
        }
        bytes
    }
}

#[cfg(feature = "encode")]
impl HeaderV3 {
    /// Serialize the header into it's on-disk representation.
    pub fn to_bytes(&self) -> [u8; CHD_V3_HEADER_SIZE as usize] {
        let header = RawHeaderV3 {
            prefix: RawHeaderPrefix::new(Version::ChdV3, CHD_V3_HEADER_SIZE),
            flags: self.flags.into(),
            compression: self.compression.into(),
            total_hunks: self.total_hunks.into(),
            logical_bytes: self.logical_bytes.into(),
            meta_offset: self.meta_offset.into(),
            md5: self.md5,
            parent_md5: self.parent_md5,
            hunk_bytes: self.hunk_bytes.into(),
            sha1: self.sha1,
            parent_sha1: self.parent_sha1,
        };
        zerocopy::transmute!(header)
    }
}

#[cfg(feature = "encode")]
impl HeaderV4 {
    /// Serialize the header into it's on-disk representation.
    pub fn to_bytes(&self) -> [u8; CHD_V4_HEADER_SIZE as usize] {
        let header = RawHeaderV4 {
            prefix: RawHeaderPrefix::new(Version::ChdV4, CHD_V4_HEADER_SIZE),
            flags: self.flags.into(),
            compression: self.compression.into(),
            total_hunks: self.total_hunks.into(),
            logical_bytes: self.logical_bytes.into(),
            meta_offset: self.meta_offset.into(),
            hunk_bytes: self.hunk_bytes.into(),
            sha1: self.sha1,
            parent_sha1: self.parent_sha1,
            raw_sha1: self.raw_sha1,
        };
        zerocopy::transmute!(header)
    }
}

#[cfg(feature = "encode")]
impl HeaderV5 {
    /// Serialize the header into it's on-disk representation.
    pub fn to_bytes(&self) -> [u8; CHD_V5_HEADER_SIZE as usize] {
        let header = RawHeaderV5 {
            prefix: RawHeaderPrefix::new(Version::ChdV5, CHD_V5_HEADER_SIZE),
            compression: self.compression.map(Into::into),
            logical_bytes: self.logical_bytes.into(),
            map_offset: self.map_offset.into(),
            meta_offset: self.meta_offset.into(),
            hunk_bytes: self.hunk_bytes.into(),
            unit_bytes: self.unit_bytes.into(),
            raw_sha1: self.raw_sha1,
            sha1: self.sha1,
            parent_sha1: self.parent_sha1,
        };
        zerocopy::transmute!(header)
    }
}

/// CHD flags for legacy V1-4 headers.
#[repr(u32)]
pub enum Flags {
    /// This CHD file has a parent.
    HasParent = 0x00000001,

    /// This CHD file is writable.
    IsWritable = 0x00000002,

    /// Undefined.
    Undefined = 0xfffffffc,
}

/// View the start of the raw header as the layout of a header version.
fn raw<T: FromBytes + KnownLayout + Immutable>(raw_header: &[u8]) -> Result<&T> {
    T::ref_from_prefix(raw_header)
        .map(|(raw, _)| raw)
        .map_err(|_| Error::InvalidData)
}

fn read_header<T: Read + Seek>(chd: &mut T) -> Result<Header> {
    let mut raw_header: [u8; CHD_MAX_HEADER_SIZE] = [0; CHD_MAX_HEADER_SIZE];

    chd.seek(SeekFrom::Start(0))?;
    chd.read_exact(&mut raw_header)?;

    let prefix: &RawHeaderPrefix = raw(&raw_header)?;
    if prefix.magic != CHD_MAGIC.as_bytes() {
        return Err(Error::InvalidData);
    }

    // ensure version is known and header size match up
    match (prefix.version.get(), prefix.length.get()) {
        (1, CHD_V1_HEADER_SIZE) => Ok(Header::V1Header(read_v1_header(raw(&raw_header)?, None)?)),
        (2, CHD_V2_HEADER_SIZE) => {
            let v2: &RawHeaderV2 = raw(&raw_header)?;
            Ok(Header::V2Header(read_v1_header(
                &v2.v1,
                Some(v2.sector_length.get()),
            )?))
        }
        (3, CHD_V3_HEADER_SIZE) => Ok(Header::V3Header(read_v3_header(raw(&raw_header)?, chd)?)),
        (4, CHD_V4_HEADER_SIZE) => Ok(Header::V4Header(read_v4_header(raw(&raw_header)?, chd)?)),
        (5, CHD_V5_HEADER_SIZE) => Ok(Header::V5Header(read_v5_header(raw(&raw_header)?)?)),
        (1 | 2 | 3 | 4 | 5, _) => Err(Error::InvalidData),
        _ => Err(Error::UnsupportedVersion),
    }
}

/// Read a V1 header, or a V2 header with its sector length.
fn read_v1_header(header: &RawHeaderV1, sector_length: Option<u32>) -> Result<HeaderV1> {
    // V1 headers have a fixed sector size.
    const CHD_V1_SECTOR_SIZE: u32 = 512;
    let sector_length = sector_length.unwrap_or(CHD_V1_SECTOR_SIZE);

    let version = header.prefix.version.get();
    let length = header.prefix.length.get();
    let flags = header.flags.get();
    let compression = header.compression.get();
    let hunk_size = header.hunk_size.get();
    let total_hunks = header.total_hunks.get();
    let cylinders = header.cylinders.get();
    let heads = header.heads.get();
    let sectors = header.sectors.get();
    let md5 = header.md5;
    let parent_md5 = header.parent_md5;

    let logical_bytes =
        (cylinders as u64) * (heads as u64) * (sectors as u64) * (sector_length as u64);

    // verify assumptions about hunk sizes.
    let hunk_bytes: u32 =
        u32::try_from(sector_length as u64 * hunk_size as u64).map_err(|_| Error::InvalidData)?;
    if hunk_bytes == 0 || hunk_size == 0 {
        return Err(Error::InvalidData);
    }

    let unit_bytes = hunk_bytes / hunk_size;
    let unit_count = (logical_bytes + unit_bytes as u64 - 1) / unit_bytes as u64;
    Ok(HeaderV1 {
        version: match version {
            1 => Version::ChdV1,
            2 => Version::ChdV2,
            _ => return Err(Error::UnsupportedVersion),
        },
        length,
        flags,
        compression,
        hunk_bytes,
        md5,
        total_hunks,
        unit_bytes,
        unit_count,
        cylinders,
        sectors,
        heads,
        hunk_size,
        parent_md5,
        logical_bytes,
        sector_length,
    })
}

fn read_v3_header<F: Read + Seek>(header: &RawHeaderV3, chd: &mut F) -> Result<HeaderV3> {
    let length = header.prefix.length.get();
    let flags = header.flags.get();
    let compression = header.compression.get();
    let total_hunks = header.total_hunks.get();
    let logical_bytes = header.logical_bytes.get();
    let meta_offset = header.meta_offset.get();
    let md5 = header.md5;
    let parent_md5 = header.parent_md5;
    let hunk_bytes = header.hunk_bytes.get();
    let sha1 = header.sha1;
    let parent_sha1 = header.parent_sha1;
    let unit_bytes = guess_unit_bytes(chd, meta_offset).unwrap_or(hunk_bytes);
    let unit_count = (logical_bytes + (unit_bytes as u64) - 1) / unit_bytes as u64;
    Ok(HeaderV3 {
        version: Version::ChdV3,
        length,
        flags,
        compression,
        hunk_bytes,
        sha1,
        total_hunks,
        logical_bytes,
        meta_offset,
        md5,
        parent_md5,
        unit_bytes,
        unit_count,
        parent_sha1,
    })
}

fn read_v4_header<F: Read + Seek>(header: &RawHeaderV4, chd: &mut F) -> Result<HeaderV4> {
    let length = header.prefix.length.get();
    let flags = header.flags.get();
    let compression = header.compression.get();
    let total_hunks = header.total_hunks.get();
    let logical_bytes = header.logical_bytes.get();
    let meta_offset = header.meta_offset.get();
    let hunk_bytes = header.hunk_bytes.get();
    let sha1 = header.sha1;
    let parent_sha1 = header.parent_sha1;
    let raw_sha1 = header.raw_sha1;

    let unit_bytes = guess_unit_bytes(chd, meta_offset).unwrap_or(hunk_bytes);
    let unit_count = (logical_bytes + unit_bytes as u64 - 1) / unit_bytes as u64;
    Ok(HeaderV4 {
        version: Version::ChdV4,
        length,
        flags,
        compression,
        hunk_bytes,
        total_hunks,
        logical_bytes,
        meta_offset,
        sha1,
        raw_sha1,
        parent_sha1,
        unit_bytes,
        unit_count,
    })
}

fn extract_bps_value(bps_meta: &[u8]) -> Option<u32> {
    fn extract_bps_inner(bps_meta: &[u8]) -> std::result::Result<u32, Box<text_io::Error>> {
        let cyls: u32;
        let heads: u32;
        let secs: u32;
        let bps: u32;
        try_scan!(bps_meta.iter().copied() => "CYLS:{},HEADS:{},SECS:{},BPS:{}\0", cyls, heads, secs, bps);
        Ok(bps)
    }
    extract_bps_inner(bps_meta).ok()
}

fn guess_unit_bytes<F: Read + Seek>(chd: &mut F, off: u64) -> Option<u32> {
    let metas: Vec<_> = MetadataRefs::from_stream(chd, off).collect();
    if let Some(hard_disk) = metas
        .iter()
        .find(|&e| e.metatag() == KnownMetadata::HardDisk as u32)
    {
        if let Ok(text) = hard_disk.read(chd) {
            let bps = extract_bps_value(&text.value);
            // Only return this if we can parse it properly. Fallback to cdrom otherwise.
            if let Some(bps) = bps {
                return Some(bps);
            }
        }
    }

    if metas.iter().any(|e| KnownMetadata::is_cdrom(e.metatag())) {
        return Some(crate::cdrom::CD_FRAME_SIZE as u32);
    }
    None
}

fn read_v5_header(header: &RawHeaderV5) -> Result<HeaderV5> {
    let length = header.prefix.length.get();
    let compression = header.compression.map(|codec| codec.get());
    let logical_bytes = header.logical_bytes.get();
    let map_offset = header.map_offset.get();
    let meta_offset = header.meta_offset.get();
    let hunk_bytes = header.hunk_bytes.get();
    let unit_bytes = header.unit_bytes.get();
    let sha1 = header.sha1;
    let parent_sha1 = header.parent_sha1;
    let raw_sha1 = header.raw_sha1;

    // guard divide by zero
    if hunk_bytes == 0 || unit_bytes == 0 {
        return Err(Error::InvalidData);
    }

    let hunk_count = ((logical_bytes + hunk_bytes as u64 - 1) / hunk_bytes as u64) as u32;
    let unit_count = (logical_bytes + unit_bytes as u64 - 1) / unit_bytes as u64;
    let map_entry_bytes = match CodecType::from_u32(compression[0]) {
        // uncompressed map entries are 4 bytes long
        Some(CodecType::None) => map::V5_UNCOMPRESSED_MAP_ENTRY_SIZE as u32,
        Some(_) => map::V5_COMPRESSED_MAP_ENTRY_SIZE as u32,
        None => return Err(Error::UnsupportedFormat),
    };

    Ok(HeaderV5 {
        version: Version::ChdV5,
        length,
        compression,
        hunk_bytes,
        logical_bytes,
        meta_offset,
        map_offset,
        sha1,
        raw_sha1,
        parent_sha1,
        unit_bytes,
        unit_count,
        hunk_count,
        map_entry_bytes,
    })
}

#[cfg(test)]
mod test {
    use crate::header::extract_bps_value;

    #[test]
    #[cfg(feature = "encode")]
    fn v5_to_bytes_roundtrip() {
        use crate::header::Header;
        use std::io::Cursor;

        let mut raw = Vec::new();
        raw.extend_from_slice(b"MComprHD");
        raw.extend_from_slice(&124u32.to_be_bytes());
        raw.extend_from_slice(&5u32.to_be_bytes());
        for codec in [b"cdlz", b"cdzl", b"cdfl", &[0u8; 4]] {
            raw.extend_from_slice(codec);
        }
        raw.extend_from_slice(&657885312u64.to_be_bytes()); // logical bytes
        raw.extend_from_slice(&464000000u64.to_be_bytes()); // map offset
        raw.extend_from_slice(&124u64.to_be_bytes()); // metadata offset
        raw.extend_from_slice(&19584u32.to_be_bytes()); // hunk bytes
        raw.extend_from_slice(&2448u32.to_be_bytes()); // unit bytes
        for seed in [0x10u8, 0x20, 0x30] {
            // raw SHA1, SHA1, parent SHA1
            raw.extend((0..20).map(|i| seed + i));
        }
        assert_eq!(raw.len(), 124);

        let Header::V5Header(header) = Header::try_read_header(&mut Cursor::new(&raw)).unwrap()
        else {
            panic!("not a V5 header");
        };
        assert_eq!(header.to_bytes()[..], raw[..]);
    }

    #[test]
    #[cfg(feature = "encode")]
    fn legacy_to_bytes_roundtrip() {
        use crate::header::Header;
        use std::io::Cursor;

        fn bytes(seed: u8, len: usize) -> impl Iterator<Item = u8> {
            (0..len as u8).map(move |i| seed.wrapping_add(i.wrapping_mul(7)))
        }

        let start = |raw: &mut Vec<u8>, length: u32, version: u32| {
            raw.extend_from_slice(b"MComprHD");
            raw.extend_from_slice(&length.to_be_bytes());
            raw.extend_from_slice(&version.to_be_bytes());
        };

        let mut headers = Vec::new();
        for version in [1u32, 2] {
            let mut raw = Vec::new();
            start(&mut raw, if version == 1 { 76 } else { 80 }, version);
            // flags (has parent), zlib, hunk size in sectors, total hunks, cylinders, heads, sectors
            for value in [1u32, 1, 8, 100, 10, 4, 20] {
                raw.extend_from_slice(&value.to_be_bytes());
            }
            raw.extend(bytes(0x11, 16)); // MD5
            raw.extend(bytes(0x22, 16)); // parent MD5
            if version == 2 {
                raw.extend_from_slice(&256u32.to_be_bytes()); // sector length
            }
            headers.push(raw);
        }

        let mut v3 = Vec::new();
        start(&mut v3, 120, 3);
        for value in [1u32, 2, 50] {
            // flags (has parent), zlib+, total hunks
            v3.extend_from_slice(&value.to_be_bytes());
        }
        v3.extend_from_slice(&(50u64 * 4096 - 100).to_be_bytes()); // logical bytes
        v3.extend_from_slice(&0u64.to_be_bytes()); // metadata offset
        v3.extend(bytes(0x33, 16)); // MD5
        v3.extend(bytes(0x44, 16)); // parent MD5
        v3.extend_from_slice(&4096u32.to_be_bytes()); // hunk bytes
        v3.extend(bytes(0x55, 20)); // SHA1
        v3.extend(bytes(0x66, 20)); // parent SHA1
        headers.push(v3);

        let mut v4 = Vec::new();
        start(&mut v4, 108, 4);
        for value in [1u32, 3, 60] {
            // flags (has parent), A/V, total hunks
            v4.extend_from_slice(&value.to_be_bytes());
        }
        v4.extend_from_slice(&(60u64 * 8192).to_be_bytes()); // logical bytes
        v4.extend_from_slice(&0u64.to_be_bytes()); // metadata offset
        v4.extend_from_slice(&8192u32.to_be_bytes()); // hunk bytes
        v4.extend(bytes(0x77, 20)); // SHA1
        v4.extend(bytes(0x88, 20)); // parent SHA1
        v4.extend(bytes(0x99, 20)); // raw SHA1
        headers.push(v4);

        for raw in headers {
            // The reader always reads the size of a V5 header.
            let mut padded = raw.clone();
            padded.resize(124, 0);
            let header = Header::try_read_header(&mut Cursor::new(&padded)).unwrap();
            assert_eq!(header.to_bytes()[..], raw[..], "version {}", raw[15]);
        }
    }

    #[test]
    fn extract_hard_drive_unit_bytes_test() {
        assert_eq!(Some(10), extract_bps_value(b"CYLS:2,HEADS:3,SECS:4,BPS:10"))
    }
}
