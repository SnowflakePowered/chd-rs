use crate::block_hash::ChdBlockChecksum;
use crate::compression::CompressionCodec;
use crate::error::{ChdError, Result};
use crate::header::ChdHeader;
use crate::map::{
    ChdMap, CompressedEntryProof, LegacyEntryType, MapEntry, UncompressedEntryProof,
    V5CompressionType,
};

#[cfg(feature = "unstable_lending_iterators")]
use crate::iter::{HunkIter, MetadataIter};

use crate::metadata::MetadataRefIter;
use byteorder::{BigEndian, WriteBytesExt};
use crc::Crc;
use num_traits::ToPrimitive;
use std::io::{Cursor, Read, Seek, SeekFrom};

/// A CHD (MAME Compressed Hunks of Data) file.
pub struct ChdFile<F: Read + Seek> {
    file: F,
    header: ChdHeader,
    // feature(generic_associated_types) to be generic over all possible parents of G: Read+Seek?
    parent: Option<Box<ChdFile<F>>>,
    map: ChdMap,
    codecs: Vec<Box<dyn CompressionCodec>>,
}

impl<F: Read + Seek> ChdFile<F> {
    /// Open a CHD file from a `Read + Seek` stream. Optionally provide a parent of the same stream
    /// type.
    ///
    /// The CHD header and hunk map are read and validated immediately.
    pub fn open(mut file: F, parent: Option<Box<ChdFile<F>>>) -> Result<ChdFile<F>> {
        let header = ChdHeader::try_read_header(&mut file)?;
        // No point in checking writable because traits are read only.
        // In the future if we want to support a Write feature, will need to ensure writable.

        // Make sure we have a parent if we have one
        if parent.is_none() && header.has_parent() {
            return Err(ChdError::RequiresParent);
        }

        let map = ChdMap::try_read_map(&header, &mut file)?;
        let codecs = header.create_compression_codecs()?;

        Ok(ChdFile {
            file,
            header,
            parent,
            map,
            codecs,
        })
    }

    /// Returns a reference to the CHD header for this CHD file.
    pub fn header(&self) -> &ChdHeader {
        &self.header
    }

    /// Returns an iterator over references to metadata entries for this CHD file.
    ///
    /// The contents of each metadata entry are lazily read.
    pub fn metadata_refs(&mut self) -> MetadataRefIter<F> {
        let offset = self.header().meta_offset();
        if let Some(offset) = offset {
            MetadataRefIter::from_stream(&mut self.file, offset)
        } else {
            MetadataRefIter::dead(&mut self.file)
        }
    }

    #[cfg(feature = "unstable_lending_iterators")]
    #[cfg_attr(docsrs, doc(cfg(unstable_lending_iterators)))]
    /// Returns an iterator over metadata entries for this CHD file.
    ///
    /// The contents of each metadata entry are lazily read.
    pub fn metadata(&mut self) -> MetadataIter<F> {
        MetadataIter::new(self.metadata_refs())
    }

    /// Returns the hunk map of this CHD File.
    pub fn map(&self) -> &ChdMap {
        &self.map
    }

    /// Returns a reference to the given hunk in this CHD file.
    ///
    /// If the requested hunk is larger than the number of hunks in the CHD file,
    /// returns `ChdError::HunkOutOfRange`.
    pub fn hunk(&mut self, hunk_num: u32) -> Result<ChdHunk<F>> {
        if hunk_num >= self.header.hunk_count() {
            return Err(ChdError::HunkOutOfRange);
        }
        Ok(ChdHunk {
            inner: self,
            hunk_num,
        })
    }

    /// Allocates a buffer with the same length as the hunk size of this CHD file.
    pub fn get_hunksized_buffer(&self) -> Vec<u8> {
        let hunk_size = self.header.hunk_size() as usize;
        vec![0u8; hunk_size]
    }

    #[cfg_attr(docsrs, doc(cfg(unstable_lending_iterators)))]
    #[cfg(feature = "unstable_lending_iterators")]
    /// Returns an iterator over the hunks of this CHD file.
    pub fn hunks(&mut self) -> HunkIter<F> {
        HunkIter::new(self)
    }

    /// Consumes the `ChdFile` and returns the underlying reader and parent if present.
    pub fn into_inner(self) -> (F, Option<Box<ChdFile<F>>>) {
        (self.file, self.parent)
    }

    /// Returns a mutable reference to the inner stream.
    pub fn inner(&mut self) -> &mut F {
        &mut self.file
    }

    /// Returns a mutable reference to the inner parent stream if present.
    pub fn inner_parent(&mut self) -> Option<&mut F> {
        self.parent.as_deref_mut().map(|f| f.inner())
    }
}

/// A reference to a compressed Hunk in a CHD file.
pub struct ChdHunk<'a, F: Read + Seek> {
    inner: &'a mut ChdFile<F>,
    hunk_num: u32,
}

