//! Helpers shared by the commands.
use crate::error::{Error, Result};
use arrayvec::ArrayVec;
use chd::header::{CodecType, Header};
use chd::metadata::Metadata;
use chd::write::{ChdWriteCodecs, ChdWriteOptions, ChdWriter, CompressedHunk, HunkCompressor};
use chd::Chd;
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Cursor, Read, Seek, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::thread;

pub(crate) type ChdFile = Chd<BufReader<File>>;

/// Progress reporting function.
///
/// The first argument is the progress so far, and the second argument is
/// the total amount of work to be done.
pub type Progress<'a> = &'a mut dyn FnMut(u64, u64);

/// The smallest hunk size accepted.
const HUNK_SIZE_MIN: u32 = 16;
/// The largest hunk size accepted.
const HUNK_SIZE_MAX: u32 = 1024 * 1024;

pub(crate) const fn make_tag(tag: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*tag)
}

pub(crate) const DVD_METADATA_TAG: u32 = make_tag(b"DVD ");

/// Open the CHD at the path, and its parent if it has one.
pub fn open_chd(path: &Path, parent: Option<&Path>) -> Result<Chd<BufReader<File>>> {
    let open = |path: &Path, parent: Option<Box<ChdFile>>| {
        let file = File::open(path).map_err(Error::file(path))?;
        Chd::open(BufReader::new(file), parent).map_err(|source| Error::OpenChd {
            path: path.to_owned(),
            source,
        })
    };
    let parent = parent.map(|parent| open(parent, None)).transpose()?;
    open(path, parent.map(Box::new))
}

/// Open a CHD without a parent to edit its header or metadata.
pub(crate) fn open_writable(path: &Path) -> Result<Chd<File>> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(Error::file(path))?;
    Chd::open(file, None).map_err(|source| Error::OpenChd {
        path: path.to_owned(),
        source,
    })
}

#[derive(Default)]
pub(crate) struct Outputs {
    // If `finish` isn't called, the incomplete files that got deleted
    paths: Vec<PathBuf>,
    done: bool,
}

impl Outputs {
    /// Create an output file, overwriting it if it exists.
    pub fn create(&mut self, path: &Path) -> Result<File> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .map_err(Error::file(path))?;
        self.paths.push(path.to_owned());
        Ok(file)
    }

    /// Keep the output files.
    pub fn finish(mut self) {
        self.done = true;
    }
}

