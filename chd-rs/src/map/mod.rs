//! Types and methods relating to the CHD hunk map.

use std::io::{Read, Seek, SeekFrom};

use bitreader::BitReader;
use num_derive::{FromPrimitive, ToPrimitive};
use num_traits::FromPrimitive;
use zerocopy::{FromBytes, FromZeros, IntoBytes};

use crate::error::{Error, Result};
use crate::header::{Header, HeaderV5};
use crate::huffman::{lookup_len, HuffmanDecoder};
use raw::{
    RawCompressedMapHeader, RawMapEntryV1, RawMapEntryV3, RawMapEntryV5, RawMapEntryV5Uncompressed,
};

mod raw;

pub(crate) const V5_UNCOMPRESSED_MAP_ENTRY_SIZE: usize = 4;
pub(crate) const V5_COMPRESSED_MAP_ENTRY_SIZE: usize = 12;
const V3_MAP_ENTRY_SIZE: usize = 16; // V3-V4
const V1_MAP_ENTRY_SIZE: usize = 8; // V1-V2
const MAP_ENTRY_FLAG_TYPE_MASK: u8 = 0x0f; // type of hunk
const MAP_ENTRY_FLAG_NO_CRC: u8 = 0x10; // no crc is present
const END_OF_LIST_COOKIE: &[u8; V3_MAP_ENTRY_SIZE] = b"EndOfListCookie\0";

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
pub struct RawMapV5(V5Map);
/// Opaque type for a legacy map.
pub struct RawMapLegacy(Vec<LegacyMapEntry>);

enum V5Map {
    /// The decoded entries of a compressed map.
    Compressed(Vec<RawMapEntryV5>),
    /// The entries of an uncompressed map.
    Uncompressed {
        entries: Vec<RawMapEntryV5Uncompressed>,
        hunk_bytes: u32,
    },
}

impl V5Map {
    fn as_bytes(&self) -> &[u8] {
        match self {
            V5Map::Compressed(entries) => entries.as_bytes(),
            V5Map::Uncompressed { entries, .. } => entries.as_bytes(),
        }
    }
}

impl From<RawMapV5> for Vec<u8> {
    fn from(map: RawMapV5) -> Self {
        map.0.as_bytes().to_vec()
    }
}

impl From<&RawMapV5> for Vec<u8> {
    fn from(map: &RawMapV5) -> Self {
        map.0.as_bytes().to_vec()
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
pub struct V5CompressedMapEntry<'a>(&'a RawMapEntryV5);

/// A CHD V5 map entry for an uncompressed hunk.
pub struct V5UncompressedMapEntry<'a>(&'a RawMapEntryV5Uncompressed, u32);

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
        CompressionTypeV5::from_u8(self.0.compression).ok_or(Error::UnsupportedFormat)
    }

    /// Returns the offset to the compressed data of this hunk.
    pub fn block_offset(&self) -> u64 {
        self.0.offset()
    }

    /// Returns the size of the compressed hunk.
    pub fn block_size(&self) -> u32 {
        self.0.length()
    }

    /// Returns the CRC16 checksum of the hunk data once uncompressed.
    pub fn hunk_crc(&self) -> u16 {
        self.0.crc.get()
    }

    /// Obtain a proof that the hunk this entry refers to is compressed.
    /// If the hunk is uncompressed, returns `Error::InvalidParameter`.
    #[inline(always)]
    pub(crate) fn prove_compressed(&self) -> Result<CompressedEntryProof> {
        match self.hunk_type()? {
            CompressionTypeV5::CompressionType0
            | CompressionTypeV5::CompressionType1
            | CompressionTypeV5::CompressionType2
            | CompressionTypeV5::CompressionType3 => {
                Ok(CompressedEntryProof(self.block_offset(), self.block_size()))
            }
            _ => Err(Error::InvalidParameter),
        }
    }

    /// Obtain a proof that the hunk this entry refers to is uncompressed.
    /// If the hunk is uncompressed, returns `Error::InvalidParameter`.
    #[inline(always)]
    pub(crate) fn prove_uncompressed(&self) -> Result<UncompressedEntryProof> {
        match self.hunk_type()? {
            CompressionTypeV5::CompressionNone => Ok(UncompressedEntryProof(
                self.block_offset(),
                self.block_size(),
            )),
            _ => Err(Error::InvalidParameter),
        }
    }
}

