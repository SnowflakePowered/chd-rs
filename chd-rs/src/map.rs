//! Types and methods relating to the CHD hunk map.

use std::convert::TryFrom;
use std::io::{BufReader, Cursor, Read, Seek, SeekFrom};

use bitreader::BitReader;
use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use num_derive::{FromPrimitive, ToPrimitive};
use num_traits::FromPrimitive;

use crate::const_assert;
use crate::error::{Error, Result};
use crate::header::{Header, HeaderV5};
use crate::huffman::{lookup_len, HuffmanDecoder};

pub(crate) const V5_UNCOMPRESSED_MAP_ENTRY_SIZE: usize = 4;
pub(crate) const V5_COMPRESSED_MAP_ENTRY_SIZE: usize = 12;
const V3_MAP_ENTRY_SIZE: usize = 16; // V3-V4
const V1_MAP_ENTRY_SIZE: usize = 8; // V1-V2
const MAP_ENTRY_FLAG_TYPE_MASK: u8 = 0x0f; // type of hunk
const MAP_ENTRY_FLAG_NO_CRC: u8 = 0x10; // no crc is present

/// The types of compression allowed for a CHD V5 hunk.
#[repr(u8)]
#[derive(FromPrimitive, ToPrimitive)]
pub enum CompressionTypeV5 {
    /// The first compression codec specified in the header for this CHD.
    CompressionType0 = 0,
    /// The second compression codec specified in the header for this CHD.
    CompressionType1 = 1,
    /// The third compression codec specified in the header for this CHD.
    CompressionType2 = 2,
    /// The fourth compression codec specified in the header for this CHD.
    CompressionType3 = 3,
    /// Uncompressed hunk.
    CompressionNone = 4,
    /// Copy from another hunk in this CHD.
    CompressionSelf = 5,
    /// Copy from another hunk in the parent CHD.
    CompressionParent = 6,
    /// Small-repetition Huffman RLE. Only used for map compression.
    CompressionRleSmall = 7,
    /// Large-repetition Huffman RLE. Only used for map compression.
    CompressionRleLarge = 8,
    /// RLE Pseudo-codec for copy-from-self with 0-repetition. Only used for map compression.
    CompressionSelf0 = 9,
    /// RLE Pseudo-codec for copy-from-self with 1-repetition. Only used for map compression.
    CompressionSelf1 = 10,
    /// RLE Pseudo-codec for copy-from-parent with self-offset. Only used for map compression.
    CompressionParentSelf = 11,
    /// RLE Pseudo-codec for copy-from-parent for 1-repetition. Only used for map compression.
    CompressionParent0 = 12,
    /// RLE Pseudo-codec for copy-from-parent for 0-repetition. Only used for map compression.
    CompressionParent1 = 13,
}

/// The types of compression allowed for a CHD V1-4 hunk.
#[repr(u8)]
#[derive(FromPrimitive, ToPrimitive)]
pub enum CompressionTypeLegacy {
    /// Invalid
    Invalid = 0,
    /// Compressed with a standard codec
    Compressed = 1,
    /// Uncompressed
    Uncompressed = 2,
    /// Uses the offset as raw data
    Mini = 3,
    /// Identical to another hunk in the same file.
    SelfHunk = 4,
    /// Identical to another hunk in the parent file.
    ParentHunk = 5,
    /// Compressed with an external algorithm
    ExternalCompressed = 6,
}

/// Opaque type for a V5 map.
pub struct RawMapV5(Vec<u8>, bool, u32);
/// Opaque type for a legacy map.
pub struct RawMapLegacy(Vec<LegacyMapEntry>);

impl From<RawMapV5> for Vec<u8> {
    fn from(map: RawMapV5) -> Self {
        map.0
    }
}

impl From<&RawMapV5> for Vec<u8> {
    fn from(map: &RawMapV5) -> Self {
        map.0.clone()
    }
}

/// A CHD V1-V4 map entry.
pub struct LegacyMapEntry {
    offset: u64,      // offset within file of data
    crc: Option<u32>, // crc32 of data
    length: u32,      // length of data
    flags: u8,        // flags
}

/// A CHD V5 map entry for a compressed hunk.
pub struct V5CompressedMapEntry<'a>(&'a [u8; V5_COMPRESSED_MAP_ENTRY_SIZE]);

