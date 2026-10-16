//! CD-ROM images made of tracks in files, such as a cue sheet and its bin files.
use super::{Toc, Track, TrackReader, TrackType, Tracks};
use crate::error::{Error, Result};
use std::fs::File;
use std::io::{BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Where the data of a track comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrackSource {
    /// The file of the track.
    pub path: PathBuf,
    /// The offset of the track in the file.
    pub offset: u64,
    /// Whether to swap the bytes of each sector, such as for little-endian audio.
    pub swap: bool,
}

/// A CD-ROM image in files: its table of contents, and the files its tracks are in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CueImage {
    /// The table of contents.
    pub toc: Toc,
    /// The source of each track.
    pub(crate) sources: Vec<TrackSource>,
}

impl CueImage {
    /// Open a CD-ROM image by its table of contents: a cue sheet (`.cue`), or a
    /// single track image (`.iso`, `.cdr`, or `.toast`).
    ///
    /// GD-ROM images (`.gdi`), Nero images (`.nrg`), cdrdao tables of contents, and cue
    /// sheets with `WAVE` files are not supported.
    pub fn open(path: &Path) -> Result<CueImage> {
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        match extension.as_str() {
            "cue" => CueImage::from_cue(path),
            "iso" | "cdr" | "toast" => CueImage::from_iso(path),
            "gdi" => Err(Error::Unsupported("GD-ROM images (.gdi) are not supported")),
            "nrg" => Err(Error::Unsupported("Nero images (.nrg) are not supported")),
            _ => Err(Error::Unsupported(
                "cdrdao tables of contents are not supported",
            )),
        }
    }

    /// A single track image, whose type is determined by its size.
    fn from_iso(path: &Path) -> Result<CueImage> {
        let size = std::fs::metadata(path).map_err(Error::file(path))?.len();
        let (track_type, data_bytes) = if size % 2048 == 0 {
            (TrackType::Mode1, 2048)
        } else if size % 2336 == 0 {
            (TrackType::Mode2, 2336)
        } else if size % 2352 == 0 {
            (TrackType::Mode2Raw, 2352)
        } else {
            return Err(Error::InvalidToc("Unrecognized track type".into()));
        };
        let frames = u32::try_from(size / data_bytes as u64)
            .map_err(|_| Error::InvalidToc("The image is too large".into()))?;
        Ok(CueImage {
            toc: Toc {
                tracks: vec![Track::new(track_type, data_bytes, frames)],
            },
            sources: vec![TrackSource {
                path: path.to_owned(),
                offset: 0,
                swap: false,
            }],
        })
    }

    /// The tracks in order. See [`Tracks`].
    pub fn tracks(&self) -> Tracks<&CueImage> {
        Tracks::new(self, self.toc.tracks.len())
    }

    /// A reader of the data of a track, from its file. See [`TrackReader`].
    pub fn track(&self, index: usize) -> Result<TrackReader<BufReader<File>>> {
        let (track, source) = self
            .toc
            .tracks
            .get(index)
            .zip(self.sources.get(index))
            .ok_or_else(|| Error::invalid(format!("There is no track {}", index + 1)))?;
        let mut file = File::open(&source.path).map_err(Error::file(&source.path))?;
        file.seek(SeekFrom::Start(source.offset))
            .map_err(Error::file(&source.path))?;
        let stride = track.frame_bytes() as usize;
        Ok(TrackReader::new(
            BufReader::new(file),
            track,
            stride,
            source.swap,
        ))
    }
}
