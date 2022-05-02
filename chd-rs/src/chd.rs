use std::io::{Seek, Read, BufReader};
use crate::header::ChdHeader;
use crate::error::{Result, ChdError};
use crate::metadata::{MetadataIter, ChdMetadata};
use std::convert::TryInto;
use std::marker::PhantomData;
use std::fs::metadata;
use std::borrow::Borrow;
use std::slice::Iter;
use crate::map;

pub struct ChdFile<'a, F: Read + Seek> {
    file: F,
    header: ChdHeader,
    parent: Option<&'a mut ChdFile<'a, F>>
}

impl<'a, F: Read + Seek> ChdFile<'a, F> {
    pub fn try_from_file(mut file: F, parent: Option<&'a mut ChdFile<'a, F>>) -> Result<ChdFile<'a, F>> {
        let header = ChdHeader::try_from_file(&mut file)?;
        if !header.validate() {
            return Err(ChdError::InvalidParameter)
        }

        // No point in checking writable because we are read only so far.

        // Make sure we have a parent if we have one
        if parent.is_some() && !header.has_parent() {
            return Err(ChdError::RequiresParent)
        }

        // todo: read hunk map
        // https://github.com/rtissera/libchdr/blob/cdcb714235b9ff7d207b703260706a364282b063/src/libchdr_chd.c#L1415
        match &header {
            ChdHeader::V5Header(v5) => {
                map::read_v5(v5, &mut file)?;
            }
            _ => unimplemented!()
        }


        // todo: find codec
        Ok(ChdFile {
            file,
            header,
            parent
        })
    }

    pub fn header(&self) -> &ChdHeader {
        &self.header
    }

    pub fn metadata(&mut self) -> Option<Vec<ChdMetadata>> {
        let offset = self.header().meta_offset();
        if let Some(offset) = offset {
            let m_iter = MetadataIter::new_from_raw_file(&mut self.file, offset);
            let metas: Vec<_> = m_iter.collect();
            return metas.iter().map(|e| e.read(&mut self.file).ok()).collect()
        }
        return None
    }
}