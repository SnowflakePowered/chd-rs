//! Extracting CD-ROM CHDs to cue sheets or cdrdao tables of contents, and bin files.
use super::{CdImage, Subcode, Toc, Track, TrackOutput, TrackReader, TrackType, FRAME_BYTES};
use crate::error::{Error, Result};
use chd::read::ChdReader;
use lender::{check_covariance, Lender, Lending};
use std::io::{self, Read, Seek, SeekFrom};
use std::ops::Range;
use std::path::{Path, PathBuf};

/// Options for extracting a CD-ROM.
#[derive(Debug, Clone, Default)]
pub struct ExtractCdOptions {
    /// The name of the bin files, or the name of the table of contents with a `.bin`
    /// extension by default. `%t` is replaced with the track number, such as `%02t` for
    /// `01`.
    pub output_bin: Option<PathBuf>,
    /// Write each track to its own bin file.
    pub split_bin: bool,
}

/// The files of an extracted CD-ROM: each bin file, then the table of
/// contents, as a [`Lender`] of [`Extracted`] files that are read as they are written.
///
/// ```no_run
/// use chd::iter::Lender;
/// use chd::Chd;
/// use rchdman::cd::{CdImage, ExtractCdOptions};
/// use std::fs::File;
/// use std::io::{self, BufReader};
///
/// # fn main() -> rchdman::Result<()> {
/// let chd = Chd::open(BufReader::new(File::open("game.chd")?), None)?;
/// let mut cd = CdImage::new(chd)?;
/// let mut files = cd.extract("game.cue".as_ref(), &ExtractCdOptions::default())?;
/// while let Some(mut file) = files.next() {
///     io::copy(&mut file.data, &mut File::create(file.name)?)?;
/// }
/// # Ok(())
/// # }
/// ```
pub struct CdExtraction<'a, F: Read + Seek> {
    image: &'a mut CdImage<F>,
    cue: bool,
    bins: Vec<BinFile>,
    /// The name of the table of contents, and its text.
    toc: (PathBuf, String),
    next: usize,
}

/// A bin file of a [`CdExtraction`].
struct BinFile {
    name: PathBuf,
    /// The indices of the tracks in the file.
    tracks: Range<usize>,
    size: u64,
}

/// A file extracted from a CD-ROM: its name, and a reader of its data.
pub struct Extracted<'a> {
    /// The path of the file.
    pub name: &'a Path,
    /// The size of the file.
    pub size: u64,
    /// The data of the file.
    pub data: Box<dyn Read + 'a>,
}

impl<F: Read + Seek> CdImage<F> {
    /// Extract the CD-ROM to a cue sheet if `output` is a `.cue` file, or a cdrdao table of
    /// contents otherwise, and bin files. See [`CdExtraction`].
    ///
    /// Cue sheets can not store subcode, so it is left out, and store audio little-endian.
    /// GD-ROM images (`.gdi`) are not supported.
    pub fn extract(
        &mut self,
        output: &Path,
        options: &ExtractCdOptions,
    ) -> Result<CdExtraction<'_, F>> {
        let extension = output
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase);
        let cue = match extension.as_deref() {
            Some("cue") => true,
            Some("gdi") => {
                return Err(Error::Unsupported(
                    "Extracting GD-ROM images (.gdi) is not supported",
                ))
            }
            _ => false,
        };

        // The tracks grouped by their bin file, and where each track starts in its file.
        let names = bin_names(&self.toc, output, options)?;
        let mut bins = Vec::new();
        let mut track_outputs = Vec::new();
        for group in names.chunk_by(|a, b| a == b) {
            let start = track_outputs.len();
            let tracks = start..start + group.len();
            let file_name = group[0]
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let (mut frame_offset, mut size) = (0, 0);
            for track in &self.toc.tracks[tracks.clone()] {
                track_outputs.push(TrackOutput {
                    file_name: file_name.clone(),
                    frame_offset,
                    byte_offset: size,
                });
                frame_offset += track.frames;
                let frame_bytes = if cue {
                    track.data_bytes
                } else {
                    track.frame_bytes()
                };
                size += track.frames.saturating_sub(track.pad_frames) as u64 * frame_bytes as u64;
            }
            bins.push(BinFile {
                name: group[0].clone(),
                tracks,
                size,
            });
        }

        let mut text = if cue {
            self.toc.cue_sheet(&track_outputs)
        } else {
            self.toc.cdrdao_toc(&track_outputs)
        };
        // chdman writes text files with the line endings of the platform.
        if cfg!(windows) {
            text = text.replace('\n', "\r\n");
        }
        Ok(CdExtraction {
            image: self,
            cue,
            bins,
            toc: (output.to_owned(), text),
            next: 0,
        })
    }
}