impl V5UncompressedMapEntry<'_> {
    /// Returns the offset to the data of this hunk.
    pub fn block_offset(&self) -> u64 {
        self.0 .0.get() as u64 * self.1 as u64
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
            self.block_offset(),
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
            Map::V5(RawMapV5(V5Map::Compressed(entries))) => entries.len(),
            Map::V5(RawMapV5(V5Map::Uncompressed { entries, .. })) => entries.len(),
            Map::Legacy(m) => m.0.len(),
        }
    }

    /// Gets the `MapEntry` for the specified hunk number if it exists.
    pub fn get_entry(&self, hunk_num: usize) -> Option<MapEntry> {
        match self {
            Map::V5(RawMapV5(V5Map::Compressed(entries))) => entries
                .get(hunk_num)
                .map(|entry| MapEntry::V5Compressed(V5CompressedMapEntry(entry))),
            Map::V5(RawMapV5(V5Map::Uncompressed {
                entries,
                hunk_bytes,
            })) => entries
                .get(hunk_num)
                .map(|entry| MapEntry::V5Uncompressed(V5UncompressedMapEntry(entry, *hunk_bytes))),
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
                    RawMapEntryV3,
                >(header, file)?)))
            }
            Header::V2Header(_) | Header::V1Header(_) => {
                Ok(Map::Legacy(RawMapLegacy(read_map_legacy::<
                    _,
                    RawMapEntryV1,
                >(header, file)?)))
            }
        }
    }

    /// Serialize a V5 map into it's on-disk representation, given the header of the CHD.
    ///
    /// Compressed maps are written as the compressed map header followed by the
    /// compressed map data. Uncompressed maps are written as is.
    ///
    /// Returns [`UnsupportedVersion`](Error::UnsupportedVersion) for V1-4 maps, and
    /// [`InvalidData`](Error::InvalidData) if the hunk data the map refers to is not
    /// stored contiguously in hunk order, which can not be represented by a compressed map.
    #[cfg(feature = "encode")]
    pub fn to_bytes(&self, header: &Header) -> Result<Vec<u8>> {
        let Header::V5Header(header) = header else {
            return Err(Error::UnsupportedVersion);
        };
        match self {
            Map::V5(RawMapV5(V5Map::Compressed(entries))) => {
                compress_map_v5(entries, header.hunk_bytes, header.unit_bytes)
            }
            Map::V5(RawMapV5(V5Map::Uncompressed { entries, .. })) => {
                Ok(entries.as_bytes().to_vec())
            }
            Map::Legacy(_) => Err(Error::UnsupportedVersion),
        }
    }
}

