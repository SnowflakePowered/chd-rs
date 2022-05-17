//! Traits and implementations for lending iterators for hunks and metadata.
//! These APIs should be considered unstable and will be replaced with APIs
//! based around GATs and `LendingIterator` once stabilized. See [rust#44265](https://github.com/rust-lang/rust/issues/44265)
//! for the tracking issue on GAT stabilization.
use crate::{ChdFile, ChdHunk};
use std::io::{Read, Seek};
use crate::Result;

// Lifted from
// https://sabrinajewson.org/blog/the-better-alternative-to-lifetime-gats
/// The lifetime of an item from a [`LendingIterator`](crate::iter::LendingIterator).
pub trait LendingIteratorLifetime<'this, ImplicitBounds: Sealed = Bounds<&'this Self>> {
    type Item;
}

mod sealed {
    pub trait Sealed: Sized {}
    pub struct Bounds<T>(T);
    impl<T> Sealed for Bounds<T> {}
}

use sealed::{Bounds, Sealed};
use crate::metadata::{ChdMetadata, MetadataRef, MetadataRefIter, ChdMetadataTag};

/// An iterator interface that lends items from a higher lifetime.
pub trait LendingIterator: for<'this> LendingIteratorLifetime<'this> {
    fn next(&mut self) -> Option<<Self as LendingIteratorLifetime<'_>>::Item>;
}

/// An iterator over the hunks of a CHD file.
pub struct HunkIter<'a, F: Read + Seek> {
    inner: &'a mut ChdFile<F>,
    last_hunk: u32,
    current_hunk: u32,
}

impl <'a, F: Read + Seek> HunkIter<'a, F> {
    pub(crate) fn new(inner: &'a mut ChdFile<F>) -> Self {
        let last_hunk = inner.header().hunk_count();
        HunkIter {
            inner,
            last_hunk,
            current_hunk: 0
        }
    }
}

impl<'this, 'a, F: Read + Seek> LendingIteratorLifetime<'this> for HunkIter<'a, F> {
    type Item = ChdHunk<'this, F>;
}

impl<'a, F: Read + Seek> LendingIterator for HunkIter<'a, F> {
    fn next(&mut self) -> Option<<Self as LendingIteratorLifetime<'_>>::Item> {
        if self.current_hunk == self.last_hunk {
            return None;
        }
        let curr = self.current_hunk;
        self.current_hunk += 1;
        self.inner.hunk(curr).ok()
    }
}

#[cfg(feature = "unsound_owning_iterators")]
impl<'a, F: Read + Seek> Iterator for HunkIter<'a, F> {
    type Item = ChdHunk<'a, F>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current_hunk == self.last_hunk {
            return None;
        }
        let curr = self.current_hunk;
        self.current_hunk += 1;
        // SAFETY: need an unbound lifetime to get 'a.
        // todo: test under miri to confirm soundness
        // todo: need GATs to do this safely.
        unsafe { (self.inner as *mut ChdFile<F>).as_mut().unwrap_unchecked() }
            .hunk(curr)
            .ok()
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

impl<'this, 'a, F: Read + Seek> LendingIteratorLifetime<'this> for MetadataIter<'a, F> {
    type Item = MetadataEntry<'this, F>;
}

impl<'a, F: Read + Seek + 'a> ChdMetadataTag for MetadataEntry<'a, F> {
    fn metatag(&self) -> u32 {
        self.meta_ref.metatag
    }
}

impl<'a, F: Read + Seek> LendingIterator for MetadataIter<'a, F> {
    fn next(&mut self) -> Option<<Self as LendingIteratorLifetime<'_>>::Item> {
        let next = self.inner.next();
        if let Some(next) = next {
            Some(MetadataEntry {
                meta_ref: next,
                file: self.inner.file
            })
        } else {
            None
        }
    }
}

#[cfg(feature = "unsound_owning_iterators")]
impl<'a, F: Read + Seek + 'a> Iterator for MetadataIter<'a, F> {
    type Item = MetadataEntry<'a, F>;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|meta_ref| {
            let file = self.inner.file as *mut F;
            MetadataEntry {
                meta_ref,
                // SAFETY: need an unbound lifetime to get 'a.
                // todo: need GATs to do this safely.
                file: unsafe { file.as_mut().unwrap_unchecked() },
            }
        })
    }
}