impl<F: Read + Seek> CdExtraction<'_, F> {
    /// The bin files, in order.
    pub fn bin_files(&self) -> impl Iterator<Item = &Path> {
        self.bins.iter().map(|bin| bin.name.as_path())
    }

    /// The total size of the files.
    pub fn size(&self) -> u64 {
        self.bins.iter().map(|bin| bin.size).sum::<u64>() + self.toc.1.len() as u64
    }

    /// Warnings about data that is left out.
    pub fn warnings(&self) -> Vec<String> {
        if !self.cue {
            return Vec::new();
        }
        (self.image.toc.tracks.iter().enumerate())
            .filter(|(_, track)| track.subcode != Subcode::None)
            .map(|(index, _)| format!(
                "Track {} has subcode data. bin/cue format cannot contain subcode data and it will be omitted.",
                index + 1
            ))
            .collect()
    }
}

impl<'lend, F: Read + Seek> Lending<'lend> for CdExtraction<'_, F> {
    type Lend = Extracted<'lend>;
}

impl<F: Read + Seek> Lender for CdExtraction<'_, F> {
    check_covariance!();

    fn next(&mut self) -> Option<Extracted<'_>> {
        let index = self.next;
        self.next += 1;
        let Some(bin) = self.bins.get(index) else {
            let (name, text) = &self.toc;
            return (index == self.bins.len()).then(|| Extracted {
                name,
                size: text.len() as u64,
                data: Box::new(text.as_bytes()),
            });
        };
        let CdImage { reader, toc } = &mut *self.image;
        let starts = toc.chd_frames().scan(0, |start, frames| {
            let track_start = *start;
            *start += frames as u64 * FRAME_BYTES as u64;
            Some(track_start)
        });
        let tracks: Vec<_> = (starts.zip(&toc.tracks))
            .skip(bin.tracks.start)
            .take(bin.tracks.len())
            .collect();
        Some(Extracted {
            name: &bin.name,
            size: bin.size,
            data: Box::new(BinReader {
                chd: Some(reader),
                track: None,
                tracks: tracks.into_iter(),
                cue: self.cue,
            }),
        })
    }
}

/// Reads the tracks of a bin file in order.
struct BinReader<'a, F: Read + Seek> {
    /// The CHD, between tracks.
    chd: Option<&'a mut ChdReader<F>>,
    /// The track being read.
    track: Option<TrackReader<&'a mut ChdReader<F>>>,
    /// The tracks left to read, and where they start in the CHD.
    tracks: std::vec::IntoIter<(u64, &'a Track)>,
    cue: bool,
}

impl<F: Read + Seek> Read for BinReader<'_, F> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if let Some(track) = &mut self.track {
                let read = track.read(buf)?;
                if read > 0 || buf.is_empty() {
                    return Ok(read);
                }
                self.chd = self.track.take().map(TrackReader::into_inner);
            }
            let Some((start, track)) = self.tracks.next() else {
                return Ok(0);
            };
            let chd = self.chd.take().expect("the CHD is not in a track");
            if let Err(error) = chd.seek(SeekFrom::Start(start)) {
                self.chd = Some(chd);
                return Err(error);
            }
            // Cue sheets store audio little-endian, without subcode.
            let swap = self.cue && track.track_type == TrackType::Audio;
            let reader = TrackReader::new(chd, track, FRAME_BYTES as usize, swap);
            self.track = Some(if self.cue {
                reader.without_subcode(track)
            } else {
                reader
            });
        }
    }
}

