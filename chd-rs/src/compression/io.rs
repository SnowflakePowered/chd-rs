use std::io::{IoSliceMut, Read};

/// Wraps a `Read` that counts the number of elements read.
///
/// Always wrap the lowest `Read` for accurate results. If wrapping a `BufRead`,
/// the position will not update if the `BufRead` associated functions are called
/// instead of `Read::read`.
///
/// If the underlying `Read` supports vectored reads, CountingReader will also
/// support vectored reads with accurate position count.
pub(crate) struct CountingReader<R: Read> {
    read: R,
    pos: usize
}

impl <R: Read> CountingReader<R> {
    pub fn new(r: R) -> CountingReader<R> {
        CountingReader {
            read: r,
            pos: 0
        }
    }

    pub fn total_read(&self) -> usize {
        self.pos
    }
}

impl <R: Read> Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read = self.read.read(buf)?;
        self.pos += read;
        Ok(read)
    }

    fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> std::io::Result<usize> {
        let read = self.read.read_vectored(bufs)?;
        self.pos += read;
        Ok(read)
    }
}
