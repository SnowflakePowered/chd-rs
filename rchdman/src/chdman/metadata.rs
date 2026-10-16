//! `addmeta`, `delmeta`, and `dumpmeta`.
use crate::common::{open_chd, open_writable};
use crate::error::{Error, Result};
use chd::metadata::Metadata;
use std::path::Path;

/// Set the `index`-th metadata entry with the tag, or add it if there is no such entry.
/// Entries with `checksum` set are included in the SHA1 of the CHD.
///
/// Text from `chdman addmeta -vt` ends with a NUL, which the value must include to match.
pub fn add_metadata(path: &Path, tag: u32, index: u32, value: &[u8], checksum: bool) -> Result<()> {
    open_writable(path)?.set_metadata(tag, index, value, checksum)?;
    Ok(())
}

/// Remove the `index`-th metadata entry with the tag.
pub fn remove_metadata(path: &Path, tag: u32, index: u32) -> Result<()> {
    open_writable(path)?.remove_metadata(tag, index)?;
    Ok(())
}

/// Read the `index`-th metadata entry with the tag.
pub fn dump_metadata(path: &Path, tag: u32, index: u32) -> Result<Metadata> {
    let mut chd = open_chd(path, None)?;
    let metadata: Vec<Metadata> = chd.metadata_refs().try_into()?;
    metadata
        .into_iter()
        .find(|entry| entry.metatag == tag && entry.index == index)
        .ok_or(Error::MetadataNotFound { tag, index })
}