impl Drop for Outputs {
    fn drop(&mut self) {
        if !self.done {
            for path in &self.paths {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// A position in an input, in either bytes or hunks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    /// A number of bytes.
    Bytes(u64),
    /// A number of hunks.
    Hunks(u64),
}

impl Position {
    fn bytes(self, hunk_bytes: u32) -> u64 {
        match self {
            Position::Bytes(bytes) => bytes,
            Position::Hunks(hunks) => hunks * hunk_bytes as u64,
        }
    }
}

/// The span of an input stream.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InputSpan {
    /// Where to start in the input.
    pub start: Option<Position>,
    /// The length of the span.
    pub length: Option<Position>,
}

impl InputSpan {
    /// Whether the span is the whole input.
    pub fn is_whole(&self) -> bool {
        self.start.is_none() && self.length.is_none()
    }

    /// The byte range of an input of the given size, with hunks of the given size.
    pub fn resolve(&self, size: u64, hunk_bytes: u32) -> Result<Range<u64>> {
        let start = self.start.map_or(0, |start| start.bytes(hunk_bytes));
        let length = self.length.map(|length| length.bytes(hunk_bytes));
        if start >= size && !(start == 0 && size == 0) {
            return Err(Error::InvalidOption(
                "Input start offset is beyond end of input".into(),
            ));
        }
        if let Some(length) = length {
            if start + length > size {
                return Err(Error::InvalidOption(
                    "Input length is larger than available input from start offset".into(),
                ));
            }
        }
        Ok(start..length.map_or(size, |length| start + length))
    }
}

/// Options for writing a CHD.
#[derive(Debug, Clone, Default)]
pub struct WriteOptions {
    compression: Option<ArrayVec<CodecType, 4>>,
    hunk_bytes: Option<u32>,
    threads: usize,
}

impl WriteOptions {
    /// Set the codecs, or none for an uncompressed CHD.
    pub fn compression(mut self, codecs: impl ChdWriteCodecs) -> Self {
        self.compression = Some(codecs.into_array_vec());
        self
    }

    /// Set the hunk size in bytes.
    pub fn hunk_bytes(mut self, hunk_bytes: u32) -> Self {
        self.hunk_bytes = Some(hunk_bytes);
        self
    }

    /// Set the number of threads to compress with, or 0 for one per processor.
    pub fn threads(mut self, threads: usize) -> Self {
        self.threads = threads;
        self
    }

    /// The codecs used to write the CHD.
    pub(crate) fn codecs(&self) -> Option<&ArrayVec<CodecType, 4>> {
        self.compression.as_ref()
    }

    pub(crate) fn resolve(
        &self,
        preset: ChdWriteOptions,
        parent: Option<&Header>,
    ) -> Result<ChdWriteOptions> {
        let invalid = |message: String| Err(Error::InvalidOption(message));
        let (default_hunk_bytes, unit_bytes) = preset.sizes();
        let parent_sizes = parent.map(|parent| (parent.hunk_size(), parent.unit_bytes()));
        if let Some((_, parent_unit_bytes)) = parent_sizes {
            if parent_unit_bytes != unit_bytes {
                return invalid(format!(
                    "Unit size {unit_bytes} bytes does not match output parent CHD unit size {parent_unit_bytes} bytes"
                ));
            }
        }
        let hunk_bytes = match (self.hunk_bytes, parent_sizes) {
            (Some(hunk_bytes), Some((parent_hunk_bytes, _))) if hunk_bytes != parent_hunk_bytes => {
                return invalid(format!(
                    "Specified hunk size {hunk_bytes} bytes does not match output parent CHD hunk size {parent_hunk_bytes} bytes"
                ))
            }
            (Some(hunk_bytes), _) if hunk_bytes < HUNK_SIZE_MIN => {
                return invalid(format!("Invalid hunk size (minimum {HUNK_SIZE_MIN})"))
            }
            (Some(hunk_bytes), _) if hunk_bytes > HUNK_SIZE_MAX => {
                return invalid(format!("Invalid hunk size (maximum {HUNK_SIZE_MAX})"))
            }
            (Some(hunk_bytes), _) => hunk_bytes,
            (None, Some((parent_hunk_bytes, _))) => parent_hunk_bytes,
            (None, None) => default_hunk_bytes,
        };
        if unit_bytes == 0 || hunk_bytes % unit_bytes != 0 {
            return invalid(format!(
                "Hunk size {hunk_bytes} bytes is not a whole multiple of {unit_bytes}"
            ));
        }

        let mut options = preset.hunk_bytes(hunk_bytes);
        if let Some(codecs) = &self.compression {
            options = options.compression(codecs);
        }
        Ok(options)
    }

    pub(crate) fn create<W: Write + Seek, F: Read + Seek>(
        &self,
        inner: W,
        options: ChdWriteOptions,
        parent: Option<&mut Chd<F>>,
        metadata: Vec<NewMetadata>,
    ) -> Result<ParallelWriter<W>> {
        let hunk_bytes = options.sizes().0;
        let mut writer = match parent {
            Some(parent) => ChdWriter::with_parent(inner, options, parent),
            None => ChdWriter::new(inner, options),
        }?;
        for entry in metadata {
            writer.add_metadata(entry.tag, entry.value.as_slice(), entry.checksum)?;
        }
        let threads = match self.threads {
            0 => thread::available_parallelism().map_or(1, |threads| threads.get()),
            threads => threads,
        };
        let compressors = (0..threads)
            .map(|_| writer.compressor())
            .collect::<chd::Result<Vec<_>>>()?;
        let hunk_bytes = hunk_bytes as usize;
        Ok(ParallelWriter {
            batch: Vec::with_capacity(threads * 8 * hunk_bytes),
            writer,
            compressors,
            hunk_bytes,
        })
    }
}

pub(crate) type NoParent = Chd<Cursor<Vec<u8>>>;

const METADATA_CHECKSUM: u8 = 1;

/// A metadata entry to add to a new CHD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NewMetadata {
    pub tag: u32,
    pub value: Vec<u8>,
    pub checksum: bool,
}

impl NewMetadata {
    /// Checksummed text metadata, which chdman stores with a terminating NUL.
    pub fn text(tag: u32, text: &str) -> Self {
        let mut value = text.as_bytes().to_vec();
        value.push(0);
        NewMetadata {
            tag,
            value,
            checksum: true,
        }
    }
}

impl From<Metadata> for NewMetadata {
    fn from(entry: Metadata) -> Self {
        NewMetadata {
            tag: entry.metatag,
            value: entry.value,
            checksum: entry.flags & METADATA_CHECKSUM != 0,
        }
    }
}

/// Writes the data of a CHD, compressing a batch of hunks at a time on every thread.
pub(crate) struct ParallelWriter<W: Write + Seek> {
    writer: ChdWriter<W>,
    compressors: Vec<HunkCompressor>,
    hunk_bytes: usize,
    batch: Vec<u8>,
}

impl<W: Write + Seek> ParallelWriter<W> {
    /// Compress and write the whole hunks of the batch.
    fn write_batch(&mut self) -> chd::Result<()> {
        let whole = self.batch.len() / self.hunk_bytes * self.hunk_bytes;
        let hunks: Vec<&[u8]> = self.batch[..whole].chunks(self.hunk_bytes).collect();
        let per_thread = hunks.len().div_ceil(self.compressors.len()).max(1);
        let compressed: Vec<Vec<CompressedHunk>> = thread::scope(|scope| {
            let workers: Vec<_> = hunks
                .chunks(per_thread)
                .zip(self.compressors.iter_mut())
                .map(|(hunks, compressor)| {
                    scope.spawn(move || {
                        hunks
                            .iter()
                            .map(|hunk| compressor.compress(hunk))
                            .collect::<chd::Result<Vec<_>>>()
                    })
                })
                .collect();
            workers
                .into_iter()
                .map(|worker| worker.join().expect("compression thread panicked"))
                .collect::<chd::Result<Vec<_>>>()
        })?;
        for hunk in compressed.into_iter().flatten() {
            self.writer.write_compressed_hunk(hunk)?;
        }
        self.batch.drain(..whole);
        Ok(())
    }

