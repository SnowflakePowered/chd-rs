//! Traits and implementations for lending iterators for hunks and metadata.
//! These APIs should be considered unstable and will be replaced with APIs
//! based around GATs and `LendingIterator` once stabilized. See [rust#44265](https://github.com/rust-lang/rust/issues/44265)
//! for the tracking issue on GAT stabilization.
use crate::Result;
use crate::{ChdFile, ChdHunk};
use std::io::{Read, Seek};
use lending_iterator::prelude::*;
use crate::metadata::{ChdMetadata, ChdMetadataTag, MetadataRef, MetadataRefIter};

#[::nougat::gat(Item)]
/// A `LendingIterator` definition re-exported from the [lending-iterator](https://crates.io/crates/lending-iterator)
/// crate. Provides an lending iterator interface with various [adapters](https://docs.rs/lending-iterator/0.1.5/lending_iterator/lending_iterator/adapters/index.html)
/// that map to those from [`Iterator`](core::iter::Iterator).
///
/// This crate defined `LendingIterator` will be replaced once a stabilized trait lands in `std`, and should
/// not be considered stable.
///
pub use lending_iterator::lending_iterator::LendingIterator;

/// An iterator over the hunks of a CHD file.
pub struct HunkIter<'a, F: Read + Seek> {
    inner: &'a mut ChdFile<F>,
    last_hunk: u32,
    current_hunk: u32,
}

impl<'a, F: Read + Seek> HunkIter<'a, F> {
    pub(crate) fn new(inner: &'a mut ChdFile<F>) -> Self {
        let last_hunk = inner.header().hunk_count();
        HunkIter {
            inner,
            last_hunk,
            current_hunk: 0,
        }
    }
}

#[::nougat::gat]
impl<'a, F: Read + Seek> LendingIterator for HunkIter<'a, F> {
    type Item<'next>
        where
            Self: 'next, = ChdHunk<'next, F>;

    fn next(&'_ mut self) -> Option<ChdHunk<'_, F>> {
        if self.current_hunk == self.last_hunk {
            return None;
        }
        let curr = self.current_hunk;
        self.current_hunk += 1;
        self.inner.hunk(curr).ok()
    }
}

/// An iterator over the metadata entries of a CHD file.
pub struct MetadataIter<'a, F: Read + Seek + 'a> {
    inner: MetadataRefIter<'a, F>,
}

impl<'a, F: Read + Seek + 'a> MetadataIter<'a, F> {
    pub(crate) fn new(inner: MetadataRefIter<'a, F>) -> Self {
        MetadataIter { inner }
    }
}

impl<'a, F: Read + Seek + 'a> MetadataEntry<'a, F> {
    /// Read the contents of the metadata from the input stream.
    pub fn read(&mut self) -> Result<ChdMetadata> {
        self.meta_ref.read(self.file)
    }
}

/// A metadata entry for a CHD file that has a reference to the source file,
/// allowing read metadata from the stream without an explicit reference.
pub struct MetadataEntry<'a, F: Read + Seek + 'a> {
    meta_ref: MetadataRef,
    file: &'a mut F,
}

impl<'a, F: Read + Seek + 'a> ChdMetadataTag for MetadataEntry<'a, F> {
    fn metatag(&self) -> u32 {
        self.meta_ref.metatag()
    }
}

#[::nougat::gat]
impl<'a, F: Read + Seek> LendingIterator for MetadataIter<'a, F> {
    type Item<'next>
        where
            Self: 'next, = MetadataEntry<'next, F>;

    fn next(&'_ mut self) -> Option<Item<'_, Self>> {
        let next = self.inner.next();
        if let Some(next) = next {
            Some(MetadataEntry {
                meta_ref: next,
                file: self.inner.file,
            })
        } else {
            None
        }
    }
}
