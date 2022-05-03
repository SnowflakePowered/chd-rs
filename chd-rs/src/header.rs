use std::io::{Read, Seek, SeekFrom, Cursor};
use std::ffi::CStr;
use byteorder::{ReadBytesExt, BigEndian};
use crate::error::{ChdError, Result};
use crate::metadata::{MetadataIter, KnownMetadata};
use crate::make_tag;
use lazy_static::lazy_static;
use regex::bytes::Regex;
use crate::header::Version::ChdV5;
use num_derive::{FromPrimitive, ToPrimitive};
use num_traits::FromPrimitive;

#[repr(u32)]
pub enum CodecType {
    None = 0,
    Zlib = 1,
    ZlibPlus = 2,
    AV = 3,
    ZLibV5 = make_tag(b"zlib"),
    ZLibCdV5 = make_tag(b"cdzl"),
    LzmaCdV5 = make_tag(b"cdlz"),
    FlacCdV5 = make_tag(b"cdfl")
}

#[repr(u32)]
pub enum Version {
    ChdV1 = 1,
    ChdV2 = 2,
    ChdV3 = 3,
    ChdV4 = 4,
    ChdV5 = 5,
}

#[repr(u8)]
#[derive(FromPrimitive, ToPrimitive)]
pub enum V5CompressionType {
    CompressionType0 = 0,
    CompressionType1 = 1,
    CompressionType2 = 2,
    CompressionType3 = 3,
    CompressionNone = 4,
    CompressionSelf = 5,
    CompressionParent = 6,
    CompressionRleSmall = 7,
    CompressionRleLarge = 8,
    CompressionSelf0 = 9,
    CompressionSelf1 = 10,
    CompressionParentSelf = 11,
    CompressionParent0 = 12,
    CompressionParent1 = 13
}

#[repr(C)]
pub struct HeaderV1 {
    pub version: Version,
    pub length: u32,
    pub flags: u32,
    pub compression: u32,
    pub hunk_size: u32,
    pub total_hunks: u32,
    pub cylinders: u32,
    pub sectors: u32,
    pub heads: u32,
    pub hunk_bytes: u32,
    pub md5: [u8; MD5_BYTES],
    pub parent_md5: [u8; MD5_BYTES],
    pub unit_bytes: u32,
    pub unit_count: u64,
    pub logical_bytes: u64,
}

#[repr(C)]
pub struct HeaderV3 {
    pub version: Version,
    pub length: u32,
    pub flags: u32,
    pub compression: u32,
    pub total_hunks: u32,
    pub logical_bytes: u64,
    pub meta_offset: u64,
    pub md5: [u8; MD5_BYTES],
    pub parent_md5: [u8; MD5_BYTES],
    pub hunk_bytes: u32,
    pub sha1: [u8; SHA1_BYTES],
    pub parent_sha1: [u8; SHA1_BYTES],
    pub unit_bytes: u32,
    pub unit_count: u64,
}

#[repr(C)]
pub struct HeaderV4 {
    pub version: Version,
    pub length: u32,
    pub flags: u32,
    pub compression: u32,
    pub total_hunks: u32,
    pub logical_bytes: u64,
    pub meta_offset: u64,
    pub hunk_bytes: u32,
    pub sha1: [u8; SHA1_BYTES],
    pub parent_sha1: [u8; SHA1_BYTES],
    pub raw_sha1: [u8; SHA1_BYTES],
    pub unit_bytes: u32,
    pub unit_count: u64,
}

#[repr(C)]
pub struct HeaderV5 {
    pub version: Version,
    pub length: u32,
    pub compression: [u32; 4],
    pub logical_bytes: u64,
    pub map_offset: u64,
    pub meta_offset: u64,
    pub hunk_bytes: u32,
    pub unit_bytes: u32,
    pub sha1: [u8; SHA1_BYTES],
    pub parent_sha1: [u8; SHA1_BYTES],
    pub raw_sha1: [u8; SHA1_BYTES],
    pub unit_count: u64,
    pub hunk_count: u32,
    pub map_entry_bytes: i32,
}

#[repr(C)]
pub enum ChdHeader {
    V1Header(HeaderV1),
    V2Header(HeaderV1),
    V3Header(HeaderV3),
    V4Header(HeaderV4),
    V5Header(HeaderV5)
}

const MD5_BYTES: usize = 16;
const SHA1_BYTES: usize = 20;

pub const CHD_MAGIC: &'static str = "MComprHD";

const CHD_HEADER_VERSION: u32 = 5;
const CHD_V1_HEADER_SIZE: u32 = 76;
const CHD_V2_HEADER_SIZE: u32 = 80;
const CHD_V3_HEADER_SIZE: u32 =  120;
const CHD_V4_HEADER_SIZE: u32 = 108;
const CHD_V5_HEADER_SIZE: u32 = 124;
pub const CHD_MAX_HEADER_SIZE: usize = CHD_V5_HEADER_SIZE as usize;
pub const COOKIE_VALUE: u32 = 0xbaadf00d;