/// Compress the entries of a V5 map.
///
/// See [`chd_file::compress_v5_map`](https://github.com/mamedev/mame/blob/74b9fae5a3ca3c99828fe41da154e3c84951414e/src/lib/util/chd.cpp#L2212).
#[cfg(feature = "encode")]
fn compress_map_v5(entries: &[RawMapEntryV5], hunk_bytes: u32, unit_bytes: u32) -> Result<Vec<u8>> {
    use crate::huffman::HuffmanEncoder;
    use bitstream_io::{BigEndian, BitWrite, BitWriter};
    use raw::V5_COMPRESSED_MAP_HEADER_SIZE;
    use CompressionTypeV5::*;

    let mut stored = entries
        .iter()
        .filter(|entry| entry.compression <= CompressionNone as u8)
        .peekable();
    let first_offset = stored.peek().map_or(0, |entry| entry.offset());
    let mut next_offset = first_offset;
    for entry in stored {
        let uncompressed = entry.compression == CompressionNone as u8;
        if entry.offset() != next_offset || (uncompressed && entry.length() != hunk_bytes) {
            return Err(Error::InvalidData);
        }
        next_offset += entry.length() as u64;
    }

    // Self and parent references that follow from the previous reference are promoted
    // to the more compact pseudo codecs.
    let mut last_self = 0;
    let mut last_parent = 0;
    let mut types = Vec::with_capacity(entries.len());
    for (hunk_num, entry) in entries.iter().enumerate() {
        let promoted = match CompressionTypeV5::from_u8(entry.compression) {
            Some(CompressionSelf) => {
                let hunk = entry.offset();
                let promoted = if hunk == last_self {
                    CompressionSelf0
                } else if hunk == last_self + 1 {
                    CompressionSelf1
                } else {
                    CompressionSelf
                };
                last_self = hunk;
                promoted as u8
            }
            Some(CompressionParent) => {
                let unit = entry.offset();
                let promoted = if unit == hunk_num as u64 * hunk_bytes as u64 / unit_bytes as u64 {
                    CompressionParentSelf
                } else if unit == last_parent {
                    CompressionParent0
                } else if unit == last_parent + (hunk_bytes / unit_bytes) as u64 {
                    CompressionParent1
                } else {
                    CompressionParent
                };
                last_parent = unit;
                promoted as u8
            }
            _ => entry.compression,
        };
        types.push(promoted);
    }

    let (mut max_length, mut max_self, mut max_parent) = (0, 0, 0);
    for (entry, &compression) in entries.iter().zip(&types) {
        match CompressionTypeV5::from_u8(compression) {
            Some(CompressionSelf) => max_self = max_self.max(entry.offset()),
            Some(CompressionParent) => max_parent = max_parent.max(entry.offset()),
            _ => max_length = max_length.max(entry.length() as u64),
        }
    }
    let bit_width = |value: u64| (u64::BITS - value.leading_zeros()) as u8;
    let length_bits = bit_width(max_length);
    let self_bits = bit_width(max_self);
    let parent_bits = bit_width(max_parent);

    // Leave room for the header, which is written once the data is compressed.
    let mut output = vec![0u8; V5_COMPRESSED_MAP_HEADER_SIZE];
    let mut writer = BitWriter::endian(&mut output, BigEndian);
    let huffman_err = |_| Error::CompressionError;
    let io_err = |_| Error::CompressionError;

    // RLE encode the types.
    let mut rle = Vec::new();
    let mut last = 0;
    for run in types.chunk_by(|a, b| a == b) {
        let mut repeats = run.len();
        if run[0] != last {
            last = run[0];
            rle.push(last);
            repeats -= 1;
        }

        while repeats > 0 {
            match repeats {
                1..=2 => {
                    rle.push(last);
                    repeats -= 1;
                }
                3..=18 => {
                    rle.extend([CompressionRleSmall as u8, (repeats - 3) as u8]);
                    repeats = 0;
                }
                _ => {
                    let count = repeats.min(3 + 16 + 255);
                    rle.extend([
                        CompressionRleLarge as u8,
                        ((count - 3 - 16) >> 4) as u8,
                        ((count - 3 - 16) & 15) as u8,
                    ]);
                    repeats -= count;
                }
            }
        }
    }

    // The RLE encoded types are Huffman encoded with a tree for 16 codes of at most 8 bits.
    let mut encoder = HuffmanEncoder::<'_, 16, 8>::new();
    encoder
        .try_compute_tree(rle.iter().map(|&value| value as u32))
        .map_err(huffman_err)?;
    encoder.export_tree_rle(&mut writer).map_err(huffman_err)?;
    for &value in &rle {
        encoder
            .encode_one(&mut writer, value as u32)
            .map_err(huffman_err)?;
    }

    // Write the data each entry needs for its type.
    for (entry, &compression) in entries.iter().zip(&types) {
        match CompressionTypeV5::from_u8(compression) {
            Some(CompressionType0 | CompressionType1 | CompressionType2 | CompressionType3) => {
                writer
                    .write_var(length_bits as u32, entry.length())
                    .map_err(io_err)?;
                writer.write_var(16, entry.crc.get()).map_err(io_err)?;
            }
            // The length of an uncompressed hunk is not stored.
            Some(CompressionNone) => writer.write_var(16, entry.crc.get()).map_err(io_err)?,
            Some(CompressionSelf) => writer
                .write_var(self_bits as u32, entry.offset())
                .map_err(io_err)?,
            Some(CompressionParent) => writer
                .write_var(parent_bits as u32, entry.offset())
                .map_err(io_err)?,
            // References that follow from the previous ones need nothing more.
            Some(
                CompressionSelf0
                | CompressionSelf1
                | CompressionParentSelf
                | CompressionParent0
                | CompressionParent1,
            ) => {}
            _ => return Err(Error::InvalidData),
        }
    }
    writer.byte_align().map_err(io_err)?;

    let mut map_header = RawCompressedMapHeader::new_zeroed();
    map_header.length = ((output.len() - V5_COMPRESSED_MAP_HEADER_SIZE) as u32).into();
    map_header.set_first_offset(first_offset);
    // The CRC covers the entries with self and parent references in their concrete form.
    map_header.crc = crate::block_hash::CRC16.checksum(entries.as_bytes()).into();
    map_header.length_bits = length_bits;
    map_header.self_bits = self_bits;
    map_header.parent_bits = parent_bits;
    output[..V5_COMPRESSED_MAP_HEADER_SIZE].copy_from_slice(map_header.as_bytes());
    Ok(output)
}

