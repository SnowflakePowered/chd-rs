use std::fs::File;
use std::io::{Seek, Read};
use std::path::Path;
use crate::header::ChdHeader;
use crate::error::{Result, ChdError};
use crate::metadata::{MetadataIter, ChdMetadata};
use crate::map;
use crate::map::ChdMap;

pub struct ChdFile<'a, F: Read + Seek> {
    file: F,
    header: ChdHeader,
    parent: Option<&'a mut ChdFile<'a, F>>,
    map: ChdMap
}

impl<'a, F: Read + Seek> ChdFile<'a, F> {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<ChdFile<'a, File>> {
        let file = File::open(path)?;
        ChdFile::open_stream(file, None)
    }

    pub fn open_stream(mut file: F, parent: Option<&'a mut ChdFile<'a, F>>) -> Result<ChdFile<'a, F>> {
        let header = ChdHeader::try_read_header(&mut file)?;
        if !header.validate() {
            return Err(ChdError::InvalidParameter)
        }

        // No point in checking writable because traits are read only.
        // In the future if we want to support a Write feature, will need to ensure writable.

        // Make sure we have a parent if we have one
        if parent.is_some() && !header.has_parent() {
            return Err(ChdError::RequiresParent)
        }

        if !header.validate_compression() {
            return Err(ChdError::UnsupportedFormat)
        }

        let map = ChdMap::try_read_map(&header, &mut file)?;

        // todo: hunk cache, not important right now but will need for C compat.

        Ok(ChdFile {
            file,
            header,
            parent,
            map
        })
    }

    pub fn header(&self) -> &ChdHeader {
        &self.header
    }

    pub fn metadata(&mut self) -> Option<MetadataIter<F>>{
        let offset = self.header().meta_offset();
        if let Some(offset) = offset {
            Some(MetadataIter::from_stream(&mut self.file, offset))
        } else {
            None
        }
    }

    pub fn map(&self) -> &ChdMap {
        &self.map
    }
}

impl <'a, F: Read + Seek> Read for ChdFile<'a, F> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        todo!()
    }
}