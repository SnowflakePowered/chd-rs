//! Types and methods relating to header data for a CHD file.
//!
//! CHD V1-5 all have different header formats that are supported by this library.
//! Common information can be accessed with the [`ChdHeader`](crate::header::ChdHeader) enum,
//! but if version-specific information is needed, all fields in each version header struct
//! can be accessed publicly.
//!
//! [`ChdHeader`](crate::header::ChdHeader) makes no ABI guarantees and is not ABI-compatible
//! with [`libchdr::chd_header`](https://github.com/rtissera/libchdr/blob/6eeb6abc4adc094d489c8ba8cafdcff9ff61251b/include/libchdr/chd.h#L302).
use crate::compression::codecs::{
    AVHuffCodec, CdFlacCodec, CdLzmaCodec, CdZlibCodec, HuffmanCodec, LzmaCodec, NoneCodec,
    RawFlacCodec, ZlibCodec,
};
use crate::compression::{CodecImplementation, CompressionCodec};
use crate::error::{ChdError, Result};
use crate::metadata::{KnownMetadata, MetadataRefIter};
use crate::{make_tag, map};
use byteorder::{BigEndian, ReadBytesExt};
use num_derive::FromPrimitive;
use num_traits::FromPrimitive;
use once_cell::sync::OnceCell;
use regex::bytes::Regex;
use std::ffi::CStr;
use std::io::{Cursor, Read, Seek, SeekFrom};

/// The types of compression codecs supported in a CHD file.
#[repr(u32)]
#[derive(FromPrimitive, Debug)]
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
    /// V5 Huffman compression
    HuffV5 = make_tag(b"huff"),
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
                LzmaCodec::new(hunk_size).map(|x| Box::new(x) as Box<dyn CompressionCodec>)
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
            #[allow(unreachable_patterns)]
            _ => Err(ChdError::UnsupportedFormat),
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
/// While all members of this struct are public, prefer the [`ChdHeader`](crate::header::ChdHeader) API over the fields
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
/// While all members of this struct are public, prefer the [`ChdHeader`](crate::header::ChdHeader) API over the fields
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
/// While all members of this struct are public, prefer the [`ChdHeader`](crate::header::ChdHeader) API over the fields
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
/// While all members of this struct are public, prefer the `ChdHeader` API over the fields
/// of this struct.
#[derive(Clone)]
pub struct HeaderV5 {
    /// The CHD version (4).
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
pub enum ChdHeader {
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
pub const CHD_MAGIC: &'static str = "MComprHD";

const CHD_V1_HEADER_SIZE: u32 = 76;
const CHD_V2_HEADER_SIZE: u32 = 80;
const CHD_V3_HEADER_SIZE: u32 = 120;
const CHD_V4_HEADER_SIZE: u32 = 108;
const CHD_V5_HEADER_SIZE: u32 = 124;

const CHD_MAX_HEADER_SIZE: usize = CHD_V5_HEADER_SIZE as usize;
// pub const COOKIE_VALUE: u32 = 0xbaadf00d;

impl ChdHeader {
    /// Reads CHD header data from the provided stream.
    ///
    /// If the header is not valid, returns `ChdError::InvalidParameter`.
    /// If the header indicates an unsupported compression format, returns `ChdError::UnsupportedFormat`
    pub fn try_read_header<F: Read + Seek>(file: &mut F) -> Result<ChdHeader> {
        let header = read_header(file)?;
        if !header.validate() {
            return Err(ChdError::InvalidParameter);
        }
        if !header.validate_compression() {
            return Err(ChdError::UnsupportedFormat);
        }
        Ok(header)
    }

    /// Returns whether or not the CHD file is compressed.
    pub const fn is_compressed(&self) -> bool {
        match self {
            ChdHeader::V1Header(c) => c.compression != CodecType::None as u32,
            ChdHeader::V2Header(c) => c.compression != CodecType::None as u32,
            ChdHeader::V3Header(c) => c.compression != CodecType::None as u32,
            ChdHeader::V4Header(c) => c.compression != CodecType::None as u32,
            ChdHeader::V5Header(c) => c.compression[0] != CodecType::None as u32,
        }
    }