/// A raw V1-V4 map entry.
trait RawLegacyMapEntry: FromBytes + zerocopy::KnownLayout + zerocopy::Immutable {
    /// Convert the raw entry, given the hunk size of the CHD.
    fn to_entry(&self, hunk_bytes: u32) -> LegacyMapEntry;
}

impl RawLegacyMapEntry for RawMapEntryV1 {
    fn to_entry(&self, hunk_bytes: u32) -> LegacyMapEntry {
        // V1-V2 entries do not store a type, so hunk sized entries are taken to be uncompressed.
        let length = self.length();
        let flags = MAP_ENTRY_FLAG_NO_CRC
            | if length == hunk_bytes {
                CompressionTypeLegacy::Uncompressed as u8
            } else {
                CompressionTypeLegacy::Compressed as u8
            };
        LegacyMapEntry {
            offset: self.offset(),
            crc: None,
            length,
            flags,
        }
    }
}

impl RawLegacyMapEntry for RawMapEntryV3 {
    fn to_entry(&self, _hunk_bytes: u32) -> LegacyMapEntry {
        LegacyMapEntry {
            offset: self.offset.get(),
            crc: Some(self.crc.get()),
            length: self.length(),
            flags: self.flags,
        }
    }
}

/// Read a V1-V4 map.
fn read_map_legacy<F: Read + Seek, E: RawLegacyMapEntry>(
    header: &Header,
    mut file: F,
) -> Result<Vec<LegacyMapEntry>> {
    let entry_size = std::mem::size_of::<E>();

    file.seek(SeekFrom::Start(header.len() as u64))?;
    let mut raw_map = vec![0u8; header.hunk_count() as usize * entry_size];
    file.read_exact(&mut raw_map)?;
    let raw_entries = <[E]>::ref_from_bytes(&raw_map).map_err(|_| Error::InvalidData)?;

    let mut max_off = 0;
    let map: Vec<LegacyMapEntry> = raw_entries
        .iter()
        .map(|entry| entry.to_entry(header.hunk_size()))
        .collect();
    for entry in &map {
        if let Some(CompressionTypeLegacy::Compressed) | Some(CompressionTypeLegacy::Uncompressed) =
            CompressionTypeLegacy::from_u8(entry.flags & MAP_ENTRY_FLAG_TYPE_MASK)
        {
            max_off = std::cmp::max(max_off, entry.offset + entry.length as u64);
        }
    }

    // The map is followed by an entry-sized prefix of the end of list cookie
    // https://github.com/rtissera/libchdr/blob/cdcb714235b9ff7d207b703260706a364282b063/src/libchdr_chd.c#L2496-L2499).
    let mut cookie = vec![0u8; entry_size];
    file.read_exact(&mut cookie)?;
    if cookie[..] != END_OF_LIST_COOKIE[..entry_size] {
        return Err(Error::InvalidFile);
    }

    if max_off > file.seek(SeekFrom::End(0))? {
        return Err(Error::InvalidFile);
    }

    Ok(map)
}

