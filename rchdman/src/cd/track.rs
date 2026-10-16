//! Reading the data of CD-ROM tracks.
use super::{CdImage, CueImage, Track, FRAME_BYTES, MAX_SECTOR_BYTES};
use crate::error::{Error, Result};
use chd::read::ChdReader;
use lender::{check_covariance_fallible, FallibleLender, FallibleLending};
use std::fs::File;
use std::io::{self, BufReader, Read, Seek};

/// Reads the data of a track as a CHD stores it: the data and subcode of each frame, with
/// audio big-endian. Pad frames are not included.
///
/// The tracks of a [`CdImage`] read from the CHD, and the tracks of a [`CueImage`] read
/// from their files.
pub struct TrackReader<R> {
    inner: R,
    /// The number of frames left to read.
    frames: u32,
    /// The size of the data and subcode of each frame.
    frame_bytes: usize,
    /// The size of each frame in the input, which may be padded.
    stride: usize,
    /// The number of bytes at the start of each frame to swap.
    swap_bytes: usize,
    /// The current frame, and the position in its data.
    frame: [u8; FRAME_BYTES as usize],
    position: usize,
}

impl<R: Read> TrackReader<R> {
    /// A reader of the track from its first frame in the input, where each frame takes
    /// `stride` bytes. If `swap` is set, the first 2352 bytes of each frame are byte swapped.
    pub(crate) fn new(inner: R, track: &Track, stride: usize, swap: bool) -> Self {
        let frame_bytes = track.frame_bytes() as usize;
        TrackReader {
            inner,
            frames: track.frames.saturating_sub(track.pad_frames),
            frame_bytes,
            stride,
            swap_bytes: if swap {
                frame_bytes.min(MAX_SECTOR_BYTES as usize)
            } else {
                0
            },
            frame: [0; FRAME_BYTES as usize],
            position: frame_bytes,
        }
    }

    /// Leave out the subcode of each frame, reading only its data.
    pub(crate) fn without_subcode(mut self, track: &Track) -> Self {
        self.frame_bytes = track.data_bytes as usize;
        self.position = self.frame_bytes;
        self.swap_bytes = self.swap_bytes.min(self.frame_bytes);
        self
    }

    /// The input, at the end of the last frame read.
    pub(crate) fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: Read> Read for TrackReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.position >= self.frame_bytes {
            if self.frames == 0 {
                return Ok(0);
            }
            self.inner.read_exact(&mut self.frame[..self.stride])?;
            for pair in self.frame[..self.swap_bytes].chunks_exact_mut(2) {
                pair.swap(0, 1);
            }
            self.frames -= 1;
            self.position = 0;
        }
        let count = buf.len().min(self.frame_bytes - self.position);
        buf[..count].copy_from_slice(&self.frame[self.position..self.position + count]);
        self.position += count;
        Ok(count)
    }
}

/// The tracks of a [`CdImage`] or a [`CueImage`] in order, as a [`FallibleLender`].
///
/// Each track is a [`TrackReader`], like [`CdImage::track`] and [`CueImage::track`]. The
/// readers of a `CdImage` borrow its CHD, so each must be dropped before the next.
///
/// ```no_run
/// use chd::iter::FallibleLender;
/// use rchdman::cd::CueImage;
/// use std::io;
///
/// # fn main() -> rchdman::Result<()> {
/// let image = CueImage::open("game.cue".as_ref())?;
/// let mut tracks = image.tracks();
/// while let Some(mut track) = tracks.next()? {
///     io::copy(&mut track, &mut io::sink())?;
/// }
/// # Ok(())
/// # }
/// ```
pub struct Tracks<I> {
    image: I,
    next: usize,
    count: usize,
}

impl<I> Tracks<I> {
    pub(crate) fn new(image: I, count: usize) -> Self {
        Tracks {
            image,
            next: 0,
            count,
        }
    }

    /// The index of the next track.
    fn advance(&mut self) -> Option<usize> {
        let index = self.next;
        (index < self.count).then(|| {
            self.next += 1;
            index
        })
    }
}

impl<'lend, F: Read + Seek> FallibleLending<'lend> for Tracks<&mut CdImage<F>> {
    type Lend = TrackReader<&'lend mut ChdReader<F>>;
}

impl<F: Read + Seek> FallibleLender for Tracks<&mut CdImage<F>> {
    type Error = Error;
    check_covariance_fallible!();

    fn next(&mut self) -> Result<Option<TrackReader<&'_ mut ChdReader<F>>>> {
        self.advance()
            .map(|index| self.image.track(index))
            .transpose()
    }
}

impl<'lend> FallibleLending<'lend> for Tracks<&CueImage> {
    type Lend = TrackReader<BufReader<File>>;
}

impl FallibleLender for Tracks<&CueImage> {
    type Error = Error;
    check_covariance_fallible!();

    fn next(&mut self) -> Result<Option<TrackReader<BufReader<File>>>> {
        self.advance()
            .map(|index| self.image.track(index))
            .transpose()
    }
}