/// A CHD V5 map entry for an uncompressed hunk.
pub struct V5UncompressedMapEntry<'a>(&'a [u8; V5_UNCOMPRESSED_MAP_ENTRY_SIZE], u32);

impl LegacyMapEntry {
    /// Returns the hunk type of the compressed entry.
    pub fn hunk_type(&self) -> Result<CompressionTypeLegacy> {
        CompressionTypeLegacy::from_u8(self.flags & MAP_ENTRY_FLAG_TYPE_MASK)
            .ok_or(Error::UnsupportedFormat)
    }

    /// Returns the offset to the compressed data of the hunk.
    pub fn block_offset(&self) -> u64 {
        self.offset
    }

    /// Returns the size of the compressed data of the hunk.
    pub fn block_size(&self) -> u32 {
        self.length
    }

    /// Returns the CRC32 checksum of the hunk data once uncompressed.
    pub fn hunk_crc(&self) -> Option<u32> {
        self.crc
    }

    /// Obtain a proof that the hunk this entry refers to is compressed.
    /// If the hunk is uncompressed, returns `Error::InvalidParameter`.
    #[inline(always)]
    pub(crate) fn prove_compressed(&self) -> Result<CompressedEntryProof> {
        match self.hunk_type()? {
            CompressionTypeLegacy::Compressed => {
                Ok(CompressedEntryProof(self.block_offset(), self.block_size()))
            }
            _ => Err(Error::InvalidParameter),
        }
    }

    /// Obtain a proof that the hunk this entry refers to is uncompressed.
    /// If the hunk is compressed, returns `Error::InvalidParameter`.
    #[inline(always)]
    pub(crate) fn prove_uncompressed(&self) -> Result<UncompressedEntryProof> {
        match self.hunk_type()? {
            CompressionTypeLegacy::Uncompressed => Ok(UncompressedEntryProof(
                self.block_offset(),
                self.block_size(),
            )),
            _ => Err(Error::InvalidParameter),
        }
    }
}

impl V5CompressedMapEntry<'_> {
    /// Returns the hunk type of the compressed entry.
    pub fn hunk_type(&self) -> Result<CompressionTypeV5> {
        CompressionTypeV5::from_u8(self.0[0]).ok_or(Error::UnsupportedFormat)
    }

    /// Returns the offset to the compressed data of this hunk.
    pub fn block_offset(&self) -> Result<u64> {
        Ok(Cursor::new(&self.0[4..]).read_u48::<BigEndian>()?)
    }

    /// Returns the size of the compressed hunk.
    pub fn block_size(&self) -> Result<u32> {
        Ok(Cursor::new(&self.0[1..]).read_u24::<BigEndian>()?)
    }

    /// Returns the CRC16 checksum of the hunk data once uncompressed.
    pub fn hunk_crc(&self) -> Result<u16> {
        Ok(Cursor::new(&self.0[10..]).read_u16::<BigEndian>()?)
    }

    /// Obtain a proof that the hunk this entry refers to is compressed.
    /// If the hunk is uncompressed, returns `Error::InvalidParameter`.
    #[inline(always)]
    pub(crate) fn prove_compressed(&self) -> Result<CompressedEntryProof> {
        match self.hunk_type()? {
            CompressionTypeV5::CompressionType0
            | CompressionTypeV5::CompressionType1
            | CompressionTypeV5::CompressionType2
            | CompressionTypeV5::CompressionType3 => Ok(CompressedEntryProof(
                self.block_offset()?,
                self.block_size()?,
            )),
            _ => Err(Error::InvalidParameter),
        }
    }

    /// Obtain a proof that the hunk this entry refers to is uncompressed.
    /// If the hunk is uncompressed, returns `Error::InvalidParameter`.
    #[inline(always)]
    pub(crate) fn prove_uncompressed(&self) -> Result<UncompressedEntryProof> {
        match self.hunk_type()? {
            CompressionTypeV5::CompressionNone => Ok(UncompressedEntryProof(
                self.block_offset()?,
                self.block_size()?,
            )),
            _ => Err(Error::InvalidParameter),
        }
    }
}

