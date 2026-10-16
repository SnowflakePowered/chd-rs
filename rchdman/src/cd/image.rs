//! CD-ROM CHDs.
use super::{Toc, Track, TrackReader, Tracks, FRAME_BYTES};
use crate::common::{NewMetadata, NoParent, ParallelWriter, WriteOptions};
use crate::error::{Error, Result};
use chd::metadata::Metadata;
use chd::read::ChdReader;
use chd::write::ChdWriteOptions;
use chd::Chd;
use std::io::{self, Read, Seek, SeekFrom, Write};

/// A CD-ROM in a CHD.
///
/// ```no_run
/// use chd::Chd;
/// use rchdman::cd::CdImage;
/// use std::fs::File;
/// use std::io::{self, BufReader};
///
/// # fn main() -> rchdman::Result<()> {
/// let chd = Chd::open(BufReader::new(File::open("game.chd")?), None)?;
/// let mut cd = CdImage::new(chd)?;
/// println!("{} tracks", cd.toc().tracks.len());
/// io::copy(&mut cd.track(0)?, &mut File::create("track1.bin")?)?;
/// # Ok(())
/// # }
/// ```
pub struct CdImage<F: Read + Seek> {
    pub(super) reader: ChdReader<F>,
    pub(super) toc: Toc,
}

impl<F: Read + Seek> CdImage<F> {
    /// The CD-ROM in a CHD, reading its table of contents from its metadata.
    ///
    /// See [`Toc::from_metadata`] for the supported metadata.
    pub fn new(mut chd: Chd<F>) -> Result<Self> {
        let metadata: Vec<Metadata> = chd.metadata_refs().try_into()?;
        let toc = Toc::from_metadata(&metadata)?;
        Ok(CdImage {
            reader: ChdReader::new(chd),
            toc,
        })
    }

    /// The table of contents.
    pub fn toc(&self) -> &Toc {
        &self.toc
    }

    /// The tracks in order. See [`Tracks`].
    pub fn tracks(&mut self) -> Tracks<&mut CdImage<F>> {
        let count = self.toc.tracks.len();
        Tracks::new(self, count)
    }

    /// A reader of the data of a track as the CHD stores it: the data and subcode of each
    /// frame, with audio big-endian. Pad frames are not included.
    pub fn track(&mut self, index: usize) -> Result<TrackReader<&mut ChdReader<F>>> {
        let track = self
            .toc
            .tracks
            .get(index)
            .ok_or_else(|| Error::invalid(format!("There is no track {}", index + 1)))?;
        let start: u64 = self.toc.chd_frames().take(index).map(u64::from).sum();
        self.reader
            .seek(SeekFrom::Start(start * FRAME_BYTES as u64))?;
        Ok(TrackReader::new(
            &mut self.reader,
            track,
            FRAME_BYTES as usize,
            false,
        ))
    }
}

/// Writes a CD-ROM to a CHD.
///
/// The table of contents is given up front, so its metadata is written before the data.
/// Each track is started with [`start_track`](CdImageWriter::start_track), then its
/// data is written as a CHD stores it: the data and subcode of each frame, with audio
/// big-endian. Frames of a track that are not written are zeroes.
///
/// ```no_run
/// use rchdman::cd::{CdImageWriter, CueImage};
/// use rchdman::WriteOptions;
/// use std::fs::File;
/// use std::io::{self, BufWriter};
///
/// # fn main() -> rchdman::Result<()> {
/// let source = CueImage::open("game.cue".as_ref())?;
/// let file = BufWriter::new(File::create("game.chd")?);
/// let mut cd = CdImageWriter::new(file, &source.toc, &WriteOptions::default())?;
/// for index in 0..source.toc.tracks.len() {
///     cd.start_track()?;
///     io::copy(&mut source.track(index)?, &mut cd)?;
/// }
/// cd.finish()?;
/// # Ok(())
/// # }
/// ```
pub struct CdImageWriter<W: Write + Seek> {
    inner: ParallelWriter<W>,
    toc: Toc,
    /// The current track, if one was started.
    track: Option<usize>,
    /// The number of whole frames written to the current track.
    frames: u32,
    /// The current frame, and the position in its data.
    frame: [u8; FRAME_BYTES as usize],
    position: usize,
}

impl<W: Write + Seek> CdImageWriter<W> {
    /// A writer of a CD-ROM CHD with the given table of contents.
    pub fn new(inner: W, toc: &Toc, options: &WriteOptions) -> Result<Self> {
        Self::create(inner, toc, options, None::<&mut NoParent>)
    }

    /// A writer of a CD-ROM CHD that refers to data in a parent CHD.
    pub fn with_parent<F: Read + Seek>(
        inner: W,
        toc: &Toc,
        options: &WriteOptions,
        parent: &mut Chd<F>,
    ) -> Result<Self> {
        Self::create(inner, toc, options, Some(parent))
    }

