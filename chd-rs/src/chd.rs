use crate::compression::CompressionCodec;
use crate::error::{ChdError, Result};
use crate::header::ChdHeader;
use crate::map::{ChdMap, LegacyEntryType, MapEntry, V5CompressionType};
use crate::metadata::IterMetadataEntry;
use byteorder::{BigEndian, WriteBytesExt};
use std::fs::File;
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::Path;

use crc::{Crc, CRC_16_IBM_3740, CRC_32_ISO_HDLC};
use num_traits::{FromPrimitive, ToPrimitive};

// CRC16 table in hashing.cpp indicates CRC16/CCITT, but constants
// are consistent with CRC16/CCITT-FALSE, which is CRC-16/IBM-3740
const CRC16: Crc<u16> = Crc::<u16>::new(&CRC_16_IBM_3740);

// The polynomial matches up (0x04c11db7 reflected = 0xedb88320), and
// checking with zlib crc32.c matches the check 0xcbf43926 for
// "12345678".
const CRC32: Crc<u32> = Crc::<u32>::new(&CRC_32_ISO_HDLC);

pub struct ChdFile<F: Read + Seek> {
    file: F,
    header: ChdHeader,
    parent: Option<Box<ChdFile<F>>>,
    map: ChdMap,
    codecs: Vec<Box<dyn CompressionCodec>>,
}

pub struct ChdHunk<'a, F: Read + Seek> {
    inner: &'a mut ChdFile<F>,
    hunk_num: u32,
    compressed_buffer: Option<Vec<u8>>,
    cached: bool,
}

impl<F: Read + Seek> ChdFile<F> {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<ChdFile<File>> {
        let file = File::open(path)?;
        ChdFile::open_stream(file, None)
    }

    pub fn open_stream(mut file: F, parent: Option<Box<ChdFile<F>>>) -> Result<ChdFile<F>> {
        let header = ChdHeader::try_read_header(&mut file)?;
        if !header.validate() {
            return Err(ChdError::InvalidParameter);
        }

        // No point in checking writable because traits are read only.
        // In the future if we want to support a Write feature, will need to ensure writable.

        // Make sure we have a parent if we have one
        if parent.is_none() && header.has_parent() {
            return Err(ChdError::RequiresParent);
        }

        if !header.validate_compression() {
            return Err(ChdError::UnsupportedFormat);
        }

        let map = ChdMap::try_read_map(&header, &mut file)?;

        // todo: hunk cache, not important right now but will need for C compat.

        let codecs = header.create_compression_codecs()?;

        Ok(ChdFile {
            file,
            header,
            parent,
            map,
            codecs,
        })
    }

    pub fn header(&self) -> &ChdHeader {
        &self.header
    }

    pub fn metadata(&mut self) -> Option<IterMetadataEntry<F>> {
        let offset = self.header().meta_offset();
        if let Some(offset) = offset {
            Some(IterMetadataEntry::from_stream(&mut self.file, offset))
        } else {
            None
        }
    }

    pub fn map(&self) -> &ChdMap {
        &self.map
    }

    pub fn hunk(&mut self, hunk_num: u32) -> Result<ChdHunk<F>> {
        if hunk_num >= self.header.hunk_count() {
            return Err(ChdError::HunkOutOfRange);
        }
        Ok(ChdHunk {
            inner: self,
            hunk_num,
            compressed_buffer: None,
            cached: false,
        })
    }

    pub fn hunk_in(
        &mut self,
        hunk_num: u32,
        compressed_buffer: Option<Vec<u8>>,
        output_buffer: Vec<u8>,
    ) -> Result<ChdHunk<F>> {
        if hunk_num >= self.header.hunk_count() {
            return Err(ChdError::HunkOutOfRange);
        }

        Ok(ChdHunk {
            inner: self,
            hunk_num,
            compressed_buffer,
            cached: false,
        })
    }
}

