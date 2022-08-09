//! Traits and implementations for lending iterators for hunks and metadata.
//! These APIs should be considered unstable and will be replaced with APIs
//! based around GATs and `LendingIterator` once stabilized. See [rust#44265](https://github.com/rust-lang/rust/issues/44265)
//! for the tracking issue on GAT stabilization.
use crate::metadata::{ChdMetadataTag, Metadata, MetadataRef, MetadataRefs};
use crate::Result;
use crate::{Chd, Hunk};
use lending_iterator::prelude::*;
use std::io::{Read, Seek};

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

#[::nougat::gat]
impl<'a, F: Read + Seek> LendingIterator for Hunks<'a, F> {
    type Item<'next>
    where
        Self: 'next,
    = Hunk<'next, F>;

    fn next(&'_ mut self) -> Option<Hunk<'_, F>> {
        if self.current_hunk == self.last_hunk {
            return None;
        }
        let curr = self.current_hunk;
        self.current_hunk += 1;
        self.inner.hunk(curr).ok()
    }
}

/// An iterator over the metadata entries of a CHD file.
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

impl<'a, F: Read + Seek + 'a> ChdMetadataTag for MetadataEntry<'a, F> {
    fn metatag(&self) -> u32 {
        self.meta_ref.metatag()
    }
}

#[::nougat::gat]
impl<'a, F: Read + Seek> LendingIterator for MetadataEntries<'a, F> {
    type Item<'next>
    where
        Self: 'next,
    = MetadataEntry<'next, F>;

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