impl<'a, F: Read + Seek> ChdHunk<'a, F> {
    /// Buffer the compressed bytes into the hunk buffer.
    fn read_compressed_in(
        &mut self,
        map_entry: CompressedEntryProof,
        comp_buf: &mut Vec<u8>,
    ) -> Result<()> {
        let offset = map_entry.block_offset();
        let length = map_entry.block_size();

        comp_buf.resize(length as usize, 0);

        self.inner.file.seek(SeekFrom::Start(offset))?;
        let read = self.inner.file.read(comp_buf)?;
        if read != length as usize {
            return Err(ChdError::ReadError);
        }
        Ok(())
    }

    fn read_uncompressed(
        &mut self,
        map_entry: UncompressedEntryProof,
        dest: &mut [u8],
    ) -> Result<usize> {
        let offset = map_entry.block_offset();
        let length = map_entry.block_size();

        if dest.len() != length as usize {
            return Err(ChdError::InvalidParameter);
        }
        self.inner.file.seek(SeekFrom::Start(offset))?;
        let read = self.inner.file.read(dest)?;
        Ok(read)
    }

    fn read_hunk_legacy(&mut self, comp_buf: &mut Vec<u8>, dest: &mut [u8]) -> Result<usize> {
        let map_entry = self
            .inner
            .map()
            .get_entry(self.hunk_num as usize)
            .ok_or(ChdError::HunkOutOfRange)?;

        match map_entry {
            MapEntry::LegacyEntry(entry) => {
                let block_len = entry.block_size() as usize;
                let block_crc = entry.hunk_crc();
                let block_off = entry.block_offset();

                match entry.hunk_type()? {
                    LegacyEntryType::Compressed => {
                        // buffer the compressed data
                        let proof = entry.prove_compressed()?;
                        self.read_compressed_in(proof, comp_buf)?;
                        let res = &self.inner.codecs[0].decompress(&comp_buf[..block_len], dest)?;

                        Crc::<u32>::verify_block_checksum(block_crc, dest, res.total_out())
                    }
                    LegacyEntryType::Uncompressed => {
                        let proof = entry.prove_uncompressed()?;
                        let res = self.read_uncompressed(proof, dest)?;
                        Crc::<u32>::verify_block_checksum(block_crc, dest, res)
                    }
                    LegacyEntryType::Mini => {
                        let mut cursor = Cursor::new(dest);
                        cursor.write_u64::<BigEndian>(entry.block_offset())?;
                        let dest = cursor.into_inner();
                        let mut bytes_read_into = std::mem::size_of::<u64>();

                        // todo: optimize this operation
                        for off in
                            std::mem::size_of::<u64>()..self.inner.header().hunk_size() as usize
                        {
                            dest[off] = dest[off - 8];
                            bytes_read_into += 1;
                        }

                        Crc::<u32>::verify_block_checksum(block_crc, dest, bytes_read_into)
                    }
                    LegacyEntryType::SelfHunk => {
                        let mut self_hunk = self.inner.hunk(block_off as u32)?;
                        let res = self_hunk.read_hunk_in(comp_buf, dest)?;
                        Ok(res)
                    }
                    LegacyEntryType::ParentHunk => match self.inner.parent.as_deref_mut() {
                        None => Err(ChdError::RequiresParent),
                        Some(parent) => {
                            let mut parent = parent.hunk(block_off as u32)?;
                            let res = parent.read_hunk_in(comp_buf, dest)?;
                            Ok(res)
                        }
                    },
                    LegacyEntryType::ExternalCompressed => Err(ChdError::UnsupportedFormat),
                    LegacyEntryType::Invalid => Err(ChdError::InvalidData),
                }
            }
            _ => Err(ChdError::InvalidParameter),
        }
    }