fn read_map_v5<F: Read + Seek>(
    header: &HeaderV5,
    mut file: F,
    is_compressed: bool,
) -> Result<RawMapV5> {
    file.seek(SeekFrom::Start(header.map_offset))?;

    if !is_compressed {
        let mut entries = vec![RawMapEntryV5Uncompressed::new_zeroed(); header.hunk_count as usize];
        file.read_exact(entries.as_mut_bytes())?;
        return Ok(RawMapV5(V5Map::Uncompressed {
            entries,
            hunk_bytes: header.hunk_bytes,
        }));
    }

    // Read compressed map parameters.
    let mut map_header = RawCompressedMapHeader::new_zeroed();
    file.read_exact(map_header.as_mut_bytes())?;

    // Read the map data
    let mut compressed: Vec<u8> = vec![0u8; map_header.length.get() as usize];
    file.read_exact(&mut compressed[..])?;

    let mut bitstream = BitReader::new(&compressed[..]);
    let decoder = HuffmanDecoder::<16, 8, { lookup_len::<8>() }>::from_tree_rle(&mut bitstream)?;

    // V5 Map data is Huffman-RLE encoded so we need to expand.
    let mut compression = Vec::with_capacity(header.hunk_count as usize);
    let mut rep_count = 0;
    let mut last_cmp = 0;
    while compression.len() < header.hunk_count as usize {
        if rep_count > 0 {
            compression.push(last_cmp);
            rep_count -= 1;
            continue;
        }

        let val = decoder.decode_one(&mut bitstream)? as u8;
        match CompressionTypeV5::from_u8(val).ok_or(Error::DecompressionError)? {
            CompressionTypeV5::CompressionRleSmall => {
                compression.push(last_cmp);
                rep_count = 2 + decoder.decode_one(&mut bitstream)?;
            }
            CompressionTypeV5::CompressionRleLarge => {
                compression.push(last_cmp);
                rep_count = 2 + 16 + (decoder.decode_one(&mut bitstream)? << 4);
                rep_count += decoder.decode_one(&mut bitstream)?;
            }
            _ => {
                compression.push(val);
                last_cmp = val;
            }
        }
    }

    // Decode each map entry.
    let mut curr_off = map_header.first_offset();
    let mut last_self = 0;
    let mut last_parent = 0;
    let mut entries = Vec::with_capacity(header.hunk_count as usize);

    for (hunk_num, &compression) in compression.iter().enumerate() {
        let mut compression = compression;
        let mut off = curr_off;
        let mut len: u32 = 0;
        let mut crc: u16 = 0;

        match CompressionTypeV5::from_u8(compression).ok_or(Error::DecompressionError)? {
            CompressionTypeV5::CompressionType0
            | CompressionTypeV5::CompressionType1
            | CompressionTypeV5::CompressionType2
            | CompressionTypeV5::CompressionType3 => {
                len = bitstream.read_u32(map_header.length_bits)?;
                curr_off += len as u64;
                crc = bitstream.read_u32(16)? as u16;
            }
            CompressionTypeV5::CompressionNone => {
                len = header.hunk_bytes;
                curr_off += len as u64;
                crc = bitstream.read_u32(16)? as u16;
            }
            CompressionTypeV5::CompressionSelf => {
                off = bitstream.read_u64(map_header.self_bits)?;
                last_self = off;
            }
            CompressionTypeV5::CompressionParent => {
                off = bitstream.read_u64(map_header.parent_bits)?;
                last_parent = off;
            }

            // Expand pseudo codecs to concrete.
            CompressionTypeV5::CompressionSelf1 => {
                last_self += 1;
                compression = CompressionTypeV5::CompressionSelf as u8;
                off = last_self;
            }
            CompressionTypeV5::CompressionSelf0 => {
                compression = CompressionTypeV5::CompressionSelf as u8;
                off = last_self;
            }
            CompressionTypeV5::CompressionParentSelf => {
                compression = CompressionTypeV5::CompressionParent as u8;
                off = ((hunk_num * header.hunk_bytes as usize) / header.unit_bytes as usize) as u64;
                last_parent = off;
            }
            CompressionTypeV5::CompressionParent1 => {
                last_parent += (header.hunk_bytes / header.unit_bytes) as u64;
                compression = CompressionTypeV5::CompressionParent as u8;
                off = last_parent;
            }
            CompressionTypeV5::CompressionParent0 => {
                compression = CompressionTypeV5::CompressionParent as u8;
                off = last_parent;
            }
            _ => return Err(Error::DecompressionError),
        }

        entries.push(RawMapEntryV5::new(compression, len, off, crc));
    }

    // Verify map CRC
    if crate::block_hash::CRC16.checksum(entries.as_bytes()) != map_header.crc.get() {
        return Err(Error::DecompressionError);
    }

    Ok(RawMapV5(V5Map::Compressed(entries)))
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use crate::header::{HeaderV5, Version};
    use std::io::Cursor;

    const HUNK_BYTES: u32 = 4096;
    const UNIT_BYTES: u32 = 512;
    const UNITS_PER_HUNK: u64 = (HUNK_BYTES / UNIT_BYTES) as u64;

    fn header(hunk_count: usize, compressed: bool) -> HeaderV5 {
        HeaderV5 {
            version: Version::ChdV5,
            length: 124,
            compression: [
                if compressed {
                    crate::make_tag(b"lzma")
                } else {
                    0
                },
                0,
                0,
                0,
            ],
            logical_bytes: hunk_count as u64 * HUNK_BYTES as u64,
            map_offset: 0,
            meta_offset: 0,
            hunk_bytes: HUNK_BYTES,
            unit_bytes: UNIT_BYTES,
            sha1: [0; 20],
            parent_sha1: [0; 20],
            raw_sha1: [0; 20],
            unit_count: hunk_count as u64 * UNITS_PER_HUNK,
            hunk_count: hunk_count as u32,
            map_entry_bytes: if compressed { 12 } else { 4 },
        }
    }

    /// Entries using every kind of reference, with runs that need both RLE forms.
    fn entries() -> Vec<RawMapEntryV5> {
        use CompressionTypeV5::*;
        let mut entries = Vec::new();
        let mut offset = 124;
        let mut data = |entries: &mut Vec<RawMapEntryV5>, compression: u8, length: u32| {
            let crc = (entries.len() as u16).wrapping_mul(31);
            entries.push(RawMapEntryV5::new(compression, length, offset, crc));
            offset += length as u64;
        };
        let reference = |entries: &mut Vec<RawMapEntryV5>, compression: u8, target: u64| {
            entries.push(RawMapEntryV5::new(compression, 0, target, 0));
        };

        // Runs of 2, 10 (RLE small) and 300 (RLE large, split in two).
        for (codec, run) in [
            (CompressionType1 as u8, 2),
            (CompressionType0 as u8, 10),
            (CompressionType2 as u8, 300),
        ] {
            for i in 0..run {
                data(&mut entries, codec, 100 + i * 3);
            }
        }
        for _ in 0..5 {
            data(&mut entries, CompressionNone as u8, HUNK_BYTES);
        }

        // Self references: arbitrary, the same as the last, and one after the last.
        for target in [7, 7, 8, 9, 40, 2] {
            reference(&mut entries, CompressionSelf as u8, target);
        }

        // Parent references: the same position, the same as the last, one hunk after the
        // last, and arbitrary.
        let hunk = entries.len() as u64;
        for target in [
            hunk * UNITS_PER_HUNK,
            hunk * UNITS_PER_HUNK,
            (hunk + 2) * UNITS_PER_HUNK,
            1234,
            1234 + UNITS_PER_HUNK,
            99,
        ] {
            reference(&mut entries, CompressionParent as u8, target);
        }

        data(&mut entries, CompressionType3 as u8, 70000);
        entries
    }

    fn compressed_map(entries: Vec<RawMapEntryV5>) -> Map {
        Map::V5(RawMapV5(V5Map::Compressed(entries)))
    }

    #[test]
    fn compressed_map_roundtrip() {
        let entries = entries();
        let header = header(entries.len(), true);
        let bytes = compressed_map(entries.clone())
            .to_bytes(&Header::V5Header(header.clone()))
            .unwrap();

        let read = read_map_v5(&header, Cursor::new(&bytes), true).unwrap();
        assert_eq!(read.0.as_bytes(), entries.as_bytes());

        // Serializing the map read back gives the same bytes.
        let reserialized = Map::V5(read).to_bytes(&Header::V5Header(header)).unwrap();
        assert_eq!(reserialized, bytes);
    }

    #[test]
    fn uncompressed_map_roundtrip() {
        let entries: Vec<_> = (0..64u32)
            .map(|hunk| RawMapEntryV5Uncompressed(if hunk % 5 == 0 { 0 } else { hunk + 1 }.into()))
            .collect();
        let header = header(entries.len(), false);
        let map = Map::V5(RawMapV5(V5Map::Uncompressed {
            entries: entries.clone(),
            hunk_bytes: HUNK_BYTES,
        }));
        let bytes = map.to_bytes(&Header::V5Header(header.clone())).unwrap();
        assert_eq!(bytes, entries.as_bytes());

        let read = read_map_v5(&header, Cursor::new(&bytes), false).unwrap();
        assert_eq!(read.0.as_bytes(), entries.as_bytes());
    }

    #[test]
    fn compressed_map_must_be_contiguous() {
        let mut entries = entries();
        let last = entries.last_mut().unwrap();
        *last = RawMapEntryV5::new(last.compression, last.length(), last.offset() + 1, 0);
        let header = Header::V5Header(header(entries.len(), true));
        assert!(matches!(
            compressed_map(entries).to_bytes(&header),
            Err(Error::InvalidData)
        ));
    }

    #[test]
    fn uncompressed_hunks_must_be_hunk_sized() {
        let entries = vec![RawMapEntryV5::new(
            CompressionTypeV5::CompressionNone as u8,
            HUNK_BYTES - 1,
            124,
            0,
        )];
        let header = Header::V5Header(header(entries.len(), true));
        assert!(matches!(
            compressed_map(entries).to_bytes(&header),
            Err(Error::InvalidData)
        ));
    }
}
