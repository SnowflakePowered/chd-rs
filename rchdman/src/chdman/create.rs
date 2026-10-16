//! `createraw`, `createhd`, `createdvd`, and `createcd`.
use crate::cd::{CdImageWriter, CueImage, Toc};
use crate::common::{
    copy_exact, open_output, ChdFile, InputSpan, NewChd, NewMetadata, Outputs, Progress,
    ProgressReader, WriteOptions, DVD_METADATA_TAG,
};
use crate::error::{Error, Result};
use crate::hd::{Geometry, HardDiskWriter, IDE_SECTOR_SIZE};
use chd::metadata::{KnownMetadata, Metadata};
use chd::write::ChdWriteOptions;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

fn open_input(path: &Path, span: InputSpan, hunk_bytes: u32) -> Result<(BufReader<File>, u64)> {
    let mut file = File::open(path).map_err(Error::file(path))?;
    let range = span.resolve(file.metadata()?.len(), hunk_bytes)?;
    file.seek(SeekFrom::Start(range.start))?;
    Ok((BufReader::new(file), range.end - range.start))
}

fn parent_metadata(parent: &mut Option<ChdFile>) -> Result<Vec<Metadata>> {
    let metadata = parent
        .as_mut()
        .map(|parent| Vec::try_from(parent.metadata_refs()))
        .transpose()?;
    Ok(metadata.unwrap_or_default())
}

/// Create a CHD of a raw file.
///
/// The unit size is required unless there is a parent, which it must match.
#[allow(clippy::too_many_arguments)]
pub fn create_raw(
    input: &Path,
    output: &Path,
    parent: Option<&Path>,
    unit_bytes: Option<u32>,
    span: InputSpan,
    options: &WriteOptions,
    force: bool,
    progress: Progress,
) -> Result<()> {
    let mut parent = open_output(output, parent, force)?;
    let unit_bytes = match (unit_bytes, &parent) {
        (Some(unit_bytes), _) => unit_bytes,
        (None, Some(parent)) => parent.header().unit_bytes(),
        (None, None) => {
            return Err(Error::invalid(
                "Unit size must be specified if no output parent CHD is supplied",
            ))
        }
    };
    let metadata = parent_metadata(&mut parent)?;
    let chd = NewChd::new(options, ChdWriteOptions::raw(unit_bytes), parent)?;
    let (input, length) = open_input(input, span, chd.hunk_bytes())?;
    if length % unit_bytes as u64 != 0 {
        return Err(Error::NotDivisible {
            size: length,
            unit_bytes,
        });
    }
    let metadata = metadata.into_iter().map(NewMetadata::from).collect();
    chd.write(output, metadata, input, length, progress)
}

/// The cylinders, heads, and sectors of a hard disk.
pub type Chs = (u32, u32, u32);

/// How to determine the geometry of a hard disk.
///
/// The geometry of a template from [`TEMPLATES`](crate::hd::TEMPLATES) is given as its CHS
/// values and sector size.
#[derive(Debug, Clone, Default)]
pub struct HardDiskOptions {
    /// The cylinders, heads, and sectors.
    pub chs: Option<Chs>,
    /// The size of a sector, the unit size of the parent or 512 bytes by default.
    pub sector_bytes: Option<u32>,
    /// The size of a blank hard disk, without an input.
    pub size: Option<u64>,
    /// An IDE identify block to store with the hard disk, and take the geometry from.
    pub ident: Option<PathBuf>,
}

