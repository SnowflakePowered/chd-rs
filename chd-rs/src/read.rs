//! Helpers and adapters for reading CHD files and hunks.
//!
//! ## Usage
//! These adapters provide `Read + Seek` implementations over individual hunks and files by keeping
//! an internal buffer of decompressed hunk data. For the best performance and flexibility,
//! [`Hunk::read_hunk_in`](crate::Hunk::read_hunk_in) should be used which will
//! avoid unnecessary buffering.
use crate::error::Result;
use crate::{Chd, Error, Hunk};
use std::io::{BufRead, Cursor, Read, Seek, SeekFrom};

/// Buffered `BufRead + Seek` adapter for [`Hunk`](crate::Hunk).
pub struct HunkBufReader {
    inner: Cursor<Vec<u8>>,
}

impl HunkBufReader {
    /// Create a new `HunkBufReader` with new buffers.
    ///
    /// New buffers are allocated and the hunk contents are immediately buffered
    /// from the stream upon creation.
    pub fn new<F: Read + Seek>(hunk: &mut Hunk<F>) -> Result<Self> {
        HunkBufReader::new_in(hunk, &mut Vec::new(), Vec::new())
    }

    /// Creates a `HunkBufReader` with the provided buffers.
    ///
    /// Ownership of `buffer` is transferred to the created `ChdHunkBufReader` and can be
    /// reacquired with [`HunkBufReader::into_inner`](crate::read::HunkBufReader::into_inner).
    ///
    /// The hunk contents are immediately buffered from the stream upon creation.
    ///
    /// `cmp_buffer` is used temporarily to hold the compressed data from the hunk. Ownership of
    /// `buffer` is taken to be used as the internal buffer for this reader.
    ///
    /// Unlike [`Hunk::read_hunk_in`](crate::Hunk::read_hunk_in), there are no
    /// length restrictions on the provided buffers.
    pub fn new_in<F: Read + Seek>(
        hunk: &mut Hunk<F>,
        cmp_buffer: &mut Vec<u8>,
        mut buffer: Vec<u8>,
    ) -> Result<Self> {
        let len = hunk.len();
        buffer.resize(len, 0);
        hunk.read_hunk_in(cmp_buffer, &mut buffer)?;
        Ok(HunkBufReader {
            inner: Cursor::new(buffer),
        })
    }

    /// Consumes the reader and returns the underlying value.
    pub fn into_inner(self) -> Vec<u8> {
        self.inner.into_inner()
    }
}

impl Read for HunkBufReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buf)
    }
}

impl Seek for HunkBufReader {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

impl BufRead for HunkBufReader {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        self.inner.fill_buf()
    }

    fn consume(&mut self, amt: usize) {
        self.inner.consume(amt)
    }
}

/// Utility adapter for [`Chd`](crate::Chd) that implements `Read`.
///
/// `ChdReader` will allocate and manage intermediate buffers to support
/// reading at a byte granularity. If performance is a concern, it is recommended
/// to instead iterate over hunk indices.
pub struct ChdReader<F: Read + Seek> {
    chd: Chd<F>,
    current_hunk: u32,
    cmp_buf: Vec<u8>,
    buf_read: Option<HunkBufReader>,
    eof: bool,
}

impl<F: Read + Seek> ChdReader<F> {
    /// Create a new `ChdReader` from an opened [`Chd`](crate::Chd).
    pub fn new(chd: Chd<F>) -> Self {
        ChdReader {
            chd,
            current_hunk: 0,
            cmp_buf: Vec::new(),
            buf_read: None,
            eof: false,
        }
    }
}

impl<F: Read + Seek> Read for ChdReader<F> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.eof {
            return Ok(0);
        }

        if self.current_hunk == 0 && self.buf_read.is_none() {
            // do init
            let mut hunk = match self.chd.hunk(self.current_hunk) {
                Ok(hunk) => hunk,
                // never was a hunk to begin with.
                Err(Error::HunkOutOfRange) => {
                    self.eof = true;
                    return Ok(0);
                }
                Err(e) => return Err(e.into()),
            };
            let buf = Vec::new();
            self.buf_read = Some(HunkBufReader::new_in(&mut hunk, &mut self.cmp_buf, buf)?)
        }

        match self.buf_read.as_mut().unwrap().read(buf) {
            Ok(0) => {
                self.current_hunk += 1;
                let mut hunk = match self.chd.hunk(self.current_hunk) {
                    Ok(hunk) => hunk,
                    // never was a hunk to begin with.
                    Err(Error::HunkOutOfRange) => {
                        self.eof = true;
                        return Ok(0);
                    }
                    Err(e) => return Err(e.into()),
                };
                let inner = self.buf_read.take();
                self.buf_read = Some(HunkBufReader::new_in(
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

impl<F: Read + Seek> Seek for ChdReader<F> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        // length of the uncompressed stream
        let len = self.chd.header().logical_bytes();
        let hunk_size = self.chd.header().hunk_size();

        let (base_pos, offset) = match pos {
            SeekFrom::Start(n) => {
                if n >= len {
                    self.eof = true;
                    return Ok(len);
                }
                let hunk_num = n / hunk_size as u64;
                let hunk_off = n % hunk_size as u64;
                return if let Ok(mut hunk) = self.chd.hunk(hunk_num as u32) {
                    self.eof = false;
                    self.current_hunk = hunk_num as u32;
                    let mut buf_read =
                        HunkBufReader::new_in(&mut hunk, &mut self.cmp_buf, Vec::new())?;
                    buf_read.inner.seek(SeekFrom::Start(hunk_off))?;
                    self.buf_read = Some(buf_read);
                    Ok(n)
                } else {
                    self.eof = true;
                    Ok(n)
                };
            }
            SeekFrom::End(n) => (len, n),
            SeekFrom::Current(n) => {
                let hunk_pos = match &self.buf_read {
                    None => 0,
                    Some(hunk) => hunk.inner.position(),
                };
                (hunk_pos, n)
            }
        };

        match base_pos.checked_add_signed(offset) {
            Some(n) => {
                let n = n;
                if n >= len {
                    self.eof = true;
                    return Ok(len);
                }
                let hunk_num = n / hunk_size as u64;
                let hunk_off = n % hunk_size as u64;
                if let Ok(mut hunk) = self.chd.hunk(hunk_num as u32) {
                    self.eof = false;
                    self.current_hunk = hunk_num as u32;
                    let mut buf_read =
                        HunkBufReader::new_in(&mut hunk, &mut self.cmp_buf, Vec::new())?;
                    buf_read.inner.seek(SeekFrom::Start(hunk_off))?;
                    self.buf_read = Some(buf_read);
                    Ok(n)
                } else {
                    self.eof = true;
                    Ok(n)
                }
            }
            None => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid seek to a negative or overflowing position",
            )),
        }
    }
}
