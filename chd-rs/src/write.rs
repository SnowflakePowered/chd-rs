//! Writing CHD files.
//!
//! ```no_run
//! use chd::write::{ChdOptions, ChdWriter};
//! use std::fs::File;
//! use std::io;
//!
//! # fn main() -> chd::Result<()> {
//! let mut input = File::open("image.bin")?;
//! let options = ChdOptions::new(4096, 512);
//! let mut chd = ChdWriter::new(File::create("image.chd")?, options)?;
//! io::copy(&mut input, &mut chd)?;
//! chd.finish()?;
//! # Ok(())
//! # }
//! ```
use crate::error::{Error, Result};
use crate::header::{Header, HeaderV5, Version};
use crate::map::Map;
use std::io::{self, Read, Seek, SeekFrom, Write};

/// The size of a V5 header.
const HEADER_BYTES: u64 = 124;

/// Options for creating a CHD file.
#[derive(Debug, Clone)]
pub struct ChdOptions {
    hunk_bytes: u32,
    unit_bytes: u32,
    logical_bytes: Option<u64>,
}

impl ChdOptions {
    /// Options for a CHD with the given hunk and unit sizes in bytes.
    ///
    /// The hunk size must be a multiple of the unit size.
    pub fn new(hunk_bytes: u32, unit_bytes: u32) -> Self {
        ChdOptions {
            hunk_bytes,
            unit_bytes,
            logical_bytes: None,
        }
    }

    /// Set the logical size of the CHD in bytes ahead of time.
    ///
    /// For uncompressed CHDs, this places the map directly after the header.
    /// Otherwise, the map is written at the end of the file, and the logical size is the
    /// number of bytes written.
    ///
    /// If fewer bytes are written than the logical bytes set, the remaining length is filled with zeroes.
    pub fn logical_bytes(mut self, logical_bytes: u64) -> Self {
        self.logical_bytes = Some(logical_bytes);
        self
    }
}

/// Writes a CHD file.
///
/// Data is written either a hunk at a time with [`write_hunk`](ChdWriter::write_hunk), or as
/// a stream of bytes with [`Write`].
///
/// The CHD must be completed with [`finish`](ChdWriter::finish),
/// otherwise the file is left incomplete.
pub struct ChdWriter<W: Write + Seek> {
    inner: W,
    options: ChdOptions,
    map: Vec<u32>,
    hunk_count: Option<u32>,
    buffer: Vec<u8>,
    written: u64,
    eof: u64,
}

impl<W: Write + Seek> ChdWriter<W> {
    /// Start writing a CHD file.
    ///
    /// Returns [`InvalidParameter`](Error::InvalidParameter) if the hunk size is not a
    /// non-zero multiple of the unit size, or if there would be too many hunks.
    pub fn new(inner: W, options: ChdOptions) -> Result<Self> {
        if options.unit_bytes == 0
            || options.hunk_bytes == 0
            || !options.hunk_bytes.is_multiple_of(options.unit_bytes)
        {
            return Err(Error::InvalidParameter);
        }

        let hunk_count = options
            .logical_bytes
            .map(|logical_bytes| {
                u32::try_from(logical_bytes.div_ceil(options.hunk_bytes as u64))
                    .map_err(|_| Error::InvalidParameter)
            })
            .transpose()?;

        let mut writer = ChdWriter {
            inner,
            options,
            map: Vec::new(),
            hunk_count,
            buffer: Vec::new(),
            written: 0,
            eof: 0,
        };

        // Write the header first. If we know the needed size of the map, preallocate it.
        writer.inner.seek(SeekFrom::Start(0))?;
        let header = writer.header(writer.options.logical_bytes.unwrap_or(0), 0);
        writer.inner.write_all(&header.to_bytes())?;
        writer.eof = HEADER_BYTES;

        if let Some(hunk_count) = hunk_count {
            let map_bytes = hunk_count as u64 * 4;
            std::io::copy(&mut std::io::repeat(0).take(map_bytes), &mut writer.inner)?;
            writer.eof += map_bytes;
        }
        Ok(writer)
    }

    /// Write the next hunk, which must be exactly the hunk size.
    ///
    /// Returns the following errors
    /// - [`InvalidState`](Error::InvalidState) if only part of a hunk was written previously with [`Write`].
    /// - [`InvalidParameter`](Error::InvalidParameter) if the hunk is the wrong size.
    /// - [`HunkOutOfRange`](Error::HunkOutOfRange) if the CHD is already as large as its logical size.
    pub fn write_hunk(&mut self, hunk: &[u8]) -> Result<()> {
        if !self.buffer.is_empty() {
            return Err(Error::InvalidState);
        }
        self.write_full_hunk(hunk)?;
        self.written += hunk.len() as u64;
        Ok(())
    }