/// Create a hard disk CHD.
///
/// If no input is provided, a blank, uncompressed hard disk is created.
/// The geometry will be guessed from the size unless CHS values, an ident, or a parent, is provided.
#[allow(clippy::too_many_arguments)]
pub fn create_hd(
    input: Option<&Path>,
    output: &Path,
    parent: Option<&Path>,
    hd: &HardDiskOptions,
    span: InputSpan,
    options: &WriteOptions,
    force: bool,
    progress: Progress,
) -> Result<Geometry> {
    let mut parent = open_output(output, parent, force)?;
    if hd.chs.is_some() && parent.is_some() {
        return Err(Error::invalid(
            "CHS geometry cannot be specified when a parent CHD is supplied",
        ));
    }
    let mut chs = hd.chs;
    let mut sector_bytes = (hd.sector_bytes)
        .or(parent.as_ref().map(|parent| parent.header().unit_bytes()))
        .unwrap_or(IDE_SECTOR_SIZE);

    // Blank hard disks are uncompressed.
    let blank = input.is_none();
    let mut write = options.clone();
    if blank {
        if write.codecs().is_some_and(|codecs| !codecs.is_empty()) {
            return Err(Error::invalid(
                "Blank hard disk images must be uncompressed",
            ));
        }
        write = write.compression(&[]);
    }
    let hunk_bytes = write
        .resolve(
            ChdWriteOptions::raw(sector_bytes),
            parent.as_ref().map(|parent| parent.header()),
        )?
        .sizes()
        .0;

    let (input, size): (Box<dyn Read>, u64) = match input {
        Some(path) => {
            if hd.size.is_some() {
                return Err(Error::invalid(
                    "Size cannot be specified when an input file is supplied",
                ));
            }
            let (input, length) = open_input(path, span, hunk_bytes)?;
            (Box::new(input), length)
        }
        None => {
            if !span.is_whole() {
                return Err(Error::invalid(
                    "Input start/length cannot be specified when no input file is supplied",
                ));
            }
            (Box::new(io::empty()), hd.size.unwrap_or(0))
        }
    };

    // The ident of the parent is used unless there is another, and the geometry is taken
    // from the ident, or the parent, unless it was given.
    let parent_metadata = parent_metadata(&mut parent)?;
    let find = |tag: KnownMetadata| {
        parent_metadata
            .iter()
            .find(|entry| entry.metatag == tag as u32 && entry.index == 0)
    };
    let mut ident = find(KnownMetadata::HardDiskIdent).map(|entry| entry.value.clone());
    if let Some(path) = &hd.ident {
        let data = std::fs::read(path).map_err(Error::file(path))?;
        if let Some(geometry) = Geometry::from_ident(&data, sector_bytes)? {
            chs = Some((geometry.cylinders, geometry.heads, geometry.sectors));
        }
        ident = Some(data);
    }
    if chs.is_none() && parent.is_some() {
        let entry = find(KnownMetadata::HardDisk).ok_or_else(|| {
            Error::InvalidMetadata("Unable to find hard disk metadata in parent CHD".into())
        })?;
        let geometry = Geometry::from_metadata(&entry.value)?;
        sector_bytes = geometry.sector_bytes;
        chs = Some((geometry.cylinders, geometry.heads, geometry.sectors));
    }

    if size % sector_bytes as u64 != 0 {
        return Err(Error::NotDivisible {
            size,
            unit_bytes: sector_bytes,
        });
    }
    let geometry =
        match chs {
            Some((cylinders, heads, sectors)) => Geometry {
                cylinders,
                heads,
                sectors,
                sector_bytes,
            },
            None if blank && size == 0 => return Err(Error::invalid(
                "Length or CHS geometry must be specified when creating a blank hard disk image",
            )),
            None => Geometry::guess(size, sector_bytes)?,
        };

    let mut outputs = Outputs::default();
    let file = BufWriter::new(outputs.create(output)?);
    let ident = ident.as_deref();
    let mut disk = match &mut parent {
        Some(parent) => HardDiskWriter::with_parent(file, geometry, ident, &write, parent)?,
        None => HardDiskWriter::new(file, geometry, ident, &write)?,
    };
    // The input fills the start of the hard disk, and the rest is zeroes.
    let length = if blank {
        0
    } else {
        size.min(geometry.logical_bytes())
    };
    copy_exact(
        ProgressReader::new(input, length, progress),
        &mut disk,
        length,
    )?;
    disk.finish()?;
    outputs.finish();
    Ok(geometry)
}

/// The size of a DVD-ROM sector.
const DVD_SECTOR_BYTES: u32 = 2048;

/// Create a DVD-ROM CHD of an image of 2048 byte sectors.
pub fn create_dvd(
    input: &Path,
    output: &Path,
    parent: Option<&Path>,
    span: InputSpan,
    options: &WriteOptions,
    force: bool,
    progress: Progress,
) -> Result<()> {
    let chd = NewChd::new(
        options,
        ChdWriteOptions::dvd(),
        open_output(output, parent, force)?,
    )?;
    let (input, length) = open_input(input, span, chd.hunk_bytes())?;
    if length % DVD_SECTOR_BYTES as u64 != 0 {
        return Err(Error::NotDivisible {
            size: length,
            unit_bytes: DVD_SECTOR_BYTES,
        });
    }
    let metadata = vec![NewMetadata::text(DVD_METADATA_TAG, "")];
    chd.write(output, metadata, input, length, progress)
}

/// Create a CD-ROM CHD of a cue sheet or ISO image.
///
/// See [`CueImage::open`] for the supported formats.
pub fn create_cd(
    input: &Path,
    output: &Path,
    parent: Option<&Path>,
    options: &WriteOptions,
    force: bool,
    progress: Progress,
) -> Result<Toc> {
    let source = CueImage::open(input)?;
    let mut parent = open_output(output, parent, force)?;
    let mut outputs = Outputs::default();
    let file = BufWriter::new(outputs.create(output)?);
    let mut cd = match &mut parent {
        Some(parent) => CdImageWriter::with_parent(file, &source.toc, options, parent)?,
        None => CdImageWriter::new(file, &source.toc, options)?,
    };

    let lengths: Vec<u64> = (source.toc.tracks.iter())
        .map(|track| {
            track.frames.saturating_sub(track.pad_frames) as u64 * track.frame_bytes() as u64
        })
        .collect();
    let total = lengths.iter().sum();
    let mut done = 0;
    for (index, length) in lengths.into_iter().enumerate() {
        cd.start_track()?;
        let mut track_progress = |read, _| progress(done + read, total);
        let track = ProgressReader::new(source.track(index)?, length, &mut track_progress);
        copy_exact(track, &mut cd, length)?;
        done += length;
    }
    cd.finish()?;
    outputs.finish();
    Ok(source.toc)
}