    /// Returns the offset of the CHD metadata, if available.
    pub const fn meta_offset(&self) -> Option<u64> {
        match self {
            ChdHeader::V1Header(_c) => None,
            ChdHeader::V2Header(_c) => None,
            ChdHeader::V3Header(c) => Some(c.meta_offset),
            ChdHeader::V4Header(c) => Some(c.meta_offset),
            ChdHeader::V5Header(c) => Some(c.meta_offset),
        }
    }

    /// Returns the flags of the CHD file, if available.
    pub const fn flags(&self) -> Option<u32> {
        match self {
            ChdHeader::V1Header(c) => Some(c.flags),
            ChdHeader::V2Header(c) => Some(c.flags),
            ChdHeader::V3Header(c) => Some(c.flags),
            ChdHeader::V4Header(c) => Some(c.flags),
            ChdHeader::V5Header(_c) => None,
        }
    }

    /// Returns the total number of hunks in the CHD file.
    pub const fn hunk_count(&self) -> u32 {
        match self {
            ChdHeader::V1Header(c) => c.total_hunks,
            ChdHeader::V2Header(c) => c.total_hunks,
            ChdHeader::V3Header(c) => c.total_hunks,
            ChdHeader::V4Header(c) => c.total_hunks,
            ChdHeader::V5Header(c) => c.hunk_count,
        }
    }

    /// Returns the size of each hunk in the CHD file in bytes.
    pub const fn hunk_size(&self) -> u32 {
        match self {
            ChdHeader::V1Header(c) => c.hunk_bytes,
            ChdHeader::V2Header(c) => c.hunk_bytes,
            ChdHeader::V3Header(c) => c.hunk_bytes,
            ChdHeader::V4Header(c) => c.hunk_bytes,
            ChdHeader::V5Header(c) => c.hunk_bytes,
        }
    }

    /// Returns the logical size of the compressed data in bytes.
    pub const fn logical_bytes(&self) -> u64 {
        match self {
            ChdHeader::V1Header(c) => c.logical_bytes,
            ChdHeader::V2Header(c) => c.logical_bytes,
            ChdHeader::V3Header(c) => c.logical_bytes,
            ChdHeader::V4Header(c) => c.logical_bytes,
            ChdHeader::V5Header(c) => c.logical_bytes,
        }
    }

    /// Returns the number of bytes per unit within each hunk.
    pub const fn unit_bytes(&self) -> u32 {
        match self {
            ChdHeader::V1Header(c) => c.unit_bytes,
            ChdHeader::V2Header(c) => c.unit_bytes,
            ChdHeader::V3Header(c) => c.unit_bytes,
            ChdHeader::V4Header(c) => c.unit_bytes,
            ChdHeader::V5Header(c) => c.unit_bytes,
        }
    }

    /// Returns the number of units per hunk.
    pub const fn unit_count(&self) -> u64 {
        match self {
            ChdHeader::V1Header(c) => c.unit_count,
            ChdHeader::V2Header(c) => c.unit_count,
            ChdHeader::V3Header(c) => c.unit_count,
            ChdHeader::V4Header(c) => c.unit_count,
            ChdHeader::V5Header(c) => c.unit_count,
        }
    }

    /// Returns whether or not this CHD file has a parent.
    pub fn has_parent(&self) -> bool {
        match self {
            ChdHeader::V5Header(c) => c.parent_sha1 != [0u8; SHA1_BYTES],
            _ => self
                .flags()
                .map(|f| (f & Flags::HasParent as u32) != 0)
                .unwrap_or(false),
        }
    }

    /// Returns the length of the header.
    pub const fn len(&self) -> u32 {
        match self {
            ChdHeader::V1Header(c) => c.length,
            ChdHeader::V2Header(c) => c.length,
            ChdHeader::V3Header(c) => c.length,
            ChdHeader::V4Header(c) => c.length,
            ChdHeader::V5Header(c) => c.length,
        }
    }