    /// Finish writing the CHD, returning the inner writer.
    pub fn finish(mut self) -> Result<W> {
        // Pad the last hunk with zeroes.
        if !self.buffer.is_empty() {
            let mut hunk = std::mem::take(&mut self.buffer);
            hunk.resize(self.options.hunk_bytes as usize, 0);
            self.write_full_hunk(&hunk)?;
        }

        let logical_bytes = match self.options.logical_bytes {
            Some(logical_bytes) if self.written > logical_bytes => return Err(Error::InvalidData),
            Some(logical_bytes) => logical_bytes,
            None => self.written,
        };

        // Hunks that were never written are zeroes.
        let hunk_count = logical_bytes.div_ceil(self.options.hunk_bytes as u64);
        let hunk_count = u32::try_from(hunk_count).map_err(|_| Error::InvalidData)?;
        self.map.resize(hunk_count as usize, 0);

        // The map is either preallocated after the header, or written at the end.
        let map_offset = if self.hunk_count.is_some() {
            HEADER_BYTES
        } else {
            self.eof
        };
        let header = self.header(logical_bytes, map_offset);
        let map = Map::new_uncompressed_v5(&self.map, self.options.hunk_bytes);
        let map_bytes = map.to_bytes(&Header::V5Header(header.clone()))?;

        self.inner.seek(SeekFrom::Start(map_offset))?;
        self.inner.write_all(&map_bytes)?;
        self.inner.seek(SeekFrom::Start(0))?;
        self.inner.write_all(&header.to_bytes())?;
        self.inner.flush()?;
        Ok(self.inner)
    }

    /// Write the given hunk to the file.
    fn write_full_hunk(&mut self, hunk: &[u8]) -> Result<()> {
        let hunk_bytes = self.options.hunk_bytes as u64;
        if hunk.len() as u64 != hunk_bytes {
            return Err(Error::InvalidParameter);
        }
        if self
            .hunk_count
            .is_some_and(|hunk_count| self.map.len() >= hunk_count as usize)
        {
            return Err(Error::HunkOutOfRange);
        }

        // Hunks of zeroes are not stored.
        if hunk.iter().all(|&byte| byte == 0) {
            self.map.push(0);
            return Ok(());
        }

        // Otherwise, append the hunk aligned to the hunk size, padding with zeroes.
        let offset = self.eof.next_multiple_of(hunk_bytes);
        let hunk_offset = u32::try_from(offset / hunk_bytes).map_err(|_| Error::InvalidData)?;
        self.inner.seek(SeekFrom::Start(self.eof))?;
        io::copy(&mut io::repeat(0).take(offset - self.eof), &mut self.inner)?;
        self.inner.write_all(hunk)?;
        self.eof = offset + hunk_bytes;
        self.map.push(hunk_offset);
        Ok(())
    }

    /// The header for an uncompressed CHD of the given logical size.
    fn header(&self, logical_bytes: u64, map_offset: u64) -> HeaderV5 {
        let ChdOptions {
            hunk_bytes,
            unit_bytes,
            ..
        } = self.options;
        HeaderV5 {
            version: Version::ChdV5,
            length: HEADER_BYTES as u32,
            // MAME does not compute SHA1s for uncompressed CHDs.
            compression: [0; 4],
            logical_bytes,
            map_offset,
            meta_offset: 0,
            hunk_bytes,
            unit_bytes,
            sha1: [0; 20],
            parent_sha1: [0; 20],
            raw_sha1: [0; 20],
            unit_count: logical_bytes.div_ceil(unit_bytes as u64),
            hunk_count: logical_bytes.div_ceil(hunk_bytes as u64) as u32,
            map_entry_bytes: 4,
        }
    }
}

