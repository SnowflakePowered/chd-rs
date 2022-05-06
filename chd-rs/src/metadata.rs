use std::convert::TryInto;
use std::io::{Read, Seek, SeekFrom, Cursor};
use crate::error::{Result, ChdError};
use byteorder::{ReadBytesExt, BigEndian};
use crate::make_tag;

const METADATA_HEADER_SIZE: usize = 16;
use num_derive::FromPrimitive;
use num_traits::FromPrimitive;

#[derive(FromPrimitive)]
#[repr(u32)]
pub enum KnownMetadata {
    None = 0,
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
    AudioVideoLaserDisc = make_tag(b"AVLD")
}

impl KnownMetadata {
    pub fn is_cdrom(tag: u32) -> bool {
        if let Some(tag) = FromPrimitive::from_u32(tag) {
            return match tag {
                KnownMetadata::CdRomOld | KnownMetadata::CdRomTrack | KnownMetadata::CdRomTrack2
                    | KnownMetadata::GdRomOld | KnownMetadata::GdRomTrack => true,
                _ => false
            }
        }
       false
    }
}

pub struct ChdMetadata {
    pub metatag: u32,
    pub value: Vec<u8>,
    pub flags: u8,
    pub index: u32,
    pub length: u64,
}

#[repr(C)]
#[derive(Clone)]
pub struct MetadataEntry {
    offset: u64,
    next: u64,
    prev: u64,
    pub(crate) length: u64,
    pub(crate) metatag: u32,
    pub(crate) flags: u8,
    pub(crate) index: u32,
}

impl MetadataEntry {
    fn read_into<F: Read + Seek>(&self, file: &mut F, buf: &mut [u8]) -> Result<()> {
        file.seek(SeekFrom::Start(self.offset + METADATA_HEADER_SIZE as u64))?;
        file.read_exact(buf)?;
        Ok(())
    }

    pub fn read<F: Read + Seek>(&self, file: &mut F) -> Result<ChdMetadata> {
        let mut buf = vec![0u8; self.length as usize];
        self.read_into(file, &mut buf)?;
        Ok(ChdMetadata {
            metatag: self.metatag,
            value: buf,
            flags: self.flags,
            index: self.index,
            length: self.length
        })
    }
}

pub struct IterMetadataEntry<'a, F: Read + Seek + 'a> {
    file: &'a mut F,
    curr_offset: u64,
    curr: Option<MetadataEntry>,
    // Just use a tuple because we rarely have more than 2 or 3 types of tag.
    indices: Vec<(u32, u32)>
}

impl <'a, F: Read + Seek + 'a> IterMetadataEntry<'a, F> {
    pub(crate) fn from_stream(file: &'a mut F, initial_offset: u64) -> Self {
        IterMetadataEntry {
            file,
            curr_offset: initial_offset,
            curr: None,
            indices: Vec::new()
        }
    }

    pub fn try_into_vec(self) -> Result<Vec<ChdMetadata>> {
        self.try_into()
    }
}

impl <'a, F: Read + Seek + 'a> TryInto<Vec<ChdMetadata>> for IterMetadataEntry<'a, F> {
    type Error = ChdError;

    fn try_into(mut self) -> std::result::Result<Vec<ChdMetadata>, Self::Error> {
        let metas = &mut self;
        let metas : Vec<_> = metas.collect();
        metas.iter().map(|e| e.read(&mut self.file))
            .collect()
    }
}

impl <'a, F: Read + Seek + 'a> Iterator for IterMetadataEntry<'a, F> {
    // really need GATs to do this properly...
    type Item = MetadataEntry;

    fn next(&mut self) -> Option<Self::Item> {
        if self.curr_offset == 0 {
            return None
        }

        fn next_inner<'a, F: Read + Seek + 'a>(s: &mut IterMetadataEntry<'a, F>) -> Result<MetadataEntry> {
            let mut raw_header: [u8; METADATA_HEADER_SIZE] = [0; METADATA_HEADER_SIZE];
            s.file.seek(SeekFrom::Start(s.curr_offset))?;
            let count = s.file.read(&mut raw_header)?;
            if count != METADATA_HEADER_SIZE {
                return Err(ChdError::MetadataNotFound)
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

            let mut new = MetadataEntry {
                offset: s.curr_offset,
                next,
                prev: 0,
                length: length as u64,
                metatag,
                flags: flags as u8,
                index
            };

            if let Some(curr) = &s.curr {
                new.prev = curr.offset;
            }
            s.curr_offset = next;
            s.curr = Some(new.clone());
            Ok(new)
        }
        return next_inner(self).ok()
    }
}