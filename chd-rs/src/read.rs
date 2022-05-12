//! Helpers and adapters for reading CHD files and hunks.
//!
//! ## Usage
//! These adapters provide `Read + Seek` implementations over individual hunks and files by keeping
//! an internal buffer of decompressed hunk data. For the best performance and flexibility,
//! [`ChdHunk::read_hunk_in`](crate::chdfile::ChdHunk::read_hunk_in) should be used which will
//! avoid unnecessary buffering.
use crate::error::Result;
use crate::{ChdError, ChdFile, ChdHunk};
use std::io::{BufRead, Cursor, Read, Seek, SeekFrom};

/// Buffered `BufRead + Seek` adapter for [`ChdHunk`](crate::chdfile::ChdHunk).
pub struct ChdHunkBufReader {
    inner: Cursor<Vec<u8>>,
}

impl ChdHunkBufReader {
    /// Create a new `ChdHunkBufReader` with new buffers.
    ///
    /// New buffers are allocated and the hunk contents are immediately buffered
    /// from the stream upon creation.
    pub fn new<F: Read + Seek>(hunk: &mut ChdHunk<F>) -> Result<Self> {
        ChdHunkBufReader::new_in(hunk, &mut Vec::new(), Vec::new())
    }

    /// Creates a `ChdHunkBufReader` with the provided buffers.
    ///
    /// Ownership of `buffer` is transferred to the created `ChdHunkBufReader` and can be
    /// reacquired with [`ChdHunkBufReader::into_inner`](crate::read::ChdHunkBufReader::into_inner).
    ///
    /// The hunk contents are immediately buffered from the stream upon creation.
    ///
    /// `cmp_buffer` is used temporarily to hold the compressed data from the hunk. Ownership of
    /// `buffer` is taken to be used as the internal buffer for this reader.
    ///
    /// Unlike [`ChdHunk::read_hunk_in`](crate::chdfile::ChdHunk::read_hunk_in), there are no
    /// length restrictions on the provided buffers.
    pub fn new_in<F: Read + Seek>(
        hunk: &mut ChdHunk<F>,
        cmp_buffer: &mut Vec<u8>,
        mut buffer: Vec<u8>,
    ) -> Result<Self> {
        let len = hunk.len();
        buffer.resize(len, 0);
        hunk.read_hunk_in(cmp_buffer, &mut buffer)?;
        Ok(ChdHunkBufReader {
            inner: Cursor::new(buffer),
        })
    }

    /// Consumes the reader and returns the underlying value.
    pub fn into_inner(self) -> Vec<u8> {
        self.inner.into_inner()
    }
}

impl Read for ChdHunkBufReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buf)
    }
}

impl Seek for ChdHunkBufReader {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

impl BufRead for ChdHunkBufReader {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        self.inner.fill_buf()
    }

    fn consume(&mut self, amt: usize) {
        self.inner.consume(amt)
    }
}

/// Utility adapter for [`ChdFile`](crate::ChdFile) that implements `Read`.
///
/// `ChdFileReader` will allocate and manage intermediate buffers to support
/// reading at a byte granularity. If performance is a concern, it is recommended
/// to instead iterate over hunk indices.
pub struct ChdFileReader<F: Read + Seek> {
    chd: ChdFile<F>,
    current_hunk: u32,
    cmp_buf: Vec<u8>,
    buf_read: Option<ChdHunkBufReader>,
    eof: bool,
}

impl<F: Read + Seek> ChdFileReader<F> {
    /// Create a new `ChdFileReader` from an opened [`ChdFile`](crate::ChdFile).
    pub fn new(chd: ChdFile<F>) -> Self {
        ChdFileReader {
            chd,
            current_hunk: 0,
            cmp_buf: Vec::new(),
            buf_read: None,
            eof: false,
        }
    }
}

impl<F: Read + Seek> Read for ChdFileReader<F> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.eof {
            return Ok(0);
        }

        if self.current_hunk == 0 && self.buf_read.is_none() {
            // do init
            let mut hunk = match self.chd.hunk(self.current_hunk) {
                Ok(hunk) => hunk,
                // never was a hunk to begin with.
                Err(ChdError::HunkOutOfRange) => {
                    self.eof = true;
                    return Ok(0);
                }
                Err(e) => return Err(e.into()),
            };
            let buf = Vec::new();
            self.buf_read = Some(ChdHunkBufReader::new_in(&mut hunk, &mut self.cmp_buf, buf)?)
        }

        match self.buf_read.as_mut().unwrap().read(buf) {
            Ok(0) => {
                self.current_hunk += 1;
                let mut hunk = match self.chd.hunk(self.current_hunk) {
                    Ok(hunk) => hunk,
                    // never was a hunk to begin with.
                    Err(ChdError::HunkOutOfRange) => {
                        self.eof = true;
                        return Ok(0);
                    }
                    Err(e) => return Err(e.into()),
                };
                let inner = self.buf_read.take();
                self.buf_read = Some(ChdHunkBufReader::new_in(
                    &mut hunk,
                    &mut self.cmp_buf,
                    inner.unwrap().into_inner(),
                )?);
                self.read(buf)
            }
            Ok(r) => Ok(r),
            Err(e) => Err(e),
        }
    }
}