impl V5UncompressedMapEntry<'_> {
    /// Returns the offset to the data of this hunk.
    pub fn block_offset(&self) -> Result<u64> {
        let off = Cursor::new(self.0).read_u32::<BigEndian>()?;
        Ok(off as u64 * self.1 as u64)
    }

    /// Returns size of the hunk data. For an uncompressed hunk, this is equal to the hunk
    /// size of the [`Chd`](crate::Chd).
    pub fn block_size(&self) -> u32 {
        self.1
    }

    /// Obtain a proof that the hunk this entry refers to is uncompressed.
    #[inline(always)]
    pub(crate) fn prove_uncompressed(&self) -> Result<UncompressedEntryProof> {
        Ok(UncompressedEntryProof(
            self.block_offset()?,
            self.block_size(),
        ))
    }
}

/// The hunk map for a CHD file.
pub enum Map {
    /// A CHD v5 hunk map.
    V5(RawMapV5), // compressed
    /// A CHD v1-4 hunk map.
    Legacy(RawMapLegacy),
}

/// A map entry for a CHD file of unspecified version.
pub enum MapEntry<'a> {
    /// A CHD v5 map entry for a compressed hunk.
    V5Compressed(V5CompressedMapEntry<'a>),
    /// A CHD v5 map entry for an uncompressed hunk.
    V5Uncompressed(V5UncompressedMapEntry<'a>),
    /// A CHD v1-4 map entry.
    LegacyEntry(&'a LegacyMapEntry),
}

/// A proof that a hunk is compressed.
/// An instance of this type can only be constructed from an compressed hunk.
pub(crate) struct CompressedEntryProof(u64, u32);
impl CompressedEntryProof {
    /// Returns the offset to the compressed data of this hunk.
    pub fn block_offset(&self) -> u64 {
        self.0
    }

    /// Returns the size of the compressed hunk.
    pub fn block_size(&self) -> u32 {
        self.1
    }
}

/// A proof that a hunk is not compressed.
/// An instance of this type can only be constructed from an uncompressed hunk.
pub(crate) struct UncompressedEntryProof(u64, u32);
impl UncompressedEntryProof {
    /// The offset to the uncompressed data of this hunk.
    pub fn block_offset(&self) -> u64 {
        self.0
    }

    /// Returns the size of the uncompressed hunk.
    pub fn block_size(&self) -> u32 {
        self.1
    }
}

/// Iterator for [`Map`](crate::map::Map).
pub struct MapEntries<'a> {
    map: &'a Map,
    curr: usize,
}

impl<'a> Iterator for MapEntries<'a> {
    type Item = MapEntry<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.curr == self.map.len() {
            None
        } else {
            let curr = self.curr;
            self.curr += 1;
            self.map.get_entry(curr)
        }
    }
}

impl Map {
    /// Gets the number of entries in the CHD Map.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        match self {
            Map::V5(m) => {
                // map_entry_bytes = 12 if compressed, else 4
                let map_entry_bytes = if m.1 {
                    V5_COMPRESSED_MAP_ENTRY_SIZE
                } else {
                    V5_UNCOMPRESSED_MAP_ENTRY_SIZE
                };
                m.0.len() / map_entry_bytes
            }
            Map::Legacy(m) => m.0.len(),
        }
    }

    /// Gets the `MapEntry` for the specified hunk number if it exists.
    pub fn get_entry(&self, hunk_num: usize) -> Option<MapEntry> {
        match self {
            Map::V5(m) => {
                let map_entry_bytes = if m.1 { 12 } else { 4 };

                let entry_slice =
                    &m.0.get(hunk_num * map_entry_bytes..(hunk_num + 1) * map_entry_bytes);
                if let &Some(entry_slice) = entry_slice {
                    return if m.1 {
                        <&[u8; 12]>::try_from(entry_slice)
                            .map(|e| MapEntry::V5Compressed(V5CompressedMapEntry(e)))
                            .ok()
                    } else {
                        <&[u8; 4]>::try_from(entry_slice)
                            .map(|e| MapEntry::V5Uncompressed(V5UncompressedMapEntry(e, m.2)))
                            .ok()
                    };
                }
                None
            }
            Map::Legacy(m) => m.0.get(hunk_num).map(MapEntry::LegacyEntry),
        }
    }

    /// Gets an iterator over the entries of this hunk map.
    pub fn iter(&self) -> MapEntries {
        MapEntries { map: self, curr: 0 }
    }

    /// Reads the hunk map from the provided stream given the parameters in the header,
    /// which must have the same stream provenance as the input header.
    pub fn try_read_map<F: Read + Seek>(header: &Header, mut file: F) -> Result<Map> {
        match header {
            Header::V5Header(v5) => {
                Ok(Map::V5(read_map_v5(v5, &mut file, header.is_compressed())?))
            }
            Header::V3Header(_) | Header::V4Header(_) => {
                Ok(Map::Legacy(RawMapLegacy(read_map_legacy::<
                    _,
                    V3_MAP_ENTRY_SIZE,
                >(header, file)?)))
            }
            Header::V2Header(_) | Header::V1Header(_) => {
                Ok(Map::Legacy(RawMapLegacy(read_map_legacy::<
                    _,
                    V1_MAP_ENTRY_SIZE,
                >(header, file)?)))
            }
        }
    }
}