impl ChdHeader {
    pub fn try_from_file<F: Read + Seek>(file: &mut F) -> Result<ChdHeader> {
        read_header(file)
    }

    pub const fn is_compressed(&self) -> bool {
        match self {
            ChdHeader::V1Header(c) => c.compression != CodecType::None as u32,
            ChdHeader::V2Header(c) =>  c.compression != CodecType::None as u32,
            ChdHeader::V3Header(c) =>  c.compression != CodecType::None as u32,
            ChdHeader::V4Header(c) =>  c.compression != CodecType::None as u32,
            ChdHeader::V5Header(c) => c.compression[0] != CodecType::None as u32,
        }
    }

    pub const fn meta_offset(&self) -> Option<u64> {
        match self {
            ChdHeader::V1Header(c) => None,
            ChdHeader::V2Header(c) => None,
            ChdHeader::V3Header(c) => Some(c.meta_offset),
            ChdHeader::V4Header(c) => Some(c.meta_offset),
            ChdHeader::V5Header(c) => Some(c.meta_offset)
        }
    }

    pub const fn flags(&self) -> Option<u32> {
        match self {
            ChdHeader::V1Header(c) => Some(c.flags),
            ChdHeader::V2Header(c) => Some(c.flags),
            ChdHeader::V3Header(c) => Some(c.flags),
            ChdHeader::V4Header(c) => Some(c.flags),
            ChdHeader::V5Header(c) => None
        }
    }

    pub const fn hunk_count(&self) -> u32 {
        match self {
            ChdHeader::V1Header(c) => c.total_hunks,
            ChdHeader::V2Header(c) => c.total_hunks,
            ChdHeader::V3Header(c) => c.total_hunks,
            ChdHeader::V4Header(c) => c.total_hunks,
            ChdHeader::V5Header(c) => c.hunk_count,
        }
    }

    pub const fn hunk_bytes(&self) -> u32 {
        match self {
            ChdHeader::V1Header(c) => c.hunk_size,
            ChdHeader::V2Header(c) => c.hunk_size,
            ChdHeader::V3Header(c) => c.hunk_bytes,
            ChdHeader::V4Header(c) => c.hunk_bytes,
            ChdHeader::V5Header(c) => c.hunk_bytes,
        }
    }

    pub fn has_parent(&self) -> bool {
        match self {
            ChdHeader::V5Header(c) => c.parent_sha1 == [0u8; SHA1_BYTES],
            _ => self.flags().map(|f| (f & Flags::HasParent as u32) != 0).unwrap_or(false)
        }
    }

    pub fn validate(&self) -> bool {
        // todo: validate compression
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
                return false
            }
        }

        // require valid hunk size
        if self.hunk_bytes() == 0 || self.hunk_bytes() >= 65536 * 256 {
            return false
        }

        // require valid hunk count
        if self.hunk_count() == 0 {
            return false
        }

        // if we use a parent make sure we have valid md5
        let parent_ok = if self.has_parent() {
            match self {
                ChdHeader::V1Header(c) => {
                    c.parent_md5 != [0u8; MD5_BYTES]
                }
                ChdHeader::V2Header(c) => {
                    c.parent_md5 != [0u8; MD5_BYTES]
                }
                ChdHeader::V3Header(c) => {
                    c.parent_md5 != [0u8; MD5_BYTES] && c.parent_sha1 != [0u8; SHA1_BYTES]
                }
                ChdHeader::V4Header(c) => {
                    c.parent_sha1 != [0u8; SHA1_BYTES]
                }
                ChdHeader::V5Header(_) => true
            }
        } else {
            true
        };

        if !parent_ok {
            return false;
        }

        // obsolete field checks are done by type system
        return true
    }
}

#[repr(u32)]
pub enum Flags {
    HasParent = 0x00000001,
    IsWritable = 0x00000002,
    Undefined = 0xfffffffc
}

