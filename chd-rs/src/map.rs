use std::convert::TryFrom;
use std::io::{BufReader, Cursor, Read, Seek, SeekFrom};
use std::os;
use bitreader::BitReader;
use crate::error::{Result, ChdError};
use crate::header::{ChdHeader, CodecType, HeaderV5};
use byteorder::{ReadBytesExt, BigEndian, WriteBytesExt};
use num_derive::{FromPrimitive, ToPrimitive};
use num_traits::{FromPrimitive, ToPrimitive};
use crc16::{CCITT_FALSE, State};
use crate::huffman::HuffmanDecoder;
use crate::map;
use crate::map::MapEntry::V5Compressed;
const V3_MAP_ENTRY_SIZE: usize = 16; // V3-V4
const V1_MAP_ENTRY_SIZE: usize = 8; // V1-V2
const MAP_ENTRY_FLAG_TYPE_MASK: u8 = 0x0f; // type of hunk
const MAP_ENTRY_FLAG_NO_CRC: u8 = 0x10; // no crc is present

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

#[repr(u8)]
#[derive(FromPrimitive, ToPrimitive)]
pub enum V34EntryType {
    Invalid = 0, // invalid
    Compressed = 1, // standard compression
    Uncompressed = 2, // uncompressed
    Mini = 3, // use offset as raw data
    SelfHunk = 4, // same as another hunk in same file
    ParentHunk = 5, // same as hunk in parent file
    ExternalCompressed = 6 // compressed with external algorithm (i.e. flac CDDA)
}

type RawMap = (Vec<u8>, bool, u32);

#[allow(unused)]
type MapList = Vec<LegacyMapEntry>;

#[allow(unused)]
pub struct LegacyMapEntry {
    offset: u64, // offset within file of data
    crc: Option<u32>, // crc32 of data
    length: u32, // length of data
    flags: u8, // flags
}

pub enum ChdMap {
    V5(RawMap), // compressed
    Legacy(MapList)
}

