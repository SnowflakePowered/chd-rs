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

/// Utility adapter for [`Chd`](crate::Chd) that implements `Read + Seek` over the logical
/// data of the CHD.
///
/// `ChdReader` will allocate and manage intermediate buffers to support
/// reading at a byte granularity. If performance is a concern, it is recommended
/// to instead iterate over hunk indices.
pub struct ChdReader<F: Read + Seek> {
    chd: Chd<F>,
    /// The position in the logical data.
    position: u64,
    cmp_buf: Vec<u8>,
    /// The decompressed contents of hunk `hunk_num`.
    hunk: Vec<u8>,
    hunk_num: Option<u32>,
}

impl<F: Read + Seek> ChdReader<F> {
    /// Create a new `ChdReader` from an opened [`Chd`](crate::Chd).
    pub fn new(chd: Chd<F>) -> Self {
        ChdReader {
            chd,
            position: 0,
            cmp_buf: Vec::new(),
            hunk: Vec::new(),
            hunk_num: None,
        }
    }

    /// Decompress the hunk if it is not the current hunk.
    fn load_hunk(&mut self, hunk_num: u32) -> Result<()> {
        if self.hunk_num == Some(hunk_num) {
            return Ok(());
        }
        // Forget the current hunk in case decompressing fails part way through.
        self.hunk_num = None;
        self.hunk.resize(self.chd.header().hunk_size() as usize, 0);
        self.chd
            .hunk(hunk_num)?
            .read_hunk_in(&mut self.cmp_buf, &mut self.hunk)?;
        self.hunk_num = Some(hunk_num);
        Ok(())
    }
}

impl<F: Read + Seek> Read for ChdReader<F> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let len = self.chd.header().logical_bytes();
        if buf.is_empty() || self.position >= len {
            return Ok(0);
        }

        let hunk_size = self.chd.header().hunk_size() as u64;
        let hunk_num =
            u32::try_from(self.position / hunk_size).map_err(|_| Error::HunkOutOfRange)?;
        self.load_hunk(hunk_num)?;

        // Read up to the end of the hunk, or the end of the logical data.
        let offset = (self.position % hunk_size) as usize;
        let available = (hunk_size - offset as u64).min(len - self.position) as usize;
        let count = buf.len().min(available);
        buf[..count].copy_from_slice(&self.hunk[offset..offset + count]);
        self.position += count as u64;
        Ok(count)
    }
}