fn read_header<T: Read + Seek>(chd: &mut T) -> Result<ChdHeader> {
    let mut raw_header: [u8; CHD_MAX_HEADER_SIZE] = [0; CHD_MAX_HEADER_SIZE];

    chd.seek(SeekFrom::Start(0))?;
    chd.read_exact(&mut raw_header)?;

    let magic = CStr::from_bytes_with_nul(&raw_header[0..9])?.to_str()?;
    if CHD_MAGIC != magic {
        return Err(ChdError::InvalidData)
    }
    let mut reader = Cursor::new(&raw_header);
    reader.seek(SeekFrom::Start(8))?;
    let length = reader.read_u32::<BigEndian>()?;
    let version = reader.read_u32::<BigEndian>()?;

    // ensure version is known and header size match up
    return match (version, length) {
        (1, CHD_V1_HEADER_SIZE) => Ok(ChdHeader::V1Header(read_v1_header(&mut reader, version, length)?)),
        (2, CHD_V2_HEADER_SIZE) => Ok(ChdHeader::V2Header(read_v1_header(&mut reader, version, length)?)),
        (3, CHD_V3_HEADER_SIZE) => Ok(ChdHeader::V3Header(read_v3_header(&mut reader, version, length, chd)?)),
        (4, CHD_V4_HEADER_SIZE) => Ok(ChdHeader::V4Header(read_v4_header(&mut reader, version, length, chd)?)),
        (5, CHD_V5_HEADER_SIZE) => Ok(ChdHeader::V5Header(read_v5_header(&mut reader, version, length)?)),
        (1 | 2 | 3 | 4 | 5, _) => Err(ChdError::InvalidData),
        _ => Err(ChdError::UnsupportedVersion)
    }
}

fn read_v1_header<T: Read + Seek>(header: &mut T, version: u32, length: u32) -> Result<HeaderV1> {
    // get sector size
    const CHD_V1_SECTOR_SIZE: u32 = 512;
    header.seek(SeekFrom::Start(76))?;
    let sector_length = match version {
        1 => CHD_V1_SECTOR_SIZE,
        _ => header.read_u32::<BigEndian>()?
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
    let logical_bytes = (cylinders as u64) * (heads as u64) * (sectors as u64) * (sector_length as u64);
    let hunk_bytes = sector_length * hunk_size;
    let unit_bytes = hunk_bytes / hunk_size;
    let unit_count = (logical_bytes + unit_bytes as u64 - 1) / unit_bytes as u64;
    Ok(HeaderV1 {
        version: match version {
            1 => Version::ChdV1,
            2 => Version::ChdV2,
            _ => return Err(ChdError::UnsupportedVersion)
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
        logical_bytes
    })
}

fn read_v3_header<T: Read + Seek, F: Read + Seek>(header: &mut T, version: u32, length: u32, chd: &mut F) -> Result<HeaderV3> {
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
    let hunk_bytes =  header.read_u32::<BigEndian>()?;
    header.seek(SeekFrom::Start(80))?;
    header.read_exact(&mut sha1)?;
    header.read_exact(&mut parent_sha1)?;
    let unit_bytes = guess_unit_bytes(chd, meta_offset).unwrap_or(hunk_bytes);
    let unit_count =  (logical_bytes + (unit_bytes as u64) - 1) / unit_bytes as u64;
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
        parent_sha1
    })
}

fn read_v4_header<T: Read + Seek, F: Read + Seek>(header: &mut T, version: u32, length: u32, chd: &mut F) -> Result<HeaderV4> {
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
    lazy_static! {
        static ref RE_BPS: Regex = Regex::new(r"(?-u)(BPS:)(\d+)").unwrap();
    }

    let metas: Vec<_> = MetadataIter::new_from_raw_file(chd, off).collect();
    if let Some(hard_disk) = metas.iter().find(|&e| e.metatag == KnownMetadata::HardDisk as u32) {
        if let Ok(text) = hard_disk.read(chd) {
            let caps = RE_BPS.captures(&text.value)
                .and_then(|c| c.get(1))
                .and_then(|c| Some(c.as_bytes()))
                .and_then(|c| std::str::from_utf8(c).ok())
                .and_then(|c| c.parse::<u32>().ok());
            // Only return this if we can parse it properly. Fallback to cdrom otherwise.
            if let Some(bps) = caps {
                return Some(bps)
            }
        }
    }

    if metas.iter().any(|e| KnownMetadata::is_cdrom(e.metatag)) {
        return Some(crate::cdrom::CD_FRAME_SIZE)
    }
    None
}

fn read_v5_header<T: Read + Seek>(header: &mut T, version: u32, length: u32) -> Result<HeaderV5> {
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
    let hunk_count = ((logical_bytes + hunk_bytes as u64 - 1) / hunk_bytes as u64) as u32;
    let unit_bytes = header.read_u32::<BigEndian>()?;
    let unit_count = (logical_bytes + unit_bytes as u64 - 1) / unit_bytes as u64;
    header.seek(SeekFrom::Start(84))?;
    header.read_exact(&mut sha1)?;
    header.read_exact(&mut parent_sha1)?;
    header.seek(SeekFrom::Start(64))?;
    header.read_exact(&mut raw_sha1)?;
    let map_entry_bytes = match compression[0] != CodecType::None as u32 {
        true => 12,
        false => 4
    };
    Ok(HeaderV5 {
        version: ChdV5,
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
