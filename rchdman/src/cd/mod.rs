//! The table of contents of CD-ROM images, and its metadata in CHD files.
//!
//! A CD-ROM CHD stores each frame of every track as 2448 bytes: the data of the sector,
//! followed by subcode if the track has any, padded with zeroes. Each track is padded to a
//! multiple of 4 frames. The tracks are described by `CHT2` metadata.
use crate::common::make_tag;
use crate::error::{Error, Result};
use chd::metadata::{KnownMetadata, Metadata};

mod cue;
mod extract;
mod image;
mod source;
mod track;

pub(crate) use cue::TrackOutput;
pub use extract::{CdExtraction, ExtractCdOptions, Extracted};
pub use image::{CdImage, CdImageWriter};
pub use source::CueImage;
pub(crate) use source::TrackSource;
pub use track::{TrackReader, Tracks};

/// The size of a frame in a CHD: a full sector and subcode.
pub const FRAME_BYTES: u32 = 2448;
/// The size of a full sector.
pub(crate) const MAX_SECTOR_BYTES: u32 = 2352;
/// The size of the subcode of a frame.
pub(crate) const SUBCODE_BYTES: u32 = 96;
/// The number of frames each track is padded to a multiple of.
pub(crate) const TRACK_PADDING: u32 = 4;
/// The most tracks a CD-ROM can have.
const MAX_TRACKS: usize = 99;

/// The tag of CD-ROM session metadata (`CHSE`).
pub(crate) const SESSION_METADATA_TAG: u32 = make_tag(b"CHSE");

/// The type of data in a track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackType {
    /// Mode 1, 2048 bytes per sector.
    Mode1,
    /// Mode 1, 2352 bytes per sector.
    Mode1Raw,
    /// Mode 2, 2336 bytes per sector.
    Mode2,
    /// Mode 2 form 1, 2048 bytes per sector.
    Mode2Form1,
    /// Mode 2 form 2, 2324 bytes per sector.
    Mode2Form2,
    /// Mode 2 with mixed forms, 2336 bytes per sector.
    Mode2FormMix,
    /// Mode 2, 2352 bytes per sector.
    Mode2Raw,
    /// Redbook audio, 2352 bytes per sector.
    Audio,
}

impl TrackType {
    /// The name of the track type in metadata, such as `MODE1_RAW`.
    pub fn name(self) -> &'static str {
        match self {
            TrackType::Mode1 => "MODE1",
            TrackType::Mode1Raw => "MODE1_RAW",
            TrackType::Mode2 => "MODE2",
            TrackType::Mode2Form1 => "MODE2_FORM1",
            TrackType::Mode2Form2 => "MODE2_FORM2",
            TrackType::Mode2FormMix => "MODE2_FORM_MIX",
            TrackType::Mode2Raw => "MODE2_RAW",
            TrackType::Audio => "AUDIO",
        }
    }

    /// Parse a track type from metadata or a cue sheet, such as `MODE1_RAW` or `MODE1/2352`,
    /// returning the type and the size of a sector.
    pub(crate) fn parse(name: &str) -> Option<(TrackType, u32)> {
        Some(match name {
            "MODE1" | "MODE1/2048" => (TrackType::Mode1, 2048),
            "MODE1_RAW" | "MODE1/2352" => (TrackType::Mode1Raw, 2352),
            "MODE2" | "MODE2/2336" => (TrackType::Mode2, 2336),
            "MODE2_FORM1" | "MODE2/2048" => (TrackType::Mode2Form1, 2048),
            "MODE2_FORM2" | "MODE2/2324" => (TrackType::Mode2Form2, 2324),
            "MODE2_FORM_MIX" => (TrackType::Mode2FormMix, 2336),
            "MODE2_RAW" | "MODE2/2352" | "CDI/2352" => (TrackType::Mode2Raw, 2352),
            "AUDIO" | "CDG" => (TrackType::Audio, 2352),
            _ => return None,
        })
    }

    /// The track type of its value in legacy `CHCD` metadata.
    fn from_legacy(value: u32) -> Option<TrackType> {
        Some(match value {
            0 => TrackType::Mode1,
            1 => TrackType::Mode1Raw,
            2 => TrackType::Mode2,
            3 => TrackType::Mode2Form1,
            4 => TrackType::Mode2Form2,
            5 => TrackType::Mode2FormMix,
            6 => TrackType::Mode2Raw,
            7 => TrackType::Audio,
            _ => return None,
        })
    }

    /// Whether the track is a mode 2 data track.
    pub(crate) fn is_mode2(self) -> bool {
        matches!(
            self,
            TrackType::Mode2
                | TrackType::Mode2Form1
                | TrackType::Mode2Form2
                | TrackType::Mode2FormMix
                | TrackType::Mode2Raw
        )
    }
}