pub enum MapEntry<'a> {
    V5Compressed(&'a [u8; 12]),
    V5Uncompressed(&'a [u8; 4], u32),
    LegacyEntry(&'a LegacyMapEntry)
}

impl <'a> MapEntry<'a> {
    pub fn is_compressed(&self) -> bool {
        match self {
            MapEntry::V5Compressed(_) => true,
            MapEntry::V5Uncompressed(_, _) => false,
            MapEntry::LegacyEntry(e) => {
                if let Some(V34EntryType::Uncompressed) =
                    V34EntryType::from_u8(e.flags & MAP_ENTRY_FLAG_TYPE_MASK) {
                    false
                } else {
                    true
                }
            }
        }
    }

    pub fn is_legacy(&self) -> bool {
        match self {
            MapEntry::LegacyEntry(_) => true,
            _ => false
        }
    }

    pub fn block_size(&self) -> Result<u32> {
        match self {
            MapEntry::V5Compressed(r) => {
                Ok(Cursor::new(&r[1..]).read_u24::<BigEndian>()?)
            }
            MapEntry::V5Uncompressed(_, hunk_bytes) => {
                Ok(*hunk_bytes)
            }
            MapEntry::LegacyEntry(e) => {
                Ok(e.length)
            }
        }
    }

    pub fn block_offset(&self) -> Result<u64> {
        match self {
            MapEntry::V5Compressed(r) => {
                Ok(Cursor::new(&r[4..]).read_u48::<BigEndian>()?)
            }
            MapEntry::V5Uncompressed(r, hunk_bytes) => {
                let off = Cursor::new(r).read_u32::<BigEndian>()?;
                Ok(off as u64 * *hunk_bytes as u64)
            }
            MapEntry::LegacyEntry(e) => {
                Ok(e.offset)
            }
        }
    }

    pub fn block_crc(&self) -> Result<Option<u32>> {
        match self {
            MapEntry::V5Compressed(r) => {
                Ok(Some(Cursor::new(&r[10..]).read_u16::<BigEndian>()? as u32))
            }
            MapEntry::V5Uncompressed(_, _) => {
                Ok(None)
            }
            MapEntry::LegacyEntry(e) => {
                Ok(e.crc)
            }
        }
    }

    pub fn block_type(&self) -> Option<u8> {
        match self {
            MapEntry::V5Compressed(r) => {
                Some(r[0])
            }
            MapEntry::V5Uncompressed(_, _) => {
                None
            }
            MapEntry::LegacyEntry(e) => {
                Some(e.flags & MAP_ENTRY_FLAG_TYPE_MASK)
            }
        }
    }
    // todo: get compression type for map entry
}

impl ChdMap {
    pub fn len(&self) -> usize {
        match self {
            ChdMap::V5(m) => {
                // map_entry_bytes = 12 if compressed, else 4
                let map_entry_bytes = if m.1 { 12 } else { 4 };
                m.0.len() / map_entry_bytes
            },
            ChdMap::Legacy(m) => {
                m.len()
            }
        }
    }

    pub fn get_entry(&self, hunk_num: usize) -> Option<MapEntry> {
        match self {
            ChdMap::V5(m) => {
                let map_entry_bytes = if m.1 { 12 } else { 4 };

                let entry_slice = &m.0.get(hunk_num * map_entry_bytes..(hunk_num + 1) * map_entry_bytes);
                if let &Some(entry_slice) = entry_slice {
                    return if m.1 {
                        <&[u8; 12]>::try_from(entry_slice).map(MapEntry::V5Compressed).ok()
                    } else {
                        <&[u8; 4]>::try_from(entry_slice).map(|e| MapEntry::V5Uncompressed(e,  m.2)).ok()
                    }
                }
                return None;
            }
            ChdMap::Legacy(m) => m.get(hunk_num).map(MapEntry::LegacyEntry)
        }
    }

    pub fn try_read_map<F: Read + Seek>(header: &ChdHeader, mut file: F) -> Result<ChdMap> {
        match header {
            ChdHeader::V5Header(v5) => {
                Ok(ChdMap::V5(map::read_map_v5(v5, &mut file, header.is_compressed())?))
            }
            ChdHeader::V3Header(_) | ChdHeader::V4Header(_) => {
                Ok(ChdMap::Legacy(map::read_map_legacy::<_, V3_MAP_ENTRY_SIZE>(header, file)?))
            }
            ChdHeader::V2Header(_) | ChdHeader::V1Header(_) => {
                Ok(ChdMap::Legacy(map::read_map_legacy::<_, V1_MAP_ENTRY_SIZE>(header, file)?))
            }
        }
    }
}

macro_rules! const_assert {
    ($($list:ident : $ty:ty),* => $expr:expr) => {{
        struct Assert<$(const $list: usize,)*>;
        impl<$(const $list: $ty,)*> Assert<$($list,)*> {
            const OK: u8 = 0 - !($expr) as u8;
        }
        Assert::<$($list,)*>::OK
    }};
    ($expr:expr) => {
        const OK: u8 = 0 - !($expr) as u8;
    };
}

fn read_map_legacy<F: Read + Seek, const MAP_ENTRY_SIZE: usize>(header: &ChdHeader, mut file: F) -> Result<Vec<LegacyMapEntry>> {
    // Probably can express this better in the type system.
    const_assert!(MAP_ENTRY_SIZE: usize => V3_MAP_ENTRY_SIZE >=
        MAP_ENTRY_SIZE && (MAP_ENTRY_SIZE == V3_MAP_ENTRY_SIZE || MAP_ENTRY_SIZE == V1_MAP_ENTRY_SIZE));
    let mut map = Vec::new();

    let mut max_off = 0;
    let mut cookie = [0u8; MAP_ENTRY_SIZE];
    file.seek(SeekFrom::Start(0))?;

    // wrap into bufreader
    let mut file = BufReader::new(file);
    file.seek(SeekFrom::Start(header.length() as u64))?;

    // SAFETY: V3_MAP_ENTRY_SIZE is strictly greater than V1_MAP_ENTRY_SIZE so it is safe to overallocate.
    // the read will instead read only to the first 8 bytes = u64 in the V1 case.
    // the alternative is to use a transmute but that's not ideal, or to wait for const_generics to mature.
    let mut entry_buf = [0u8; V3_MAP_ENTRY_SIZE];
    for _ in 0..header.hunk_count() {
        file.read_exact(&mut entry_buf[0..MAP_ENTRY_SIZE])?;
        let entry = match MAP_ENTRY_SIZE {
            V3_MAP_ENTRY_SIZE => read_map_entry_v3(&entry_buf)?,
            V1_MAP_ENTRY_SIZE => {
                let mut read = Cursor::new(entry_buf);
                let entry = read.read_u64::<BigEndian>()?;
                read_map_entry_v1(entry, header.hunk_bytes())
            }
            _ => unreachable!()
        };

        if let Some(V34EntryType::Compressed) | Some(V34EntryType::Uncompressed) =
            V34EntryType::from_u8(entry.flags & MAP_ENTRY_FLAG_TYPE_MASK) {
            max_off = std::cmp::max(max_off, entry.offset + entry.length as u64);
        }
        map.push(entry);
    }

    // verify cookie
    file.read_exact(&mut cookie)?;

    // need to confirm this is the same behaviour
    if &cookie[..] < b"EndOfListCookie\0" {
        return Err(ChdError::InvalidFile)
    }

    if max_off > file.seek(SeekFrom::End(0))? {
        return Err(ChdError::InvalidFile)
    }

    Ok(map)
}

#[inline]
fn read_map_entry_v1(val: u64, hunk_bytes: u32) -> LegacyMapEntry {
    let length = (val >> 44) as u32;
    let flags = MAP_ENTRY_FLAG_NO_CRC |
        if length == hunk_bytes {
            V34EntryType::Uncompressed as u8
        } else {
            V34EntryType::Compressed as u8
        };
    let offset = (val << 20) >> 20;
    LegacyMapEntry {
        offset,
        crc: None,
        length,
        flags
    }
}

#[inline]
fn read_map_entry_v3(buf: &[u8; V3_MAP_ENTRY_SIZE]) -> Result<LegacyMapEntry> {
    let mut read = Cursor::new(buf);
    let offset = read.read_u64::<BigEndian>()?;
    let crc = read.read_u32::<BigEndian>()?;
    // this widening shift is likely wrong... what we really want is bottom out to 0.
    let length: u32 = read.read_u16::<BigEndian>()? as u32 | buf[14].checked_shl(16).unwrap_or(0) as u32;
    let flags = buf[15];
    Ok(LegacyMapEntry {
        offset,
        crc: Some(crc),
        length,
        flags
    })
}

fn read_map_v5<F: Read + Seek>(header: &HeaderV5, mut file: F, is_compressed: bool) -> Result<RawMap>{
    let map_size = header.hunk_count as usize * header.map_entry_bytes as usize;
    let mut raw_map = vec![0u8; map_size];

    if !is_compressed {
        file.seek(SeekFrom::Start(header.map_offset))?;
        file.read_exact(&mut raw_map[..])?;
        return Ok((raw_map, is_compressed, header.hunk_bytes));
    }

    // read compressed params
    file.seek(SeekFrom::Start(header.map_offset))?;

    let map_bytes = file.read_u32::<BigEndian>()?;
    let first_offs = file.read_u48::<BigEndian>()?;
    let map_crc = file.read_u16::<BigEndian>()?;
    let length_bits = file.read_u8()?;
    let self_bits = file.read_u8()?;
    let parent_bits = file.read_u8()?;

    // read map
    let mut compressed: Vec<u8> = vec![0u8; map_bytes as usize];
    file.seek(SeekFrom::Start(header.map_offset + 16))?;
    file.read_exact(&mut compressed[..])?;

    let mut bitstream = BitReader::new(&compressed[..]);
    let mut decoder = HuffmanDecoder::from_tree_rle(16, 8, &mut bitstream)?;

    let mut rep_count = 0;
    let mut last_cmp = 0;

    for map_slice in raw_map.chunks_exact_mut(12) {
        if rep_count > 0 {
            map_slice[0] = last_cmp;
            rep_count -= 1;
        } else {
            let val = decoder.decode_one(&mut bitstream)? as u8;
            match V5CompressionType::from_u8(val).ok_or(ChdError::DecompressionError)? {
                V5CompressionType::CompressionRleSmall => { // COMPRESSION_RLE_SMALL
                    map_slice[0] = last_cmp;
                    rep_count = 2 + decoder.decode_one(&mut bitstream)?;
                },
                V5CompressionType::CompressionRleLarge => {// COMPRESSION_RLE_LARGE
                    map_slice[0] = last_cmp;
                    rep_count = 2 + 16 + (decoder.decode_one(&mut bitstream)? << 4);
                    rep_count += decoder.decode_one(&mut bitstream)?;
                }
                _ => {
                    map_slice[0] = val;
                    last_cmp = val;
                }
            }
        }
    }

    // iterate hunks
    let mut curr_off = first_offs;
    let mut last_self = 0;
    let mut last_parent = 0;

    for hunk in 0..header.hunk_count as usize {
        let map_slice = &mut raw_map[(hunk*12)..((hunk+1) * 12)];
        let mut off = curr_off;
        let mut len: u32 = 0;
        let mut crc: u16 = 0;

        match V5CompressionType::from_u8(map_slice[0]).ok_or(ChdError::DecompressionError)? {
            V5CompressionType::CompressionType0 | V5CompressionType::CompressionType1
                | V5CompressionType::CompressionType2 | V5CompressionType::CompressionType3 => {
                len = bitstream.read_u32(length_bits)?;
                curr_off += len as u64;
                crc = bitstream.read_u16(16)?;
            }
            V5CompressionType::CompressionNone => {
                len = header.hunk_bytes;
                curr_off += len as u64;
                crc = bitstream.read_u16(16)?;
            }
            V5CompressionType::CompressionSelf => {
                off = bitstream.read_u64(self_bits)?;
                last_self = off;
            }
            V5CompressionType::CompressionParent => {
                off = bitstream.read_u64(parent_bits)?;
                last_parent = off;
            }

            // pseudo types
            V5CompressionType::CompressionSelf1 => last_self += 1,
            V5CompressionType::CompressionSelf0 => {
                map_slice[0] = V5CompressionType::CompressionSelf as u8;
                off = last_self;
            }
            V5CompressionType::CompressionParentSelf => {
                map_slice[0] = V5CompressionType::CompressionParent as u8;
                off = ((hunk * header.hunk_bytes as usize) / header.unit_bytes as usize) as u64;
                last_parent = off;
            }
            V5CompressionType::CompressionParent1 => last_parent += (header.hunk_bytes / header.unit_bytes) as u64,
            V5CompressionType::CompressionParent0 => {
                map_slice[0] = V5CompressionType::CompressionParent as u8;
                off = last_parent;
            }
            _ => {
                return Err(ChdError::DecompressionError)
            }
        }

        let mut cursor = Cursor::new(map_slice);
        cursor.seek(SeekFrom::Start(1))?;
        cursor.write_u24::<BigEndian>(len)?;
        cursor.write_u48::<BigEndian>(off)?;
        cursor.write_u16::<BigEndian>(crc)?;
    }

    if State::<CCITT_FALSE>::calculate(&raw_map[0..header.hunk_count as usize * 12]) != map_crc {
        return Err(ChdError::DecompressionError)
    }

    Ok((raw_map, is_compressed, header.hunk_bytes))
}
