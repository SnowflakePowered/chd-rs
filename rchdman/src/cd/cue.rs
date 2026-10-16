//! Reading cue sheets, and writing cue sheets and cdrdao tables of contents.
use super::{msf, CueImage, Subcode, Toc, Track, TrackSource, TrackType};
use crate::error::{Error, Result};
use std::fmt::Write;
use std::io::Cursor;
use std::path::Path;
use std::time::Duration;

impl CueImage {
    /// Read a cue sheet, finding the lengths of the tracks from the sizes of their files.
    pub(super) fn from_cue(path: &Path) -> Result<CueImage> {
        let text = std::fs::read_to_string(path).map_err(Error::file(path))?;
        let directory = path.parent().unwrap_or(Path::new(""));
        CueSheet::parse(&text, directory)?.resolve(|path| {
            std::fs::metadata(path)
                .map(|metadata| metadata.len())
                .map_err(Error::file(path))
        })
    }
}

/// The tracks of a cue sheet, before the lengths of the tracks are known.
#[derive(Debug, Clone)]
struct CueSheet {
    tracks: Vec<CueTrack>,
    /// Whether the disc has more than one session.
    multisession: bool,
    /// Whether the tracks are in more than one file.
    multibin: bool,
}

/// A track of a cue sheet, before its length is known.
#[derive(Debug, Clone)]
struct CueTrack {
    track: Track,
    source: TrackSource,
    /// The frame of `INDEX 00` in the file, or of `INDEX 01` if there is no `INDEX 00`.
    index0: u32,
    /// The lead-out after the track, in frames, for multisession discs.
    lead_out: Option<u32>,
}

impl CueSheet {
    /// Parse the text of a cue sheet with files in `directory`.
    fn parse(text: &str, directory: &Path) -> Result<CueSheet> {
        let cue = rcue::parser::parse(&mut Cursor::new(text), false)
            .map_err(|error| Error::InvalidToc(format!("Error parsing cue sheet: {error}")))?;

        let mut state = CueState::default();
        for (key, value) in &cue.comments {
            state.remark(key, value, None)?;
        }
        let mut tracks: Vec<CueTrack> = Vec::new();
        for file in &cue.files {
            // Only the first track after `FILE` takes the byte order of the file.
            let mut swap = match file.format.to_ascii_uppercase().as_str() {
                "BINARY" => false,
                "MOTOROLA" => true,
                "WAVE" => {
                    return Err(Error::Unsupported(
                        "Cue sheets with WAVE files are not supported",
                    ))
                }
                format => return Err(Error::InvalidToc(format!("Unhandled track type {format}"))),
            };
            for (key, value) in &file.comments {
                state.remark(key, value, tracks.last_mut())?;
            }
            for cue_track in &file.tracks {
                if cue_track.no.parse::<usize>().ok() != Some(tracks.len() + 1) {
                    return Err(Error::InvalidToc(format!(
                        "Unexpected track number {}",
                        cue_track.no
                    )));
                }
                let source = TrackSource {
                    path: directory.join(&file.file),
                    offset: 0,
                    swap: std::mem::take(&mut swap),
                };
                tracks.push(CueTrack::new(
                    cue_track,
                    source,
                    state.track(&cue_track.format)?,
                )?);
                for (key, value) in &cue_track.comments {
                    state.remark(key, value, tracks.last_mut())?;
                }
            }
        }
        if state.gd_rom_areas == (true, true) {
            return Err(Error::Unsupported("GD-ROM cue sheets are not supported"));
        }
        if tracks.is_empty() {
            return Err(Error::InvalidToc("No tracks in cue sheet".into()));
        }

        let mut files: Vec<&str> = cue.files.iter().map(|file| file.file.as_str()).collect();
        files.dedup();
        Ok(CueSheet {
            tracks,
            multisession: state.multisession,
            multibin: files.len() > 1,
        })
    }