/// The type of subcode stored with a track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subcode {
    /// Cooked subcode, 96 bytes per frame.
    Normal,
    /// Raw, uninterleaved subcode, 96 bytes per frame.
    Raw,
    /// No subcode.
    None,
}

impl Subcode {
    /// The name of the subcode type in metadata.
    pub fn name(self) -> &'static str {
        match self {
            Subcode::Normal => "RW",
            Subcode::Raw => "RW_RAW",
            Subcode::None => "NONE",
        }
    }

    /// Parse a subcode type, where anything unknown is no subcode.
    pub(crate) fn parse(name: &str) -> Subcode {
        match name {
            "RW" => Subcode::Normal,
            "RW_RAW" => Subcode::Raw,
            _ => Subcode::None,
        }
    }

    /// The subcode type of its value in legacy `CHCD` metadata.
    fn from_legacy(value: u32) -> Subcode {
        match value {
            0 => Subcode::Normal,
            1 => Subcode::Raw,
            _ => Subcode::None,
        }
    }

    /// The size of the subcode of a frame.
    pub fn bytes(self) -> u32 {
        match self {
            Subcode::None => 0,
            _ => SUBCODE_BYTES,
        }
    }
}

/// A track of a CD-ROM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    /// The type of data in the track.
    pub track_type: TrackType,
    /// The size of the data of each sector.
    pub data_bytes: u32,
    /// The type of subcode stored with the track.
    pub subcode: Subcode,
    /// The number of frames stored, including the pregap if it is stored.
    pub frames: u32,
    /// The number of frames at the end of the track that are padding, which are stored as zeroes.
    pub pad_frames: u32,
    /// The number of frames of pregap.
    pub pregap: u32,
    /// The type of data in the pregap.
    pub pregap_type: TrackType,
    /// The size of the data of each sector of the pregap, or 0 if the pregap is not stored.
    pub pregap_data_bytes: u32,
    /// The type of subcode of the pregap.
    pub pregap_subcode: Subcode,
    /// The number of frames of postgap, which is not stored.
    pub postgap: u32,
    /// The session of the track, from 0.
    pub session: u32,
}

impl Track {
    /// A track of the given type with no gaps, in the first session.
    pub fn new(track_type: TrackType, data_bytes: u32, frames: u32) -> Self {
        Track {
            track_type,
            data_bytes,
            subcode: Subcode::None,
            frames,
            pad_frames: 0,
            pregap: 0,
            pregap_type: TrackType::Mode1,
            pregap_data_bytes: 0,
            pregap_subcode: Subcode::None,
            postgap: 0,
            session: 0,
        }
    }

    /// The size of a frame in the track's source, the data and subcode.
    pub fn frame_bytes(&self) -> u32 {
        self.data_bytes + self.subcode.bytes()
    }

    /// The number of frames of zeroes after the track in a CHD, to pad it to a multiple of 4.
    pub fn extra_frames(&self) -> u32 {
        self.frames.next_multiple_of(TRACK_PADDING) - self.frames
    }

    /// Parse `CHT2` or legacy `CHTR` metadata of the track with the given number.
    pub(crate) fn from_metadata(text: &str, number: usize) -> Result<Track> {
        let invalid = || Error::InvalidMetadata(format!("Invalid CD-ROM track metadata '{text}'"));
        let field = |key: &str| {
            text.split_ascii_whitespace()
                .find_map(|field| field.strip_prefix(key)?.strip_prefix(':'))
        };
        let number_field =
            |key: &str| field(key).map(|value| value.parse::<u32>().map_err(|_| invalid()));

        if number_field("TRACK").transpose()? != Some(number as u32) {
            return Err(Error::InvalidMetadata(format!(
                "Unexpected CD-ROM track in metadata '{text}'"
            )));
        }
        let (track_type, data_bytes) = field("TYPE")
            .and_then(TrackType::parse)
            .ok_or_else(invalid)?;
        let frames = number_field("FRAMES").ok_or_else(invalid)??;
        let mut track = Track::new(track_type, data_bytes, frames);
        track.subcode = Subcode::parse(field("SUBTYPE").unwrap_or("NONE"));
        track.pregap = number_field("PREGAP").transpose()?.unwrap_or(0);
        track.postgap = number_field("POSTGAP").transpose()?.unwrap_or(0);
        if track.pregap > 0 {
            // Without a `V`, the pregap is not stored and its type is unused.
            if let Some((pregap_type, data_bytes)) = field("PGTYPE")
                .and_then(|pgtype| pgtype.strip_prefix('V'))
                .and_then(TrackType::parse)
            {
                track.pregap_type = pregap_type;
                track.pregap_data_bytes = data_bytes;
            }
            track.pregap_subcode = Subcode::parse(field("PGSUB").unwrap_or("NONE"));
        }
        Ok(track)
    }

