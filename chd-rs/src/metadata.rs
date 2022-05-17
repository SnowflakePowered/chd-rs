//! Types and methods relating to metadata stored in a CHD file.

use crate::error::{ChdError, Result};
use crate::make_tag;
use byteorder::{BigEndian, ReadBytesExt};
use std::convert::TryInto;
use std::io::{Cursor, Read, Seek, SeekFrom};

const METADATA_HEADER_SIZE: usize = 16;
use num_derive::FromPrimitive;
use num_traits::FromPrimitive;

/// A list of well-known metadata tags.
#[derive(FromPrimitive, Copy, Clone)]
#[repr(u32)]
pub enum KnownMetadata {
    Wildcard = 0,
    HardDisk = make_tag(b"GDDD"),
    HardDiskIdent = make_tag(b"IDNT"),
    HardDiskKey = make_tag(b"KEY "),
    PcmciaCIS = make_tag(b"CIS "),
    CdRomOld = make_tag(b"CHCD"),
    CdRomTrack = make_tag(b"CHTR"),
    CdRomTrack2 = make_tag(b"CHT2"),
    GdRomOld = make_tag(b"CHGT"),
    GdRomTrack = make_tag(b"CHGD"),
    AudioVideo = make_tag(b"AVAV"),
    AudioVideoLaserDisc = make_tag(b"AVLD"),
}

impl KnownMetadata {
    /// Returns whether a given tag indicates that the CHD contains CDROM data.
    pub fn is_cdrom(tag: u32) -> bool {
        if let Some(tag) = FromPrimitive::from_u32(tag) {
            return matches!(tag, KnownMetadata::CdRomOld
                | KnownMetadata::CdRomTrack
                | KnownMetadata::CdRomTrack2
                | KnownMetadata::GdRomOld
                | KnownMetadata::GdRomTrack);
        }
        false
    }
}

/// Trait for structs that contain or represent tagged metadata.
pub trait ChdMetadataTag {
    fn metatag(&self) -> u32;
}

impl ChdMetadataTag for KnownMetadata {
    fn metatag(&self) -> u32 {
        *self as u32
    }
}

/// A complete CHD metadata entry with contents read into memory.
#[derive(Debug)]
pub struct ChdMetadata {
    /// The FourCC metadata tag.
    pub metatag: u32,
    /// The contents of this metadata entry.
    pub value: Vec<u8>,
    /// The flags of this metadata entry.
    pub flags: u8,
    /// The index of this metadata entry relative to the beginning of the metadata section.
    pub index: u32,
    /// The length of this metadata entry.
    pub length: u32,
}

impl ChdMetadataTag for ChdMetadata {
    fn metatag(&self) -> u32 {
        self.metatag
    }
}

/// A reference to a metadata entry within the CHD file.
#[repr(C)]
#[derive(Clone)]
pub struct MetadataRef {
    offset: u64,
    next: u64,
    prev: u64,
    pub(crate) length: u32,
    pub(crate) metatag: u32,
    pub(crate) flags: u8,
    pub(crate) index: u32,
}

impl MetadataRef {
    fn read_into<F: Read + Seek>(&self, file: &mut F, buf: &mut [u8]) -> Result<()> {
        file.seek(SeekFrom::Start(self.offset + METADATA_HEADER_SIZE as u64))?;
        file.read_exact(buf)?;
        Ok(())
    }

    /// Read the contents of the metadata from the input stream. The `ChdMetadataRef` must have
    /// the same provenance as the input stream for a successful read.
    pub fn read<F: Read + Seek>(&self, file: &mut F) -> Result<ChdMetadata> {
        let mut buf = vec![0u8; self.length as usize];
        self.read_into(file, &mut buf)?;
        Ok(ChdMetadata {
            metatag: self.metatag,
            value: buf,
            flags: self.flags,
            index: self.index,
            length: self.length,
        })
    }
}

impl ChdMetadataTag for MetadataRef {
    fn metatag(&self) -> u32 {
        self.metatag
    }
}

/// An iterator over references to the metadata entries of a CHD file.
/// If `unstable_lending_iterators` is enabled, metadata can be
/// more ergonomically iterated over with [`MetadataIter`](crate::iter::MetadataIter).
pub struct MetadataRefIter<'a, F: Read + Seek + 'a> {
    pub(crate) file: &'a mut F,
    curr_offset: u64,
    curr: Option<MetadataRef>,
    // Just use a tuple because we rarely have more than 2 or 3 types of tag.
    indices: Vec<(u32, u32)>,
}

impl<'a, F: Read + Seek + 'a> MetadataRefIter<'a, F> {
    pub(crate) fn from_stream(file: &'a mut F, initial_offset: u64) -> Self {
        MetadataRefIter {
            file,
            curr_offset: initial_offset,
            curr: None,
            indices: Vec::new(),
        }
    }

    pub(crate) fn dead(file: &'a mut F,) -> Self {
        MetadataRefIter {
            file,
            curr_offset: 0,
            curr: None,
            indices: Vec::new()
        }
    }

    /// Consumes the iterator, collecting all remaining metadata references and
    /// reads all their contents into a `Vec<ChdMetadata>`.
    pub fn try_into_vec(self) -> Result<Vec<ChdMetadata>> {
        self.try_into()
    }
}

impl<'a, F: Read + Seek + 'a> TryInto<Vec<ChdMetadata>> for MetadataRefIter<'a, F> {
    type Error = ChdError;

    fn try_into(mut self) -> std::result::Result<Vec<ChdMetadata>, Self::Error> {
        let metas = &mut self;
        let metas: Vec<_> = metas.collect();
        metas.iter().map(|e| e.read(&mut self.file)).collect()
    }
}

impl<'a, F: Read + Seek + 'a> Iterator for MetadataRefIter<'a, F> {
    // really need GATs to do this properly...
    type Item = MetadataRef;

    fn next(&mut self) -> Option<Self::Item> {
        if self.curr_offset == 0 {
            return None;
        }

        fn next_inner<'a, F: Read + Seek + 'a>(
            s: &mut MetadataRefIter<'a, F>,
        ) -> Result<MetadataRef> {
            let mut raw_header: [u8; METADATA_HEADER_SIZE] = [0; METADATA_HEADER_SIZE];
            s.file.seek(SeekFrom::Start(s.curr_offset))?;
            let count = s.file.read(&mut raw_header)?;
            if count != METADATA_HEADER_SIZE {
                return Err(ChdError::MetadataNotFound);
            }
            let mut cursor = Cursor::new(raw_header);
            cursor.seek(SeekFrom::Start(0))?;

            // extract data
            let metatag = cursor.read_u32::<BigEndian>()?;
            let length = cursor.read_u32::<BigEndian>()?;
            let next = cursor.read_u64::<BigEndian>()?;

            let flags = length >> 24;
            // mask off flags
            let length = length & 0x00ffffff;

            let mut index = 0;

            for indice in s.indices.iter_mut() {
                if indice.0 == metatag {
                    index = indice.1;
                    // increment current index
                    indice.1 += 1;
                    break;
                }
            }

            if index == 0 {
                s.indices.push((metatag, 1))
            }

            let mut new = MetadataRef {
                offset: s.curr_offset,
                next,
                prev: 0,
                length,
                metatag,
                flags: flags as u8,
                index,
            };

            if let Some(curr) = &s.curr {
                new.prev = curr.offset;
            }
            s.curr_offset = next;
            s.curr = Some(new.clone());
            Ok(new)
        }
        next_inner(self).ok()
    }
}