    /// Find the length and offset of each track from the sizes of the files.
    fn resolve(mut self, file_size: impl Fn(&Path) -> Result<u64>) -> Result<CueImage> {
        for index in 0..self.tracks.len() {
            // Audio is always swapped, since bin files store it little-endian.
            if self.tracks[index].track.track_type == TrackType::Audio {
                self.tracks[index].source.swap = true;
            }
            let (offset, frames) = self.track_extent(index, &file_size)?;
            self.tracks[index].source.offset = offset;
            self.tracks[index].track.frames = frames;
            if self.multisession && !self.multibin {
                self.apply_lead_outs(index);
            }
        }

        let (tracks, sources) = self
            .tracks
            .into_iter()
            .map(|track| (track.track, track.source))
            .unzip();
        Ok(CueImage {
            toc: Toc { tracks },
            sources,
        })
    }

    /// The offset of the track in its file, after the earlier tracks in the same file, and
    /// its length. A track that shares its file with the next track ends at the next track's
    /// `INDEX 00`. Otherwise, it ends at the end of its file.
    fn track_extent(
        &self,
        index: usize,
        file_size: impl Fn(&Path) -> Result<u64>,
    ) -> Result<(u64, u32)> {
        let track = &self.tracks[index];
        let same_file = |other: &CueTrack| other.source.path == track.source.path;
        let offset = match index.checked_sub(1).map(|previous| &self.tracks[previous]) {
            Some(previous) if same_file(previous) => {
                previous.source.offset
                    + previous.track.frames as u64 * previous.track.frame_bytes() as u64
            }
            _ => 0,
        };

        let frames = match self.tracks.get(index + 1) {
            Some(next) if same_file(next) => match next.index0.saturating_sub(track.index0) {
                0 => {
                    return Err(Error::InvalidToc(format!(
                        "unable to determine size of track {}, missing INDEX 01 markers?",
                        index + 1
                    )))
                }
                frames => frames,
            },
            _ => {
                let size = file_size(&track.source.path)?;
                if size == 0 {
                    return Err(Error::InvalidToc(format!(
                        "couldn't find bin file [{}]",
                        track.source.path.display()
                    )));
                }
                let frames = size.saturating_sub(offset) / track.track.frame_bytes() as u64;
                u32::try_from(frames)
                    .map_err(|_| Error::InvalidToc("The track is too large".into()))?
            }
        };
        Ok((offset, frames))
    }

    /// A single file with multiple sessions has the lead-out of each session between its
    /// tracks, which is stored as padding at the end of the last track of the session.
    fn apply_lead_outs(&mut self, index: usize) {
        let next_index0 = self.tracks.get(index + 1).map(|next| next.index0);
        let track = &mut self.tracks[index];
        if let Some(lead_out) = track.lead_out {
            let end = lead_out.saturating_sub(track.index0);
            if track.track.frames >= end {
                track.track.frames = end;
                if let Some(next_index0) = next_index0 {
                    track.lead_out = Some(next_index0.saturating_sub(lead_out));
                }
            }
        }

        let Some(previous) = index.checked_sub(1) else {
            return;
        };
        let (before, after) = self.tracks.split_at_mut(index);
        let (previous, track) = (&mut before[previous], &mut after[0]);
        let image = previous.source.path.extension();
        if let Some(lead_out) = previous.lead_out {
            if !image.is_some_and(|extension| extension.eq_ignore_ascii_case("img")) {
                previous.track.pad_frames += lead_out;
                track.track.frames = track.track.frames.saturating_sub(lead_out);
                track.source.offset += lead_out as u64 * track.track.frame_bytes() as u64;
            }
        }
    }
}

impl CueTrack {
    /// A track of a cue sheet, with its gaps and its `INDEX 00`.
    fn new(
        cue_track: &rcue::cue::Track,
        source: TrackSource,
        mut track: Track,
    ) -> Result<CueTrack> {
        let index = |number: u32| {
            cue_track
                .indices
                .iter()
                .find(|(index, _)| index.parse::<u32>().ok() == Some(number))
                .map(|(_, time)| frames(*time))
        };
        let index1 = index(1).ok_or_else(|| {
            Error::InvalidToc(format!("track {} is missing INDEX 01 marker", cue_track.no))
        })?;
        if let Some(pregap) = cue_track.pregap {
            track.pregap = frames(pregap);
        }
        // A pregap from `INDEX 00` is stored, and a pregap from `PREGAP` is not.
        let index0 = match index(0) {
            Some(index0) if track.pregap == 0 => {
                track.pregap = index1.saturating_sub(index0);
                track.pregap_type = track.track_type;
                track.pregap_data_bytes = track.data_bytes;
                index0
            }
            Some(index0) => index0,
            None => index1,
        };
        if let Some(postgap) = cue_track.postgap {
            track.postgap = frames(postgap);
        }
        Ok(CueTrack {
            track,
            source,
            index0,
            lead_out: None,
        })
    }
}