    /// The `CHT2` metadata of the track, without the terminating NUL.
    pub fn metadata(&self, number: usize) -> String {
        // A `V` marks that the pregap is stored.
        let pregap_type = match self.pregap_data_bytes {
            0 => self.pregap_type.name().to_owned(),
            _ => format!("V{}", self.pregap_type.name()),
        };
        format!(
            "TRACK:{number} TYPE:{} SUBTYPE:{} FRAMES:{} PREGAP:{} PGTYPE:{pregap_type} PGSUB:{} POSTGAP:{}",
            self.track_type.name(),
            self.subcode.name(),
            self.frames,
            self.pregap,
            self.pregap_subcode.name(),
            self.postgap,
        )
    }
}

/// The table of contents of a CD-ROM.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Toc {
    /// The tracks, in order.
    pub tracks: Vec<Track>,
}

impl Toc {
    /// The number of sessions.
    pub fn sessions(&self) -> u32 {
        self.tracks
            .iter()
            .map(|track| track.session + 1)
            .max()
            .unwrap_or(1)
    }

    /// The number of frames of each track in a CHD, including padding.
    pub fn chd_frames(&self) -> impl Iterator<Item = u32> + '_ {
        self.tracks
            .iter()
            .map(|track| track.frames + track.extra_frames())
    }

    /// The size of the data of a CHD of the CD-ROM.
    pub fn logical_bytes(&self) -> u64 {
        self.chd_frames().map(u64::from).sum::<u64>() * FRAME_BYTES as u64
    }

    /// The metadata entries of the CD-ROM, as tags and values without the terminating NUL,
    /// in the order chdman writes them.
    pub fn metadata(&self) -> Vec<(u32, String)> {
        let mut entries = Vec::new();
        let mut session = None;
        for (index, track) in self.tracks.iter().enumerate() {
            if self.sessions() > 1 && session != Some(track.session) {
                entries.push((
                    SESSION_METADATA_TAG,
                    format!("SESSION:{}", track.session + 1),
                ));
                session = Some(track.session);
            }
            entries.push((KnownMetadata::CdRomTrack2 as u32, track.metadata(index + 1)));
        }
        entries
    }

    /// Read the table of contents from the metadata of a CD-ROM CHD.
    ///
    /// Supports `CHT2` metadata and the legacy `CHTR` and `CHCD` metadata. GD-ROMs are not
    /// supported.
    pub fn from_metadata(metadata: &[Metadata]) -> Result<Toc> {
        let mut toc = Toc::default();
        let mut session = 0;
        for entry in metadata {
            let text = || {
                String::from_utf8_lossy(&entry.value)
                    .trim_end_matches('\0')
                    .to_owned()
            };
            match entry.metatag {
                tag if tag == SESSION_METADATA_TAG => {
                    let text = text();
                    session = text
                        .strip_prefix("SESSION:")
                        .and_then(|session| session.trim().parse::<u32>().ok())
                        .and_then(|session| session.checked_sub(1))
                        .ok_or_else(|| {
                            Error::InvalidMetadata(format!("Invalid session metadata '{text}'"))
                        })?;
                }
                tag if tag == KnownMetadata::CdRomTrack2 as u32
                    || tag == KnownMetadata::CdRomTrack as u32 =>
                {
                    let mut track = Track::from_metadata(&text(), toc.tracks.len() + 1)?;
                    track.session = session;
                    toc.tracks.push(track);
                }
                tag if tag == KnownMetadata::GdRomTrack as u32
                    || tag == KnownMetadata::GdRomOld as u32 =>
                {
                    return Err(Error::Unsupported("GD-ROM CHDs are not supported"));
                }
                _ => {}
            }
        }
        if toc.tracks.is_empty() {
            if let Some(old) = metadata
                .iter()
                .find(|entry| entry.metatag == KnownMetadata::CdRomOld as u32)
            {
                toc = Toc::from_legacy_metadata(&old.value)?;
            }
        }
        if toc.tracks.is_empty() {
            return Err(Error::InvalidMetadata(
                "Unable to find CD-ROM metadata in the CHD".into(),
            ));
        }
        Ok(toc)
    }

    /// Parse the binary `CHCD` metadata of old CHDs: the number of tracks, then six words
    /// for each track, in either byte order.
    fn from_legacy_metadata(value: &[u8]) -> Result<Toc> {
        let invalid = || Error::InvalidMetadata("Invalid legacy CD-ROM metadata".into());
        let words: Vec<[u8; 4]> = value
            .chunks_exact(4)
            .map(|word| word.try_into().unwrap())
            .collect();
        let (&count, words) = words.split_first().ok_or_else(invalid)?;
        let word: fn([u8; 4]) -> u32 = if u32::from_le_bytes(count) as usize > MAX_TRACKS {
            u32::from_be_bytes
        } else {
            u32::from_le_bytes
        };
        let count = word(count) as usize;
        if count > MAX_TRACKS || words.len() < count * 6 {
            return Err(invalid());
        }

        let tracks = words[..count * 6]
            .chunks_exact(6)
            .map(|fields| {
                let [track_type, subcode, data_bytes, _subcode_bytes, frames, _extra_frames] =
                    std::array::from_fn(|index| word(fields[index]));
                let track_type = TrackType::from_legacy(track_type).ok_or_else(invalid)?;
                let mut track = Track::new(track_type, data_bytes, frames);
                track.subcode = Subcode::from_legacy(subcode);
                Ok(track)
            })
            .collect::<Result<_>>()?;
        Ok(Toc { tracks })
    }
}

