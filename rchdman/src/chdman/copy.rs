//! `copy`.
use crate::cd::{Toc, FRAME_BYTES, SESSION_METADATA_TAG};
use crate::common::{
    open_chd, open_output, InputSpan, NewChd, NewMetadata, Progress, WriteOptions, DVD_METADATA_TAG,
};
use crate::error::{Error, Result};
use chd::metadata::{KnownMetadata, Metadata};
use chd::read::ChdReader;
use chd::write::ChdWriteOptions;
use std::io::{Seek, SeekFrom};
use std::path::Path;

/// The preset chdman uses by default for a CHD with the given metadata and unit size.
fn preset(metadata: &[Metadata], unit_bytes: u32) -> ChdWriteOptions {
    let tags: Vec<u32> = metadata.iter().map(|entry| entry.metatag).collect();
    let has = |tag: u32| tags.contains(&tag);
    let cd = tags
        .iter()
        .any(|&tag| KnownMetadata::is_cdrom(tag) || tag == SESSION_METADATA_TAG);
    if has(KnownMetadata::HardDisk as u32) || has(DVD_METADATA_TAG) {
        ChdWriteOptions::raw(unit_bytes)
    } else if has(KnownMetadata::AudioVideo as u32) {
        ChdWriteOptions::laserdisc(unit_bytes)
    } else if cd && unit_bytes == FRAME_BYTES {
        ChdWriteOptions::cd()
    } else {
        ChdWriteOptions::raw(unit_bytes)
    }
}

/// Clone the metadata of a CHD, upgrading legacy CD-ROM metadata to `CHT2`.
fn clone_metadata(metadata: Vec<Metadata>) -> Result<Vec<NewMetadata>> {
    if metadata
        .iter()
        .any(|entry| entry.metatag == KnownMetadata::GdRomOld as u32)
    {
        return Err(Error::Unsupported(
            "Copying legacy GD-ROM CHDs is not supported",
        ));
    }
    let legacy_cd = |entry: &Metadata| {
        entry.metatag == KnownMetadata::CdRomOld as u32
            || entry.metatag == KnownMetadata::CdRomTrack as u32
    };
    let upgraded = if metadata.iter().any(legacy_cd) {
        Toc::from_metadata(&metadata)?.metadata()
    } else {
        Vec::new()
    };
    Ok(metadata
        .into_iter()
        .filter(|entry| !legacy_cd(entry))
        .map(NewMetadata::from)
        .chain(
            upgraded
                .iter()
                .map(|(tag, text)| NewMetadata::text(*tag, text)),
        )
        .collect())
}

/// Copy the data and metadata of a CHD to a new CHD, such as to recompress it.
///
/// Legacy CD-ROM metadata is upgraded to `CHT2` metadata. Legacy GD-ROM CHDs are not
/// supported.
#[allow(clippy::too_many_arguments)]
pub fn copy(
    input: &Path,
    input_parent: Option<&Path>,
    output: &Path,
    output_parent: Option<&Path>,
    span: InputSpan,
    options: &WriteOptions,
    force: bool,
    progress: Progress,
) -> Result<()> {
    let mut input = open_chd(input, input_parent)?;
    let header = input.header().clone();
    let metadata: Vec<Metadata> = input.metadata_refs().try_into()?;

    // The hunk size of the input is the default.
    let preset = preset(&metadata, header.unit_bytes()).hunk_bytes(header.hunk_size());
    let chd = NewChd::new(options, preset, open_output(output, output_parent, force)?)?;
    let (hunk_bytes, input_hunk_bytes) = (chd.hunk_bytes(), header.hunk_size());
    if hunk_bytes % input_hunk_bytes != 0 && input_hunk_bytes % hunk_bytes != 0 {
        return Err(Error::invalid(
            "Hunk size is not a whole multiple or factor of input hunk size",
        ));
    }

    let range = span.resolve(header.logical_bytes(), input_hunk_bytes)?;
    let length = range.end - range.start;
    let mut reader = ChdReader::new(input);
    reader.seek(SeekFrom::Start(range.start))?;
    chd.write(output, clone_metadata(metadata)?, reader, length, progress)
}