/// The state of chdman while reading a cue sheet.
#[derive(Default)]
struct CueState {
    session: u32,
    multisession: bool,
    /// Whether the single-density and high-density areas of a GD-ROM were seen.
    gd_rom_areas: (bool, bool),
}

impl CueState {
    /// Apply a `REM` line, which comes after `track` if there is one.
    ///
    /// The lead-in of a session and the pregap of a session only affect physical offsets,
    /// which a CHD does not store, so they are only checked.
    fn remark(&mut self, key: &str, value: &str, track: Option<&mut CueTrack>) -> Result<()> {
        let first = || value.split_ascii_whitespace().next().unwrap_or("");
        match key {
            // IsoBuster extension.
            "SESSION" => {
                let session = first().parse::<u32>().unwrap_or(0).saturating_sub(1);
                self.session = session;
                self.multisession |= session >= 1;
            }
            "LEAD-OUT" => {
                let lead_out = msf_to_frames(first())?;
                if let Some(track) = track {
                    track.lead_out = Some(lead_out);
                }
            }
            // The pregap of the session is a Redump extension.
            "LEAD-IN" => {
                msf_to_frames(first())?;
            }
            "PREGAP" if self.multisession => {
                msf_to_frames(first())?;
            }
            "SINGLE-DENSITY" if value.starts_with("AREA") => self.gd_rom_areas.0 = true,
            "HIGH-DENSITY" if value.starts_with("AREA") => self.gd_rom_areas.1 = true,
            _ => {}
        }
        Ok(())
    }

    /// A new track of the given type, from `TRACK`.
    fn track(&self, format: &str) -> Result<Track> {
        let (track_type, data_bytes) = TrackType::parse(&format.to_ascii_uppercase())
            .ok_or_else(|| Error::InvalidToc(format!("Unknown track type [{format}]")))?;
        let mut track = Track::new(track_type, data_bytes, 0);
        track.session = self.session;
        // CD+G stores its subcode with the track.
        if format.eq_ignore_ascii_case("CDG") {
            track.subcode = Subcode::Raw;
        }
        Ok(track)
    }
}

/// The number of frames in a cue sheet time, which rcue stores as a `Duration`.
fn frames(time: Duration) -> u32 {
    let subsecond = (time.subsec_nanos() as u64 * 75 + 500_000_000) / 1_000_000_000;
    (time.as_secs() * 75 + subsecond) as u32
}

/// Parse a time `MM:SS:FF` into frames.
fn msf_to_frames(time: &str) -> Result<u32> {
    let invalid = || Error::InvalidToc(format!("Invalid time '{time}'"));
    let parts = time
        .split(':')
        .map(|part| part.parse::<u32>().map_err(|_| invalid()))
        .collect::<Result<Vec<_>>>()?;
    match parts[..] {
        [minutes, seconds, frames] => Ok((minutes * 60 + seconds) * 75 + frames),
        _ => Err(invalid()),
    }
}

/// Where each track is written when extracting a CD-ROM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrackOutput {
    /// The name of the file of the track, as written in the cue sheet.
    pub file_name: String,
    /// The frame the track starts at in its file.
    pub frame_offset: u32,
    /// The byte offset the track starts at in its file.
    pub byte_offset: u64,
}