impl<'a, F: Read + Seek> ChdHunk<'a, F> {
    /// Buffer the compressed bytes into the hunk buffer.
    fn buffer_compressed(&mut self) -> Result<()> {
        // Ideally I don't  want to reacquire the map_entry but I'm not sure how to solve the
        // lifetime requirements here.
        let map_entry = self
            .inner
            .map()
            .get_entry(self.hunk_num as usize)
            .ok_or(ChdError::HunkOutOfRange)?;

        if !map_entry.is_compressed() {
            return Err(ChdError::InvalidParameter);
        }
        let offset = map_entry.block_offset()?;
        let length = map_entry.block_size()?;

        // reuse the buffer if it's already allocated.
        let buf = if let Some(buffer) = &mut self.compressed_buffer {
            buffer.fill(0);
            buffer.resize(length as usize, 0);
            buffer
        } else {
            self.compressed_buffer = Some(vec![0u8; length as usize]);
            self.compressed_buffer
                .as_deref_mut()
                .ok_or(ChdError::OutOfMemory)?
        };

        self.inner.file.seek(SeekFrom::Start(offset))?;
        let read = self.inner.file.read(buf)?;
        if read != length as usize {
            return Err(ChdError::ReadError);
        }
        Ok(())
    }

    fn read_uncompressed(&mut self, dest: &mut [u8]) -> Result<usize> {
        // Ideally I don't  want to reacquire the map_entry but I'm not sure how to solve the
        // lifetime requirements here.
        let map_entry = self
            .inner
            .map()
            .get_entry(self.hunk_num as usize)
            .ok_or(ChdError::HunkOutOfRange)?;

        if map_entry.is_compressed() {
            return Err(ChdError::InvalidParameter);
        }
        let offset = map_entry.block_offset()?;
        let length = map_entry.block_size()?;

        if dest.len() != length as usize {
            return Err(ChdError::InvalidParameter);
        }
        self.inner.file.seek(SeekFrom::Start(offset))?;
        let read = self.inner.file.read(dest)?;
        Ok(read)
    }

    /// Returns the underlying buffer that stores the compressed data for this hunk.
    pub fn into_buffer(self) -> Option<Vec<u8>> {
        self.compressed_buffer
    }

    fn read_hunk_legacy(&mut self, dest: &mut [u8]) -> Result<usize> {
        let map_entry = self
            .inner
            .map()
            .get_entry(self.hunk_num as usize)
            .ok_or(ChdError::HunkOutOfRange)?;

        if !map_entry.is_legacy() {
            return Err(ChdError::InvalidParameter);
        }

        let block_len = map_entry.block_size()? as usize;
        let block_crc = map_entry.block_crc()?;
        let block_off = map_entry.block_offset()?;

        let value = match map_entry {
            MapEntry::LegacyEntry(entry) => {
                match entry.entry_type()? {
                    LegacyEntryType::Compressed => {
                        // buffer the compressed data
                        self.buffer_compressed()?;

                        return if let Some(buffer) = &self.compressed_buffer {
                            let res =
                                &self.inner.codecs[0].decompress(&buffer[..block_len], dest)?;

                            // #[cfg(feature = "checksum")]
                            match block_crc {
                                Some(crc) if CRC32.checksum(dest) != crc => {
                                    return Err(ChdError::DecompressionError)
                                }
                                _ => (),
                            };
                            Ok(res.total_out())
                        } else {
                            Err(ChdError::OutOfMemory)
                        };
                    }
                    LegacyEntryType::Uncompressed => {
                        let res = self.read_uncompressed(dest)?;

                        match block_crc {
                            Some(crc) if CRC32.checksum(&dest) != crc => {
                                return Err(ChdError::DecompressionError)
                            }
                            _ => (),
                        };
                        Ok(res)
                    }
                    LegacyEntryType::Mini => {
                        let mut cursor = Cursor::new(dest);
                        cursor.write_u64::<BigEndian>(entry.offset())?;
                        let dest = cursor.into_inner();
                        let mut bytes_read_into = std::mem::size_of::<u64>();

                        // todo: optimize this operation
                        for off in
                            std::mem::size_of::<u64>()..self.inner.header().hunk_bytes() as usize
                        {
                            dest[off] = dest[off - 8];
                            bytes_read_into += 1;
                        }

                        match block_crc {
                            Some(crc) if CRC32.checksum(dest) != crc => {
                                return Err(ChdError::DecompressionError)
                            }
                            _ => (),
                        };
                        Ok(bytes_read_into)
                    }
                    LegacyEntryType::SelfHunk => {
                        // todo: optimize to reuse internal buffers
                        let mut self_hunk = self.inner.hunk(block_off as u32)?;
                        let res = self_hunk.read_hunk(dest)?;
                        let c = self_hunk.into_buffer();
                        self.compressed_buffer = c;
                        Ok(res)
                    }
                    LegacyEntryType::ParentHunk => {
                        // todo: optimize to reuse internal buffers
                        match self.inner.parent.as_deref_mut() {
                            None => Err(ChdError::RequiresParent),
                            Some(parent) => {
                                let mut parent = parent.hunk(block_off as u32)?;
                                let res = parent.read_hunk(dest)?;
                                let c = parent.into_buffer();
                                self.compressed_buffer = c;
                                Ok(res)
                            }
                        }
                    }
                    LegacyEntryType::ExternalCompressed => Err(ChdError::UnsupportedFormat),
                    LegacyEntryType::Invalid => Err(ChdError::InvalidData),
                }
            }
            _ => Err(ChdError::InvalidParameter),
        }?;
        self.cached = true;
        Ok(value)
    }

