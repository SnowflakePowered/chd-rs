use std::io::{Read, Seek, SeekFrom, Cursor};
use std::ffi::CStr;
use byteorder::{ReadBytesExt, BigEndian};
use crate::error::{ChdError, Result};
use std::fs::read;
use crate::metadata::{MetadataIter, KnownMetadata};
use crate::make_tag;
use lazy_static::lazy_static;
use regex::bytes::{Regex, Captures};

#[repr(u32)]
pub enum Codec {
    None = 0,
    ZLib = make_tag(b"zlib"),
    ZLibCd = make_tag(b"cdzl"),
    LzmaCd = make_tag(b"cdlz"),
    FlacCd = make_tag(b"cdfl")
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

#[repr(C)]
pub struct Header {
    pub length: u32,
    pub version: u32,
    pub flags: Option<u32>,
    pub compression: [u32; 4],
    pub hunk_bytes: u32,
    pub total_hunks: u32,
    pub logical_bytes: u64,
    pub meta_offset: u64,
    pub map_offset: Option<u64>,
    pub md5:  Option<[u8; MD5_BYTES]>,
    pub parent_md5:  Option<[u8; MD5_BYTES]>,
    pub sha1: Option<[u8; SHA1_BYTES]>,
    pub raw_sha1: Option<[u8; SHA1_BYTES]>,
    pub parent_sha1: Option<[u8; SHA1_BYTES]>,
    pub unit_bytes: u32,
    pub unit_count: u64,
    pub hunk_count: Option<u32>,
    pub map_entry_bytes: Option<u32>,
    pub raw_map: Option<Vec<u8>>,
    #[deprecated]
    pub cylinders: Option<u32>,
    #[deprecated]
    pub sectors: Option<u32>,
    #[deprecated]
    pub heads: Option<u32>,
    #[deprecated]
    pub hunk_size: Option<u32>,
}

#[repr(u32)]
pub enum Compression {
    None = 0,
    Zlib = 1,
    ZlibPlus = 2,
    AV = 3
}

#[inline]
const fn chd_compressed(compression: &[u32; 4]) -> bool {
    compression[0] != Codec::None as u32
}

pub fn read_header<T: Read + Seek>(chd: &mut T) -> Result<Header> {
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
        (1, CHD_V1_HEADER_SIZE) | (2, CHD_V2_HEADER_SIZE) => read_v1_header(&mut reader, version, length),
        (3, CHD_V3_HEADER_SIZE) => read_v3_header(&mut reader, version, length, chd),
        (4, CHD_V4_HEADER_SIZE) => read_v4_header(&mut reader, version, length, chd),
        (5, CHD_V5_HEADER_SIZE) => read_v5_header(&mut reader, version, length),
        (1 | 2 | 3 | 4 | 5, _) => Err(ChdError::InvalidData),
        _ => Err(ChdError::UnsupportedVersion)
    }
}

#[allow(deprecated)]
fn read_v1_header<T: Read + Seek>(header: &mut T, version: u32, length: u32) -> Result<Header> {
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
    let meta_offset = 0;
    Ok(Header {
        version,
        length,
        flags: Some(flags),
        compression: [compression, 0, 0, 0],
        hunk_bytes,
        total_hunks,
        logical_bytes,
        meta_offset,
        map_offset: None,
        md5: Some(md5),
        parent_md5: Some(md5),
        sha1: None,
        raw_sha1: None,
        parent_sha1: None,
        unit_bytes,
        unit_count,
        hunk_count: None,
        map_entry_bytes: None,
        raw_map: None,
        cylinders: Some(cylinders),
        sectors: Some(sectors),
        heads: Some(heads),
        hunk_size: Some(hunk_size)
    })
}

#[allow(deprecated)]
fn read_v3_header<T: Read + Seek, F: Read + Seek>(header: &mut T, version: u32, length: u32, chd: &mut F) -> Result<Header> {
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
    Ok(Header {
        version,
        length,
        flags: Some(flags),
        compression: [compression, 0, 0, 0],
        hunk_bytes,
        total_hunks,
        logical_bytes,
        meta_offset,
        map_offset: None,
        md5: Some(md5),
        parent_md5: Some(md5),
        sha1: None,
        raw_sha1: None,
        parent_sha1: None,
        unit_bytes,
        unit_count,
        hunk_count: None,
        map_entry_bytes: None,
        raw_map: None,
        cylinders: None,
        sectors: None,
        heads: None,
        hunk_size: None
    })
}

#[allow(deprecated)]
fn read_v4_header<T: Read + Seek, F: Read + Seek>(header: &mut T, version: u32, length: u32, chd: &mut F) -> Result<Header> {
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
    Ok(Header {
        version,
        length,
        flags: Some(flags),
        compression: [compression, 0, 0, 0],
        hunk_bytes,
        total_hunks,
        logical_bytes,
        meta_offset,
        map_offset: None,
        md5: None,
        parent_md5: None,
        sha1: Some(sha1),
        raw_sha1: Some(raw_sha1),
        parent_sha1: Some(parent_sha1),
        unit_bytes,
        unit_count,
        hunk_count: None,
        map_entry_bytes: None,
        raw_map: None,
        cylinders: None,
        sectors: None,
        heads: None,
        hunk_size: None
    })
}

fn guess_unit_bytes<F: Read + Seek>(chd: &mut F, off: u64) -> Option<u32> {
    lazy_static! {
        static ref RE_BPS: Regex = Regex::new(r"(?-u)(BPS:)(\d+)").unwrap();
    }

    let metas: Vec<_> = MetadataIter::new(chd, off).collect();
    if let Some(hard_disk) = metas.iter().find(|&e| e.metatag == KnownMetadata::HardDisk as u32) {
        if let Ok(text) = hard_disk.read(chd) {
            let caps = RE_BPS.captures(&text)
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


#[allow(deprecated)]
fn read_v5_header<T: Read + Seek>(header: &mut T, version: u32, length: u32) -> Result<Header> {
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
    let map_entry_bytes = match chd_compressed(&compression) {
        true => 12,
        false => 4
    };
    Ok(Header {
        version,
        length,
        flags: None,
        compression,
        hunk_bytes,
        total_hunks: hunk_count,
        logical_bytes,
        meta_offset,
        map_offset: Some(map_offset),
        md5: None,
        parent_md5: None,
        sha1: Some(sha1),
        raw_sha1: Some(raw_sha1),
        parent_sha1: Some(parent_sha1),
        unit_bytes,
        unit_count,
        hunk_count: Some(hunk_count),
        map_entry_bytes: Some(map_entry_bytes),
        raw_map: None,
        cylinders: None,
        sectors: None,
        heads: None,
        hunk_size: None
    })
}