/// Format a number of frames as `MM:SS:FF`.
pub(crate) fn msf(frames: u32) -> String {
    let (minutes, seconds, frames) = (frames / (75 * 60), (frames / 75) % 60, frames % 75);
    format!("{minutes:02}:{seconds:02}:{frames:02}")
}

#[cfg(test)]
mod tests {
    use super::{msf, Toc, Track, TrackType};
    use chd::metadata::{KnownMetadata, Metadata};

    fn metadata(tag: u32, value: impl Into<Vec<u8>>) -> Metadata {
        let value = value.into();
        Metadata {
            metatag: tag,
            length: value.len() as u32,
            value,
            flags: 1,
            index: 0,
        }
    }

    #[test]
    fn track_metadata_roundtrip() {
        for text in [
            "TRACK:1 TYPE:MODE2_RAW SUBTYPE:NONE FRAMES:36584 PREGAP:0 PGTYPE:MODE1 PGSUB:NONE POSTGAP:0",
            "TRACK:2 TYPE:AUDIO SUBTYPE:NONE FRAMES:26475 PREGAP:150 PGTYPE:VAUDIO PGSUB:NONE POSTGAP:0",
            "TRACK:3 TYPE:MODE1 SUBTYPE:RW FRAMES:100 PREGAP:150 PGTYPE:MODE1 PGSUB:NONE POSTGAP:150",
        ] {
            let number = text[6..7].parse().unwrap();
            assert_eq!(
                Track::from_metadata(text, number).unwrap().metadata(number),
                text
            );
        }
    }

    #[test]
    fn sessions() {
        let first = Track::new(TrackType::Mode1, 2048, 10);
        let mut second = Track::new(TrackType::Audio, 2352, 20);
        second.session = 1;
        let toc = Toc {
            tracks: vec![first, second],
        };
        let entries: Vec<Metadata> = toc
            .metadata()
            .iter()
            .map(|(tag, text)| metadata(*tag, format!("{text}\0")))
            .collect();
        assert_eq!(entries.len(), 4);
        assert_eq!(Toc::from_metadata(&entries).unwrap(), toc);
        assert_eq!(toc.logical_bytes(), (12 + 20) * 2448);
    }

    #[test]
    fn legacy_track_metadata() {
        let toc = Toc::from_metadata(&[metadata(
            KnownMetadata::CdRomTrack as u32,
            "TRACK:1 TYPE:MODE1 SUBTYPE:NONE FRAMES:1000\0",
        )])
        .unwrap();
        assert_eq!(
            toc.tracks[0].metadata(1),
            "TRACK:1 TYPE:MODE1 SUBTYPE:NONE FRAMES:1000 PREGAP:0 PGTYPE:MODE1 PGSUB:NONE POSTGAP:0"
        );
    }

    #[test]
    fn legacy_binary_metadata() {
        // One audio track of 100 frames, in either byte order.
        for to_bytes in [u32::to_le_bytes, u32::to_be_bytes] {
            let value: Vec<u8> = [1, 7, 2, 2352, 0, 100, 0]
                .into_iter()
                .flat_map(to_bytes)
                .collect();
            let toc =
                Toc::from_metadata(&[metadata(KnownMetadata::CdRomOld as u32, value)]).unwrap();
            assert_eq!(toc.tracks, [Track::new(TrackType::Audio, 2352, 100)]);
        }
    }

    #[test]
    fn msf_format() {
        assert_eq!(msf(150), "00:02:00");
        assert_eq!(msf(75 * 60 * 3 + 75 * 4 + 5), "03:04:05");
    }
}