    fn buffer_hunk_v5(&mut self, dest: &mut [u8]) -> Result<usize> {
        let map_entry = self
            .inner
            .map()
            .get_entry(self.hunk_num as usize)
            .ok_or(ChdError::HunkOutOfRange)?;

        if map_entry.is_legacy() {
            return Err(ChdError::InvalidParameter);
        }

        // block_off is already accurate for uncompressed case
        let block_off = map_entry.block_offset()?;
        let block_len = map_entry.block_size()? as usize;
        let block_crc = map_entry.block_crc()?;

        let has_parent = self.inner.header.has_parent();

        return if !map_entry.is_compressed() {
            match (block_off, has_parent) {
                (0, false) => {
                    dest.fill(0);
                    Ok(dest.len())
                }
                (0, true) => {
                    if let Some(parent) = self.inner.parent.as_deref_mut() {
                        // todo: optimize to reuse buffers
                        let mut parent = parent.hunk(self.hunk_num)?;
                        let res = parent.read_hunk(dest)?;
                        let c = parent.into_buffer();
                        self.compressed_buffer = c;
                        Ok(res)
                    } else {
                        Err(ChdError::RequiresParent)
                    }
                }
                (_offset, _) => {
                    // read_uncompressed will handle the proper offset for us automatically.
                    let res = self.read_uncompressed(dest)?;
                    Ok(res)
                }
            }
        } else {
            // compressed case
            match map_entry
                .block_type()
                .map(V5CompressionType::from_u8)
                .flatten()
            {
                Some(
                    comptype @ V5CompressionType::CompressionType0
                    | comptype @ V5CompressionType::CompressionType1
                    | comptype @ V5CompressionType::CompressionType2
                    | comptype @ V5CompressionType::CompressionType3,
                ) => {
                    // buffer the compressed data
                    self.buffer_compressed()?;

                    if let Some(codec) = self.inner.codecs.get_mut(comptype.to_usize().unwrap()) {
                        if let Some(buffer) = self.compressed_buffer.as_deref_mut() {
                            let res = codec.decompress(buffer, dest)?;

                            match block_crc.and_then(|f| f.to_u16()) {
                                Some(crc) if CRC16.checksum(dest) != crc => {
                                    Err(ChdError::DecompressionError)
                                }
                                _ => Ok(res.total_out()),
                            }
                        } else {
                            Err(ChdError::OutOfMemory)
                        }
                    } else {
                        Err(ChdError::UnsupportedFormat)
                    }
                }
                Some(V5CompressionType::CompressionNone) => {
                    let res = self.read_uncompressed(dest)?;

                    match block_crc.and_then(|f| f.to_u16()) {
                        Some(crc) if CRC16.checksum(dest) != crc => {
                            return Err(ChdError::DecompressionError)
                        }
                        _ => Ok(res),
                    }
                }
                Some(V5CompressionType::CompressionSelf) => {
                    // todo: optimize to reuse buffer
                    let mut self_hunk = self.inner.hunk(block_off as u32)?;
                    let res = self_hunk.read_hunk(dest)?;
                    let c = self_hunk.into_buffer();
                    self.compressed_buffer = c;
                    Ok(res)
                }
                Some(V5CompressionType::CompressionParent) => {
                    // todo: optimize to reuse internal buffers
                    let hunk_bytes = self.inner.header().hunk_bytes();
                    let unit_bytes = self.inner.header().unit_bytes();
                    let units_in_hunk = hunk_bytes / unit_bytes;

                    match self.inner.parent.as_deref_mut() {
                        None => Err(ChdError::RequiresParent),
                        Some(parent) => {
                            let mut buf = vec![0u8; hunk_bytes as usize];

                            let mut parent_hunk = parent.hunk(block_off as u32 / units_in_hunk)?;
                            let res_1 = parent_hunk.read_hunk(&mut buf)?;
                            let c = parent_hunk.into_buffer();

                            if block_off % units_in_hunk as u64 == 0 {
                                dest.copy_from_slice(&buf);
                                self.compressed_buffer = c;
                                return Ok(res_1);
                            }

                            let remainder_in_hunk = block_off as usize % units_in_hunk as usize;
                            let hunk_split =
                                (units_in_hunk as usize - remainder_in_hunk) * unit_bytes as usize;

                            dest[..hunk_split].copy_from_slice(
                                &buf[remainder_in_hunk * unit_bytes as usize..][..hunk_split],
                            );

                            let mut parent_hunk =
                                parent.hunk((block_off as u32 / units_in_hunk) + 1)?;
                            let res_2 = parent_hunk.read_hunk(&mut buf)?;
                            let _c = parent_hunk.into_buffer();

                            dest[hunk_split..].copy_from_slice(
                                &buf[..remainder_in_hunk
                                    * self.inner.header().unit_bytes() as usize],
                            );
                            self.compressed_buffer = None;

                            match block_crc.and_then(|f| f.to_u16()) {
                                Some(crc) if CRC16.checksum(&dest) != crc => {
                                    return Err(ChdError::DecompressionError)
                                }
                                _ => Ok(hunk_split + remainder_in_hunk * unit_bytes as usize),
                            }
                        }
                    }
                }
                _ => Err(ChdError::UnsupportedFormat),
            }
        };
    }