/// The bin file of each track.
fn bin_names(toc: &Toc, output: &Path, options: &ExtractCdOptions) -> Result<Vec<PathBuf>> {
    let template = match &options.output_bin {
        Some(bin) => {
            let bin = bin.to_string_lossy().into_owned();
            if bin.contains('.') {
                bin
            } else {
                bin + ".bin"
            }
        }
        None => {
            let mut base = output.with_extension("").to_string_lossy().into_owned();
            if options.split_bin {
                base += if toc.tracks.len() >= 10 {
                    " (Track %02t)"
                } else {
                    " (Track %t)"
                };
            }
            base + ".bin"
        }
    };
    if template.contains('"') {
        return Err(Error::invalid(format!(
            "Output bin filename ({template}) must not contain quotation marks"
        )));
    }

    (1..=toc.tracks.len())
        .map(|number| {
            let (name, found) = replace_track_number(&template, options.split_bin.then_some(number));
            if options.split_bin && !found {
                return Err(Error::invalid("A track number variable (%t) must be specified in the output bin filename when --splitbin is enabled"));
            }
            Ok(PathBuf::from(name))
        })
        .collect()
}

/// Replace `%t` variables, such as `%02t`, with the track number like printf. Pairs of `%`
/// are escaped `%`s, and variables are left as is without a track number. Returns whether a
/// variable was found.
fn replace_track_number(template: &str, number: Option<usize>) -> (String, bool) {
    let mut result = String::with_capacity(template.len());
    let mut found = false;
    let mut rest = template;
    while let Some(start) = rest.find('%') {
        result.push_str(&rest[..start]);
        let percents = &rest[start..];
        let run = percents.len() - percents.trim_start_matches('%').len();
        let after = &percents[run..];
        let flags = after.trim_start_matches(|c: char| c.is_ascii_digit() || c == '+' || c == '-');
        let spec = &after[..after.len() - flags.len()];
        match flags.strip_prefix('t') {
            // A variable follows an odd number of `%`s.
            Some(rest_after) if run % 2 == 1 => {
                found = true;
                rest = rest_after;
                result.push_str(&percents[..run - 1]);
                let Some(number) = number else {
                    result.push_str(&percents[run - 1..run + spec.len() + 1]);
                    continue;
                };
                // Format the number like printf `%<spec>d`, such as `02` to pad to two digits.
                let digits = spec.trim_start_matches(['+', '-']);
                let width = digits.parse::<usize>().unwrap_or(0);
                let number = format!("{}{number}", if spec.starts_with('+') { "+" } else { "" });
                result.push_str(&if spec.starts_with('-') {
                    format!("{number:<width$}")
                } else if digits.starts_with('0') {
                    format!("{number:0>width$}")
                } else {
                    format!("{number:>width$}")
                });
            }
            _ => {
                result.push_str(&percents[..run]);
                rest = after;
            }
        }
    }
    result.push_str(rest);
    (result, found)
}

#[cfg(test)]
mod tests {
    use super::replace_track_number;

    #[test]
    fn track_number_variables() {
        assert_eq!(
            replace_track_number("game (Track %02t).bin", Some(3)),
            ("game (Track 03).bin".to_owned(), true)
        );
        assert_eq!(
            replace_track_number("game (Track %t).bin", Some(12)),
            ("game (Track 12).bin".to_owned(), true)
        );
        assert_eq!(
            replace_track_number("100%% %t.bin", Some(1)),
            ("100%% 1.bin".to_owned(), true)
        );
        assert_eq!(
            replace_track_number("100%%t.bin", Some(1)),
            ("100%%t.bin".to_owned(), false)
        );
        assert_eq!(
            replace_track_number("game %02t.bin", None),
            ("game %02t.bin".to_owned(), true)
        );
        assert_eq!(
            replace_track_number("game.bin", Some(1)),
            ("game.bin".to_owned(), false)
        );
    }
}