fn read_map_legacy<F: Read + Seek, const MAP_ENTRY_SIZE: usize>(
    header: &Header,
    mut file: F,
) -> Result<Vec<LegacyMapEntry>> {
    // Probably can express this better in the type system once const generics get a bit more stabilized.
    // Essentially we ensure at compile time that the only possible MAP_ENTRY_SIZEs are
    // V3_MAP_ENTRY_SIZE or V1_MAP_ENTRY_SIZE.
    const_assert!(MAP_ENTRY_SIZE: usize => V3_MAP_ENTRY_SIZE >=
        MAP_ENTRY_SIZE && (MAP_ENTRY_SIZE == V3_MAP_ENTRY_SIZE || MAP_ENTRY_SIZE == V1_MAP_ENTRY_SIZE));
    let mut map = Vec::new();

    let mut max_off = 0;
    let mut cookie = [0u8; MAP_ENTRY_SIZE];
    file.seek(SeekFrom::Start(0))?;

    // wrap into bufreader
    let mut file = BufReader::new(file);
    file.seek(SeekFrom::Start(header.len() as u64))?;

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
                read_map_entry_v1(entry, header.hunk_size())
            }
            _ => unreachable!(),
        };

        if let Some(CompressionTypeLegacy::Compressed) | Some(CompressionTypeLegacy::Uncompressed) =
            CompressionTypeLegacy::from_u8(entry.flags & MAP_ENTRY_FLAG_TYPE_MASK)
        {
            max_off = std::cmp::max(max_off, entry.offset + entry.length as u64);
        }
        map.push(entry);
    }

    // verify cookie
    file.read_exact(&mut cookie)?;

    // need to confirm this is the same behaviour
    if &cookie[..] < b"EndOfListCookie\0" {
        return Err(Error::InvalidFile);
    }

    if max_off > file.seek(SeekFrom::End(0))? {
        return Err(Error::InvalidFile);
    }

    Ok(map)
}

#[inline]
fn read_map_entry_v1(val: u64, hunk_bytes: u32) -> LegacyMapEntry {
    let length = (val >> 44) as u32;
    let flags = MAP_ENTRY_FLAG_NO_CRC
        | if length == hunk_bytes {
            CompressionTypeLegacy::Uncompressed as u8
        } else {
            CompressionTypeLegacy::Compressed as u8
        };
    let offset = (val << 20) >> 20;
    LegacyMapEntry {
        offset,
        crc: None,
        length,
        flags,
    }
}

#[inline]
fn read_map_entry_v3(buf: &[u8; V3_MAP_ENTRY_SIZE]) -> Result<LegacyMapEntry> {
    let mut read = Cursor::new(buf);
    let offset = read.read_u64::<BigEndian>()?;
    let crc = read.read_u32::<BigEndian>()?;
    // confirm widening shift.
    let length: u32 = read.read_u16::<BigEndian>()? as u32 | (buf[14] as u32) << 16;
    let flags = buf[15];
    Ok(LegacyMapEntry {
        offset,
        crc: Some(crc),
        length,
        flags,
    })
}