    /// Write the rest of the data, and finish the CHD.
    pub fn finish(mut self) -> Result<W> {
        self.write_batch()?;
        // Only the end of the data can be part of a hunk, which the writer pads.
        self.writer.write_all(&self.batch)?;
        let mut inner = self.writer.finish()?;
        inner.flush()?;
        Ok(inner)
    }
}

impl<W: Write + Seek> Write for ParallelWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let count = buf.len().min(self.batch.capacity() - self.batch.len());
        self.batch.extend_from_slice(&buf[..count]);
        if self.batch.len() == self.batch.capacity() {
            self.write_batch()?;
        }
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A reader that reports the number of bytes read from it as progress.
pub(crate) struct ProgressReader<'a, R> {
    inner: R,
    done: u64,
    total: u64,
    progress: Progress<'a>,
}

impl<'a, R: Read> ProgressReader<'a, R> {
    /// Read `total` bytes of the input.
    pub fn new(inner: R, total: u64, progress: Progress<'a>) -> Self {
        progress(0, total);
        ProgressReader {
            inner,
            done: 0,
            total,
            progress,
        }
    }
}

impl<R: Read> Read for ProgressReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.done += read as u64;
        (self.progress)(self.done, self.total);
        Ok(read)
    }
}

/// Copy exactly `length` bytes of the input to the output.
pub(crate) fn copy_exact(input: impl Read, output: &mut impl Write, length: u64) -> Result<()> {
    if io::copy(&mut input.take(length), output)? != length {
        return Err(Error::InputEnded);
    }
    Ok(())
}

/// Fail if the output exists, unless overwriting it is forced.
pub(crate) fn check_output(path: &Path, force: bool) -> Result<()> {
    if !force && path.exists() {
        return Err(Error::OutputExists(path.to_owned()));
    }
    Ok(())
}

/// Open the parent of an output CHD if there is one, and check that the output can be
/// written.
pub(crate) fn open_output(
    output: &Path,
    parent: Option<&Path>,
    force: bool,
) -> Result<Option<ChdFile>> {
    let parent = parent.map(|parent| open_chd(parent, None)).transpose()?;
    check_output(output, force)?;
    Ok(parent)
}

/// A CHD file to create, with its options resolved.
pub(crate) struct NewChd<'a> {
    options: ChdWriteOptions,
    parent: Option<ChdFile>,
    write: &'a WriteOptions,
}

impl<'a> NewChd<'a> {
    /// Resolve the options for a new CHD over a preset. See [`WriteOptions::resolve`].
    pub fn new(
        write: &'a WriteOptions,
        preset: ChdWriteOptions,
        parent: Option<ChdFile>,
    ) -> Result<Self> {
        let options = write.resolve(preset, parent.as_ref().map(|parent| parent.header()))?;
        Ok(NewChd {
            options,
            parent,
            write,
        })
    }

    /// The hunk size of the CHD.
    pub fn hunk_bytes(&self) -> u32 {
        self.options.sizes().0
    }

    pub fn write(
        mut self,
        path: &Path,
        metadata: Vec<NewMetadata>,
        input: impl Read,
        length: u64,
        progress: Progress,
    ) -> Result<()> {
        let mut outputs = Outputs::default();
        let file = BufWriter::new(outputs.create(path)?);
        let mut chd = self.write.create(
            file,
            self.options.logical_bytes(length),
            self.parent.as_mut(),
            metadata,
        )?;
        copy_exact(
            ProgressReader::new(input, length, progress),
            &mut chd,
            length,
        )?;
        chd.finish()?;
        outputs.finish();
        Ok(())
    }
}