    pub(crate) fn create_compression_codecs(&self) -> Result<Vec<Box<dyn CompressionCodec>>> {
        match self {
            ChdHeader::V1Header(c) => CodecType::from_u32(c.compression)
                .map(|e| e.init(self.hunk_size()))
                .ok_or(ChdError::UnsupportedFormat)?
                .map(|e| vec![e]),
            ChdHeader::V2Header(c) => CodecType::from_u32(c.compression)
                .map(|e| e.init(self.hunk_size()))
                .ok_or(ChdError::UnsupportedFormat)?
                .map(|e| vec![e]),
            ChdHeader::V3Header(c) => CodecType::from_u32(c.compression)
                .map(|e| e.init(self.hunk_size()))
                .ok_or(ChdError::UnsupportedFormat)?
                .map(|e| vec![e]),
            ChdHeader::V4Header(c) => CodecType::from_u32(c.compression)
                .map(|e| e.init(self.hunk_size()))
                .ok_or(ChdError::UnsupportedFormat)?
                .map(|e| vec![e]),
            ChdHeader::V5Header(c) => c
                .compression
                .map(CodecType::from_u32)
                .map(|f| f.ok_or(ChdError::UnsupportedFormat))
                .map(|f| f.and_then(|f| f.init(self.hunk_size())))
                .into_iter()
                .collect(),
        }
    }