impl Toc {
    /// A cue sheet for the tracks written to the outputs.
    pub(crate) fn cue_sheet(&self, outputs: &[TrackOutput]) -> String {
        let mut cue = String::new();
        let mut session = None;
        for (index, (track, output)) in self.tracks.iter().zip(outputs).enumerate() {
            if self.sessions() > 1 && session != Some(track.session) {
                writeln!(cue, "REM SESSION {:02}", track.session + 1).unwrap();
                session = Some(track.session);
            }
            if output.byte_offset == 0 {
                writeln!(cue, "FILE \"{}\" BINARY", output.file_name).unwrap();
            }
            let mode = match track.track_type {
                TrackType::Audio => "AUDIO".to_owned(),
                TrackType::Mode1 | TrackType::Mode1Raw => format!("MODE1/{:04}", track.data_bytes),
                _ => format!("MODE2/{:04}", track.data_bytes),
            };
            writeln!(cue, "  TRACK {:02} {mode}", index + 1).unwrap();
            let start = output.frame_offset;
            if track.pregap > 0 && track.pregap_data_bytes == 0 {
                writeln!(cue, "    PREGAP {}", msf(track.pregap)).unwrap();
                writeln!(cue, "    INDEX 01 {}", msf(start)).unwrap();
            } else if track.pregap > 0 {
                writeln!(cue, "    INDEX 00 {}", msf(start)).unwrap();
                writeln!(cue, "    INDEX 01 {}", msf(start + track.pregap)).unwrap();
            } else {
                writeln!(cue, "    INDEX 01 {}", msf(start)).unwrap();
            }
            if track.postgap > 0 {
                writeln!(cue, "    POSTGAP {}", msf(track.postgap)).unwrap();
            }
        }
        cue
    }