    /// Decompresses the hunk into the cache.
    /// This is necessary because CHD is not streaming and can only read at a granularity of
    /// hunk_size, in order to support `Read`.
    pub fn read_hunk(&mut self, dest: &mut [u8]) -> Result<usize> {
        // todo: lift CompressedBuffer out of read_hunk
        if dest.len() < self.inner.header.hunk_bytes() as usize {
            return Err(ChdError::HunkOutOfRange);
        }

        // https://github.com/rtissera/libchdr/blob/6eeb6abc4adc094d489c8ba8cafdcff9ff61251b/src/libchdr_chd.c#L2233
        match self.inner.header() {
            ChdHeader::V5Header(_) => self.buffer_hunk_v5(dest),

            // We purposefully avoid a `_` pattern here.
            // When CHD v6 is released, this should fail to compile unless
            // the case is explicitly added.
            ChdHeader::V1Header(_)
            | ChdHeader::V2Header(_)
            | ChdHeader::V3Header(_)
            | ChdHeader::V4Header(_) => self.read_hunk_legacy(dest),
        }
    }
}

impl<'a, F: Read + Seek> Read for ChdHunk<'a, F> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        todo!()
        // match self.read_hunk() {
        //     Ok(size) => Ok(size),
        //     Err(e) => Err(std::io::Error::new(ErrorKind::Other,e))
        // }
    }
}

impl<'a, F: Read + Seek> Read for ChdFile<F> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        todo!()
    }
}
