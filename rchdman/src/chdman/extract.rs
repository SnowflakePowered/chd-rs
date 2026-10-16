//! `extractraw`, `extracthd`, `extractdvd`, and `extractcd`.
use crate::cd::{CdImage, ExtractCdOptions};
use crate::common::{
    check_output, copy_exact, open_chd, InputSpan, Outputs, Progress, ProgressReader,
};
use crate::error::Result;
use chd::iter::Lender;
use chd::read::ChdReader;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Extract the data of a CHD, or a span of it, to a file. The file is removed if
/// extracting fails.
pub fn extract_raw(
    input: &Path,
    parent: Option<&Path>,
    output: &Path,
    span: InputSpan,
    force: bool,
    progress: Progress,
) -> Result<()> {
    let chd = open_chd(input, parent)?;
    let header = chd.header();
    let range = span.resolve(header.logical_bytes(), header.hunk_size())?;
    check_output(output, force)?;
    let mut reader = ChdReader::new(chd);
    reader.seek(SeekFrom::Start(range.start))?;
    let length = range.end - range.start;

    let mut outputs = Outputs::default();
    let mut file = BufWriter::new(outputs.create(output)?);
    copy_exact(
        ProgressReader::new(reader, length, progress),
        &mut file,
        length,
    )?;
    file.flush()?;
    outputs.finish();
    Ok(())
}

/// The files written by [`extract_cd`].
#[derive(Debug, Clone, Default)]
pub struct ExtractedCd {
    /// The bin files.
    pub bin_files: Vec<PathBuf>,
    /// Warnings about data that is left out.
    pub warnings: Vec<String>,
}

/// Extract a CD-ROM CHD to a cue sheet (`.cue`) or cdrdao table of contents, and bin files.
/// See [`CdImage::extract`]. The files are removed if extracting fails.
pub fn extract_cd(
    input: &Path,
    parent: Option<&Path>,
    output: &Path,
    options: &ExtractCdOptions,
    force: bool,
    progress: Progress,
) -> Result<ExtractedCd> {
    let mut image = CdImage::new(open_chd(input, parent)?)?;
    check_output(output, force)?;
    let mut files = image.extract(output, options)?;
    let extracted = ExtractedCd {
        bin_files: files.bin_files().map(Path::to_owned).collect(),
        warnings: files.warnings(),
    };
    for bin in &extracted.bin_files {
        check_output(bin, force)?;
    }

    let total = files.size();
    let mut done = 0;
    let mut outputs = Outputs::default();
    while let Some(file) = files.next() {
        let mut writer = BufWriter::new(outputs.create(file.name)?);
        let mut file_progress = |read, _| progress(done + read, total);
        let data = ProgressReader::new(file.data, file.size, &mut file_progress);
        copy_exact(data, &mut writer, file.size)?;
        writer.flush()?;
        done += file.size;
    }
    outputs.finish();
    Ok(extracted)
}