impl<F: Read + Seek> Seek for ChdReader<F> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let position = match pos {
            SeekFrom::Start(position) => Some(position),
            SeekFrom::End(offset) => self.chd.header().logical_bytes().checked_add_signed(offset),
            SeekFrom::Current(offset) => self.position.checked_add_signed(offset),
        };
        self.position = position.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid seek to a negative or overflowing position",
            )
        })?;
        Ok(self.position)
    }
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::{ChdReader, HunkBufReader};
    use crate::header::CodecType;
    use crate::write::{ChdWriteOptions, ChdWriter};
    use crate::Chd;
    use rand::rngs::StdRng;
    use rand::{RngExt, SeedableRng};
    use std::io::{BufRead, Cursor, Read, Seek, SeekFrom, Write};

    const HUNK_BYTES: u32 = 4096;

    /// Data that is not a whole number of hunks, so the last hunk is padded.
    fn data() -> Vec<u8> {
        let mut data = vec![0u8; 5 * HUNK_BYTES as usize + 1234];
        StdRng::seed_from_u64(1).fill(&mut data[..]);
        // Some compressible hunks and a repeated hunk too.
        data[..HUNK_BYTES as usize].fill(7);
        let (first, rest) = data.split_at_mut(2 * HUNK_BYTES as usize);
        rest[..HUNK_BYTES as usize].copy_from_slice(&first[HUNK_BYTES as usize..]);
        data
    }

    fn chd(data: &[u8]) -> Chd<Cursor<Vec<u8>>> {
        let options = ChdWriteOptions::new(HUNK_BYTES, 512).compression(&[CodecType::ZLibV5]);
        let mut writer = ChdWriter::new(Cursor::new(Vec::new()), options).unwrap();
        writer.write_all(data).unwrap();
        let mut chd = writer.finish().unwrap();
        chd.set_position(0);
        Chd::open(chd, None).unwrap()
    }

    #[test]
    fn read_to_end_reads_the_logical_data() {
        let data = data();
        let mut reader = ChdReader::new(chd(&data));
        let mut read = Vec::new();
        reader.read_to_end(&mut read).unwrap();
        assert_eq!(read.len(), data.len());
        assert!(read == data);
    }

    #[test]
    fn empty_reads_do_not_advance() {
        let data = data();
        let mut reader = ChdReader::new(chd(&data));
        let mut byte = [0u8; 1];
        reader.read_exact(&mut byte).unwrap();
        for _ in 0..10 {
            assert_eq!(reader.read(&mut []).unwrap(), 0);
        }
        reader.read_exact(&mut byte).unwrap();
        assert_eq!(byte[0], data[1]);
    }

    #[test]
    fn seek_from_start_end_and_current() {
        let data = data();
        let mut reader = ChdReader::new(chd(&data));
        let mut expected = Cursor::new(&data);
        let len = data.len() as i64;

        let seeks = [
            SeekFrom::Start(0),
            SeekFrom::Start(5000),
            SeekFrom::Current(100),
            SeekFrom::Current(-200),
            SeekFrom::Current(HUNK_BYTES as i64 * 2),
            SeekFrom::End(-10),
            SeekFrom::End(-(len)),
            SeekFrom::Start(HUNK_BYTES as u64 - 1),
            SeekFrom::Current(1),
        ];
        for seek in seeks {
            let ours = reader.seek(seek).unwrap();
            let theirs = expected.seek(seek).unwrap();
            assert_eq!(ours, theirs, "{seek:?}");

            let mut ours = [0u8; 300];
            let mut theirs = [0u8; 300];
            let ours_len = read_up_to(&mut reader, &mut ours);
            let theirs_len = read_up_to(&mut expected, &mut theirs);
            assert_eq!(ours[..ours_len], theirs[..theirs_len], "{seek:?}");
        }
    }

    #[test]
    fn random_seeks_and_reads() {
        let data = data();
        let mut reader = ChdReader::new(chd(&data));
        let mut expected = Cursor::new(&data);
        let mut rng = StdRng::seed_from_u64(2);
        for _ in 0..500 {
            let seek = match rng.random_range(0..3) {
                0 => SeekFrom::Start(rng.random_range(0..data.len() as u64)),
                1 => SeekFrom::End(-rng.random_range(1..=data.len() as i64)),
                _ => {
                    let position = expected.position() as i64;
                    SeekFrom::Current(rng.random_range(-position..data.len() as i64 - position))
                }
            };
            assert_eq!(
                reader.seek(seek).unwrap(),
                expected.seek(seek).unwrap(),
                "{seek:?}"
            );

            let len = rng.random_range(0..3 * HUNK_BYTES as usize);
            let mut ours = vec![0u8; len];
            let mut theirs = vec![0u8; len];
            let ours_len = read_up_to(&mut reader, &mut ours);
            let theirs_len = read_up_to(&mut expected, &mut theirs);
            assert_eq!(ours_len, theirs_len, "{seek:?} then read {len}");
            assert!(
                ours[..ours_len] == theirs[..theirs_len],
                "{seek:?} then read {len}"
            );
        }
    }

    #[test]
    fn seek_past_the_end() {
        let data = data();
        let mut reader = ChdReader::new(chd(&data));
        let past = data.len() as u64 + 100;
        assert_eq!(reader.seek(SeekFrom::Start(past)).unwrap(), past);
        assert_eq!(reader.read(&mut [0u8; 10]).unwrap(), 0);
        assert_eq!(reader.stream_position().unwrap(), past);

        // Seeking back is fine.
        reader.seek(SeekFrom::Start(3)).unwrap();
        let mut byte = [0u8; 1];
        reader.read_exact(&mut byte).unwrap();
        assert_eq!(byte[0], data[3]);
    }

    #[test]
    fn hunk_buf_reader() {
        let data = data();
        let mut chd = chd(&data);
        let mut buffer = Vec::new();
        for hunk_num in 0..chd.header().hunk_count() {
            let mut expected = chd.get_hunksized_buffer();
            chd.hunk(hunk_num)
                .unwrap()
                .read_hunk_in(&mut Vec::new(), &mut expected)
                .unwrap();

            // A reused buffer that is larger than a hunk is resized.
            buffer.resize(3 * HUNK_BYTES as usize, 0xff);
            let mut reader =
                HunkBufReader::new_in(&mut chd.hunk(hunk_num).unwrap(), &mut Vec::new(), buffer)
                    .unwrap();
            assert_eq!(reader.fill_buf().unwrap(), &expected[..]);
            reader.consume(100);
            reader.seek(SeekFrom::Current(-50)).unwrap();
            let mut rest = Vec::new();
            reader.read_to_end(&mut rest).unwrap();
            assert_eq!(rest, &expected[50..]);
            buffer = reader.into_inner();
        }
    }

    /// Read until the buffer is full or the end of the stream.
    fn read_up_to(reader: &mut impl Read, buf: &mut [u8]) -> usize {
        let mut total = 0;
        while total < buf.len() {
            match reader.read(&mut buf[total..]).unwrap() {
                0 => break,
                n => total += n,
            }
        }
        total
    }
}
