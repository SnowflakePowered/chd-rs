use std::fs::File;
use std::io::{Seek, Read, SeekFrom, ErrorKind};
use std::path::Path;
use crate::compression::CompressionCodec;
use crate::header::ChdHeader;
use crate::error::{Result, ChdError};
use crate::metadata::IterMetadataEntry;
use crate::map::ChdMap;

pub struct ChdFile<'a, F: Read + Seek> {
    file: F,
    header: ChdHeader,
    parent: Option<&'a mut ChdFile<'a, F>>,
    map: ChdMap,
    codecs: Vec<Box<dyn CompressionCodec>>
}

pub struct ChdHunk<'a, F: Read + Seek> {
    inner: &'a mut ChdFile<'a, F>,
    hunk_num: u32,
    buffer: Option<Vec<u8>>
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

        let codecs = header.create_compression_codecs()?;

        Ok(ChdFile {
            file,
            header,
            parent,
            map,
            codecs
        })
    }

    pub fn header(&self) -> &ChdHeader {
        &self.header
    }

    pub fn metadata(&mut self) -> Option<IterMetadataEntry<F>>{
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

    pub fn hunk(&'a mut self, hunk_num: u32) -> Result<ChdHunk<'a, F>> {
        if hunk_num >= self.header.hunk_count() {
            return Err(ChdError::HunkOutOfRange)
        }

        Ok(ChdHunk {
            inner: self,
            hunk_num,
            buffer: None
        })
    }

    pub fn hunk_into(&'a mut self, hunk_num: u32, buffer: Vec<u8>) -> Result<ChdHunk<'a, F>> {
        if hunk_num >= self.header.hunk_count() {
            return Err(ChdError::HunkOutOfRange)
        }

        Ok(ChdHunk {
            inner: self,
            hunk_num,
            buffer: Some(buffer)
        })
    }
}

impl <'a, F: Read + Seek> ChdHunk<'a, F> {

    /// Buffer the compressed bytes into the hunk buffer.
    fn buffer_compressed(&mut self) -> Result<()> {
        let map_entry = self.inner.map().get_entry(self.hunk_num as usize)
            .ok_or(ChdError::HunkOutOfRange)?;
        if !map_entry.is_compressed() {
            return Err(ChdError::InvalidParameter);
        }

        let offset = map_entry.block_offset()?;
        let length = map_entry.block_size()?;

        // reuse the buffer if it's already allocated.
        let buf = if let Some(buffer) = &mut self.buffer {
            buffer.fill(0);
            buffer.resize(length as usize, 0);
            buffer
        } else {
            self.buffer = Some(vec![0u8; length as usize]);
            self.buffer.as_deref_mut().unwrap()
        };

        self.inner.file.seek(SeekFrom::Start(offset))?;
        let read = self.inner.file.read(buf)?;
        if read != length as usize {
            return Err(ChdError::ReadError);
        }
        Ok(())
    }

    /// Returns the underlying buffer that stores the compressed data for this hunk.
    pub fn into_buffer(self) -> Option<Vec<u8>> {
        self.buffer
    }

    fn read_hunk_legacy(&mut self, buf: &mut [u8]) -> Result<usize> {
        todo!()
    }

    fn read_hunk_v5(&mut self, buf: &mut [u8]) -> Result<usize> {
        todo!()
    }

    fn read_hunk(&mut self, buf: &mut [u8]) -> Result<usize> {
        // https://github.com/rtissera/libchdr/blob/6eeb6abc4adc094d489c8ba8cafdcff9ff61251b/src/libchdr_chd.c#L2233
        match self.inner.header() {
            ChdHeader::V5Header(_) => self.read_hunk_v5(buf),

            // We purposefully avoid a `_` pattern here.
            // When CHD v6 is released, this should fail to compile unless
            // the case is explicitly added.
            ChdHeader::V1Header(_) | ChdHeader::V2Header(_)
                | ChdHeader::V3Header(_) | ChdHeader::V4Header(_) => self.read_hunk_legacy(buf)
        }
    }
}

impl <'a, F: Read + Seek> Read for ChdHunk<'a, F> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self.read_hunk(buf) {
            Ok(size) => Ok(size),
            Err(e) => Err(std::io::Error::new(ErrorKind::Other,e))
        }
    }
}

impl <'a, F: Read + Seek> Read for ChdFile<'a, F> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        todo!()
    }
}