    /// Validate the header.
    fn validate(&self) -> bool {
        let length_valid = match self {
            ChdHeader::V1Header(c) => c.length == CHD_V1_HEADER_SIZE,
            ChdHeader::V2Header(c) => c.length == CHD_V2_HEADER_SIZE,
            ChdHeader::V3Header(c) => c.length == CHD_V3_HEADER_SIZE,
            ChdHeader::V4Header(c) => c.length == CHD_V4_HEADER_SIZE,
            ChdHeader::V5Header(c) => c.length == CHD_V5_HEADER_SIZE,
        };

        if !length_valid {
            return false;
        }

        // Do not validate V5 header
        if let ChdHeader::V5Header(_) = self {
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
                ChdHeader::V1Header(c) => c.parent_md5 != [0u8; MD5_BYTES],
                ChdHeader::V2Header(c) => c.parent_md5 != [0u8; MD5_BYTES],
                ChdHeader::V3Header(c) => {
                    c.parent_md5 != [0u8; MD5_BYTES] && c.parent_sha1 != [0u8; SHA1_BYTES]
                }
                ChdHeader::V4Header(c) => c.parent_sha1 != [0u8; SHA1_BYTES],
                ChdHeader::V5Header(_) => true,
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
            ChdHeader::V1Header(c) => ChdHeader::validate_legacy_compression(c.compression),
            ChdHeader::V2Header(c) => ChdHeader::validate_legacy_compression(c.compression),
            ChdHeader::V3Header(c) => ChdHeader::validate_legacy_compression(c.compression),
            ChdHeader::V4Header(c) => ChdHeader::validate_legacy_compression(c.compression),
            ChdHeader::V5Header(c) => c
                .compression
                .map(ChdHeader::validate_v5_compression)
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

fn read_header<T: Read + Seek>(chd: &mut T) -> Result<ChdHeader> {
    let mut raw_header: [u8; CHD_MAX_HEADER_SIZE] = [0; CHD_MAX_HEADER_SIZE];

    chd.seek(SeekFrom::Start(0))?;
    chd.read_exact(&mut raw_header)?;

    let magic = CStr::from_bytes_with_nul(&raw_header[0..9])?.to_str()?;
    if CHD_MAGIC != magic {
        return Err(ChdError::InvalidData);
    }
    let mut reader = Cursor::new(&raw_header);
    reader.seek(SeekFrom::Start(8))?;
    let length = reader.read_u32::<BigEndian>()?;
    let version = reader.read_u32::<BigEndian>()?;

    // ensure version is known and header size match up
    match (version, length) {
        (1, CHD_V1_HEADER_SIZE) => Ok(ChdHeader::V1Header(read_v1_header(
            &mut reader,
            version,
            length,
        )?)),
        (2, CHD_V2_HEADER_SIZE) => Ok(ChdHeader::V2Header(read_v1_header(
            &mut reader,
            version,
            length,
        )?)),
        (3, CHD_V3_HEADER_SIZE) => Ok(ChdHeader::V3Header(read_v3_header(
            &mut reader,
            length,
            chd,
        )?)),
        (4, CHD_V4_HEADER_SIZE) => Ok(ChdHeader::V4Header(read_v4_header(
            &mut reader,
            length,
            chd,
        )?)),
        (5, CHD_V5_HEADER_SIZE) => Ok(ChdHeader::V5Header(read_v5_header(&mut reader, length)?)),
        (1 | 2 | 3 | 4 | 5, _) => Err(ChdError::InvalidData),
        _ => Err(ChdError::UnsupportedVersion),
    }
}

fn read_v1_header<T: Read + Seek>(header: &mut T, version: u32, length: u32) -> Result<HeaderV1> {
    // get sector size
    const CHD_V1_SECTOR_SIZE: u32 = 512;
    header.seek(SeekFrom::Start(76))?;
    let sector_length = match version {
        1 => CHD_V1_SECTOR_SIZE,
        _ => header.read_u32::<BigEndian>()?,
    };
    let mut md5: [u8; MD5_BYTES] = [0; MD5_BYTES];
    let mut parent_md5: [u8; MD5_BYTES] = [0; MD5_BYTES];

    header.seek(SeekFrom::Start(16))?;
    let flags = header.read_u32::<BigEndian>()?;
    let compression = header.read_u32::<BigEndian>()?;
    let hunk_size = header.read_u32::<BigEndian>()?;
    let total_hunks = header.read_u32::<BigEndian>()?;
    let cylinders = header.read_u32::<BigEndian>()?;
    let heads = header.read_u32::<BigEndian>()?;
    let sectors = header.read_u32::<BigEndian>()?;
    header.read_exact(&mut md5)?;
    header.read_exact(&mut parent_md5)?;

    let logical_bytes =
        (cylinders as u64) * (heads as u64) * (sectors as u64) * (sector_length as u64);

    // verify assumptions about hunk sizes.
    let hunk_bytes: u32 = u32::try_from(sector_length as u64 * hunk_size as u64)
        .map_err(|_| ChdError::InvalidData)?;
    if hunk_bytes == 0 || hunk_size == 0 {
        return Err(ChdError::InvalidData);
    }

    let unit_bytes = hunk_bytes / hunk_size;
    let unit_count = (logical_bytes + unit_bytes as u64 - 1) / unit_bytes as u64;
    Ok(HeaderV1 {
        version: match version {
            1 => Version::ChdV1,
            2 => Version::ChdV2,
            _ => return Err(ChdError::UnsupportedVersion),
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

fn read_v3_header<T: Read + Seek, F: Read + Seek>(
    header: &mut T,
    length: u32,
    chd: &mut F,
) -> Result<HeaderV3> {
    header.seek(SeekFrom::Start(16))?;
    let mut md5: [u8; MD5_BYTES] = [0; MD5_BYTES];
    let mut parent_md5: [u8; MD5_BYTES] = [0; MD5_BYTES];
    let mut sha1: [u8; SHA1_BYTES] = [0; SHA1_BYTES];
    let mut parent_sha1: [u8; SHA1_BYTES] = [0; SHA1_BYTES];

    let flags = header.read_u32::<BigEndian>()?;
    let compression = header.read_u32::<BigEndian>()?;
    let total_hunks = header.read_u32::<BigEndian>()?;
    let logical_bytes = header.read_u64::<BigEndian>()?;
    let meta_offset = header.read_u64::<BigEndian>()?;
    header.read_exact(&mut md5)?;
    header.read_exact(&mut parent_md5)?;
    let hunk_bytes = header.read_u32::<BigEndian>()?;
    header.seek(SeekFrom::Start(80))?;
    header.read_exact(&mut sha1)?;
    header.read_exact(&mut parent_sha1)?;
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

fn read_v4_header<T: Read + Seek, F: Read + Seek>(
    header: &mut T,
    length: u32,
    chd: &mut F,
) -> Result<HeaderV4> {
    header.seek(SeekFrom::Start(16))?;
    let mut sha1: [u8; SHA1_BYTES] = [0; SHA1_BYTES];
    let mut parent_sha1: [u8; SHA1_BYTES] = [0; SHA1_BYTES];
    let mut raw_sha1: [u8; SHA1_BYTES] = [0; SHA1_BYTES];

    let flags = header.read_u32::<BigEndian>()?;
    let compression = header.read_u32::<BigEndian>()?;
    let total_hunks = header.read_u32::<BigEndian>()?;
    let logical_bytes = header.read_u64::<BigEndian>()?;
    let meta_offset = header.read_u64::<BigEndian>()?;
    let hunk_bytes = header.read_u32::<BigEndian>()?;

    header.seek(SeekFrom::Start(48))?;
    header.read_exact(&mut sha1)?;
    header.read_exact(&mut parent_sha1)?;
    header.read_exact(&mut raw_sha1)?;

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

fn guess_unit_bytes<F: Read + Seek>(chd: &mut F, off: u64) -> Option<u32> {
    static RE_BPS: OnceCell<Regex> = OnceCell::new();
    let bps_regex: &'static Regex = RE_BPS.get_or_init(|| Regex::new(r"(?-u)(BPS:)(\d+)").unwrap());

    let metas: Vec<_> = MetadataRefIter::from_stream(chd, off).collect();
    if let Some(hard_disk) = metas
        .iter()
        .find(|&e| e.metatag == KnownMetadata::HardDisk as u32)
    {
        if let Ok(text) = hard_disk.read(chd) {
            let caps = bps_regex
                .captures(&text.value)
                .and_then(|c| c.get(1))
                .map(|c| c.as_bytes())
                .and_then(|c| std::str::from_utf8(c).ok())
                .and_then(|c| c.parse::<u32>().ok());
            // Only return this if we can parse it properly. Fallback to cdrom otherwise.
            if let Some(bps) = caps {
                return Some(bps);
            }
        }
    }

    if metas.iter().any(|e| KnownMetadata::is_cdrom(e.metatag)) {
        return Some(crate::cdrom::CD_FRAME_SIZE as u32);
    }
    None
}

fn read_v5_header<T: Read + Seek>(header: &mut T, length: u32) -> Result<HeaderV5> {
    header.seek(SeekFrom::Start(16))?;
    let mut sha1: [u8; SHA1_BYTES] = [0; SHA1_BYTES];
    let mut parent_sha1: [u8; SHA1_BYTES] = [0; SHA1_BYTES];
    let mut raw_sha1: [u8; SHA1_BYTES] = [0; SHA1_BYTES];
    let mut compression: [u32; 4] = [0; 4];
    header.read_u32_into::<BigEndian>(&mut compression)?;
    let logical_bytes = header.read_u64::<BigEndian>()?;
    let map_offset = header.read_u64::<BigEndian>()?;
    let meta_offset = header.read_u64::<BigEndian>()?;
    let hunk_bytes = header.read_u32::<BigEndian>()?;
    let unit_bytes = header.read_u32::<BigEndian>()?;

    // guard divide by zero
    if hunk_bytes == 0 || unit_bytes == 0 {
        return Err(ChdError::InvalidData);
    }

    let hunk_count = ((logical_bytes + hunk_bytes as u64 - 1) / hunk_bytes as u64) as u32;
    let unit_count = (logical_bytes + unit_bytes as u64 - 1) / unit_bytes as u64;
    header.seek(SeekFrom::Start(84))?;
    header.read_exact(&mut sha1)?;
    header.read_exact(&mut parent_sha1)?;
    header.seek(SeekFrom::Start(64))?;
    header.read_exact(&mut raw_sha1)?;
    let map_entry_bytes = match CodecType::from_u32(compression[0]) {
        // uncompressed map entries are 4 bytes long
        Some(CodecType::None) => map::V5_UNCOMPRESSED_MAP_ENTRY_SIZE as u32,
        Some(_) => map::V5_COMPRESSED_MAP_ENTRY_SIZE as u32,
        None => return Err(ChdError::UnsupportedFormat),
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