    fn create<F: Read + Seek>(
        inner: W,
        toc: &Toc,
        options: &WriteOptions,
        parent: Option<&mut Chd<F>>,
    ) -> Result<Self> {
        let preset = options.resolve(
            ChdWriteOptions::cd().logical_bytes(toc.logical_bytes()),
            parent.as_ref().map(|parent| parent.header()),
        )?;
        let metadata = toc
            .metadata()
            .iter()
            .map(|(tag, text)| NewMetadata::text(*tag, text))
            .collect();
        Ok(CdImageWriter {
            inner: options.create(inner, preset, parent, metadata)?,
            toc: toc.clone(),
            track: None,
            frames: 0,
            frame: [0; FRAME_BYTES as usize],
            position: 0,
        })
    }

    /// Finish the current track and start the next track of the table of contents.
    ///
    /// Returns [`InvalidOption`](Error::InvalidOption) if every track was already started.
    pub fn start_track(&mut self) -> Result<&Track> {
        let next = self.track.map_or(0, |track| track + 1);
        if next >= self.toc.tracks.len() {
            return Err(Error::invalid(
                "Every track of the CD-ROM was already written",
            ));
        }
        self.finish_track()?;
        self.track = Some(next);
        self.frames = 0;
        Ok(&self.toc.tracks[next])
    }

    /// Finish the CHD, writing zeroes for any frames and tracks that were not written.
    pub fn finish(mut self) -> Result<W> {
        while self.track.map_or(0, |track| track + 1) < self.toc.tracks.len() {
            self.start_track()?;
        }
        self.finish_track()?;
        self.inner.finish()
    }

    /// Write the rest of the current track as zeroes, and pad it to a multiple of 4 frames.
    fn finish_track(&mut self) -> io::Result<()> {
        let Some(track) = self.track.map(|track| &self.toc.tracks[track]) else {
            return Ok(());
        };
        let total = track.frames + track.extra_frames();
        if self.position > 0 {
            self.write_frame()?;
        }
        while self.frames < total {
            self.write_frame()?;
        }
        Ok(())
    }

    /// Write the current frame to the CHD, padded with zeroes.
    fn write_frame(&mut self) -> io::Result<()> {
        self.frame[self.position..].fill(0);
        self.inner.write_all(&self.frame)?;
        self.frames += 1;
        self.position = 0;
        Ok(())
    }
}

impl<W: Write + Seek> Write for CdImageWriter<W> {
    /// Writes the data of the current track, up to the number of frames in the track.
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let Some(track) = self.track.map(|track| &self.toc.tracks[track]) else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "start_track must be called before writing a track",
            ));
        };
        let frame_bytes = track.frame_bytes() as usize;
        if buf.is_empty() || self.frames >= track.frames {
            return Ok(0);
        }
        let count = buf.len().min(frame_bytes - self.position);
        self.frame[self.position..self.position + count].copy_from_slice(&buf[..count]);
        self.position += count;
        if self.position == frame_bytes {
            self.write_frame()?;
        }
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{CdImage, CdImageWriter};
    use crate::cd::{Toc, Track, TrackType};
    use crate::WriteOptions;
    use chd::iter::FallibleLender;
    use chd::Chd;
    use std::io::{Cursor, Read, Write};

    #[test]
    fn write_and_read_tracks() {
        let toc = Toc {
            tracks: vec![
                Track::new(TrackType::Mode1, 2048, 5),
                Track::new(TrackType::Audio, 2352, 3),
            ],
        };
        let data: Vec<Vec<u8>> = toc
            .tracks
            .iter()
            .enumerate()
            .map(|(index, track)| {
                let bytes = (track.frames * track.frame_bytes()) as usize;
                (0..bytes).map(|byte| (byte * 7 + index) as u8).collect()
            })
            .collect();

        let mut writer =
            CdImageWriter::new(Cursor::new(Vec::new()), &toc, &WriteOptions::default()).unwrap();
        for track in &data {
            writer.start_track().unwrap();
            writer.write_all(track).unwrap();
        }
        let chd = writer.finish().unwrap().into_inner();

        let mut image = CdImage::new(Chd::open(Cursor::new(chd), None).unwrap()).unwrap();
        assert_eq!(image.toc(), &toc);
        let mut tracks = image.tracks();
        let mut read = Vec::new();
        while let Some(mut track) = tracks.next().unwrap() {
            let mut bytes = Vec::new();
            track.read_to_end(&mut bytes).unwrap();
            read.push(bytes);
        }
        assert!(read == data);

        let mut second = Vec::new();
        image.track(1).unwrap().read_to_end(&mut second).unwrap();
        assert!(second == data[1]);
        assert!(image.track(2).is_err());
    }
}
