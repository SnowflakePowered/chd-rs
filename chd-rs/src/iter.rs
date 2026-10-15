//! Lending iterators for hunks and metadata.
//!
//! ## Iterating over hunks with `Lender`
//! [`Lender`] allows a more ergonomic interface to iterate over hunks.
//!```no_run
//! use std::fs::File;
//! use std::io::BufReader;
//! use chd::iter::Lender;
//! use chd::Chd;
//!
//! # fn main() -> chd::Result<()> {
//! let mut f = BufReader::new(File::open("file.chd")?);
//! let mut chd = Chd::open(&mut f, None)?;
//!
//! // buffer to store uncompressed hunk data must be the same length as the hunk size.
//! let mut hunk_buf = chd.get_hunksized_buffer();
//! // buffer to store compressed data.
//! let mut cmp_buf = Vec::new();
//! let mut hunks = chd.hunks();
//! while let Some(mut hunk) = hunks.next() {
//!    hunk.read_hunk_in(&mut cmp_buf, &mut hunk_buf)?;
//! }
//! # Ok(())
//! # }
//! ```
//!
//! ## Iterating over metadata with `FallibleLender`
//! [`FallibleLender`] allows iterating over metadata without keeping a reference to the source file.
//!```no_run
//! use std::fs::File;
//! use std::io::BufReader;
//! use chd::iter::FallibleLender;
//! use chd::Chd;
//!
//! # fn main() -> chd::Result<()> {
//! let mut f = BufReader::new(File::open("file.chd")?);
//! let mut chd = Chd::open(&mut f, None)?;
//! let mut metadata = chd.metadata();
//! while let Some(mut metadata) = metadata.next()? {
//!    let metadata = metadata.read()?;
//! }
//! # Ok(())
//! # }
//! ```
use crate::metadata::{Metadata, MetadataRef, MetadataRefs, MetadataTag};
use crate::{Chd, Error, Hunk, Result};
use lender::{check_covariance, check_covariance_fallible, FallibleLending, Lending};
use std::io::{Read, Seek};

/// The lending iterator traits re-exported from the [lender](https://crates.io/crates/lender) crate,
/// which provide adapters that map to those from [`Iterator`](core::iter::Iterator).
pub use lender::{FallibleLender, Lender};

/// An iterator over the hunks of a CHD file.
pub struct Hunks<'a, F: Read + Seek> {
    inner: &'a mut Chd<F>,
    last_hunk: u32,
    current_hunk: u32,
}

impl<'a, F: Read + Seek> Hunks<'a, F> {
    pub(crate) fn new(inner: &'a mut Chd<F>) -> Self {
        let last_hunk = inner.header().hunk_count();
        Hunks {
            inner,
            last_hunk,
            current_hunk: 0,
        }
    }
}

impl<'lend, F: Read + Seek> Lending<'lend> for Hunks<'_, F> {
    type Lend = Hunk<'lend, F>;
}

impl<F: Read + Seek> Lender for Hunks<'_, F> {
    check_covariance!();

    fn next(&mut self) -> Option<Hunk<'_, F>> {
        if self.current_hunk == self.last_hunk {
            return None;
        }
        let curr = self.current_hunk;
        self.current_hunk += 1;
        self.inner.hunk(curr).ok()
    }
}

/// An iterator over the metadata entries of a CHD file.
///
/// If an entry can not be read, returns the error and then ends.
pub struct MetadataEntries<'a, F: Read + Seek + 'a> {
    inner: MetadataRefs<'a, F>,
}

impl<'a, F: Read + Seek + 'a> MetadataEntries<'a, F> {
    pub(crate) fn new(inner: MetadataRefs<'a, F>) -> Self {
        MetadataEntries { inner }
    }
}

impl<'a, F: Read + Seek + 'a> MetadataEntry<'a, F> {
    /// Read the contents of the metadata from the input stream.
    pub fn read(&mut self) -> Result<Metadata> {
        self.meta_ref.read(self.file)
    }
}

/// A metadata entry for a CHD file that has a reference to the source file,
/// allowing read metadata from the stream without an explicit reference.
pub struct MetadataEntry<'a, F: Read + Seek + 'a> {
    meta_ref: MetadataRef,
    file: &'a mut F,
}

impl<'a, F: Read + Seek + 'a> MetadataTag for MetadataEntry<'a, F> {
    fn metatag(&self) -> u32 {
        self.meta_ref.metatag()
    }
}

impl<'lend, F: Read + Seek> FallibleLending<'lend> for MetadataEntries<'_, F> {
    type Lend = MetadataEntry<'lend, F>;
}

impl<F: Read + Seek> FallibleLender for MetadataEntries<'_, F> {
    type Error = Error;
    check_covariance_fallible!();

    fn next(&mut self) -> Result<Option<MetadataEntry<'_, F>>> {
        let Some(meta_ref) = self.inner.next().transpose()? else {
            return Ok(None);
        };
        Ok(Some(MetadataEntry {
            meta_ref,
            file: self.inner.file,
        }))
    }
}
