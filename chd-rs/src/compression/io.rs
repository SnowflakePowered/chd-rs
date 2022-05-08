use std::io::{IoSliceMut, Read};
use claxon::input::ReadBytes;

/// Wraps a `Read` that counts the number of elements read.
///
/// Always wrap the lowest `Read` for accurate results. If wrapping a `BufRead`,
/// the position will not update if the `BufRead` associated functions are called
/// instead of `Read::read`.
///
/// If the underlying `Read` supports vectored reads, CountingReader will also
/// support vectored reads with accurate position count.
pub(crate) struct CountingReader<R> {
    inner: R,
    pos: usize
}

impl <R> CountingReader<R> {
    pub fn new(r: R) -> CountingReader<R> {
        CountingReader {
            inner: r,
            pos: 0
        }
    }

    pub fn total_read(&self) -> usize {
        self.pos
    }
}

impl <R: Read> Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.pos += read;
        Ok(read)
    }

    fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> std::io::Result<usize> {
        let read = self.inner.read_vectored(bufs)?;
        self.pos += read;
        Ok(read)
    }
}

impl <R: ReadBytes> ReadBytes for CountingReader<R> {
    fn read_u8(&mut self) -> std::io::Result<u8> {
        let res = self.inner.read_u8()?;
        self.pos += 1;
        Ok(res)
    }

    fn read_u8_or_eof(&mut self) -> std::io::Result<Option<u8>> {
        let res = self.inner.read_u8_or_eof()?;
        if res.is_some() {
            self.pos += 1;
        }
        Ok(res)
    }

    fn read_into(&mut self, buffer: &mut [u8]) -> std::io::Result<()> {
        self.inner.read_into(buffer)?;
        self.pos += buffer.len();
        Ok(())
    }

    fn skip(&mut self, amount: u32) -> std::io::Result<()> {
        self.inner.skip(amount)?;
        self.pos += amount as usize;
        Ok(())
    }
}