    /// A cdrdao table of contents for the tracks written to the outputs.
    pub(crate) fn cdrdao_toc(&self, outputs: &[TrackOutput]) -> String {
        let any = |predicate: fn(&Track) -> bool| self.tracks.iter().any(predicate);
        let mode1 = any(|track| matches!(track.track_type, TrackType::Mode1 | TrackType::Mode1Raw));
        let mode2 = any(|track| track.track_type.is_mode2());
        let audio = any(|track| track.track_type == TrackType::Audio);
        let mut text = String::from(if mode2 {
            "CD_ROM_XA\n\n\n"
        } else if audio && !mode1 {
            "CD_DA\n\n\n"
        } else {
            "CD_ROM\n\n\n"
        });

        for (index, (track, output)) in self.tracks.iter().zip(outputs).enumerate() {
            writeln!(text, "// Track {}", index + 1).unwrap();
            let mode = match track.subcode {
                Subcode::None => track.track_type.name().to_owned(),
                subcode => format!("{} {}", track.track_type.name(), subcode.name()),
            };
            writeln!(text, "TRACK {mode}").unwrap();
            text.push_str("NO COPY\n");
            if track.track_type == TrackType::Audio {
                text.push_str("NO PRE_EMPHASIS\n");
                text.push_str("TWO_CHANNEL_AUDIO\n");
            }
            if track.pregap > 0 {
                writeln!(text, "ZERO {mode} {}", msf(track.pregap)).unwrap();
            }
            let offset = match output.byte_offset {
                0 => String::new(),
                offset => format!("#{offset} "),
            };
            writeln!(
                text,
                "DATAFILE \"{}\" {offset}{} // length in bytes: {}",
                output.file_name,
                msf(track.frames),
                track.frames as u64 * track.frame_bytes() as u64
            )
            .unwrap();
            if track.pregap > 0 {
                writeln!(text, "START {}", msf(track.pregap)).unwrap();
            }
            text.push_str("\n\n");
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::{CueSheet, TrackOutput};
    use crate::cd::CueImage;
    use crate::error::{Error, Result};
    use std::path::Path;

    /// Read a cue sheet whose files have the given sizes.
    fn image(cue: &str, files: &[(&str, u64)]) -> Result<CueImage> {
        CueSheet::parse(cue, Path::new(""))?.resolve(|path| {
            files
                .iter()
                .find(|(name, _)| Path::new(name) == path)
                .map(|(_, size)| *size)
                .ok_or_else(|| Error::InvalidToc(format!("no file {}", path.display())))
        })
    }

    fn metadata(cue: &str, files: &[(&str, u64)]) -> Vec<String> {
        let image = image(cue, files).unwrap();
        (image.toc.tracks.iter().enumerate())
            .map(|(index, track)| track.metadata(index + 1))
            .collect()
    }

    /// The first tracks of Wipeout XL (USA), and the sizes of their files.
    const WIPEOUT: &str = r#"FILE "Wipeout XL (USA) (Track 01).bin" BINARY
  TRACK 01 MODE2/2352
    INDEX 01 00:00:00
FILE "Wipeout XL (USA) (Track 02).bin" BINARY
  TRACK 02 AUDIO
    INDEX 00 00:00:00
    INDEX 01 00:02:00
"#;
    const WIPEOUT_FILES: [(&str, u64); 2] = [
        ("Wipeout XL (USA) (Track 01).bin", 86045568),
        ("Wipeout XL (USA) (Track 02).bin", 62269200),
    ];

    #[test]
    fn single_bin() {
        let cue = "FILE \"game.bin\" BINARY\n  TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\n";
        assert_eq!(
            metadata(cue, &[("game.bin", 2352 * 268743)]),
            ["TRACK:1 TYPE:MODE2_RAW SUBTYPE:NONE FRAMES:268743 PREGAP:0 PGTYPE:MODE1 PGSUB:NONE POSTGAP:0"]
        );
    }

    #[test]
    fn redump_multi_bin() {
        // chdman's metadata for Wipeout XL (USA).
        assert_eq!(
            metadata(WIPEOUT, &WIPEOUT_FILES),
            [
                "TRACK:1 TYPE:MODE2_RAW SUBTYPE:NONE FRAMES:36584 PREGAP:0 PGTYPE:MODE1 PGSUB:NONE POSTGAP:0",
                "TRACK:2 TYPE:AUDIO SUBTYPE:NONE FRAMES:26475 PREGAP:150 PGTYPE:VAUDIO PGSUB:NONE POSTGAP:0",
            ]
        );
        let image = image(WIPEOUT, &WIPEOUT_FILES).unwrap();
        assert!(!image.sources[0].swap);
        assert!(image.sources[1].swap);
    }

    #[test]
    fn tracks_in_one_file() {
        let cue = "FILE \"game.bin\" BINARY
  TRACK 01 MODE1/2352
    INDEX 01 00:00:00
  TRACK 02 AUDIO
    INDEX 00 00:10:00
    INDEX 01 00:12:00
  TRACK 03 AUDIO
    PREGAP 00:02:00
    INDEX 01 00:20:00
    POSTGAP 00:01:00
";
        let image = image(cue, &[("game.bin", 2352 * 2000)]).unwrap();
        let frames: Vec<u32> = image.toc.tracks.iter().map(|track| track.frames).collect();
        let offsets: Vec<u64> = image.sources.iter().map(|source| source.offset).collect();
        assert_eq!(frames, [750, 750, 500]);
        assert_eq!(offsets, [0, 750 * 2352, 1500 * 2352]);
        assert_eq!(
            image.toc.tracks[2].metadata(3),
            "TRACK:3 TYPE:AUDIO SUBTYPE:NONE FRAMES:500 PREGAP:150 PGTYPE:MODE1 PGSUB:NONE POSTGAP:75"
        );
    }

    #[test]
    fn cue_sheet_like_chdman() {
        let image = image(WIPEOUT, &WIPEOUT_FILES).unwrap();
        let outputs: Vec<TrackOutput> = WIPEOUT_FILES
            .iter()
            .map(|(name, _)| TrackOutput {
                file_name: name.to_string(),
                frame_offset: 0,
                byte_offset: 0,
            })
            .collect();
        assert_eq!(image.toc.cue_sheet(&outputs), WIPEOUT);
    }

    #[test]
    fn unsupported() {
        for cue in [
            "FILE \"a.wav\" WAVE\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n",
            "REM SINGLE-DENSITY AREA\nFILE \"a.bin\" BINARY\n  TRACK 01 MODE1/2352\n    INDEX 01 00:00:00\nREM HIGH-DENSITY AREA\nFILE \"b.bin\" BINARY\n  TRACK 02 MODE1/2352\n    INDEX 01 00:00:00\n",
            "FILE \"a.bin\" BINARY\n  TRACK 01 MODE1/2352\n",
        ] {
            assert!(image(cue, &[("a.bin", 2352), ("b.bin", 2352)]).is_err());
        }
    }
}