    fn read_hunk_v5(&mut self, comp_buf: &mut Vec<u8>, dest: &mut [u8]) -> Result<usize> {
        let map_entry = self
            .inner
            .map()
            .get_entry(self.hunk_num as usize)
            .ok_or(ChdError::HunkOutOfRange)?;

        let has_parent = self.inner.header.has_parent();

        match map_entry {
            MapEntry::V5Compressed(entry) => {
                let block_off = entry.block_offset()?;
                let block_crc = Some(entry.hunk_crc()?);
                match entry.hunk_type()? {
                    comptype @ V5CompressionType::CompressionType0
                    | comptype @ V5CompressionType::CompressionType1
                    | comptype @ V5CompressionType::CompressionType2
                    | comptype @ V5CompressionType::CompressionType3 => {
                        // buffer the compressed data
                        let proof = entry.prove_compressed()?;

                        self.read_compressed_in(proof, comp_buf)?;

                        if let Some(codec) = self.inner.codecs.get_mut(comptype.to_usize().unwrap())
                        {
                            let res = codec.decompress(comp_buf, dest)?;
                            Crc::<u16>::verify_block_checksum(block_crc, dest, res.total_out())
                        } else {
                            Err(ChdError::UnsupportedFormat)
                        }
                    }
                    V5CompressionType::CompressionNone => {
                        let proof = entry.prove_uncompressed()?;
                        let res = self.read_uncompressed(proof, dest)?;
                        Crc::<u16>::verify_block_checksum(block_crc, dest, res)
                    }
                    V5CompressionType::CompressionSelf => {
                        let mut self_hunk = self.inner.hunk(block_off as u32)?;
                        let res = self_hunk.read_hunk_in(comp_buf, dest)?;
                        Ok(res)
                    }
                    V5CompressionType::CompressionParent => {
                        let hunk_bytes = self.inner.header().hunk_size();
                        let unit_bytes = self.inner.header().unit_bytes();
                        let units_in_hunk = hunk_bytes / unit_bytes;

                        match self.inner.parent.as_deref_mut() {
                            None => Err(ChdError::RequiresParent),
                            Some(parent) => {
                                let mut buf = vec![0u8; hunk_bytes as usize];

                                let mut parent_hunk =
                                    parent.hunk(block_off as u32 / units_in_hunk)?;
                                let res_1 = parent_hunk.read_hunk_in(comp_buf, &mut buf)?;

                                if block_off % units_in_hunk as u64 == 0 {
                                    dest.copy_from_slice(&buf);
                                    return Ok(res_1);
                                }

                                let remainder_in_hunk = block_off as usize % units_in_hunk as usize;
                                let hunk_split = (units_in_hunk as usize - remainder_in_hunk)
                                    * unit_bytes as usize;

                                dest[..hunk_split].copy_from_slice(
                                    &buf[remainder_in_hunk * unit_bytes as usize..][..hunk_split],
                                );

                                let mut parent_hunk =
                                    parent.hunk((block_off as u32 / units_in_hunk) + 1)?;
                                let _res_2 = parent_hunk.read_hunk_in(comp_buf, &mut buf)?;

                                dest[hunk_split..].copy_from_slice(
                                    &buf[..remainder_in_hunk
                                        * self.inner.header().unit_bytes() as usize],
                                );
                                Crc::<u16>::verify_block_checksum(
                                    block_crc,
                                    dest,
                                    hunk_split + remainder_in_hunk * unit_bytes as usize,
                                )
                            }
                        }
                    }
                    _ => Err(ChdError::UnsupportedFormat),
                }
            }
            MapEntry::V5Uncompressed(entry) => {
                match (entry.block_offset()?, has_parent) {
                    (0, false) => {
                        dest.fill(0);
                        Ok(dest.len())
                    }
                    (0, true) => {
                        if let Some(parent) = self.inner.parent.as_deref_mut() {
                            let mut parent = parent.hunk(self.hunk_num)?;
                            let res = parent.read_hunk_in(comp_buf, dest)?;
                            Ok(res)
                        } else {
                            Err(ChdError::RequiresParent)
                        }
                    }
                    (_offset, _) => {
                        // read_uncompressed will handle the proper offset for us automatically.
                        let proof = entry.prove_uncompressed()?;
                        let res = self.read_uncompressed(proof, dest)?;
                        Ok(res)
                    }
                }
            }
            MapEntry::LegacyEntry(_) => Err(ChdError::InvalidParameter),
        }
    }

    /// Decompresses the hunk into output, using the provided temporary buffer to hold the
    /// compressed hunk. The size of the output buffer must be equal to the hunk size of the
    /// CHD file.
    ///
    /// Returns the number of bytes decompressed on success, which should be the length of
    /// the output buffer.
    pub fn read_hunk_in(
        &mut self,
        compressed_buffer: &mut Vec<u8>,
        output: &mut [u8],
    ) -> Result<usize> {
        if output.len() != self.inner.header.hunk_size() as usize {
            return Err(ChdError::OutOfMemory);
        }

        match self.inner.map() {
            ChdMap::V5(_) => self.read_hunk_v5(compressed_buffer, output),
            ChdMap::Legacy(_) => self.read_hunk_legacy(compressed_buffer, output),
        }
    }

    /// Read the raw, compressed contents of the hunk into the provided buffer.
    ///
    /// Returns the number of bytes read on success.
    pub fn read_raw_in(&mut self, output: &mut Vec<u8>) -> Result<usize> {
        let map_entry = self
            .inner
            .map()
            .get_entry(self.hunk_num as usize)
            .ok_or(ChdError::HunkOutOfRange)?;

        let (offset, size) = match map_entry {
            MapEntry::V5Compressed(map_entry) => {
                (map_entry.block_offset()?, map_entry.block_size()?)
            }
            MapEntry::V5Uncompressed(map_entry) => {
                (map_entry.block_offset()?, map_entry.block_size())
            }
            MapEntry::LegacyEntry(map_entry) => (map_entry.block_offset(), map_entry.block_size()),
        };

        output.resize(size as usize, 0);
        self.inner.file.seek(SeekFrom::Start(offset))?;
        let read = self.inner.file.read(output)?;
        Ok(read)
    }

    #[allow(clippy::len_without_is_empty)]
    /// Returns the length of this hunk in bytes.
    pub fn len(&self) -> usize {
        self.inner.header.hunk_size() as usize
    }
}