fn read_map_v5<F: Read + Seek>(
    header: &HeaderV5,
    mut file: F,
    is_compressed: bool,
) -> Result<RawMapV5> {
    let map_size = header.hunk_count as usize * header.map_entry_bytes as usize;
    let mut raw_map = vec![0u8; map_size];

    if !is_compressed {
        file.seek(SeekFrom::Start(header.map_offset))?;
        file.read_exact(&mut raw_map[..])?;
        return Ok(RawMapV5(raw_map, is_compressed, header.hunk_bytes));
    }

    // Read compressed map parameters.
    file.seek(SeekFrom::Start(header.map_offset))?;

    let map_bytes = file.read_u32::<BigEndian>()?;
    let first_offs = file.read_u48::<BigEndian>()?;
    let map_crc = file.read_u16::<BigEndian>()?;
    let length_bits = file.read_u8()?;
    let self_bits = file.read_u8()?;
    let parent_bits = file.read_u8()?;

    // Read the map data
    let mut compressed: Vec<u8> = vec![0u8; map_bytes as usize];
    file.seek(SeekFrom::Start(header.map_offset + 16))?;
    file.read_exact(&mut compressed[..])?;

    let mut bitstream = BitReader::new(&compressed[..]);
    let decoder = HuffmanDecoder::<16, 8, { lookup_len::<8>() }>::from_tree_rle(&mut bitstream)?;

    let mut rep_count = 0;
    let mut last_cmp = 0;

    // V5 Map data is Huffman-RLE encoded so we need to expand.
    for map_slice in raw_map.chunks_exact_mut(V5_COMPRESSED_MAP_ENTRY_SIZE) {
        if rep_count > 0 {
            map_slice[0] = last_cmp;
            rep_count -= 1;
        } else {
            let val = decoder.decode_one(&mut bitstream)? as u8;
            match CompressionTypeV5::from_u8(val).ok_or(Error::DecompressionError)? {
                CompressionTypeV5::CompressionRleSmall => {
                    // COMPRESSION_RLE_SMALL
                    map_slice[0] = last_cmp;
                    rep_count = 2 + decoder.decode_one(&mut bitstream)?;
                }
                CompressionTypeV5::CompressionRleLarge => {
                    // COMPRESSION_RLE_LARGE
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

    // Iterate and decompress each map entry.
    let mut curr_off = first_offs;
    let mut last_self = 0;
    let mut last_parent = 0;

    for (hunk_num, map_slice) in raw_map
        .chunks_exact_mut(V5_COMPRESSED_MAP_ENTRY_SIZE)
        .enumerate()
    {
        let mut off = curr_off;
        let mut len: u32 = 0;
        let mut crc: u16 = 0;

        match CompressionTypeV5::from_u8(map_slice[0]).ok_or(Error::DecompressionError)? {
            CompressionTypeV5::CompressionType0
            | CompressionTypeV5::CompressionType1
            | CompressionTypeV5::CompressionType2
            | CompressionTypeV5::CompressionType3 => {
                len = bitstream.read_u32(length_bits)?;
                curr_off += len as u64;
                crc = bitstream.read_u32(16)? as u16;
            }
            CompressionTypeV5::CompressionNone => {
                len = header.hunk_bytes;
                curr_off += len as u64;
                crc = bitstream.read_u32(16)? as u16;
            }
            CompressionTypeV5::CompressionSelf => {
                off = bitstream.read_u64(self_bits)?;
                last_self = off;
            }
            CompressionTypeV5::CompressionParent => {
                off = bitstream.read_u64(parent_bits)?;
                last_parent = off;
            }

            // Expand pseudo codecs to concrete.
            CompressionTypeV5::CompressionSelf1 => {
                last_self += 1;
                map_slice[0] = CompressionTypeV5::CompressionSelf as u8;
                off = last_self;
            }
            CompressionTypeV5::CompressionSelf0 => {
                map_slice[0] = CompressionTypeV5::CompressionSelf as u8;
                off = last_self;
            }
            CompressionTypeV5::CompressionParentSelf => {
                map_slice[0] = CompressionTypeV5::CompressionParent as u8;
                off = ((hunk_num * header.hunk_bytes as usize) / header.unit_bytes as usize) as u64;
                last_parent = off;
            }
            CompressionTypeV5::CompressionParent1 => {
                last_parent += (header.hunk_bytes / header.unit_bytes) as u64
            }
            CompressionTypeV5::CompressionParent0 => {
                map_slice[0] = CompressionTypeV5::CompressionParent as u8;
                off = last_parent;
            }
            _ => return Err(Error::DecompressionError),
        }

        let mut cursor = Cursor::new(&mut map_slice[1..]);
        cursor.write_u24::<BigEndian>(len)?;
        cursor.write_u48::<BigEndian>(off)?;
        cursor.write_u16::<BigEndian>(crc)?;
    }

    // Verify map CRC
    if crate::block_hash::CRC16.checksum(&raw_map[0..header.hunk_count as usize * 12]) != map_crc {
        return Err(Error::DecompressionError);
    }

    Ok(RawMapV5(raw_map, is_compressed, header.hunk_bytes))
}