impl<W: Write + Seek> Write for ChdWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let hunk_bytes = self.options.hunk_bytes as usize;
        let len = buf.len().min(hunk_bytes - self.buffer.len());
        self.buffer.extend_from_slice(&buf[..len]);
        self.written += len as u64;

        if self.buffer.len() == hunk_bytes {
            let hunk = std::mem::take(&mut self.buffer);
            self.write_full_hunk(&hunk)?;
            self.buffer = hunk;
            self.buffer.clear();
        }
        Ok(len)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::{ChdOptions, ChdWriter};
    use crate::map::MapEntry;
    use crate::{Chd, Error};
    use std::io::{Cursor, Write};

    const HUNK_BYTES: u32 = 4096;
    const UNIT_BYTES: u32 = 512;

    /// Data with zero hunks, a repeated hunk, and a partial last hunk.
    fn data() -> Vec<u8> {
        let mut data = Vec::new();
        for hunk in 0..6u8 {
            let fill = if hunk == 1 || hunk == 4 { 0 } else { hunk + 1 };
            data.extend(std::iter::repeat_n(fill, HUNK_BYTES as usize));
        }
        data.extend(std::iter::repeat_n(9u8, 1000));
        data
    }

    fn write(options: ChdOptions, data: &[u8]) -> Vec<u8> {
        let mut writer = ChdWriter::new(Cursor::new(Vec::new()), options).unwrap();
        // Write in uneven pieces to exercise buffering.
        for chunk in data.chunks(1500) {
            writer.write_all(chunk).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    /// Read all the hunks of a CHD, returning the data and the map offsets.
    fn read(chd: Vec<u8>) -> (Vec<u8>, u64, Vec<u64>) {
        let mut chd = Chd::open(Cursor::new(chd), None).unwrap();
        let logical_bytes = chd.header().logical_bytes();
        let offsets = chd
            .map()
            .iter()
            .map(|entry| match entry {
                MapEntry::V5Uncompressed(entry) => entry.block_offset(),
                _ => panic!("not an uncompressed V5 map"),
            })
            .collect();

        let mut data = Vec::new();
        let mut hunk = chd.get_hunksized_buffer();
        let mut compressed = Vec::new();
        for hunk_num in 0..chd.header().hunk_count() {
            chd.hunk(hunk_num)
                .unwrap()
                .read_hunk_in(&mut compressed, &mut hunk)
                .unwrap();
            data.extend_from_slice(&hunk);
        }
        (data, logical_bytes, offsets)
    }

    #[test]
    fn roundtrip() {
        let data = data();
        for options in [
            ChdOptions::new(HUNK_BYTES, UNIT_BYTES),
            ChdOptions::new(HUNK_BYTES, UNIT_BYTES).logical_bytes(data.len() as u64),
        ] {
            let preallocated = options.logical_bytes.is_some();
            let (read, logical_bytes, offsets) = read(write(options, &data));
            assert_eq!(logical_bytes, data.len() as u64);
            assert_eq!(&read[..data.len()], &data[..]);
            assert!(read[data.len()..].iter().all(|&byte| byte == 0));

            // Zero hunks are not stored, and stored hunks are aligned to the hunk size.
            assert_eq!(offsets.len(), 7);
            assert_eq!((offsets[1], offsets[4]), (0, 0));
            assert!(offsets
                .iter()
                .all(|&offset| offset % HUNK_BYTES as u64 == 0));
            if preallocated {
                // The map follows the header, so the first hunk is aligned after it.
                assert_eq!(offsets[0], HUNK_BYTES as u64);
            }
        }
    }

    #[test]
    fn unwritten_hunks_are_zero() {
        let data = vec![7u8; HUNK_BYTES as usize];
        let options = ChdOptions::new(HUNK_BYTES, UNIT_BYTES).logical_bytes(4 * HUNK_BYTES as u64);
        let (read, logical_bytes, offsets) = read(write(options, &data));
        assert_eq!(logical_bytes, 4 * HUNK_BYTES as u64);
        assert_eq!(&read[..data.len()], &data[..]);
        assert!(read[data.len()..].iter().all(|&byte| byte == 0));
        assert_eq!(&offsets[1..], &[0, 0, 0]);
    }

    #[test]
    fn write_hunks() {
        let data = data();
        let mut writer = ChdWriter::new(
            Cursor::new(Vec::new()),
            ChdOptions::new(HUNK_BYTES, UNIT_BYTES),
        )
        .unwrap();
        for hunk in data.chunks_exact(HUNK_BYTES as usize) {
            writer.write_hunk(hunk).unwrap();
        }
        let (read, logical_bytes, _) = read(writer.finish().unwrap().into_inner());
        assert_eq!(logical_bytes, 6 * HUNK_BYTES as u64);
        assert_eq!(&read[..], &data[..6 * HUNK_BYTES as usize]);
    }

    #[test]
    fn invalid_options() {
        for (hunk_bytes, unit_bytes) in [(0, 512), (4096, 0), (4096, 1000)] {
            assert!(matches!(
                ChdWriter::new(
                    Cursor::new(Vec::new()),
                    ChdOptions::new(hunk_bytes, unit_bytes)
                ),
                Err(Error::InvalidParameter)
            ));
        }
    }

    #[test]
    fn invalid_writes() {
        let options = ChdOptions::new(HUNK_BYTES, UNIT_BYTES);
        let mut writer = ChdWriter::new(Cursor::new(Vec::new()), options.clone()).unwrap();
        assert!(matches!(
            writer.write_hunk(&[0; 100]),
            Err(Error::InvalidParameter)
        ));

        // Hunks can not be written in the middle of a partially written hunk.
        writer.write_all(&[1; 100]).unwrap();
        assert!(matches!(
            writer.write_hunk(&[0; HUNK_BYTES as usize]),
            Err(Error::InvalidState)
        ));

        // No more than the logical size can be written.
        let mut writer = ChdWriter::new(
            Cursor::new(Vec::new()),
            options.logical_bytes(HUNK_BYTES as u64),
        )
        .unwrap();
        writer.write_hunk(&[1; HUNK_BYTES as usize]).unwrap();
        assert!(matches!(
            writer.write_hunk(&[1; HUNK_BYTES as usize]),
            Err(Error::HunkOutOfRange)
        ));
    }
}
