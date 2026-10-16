//! `info` and `benchmark`.
use crate::common::{open_chd, Progress};
use crate::error::{Error, Result};
use chd::header::{CodecType, Header};
use chd::iter::Lender;
use chd::map::{CompressionTypeLegacy, CompressionTypeV5, MapEntry};
use chd::metadata::Metadata;
use chd::Chd;
use std::io::{Read, Seek};
use std::path::Path;
use std::time::{Duration, Instant};

/// Information about a CHD.
pub struct ChdInfo {
    /// The header.
    pub header: Header,
    /// The size of the file.
    pub file_size: u64,
    /// The metadata entries.
    pub metadata: Vec<Metadata>,
    /// The number of hunks stored in some way.
    pub hunk_usage: Vec<(HunkStorage, u64)>,
}

impl ChdInfo {
    /// Read the information about the CHD at the path.
    pub fn open(path: &Path) -> Result<ChdInfo> {
        let file_size = std::fs::metadata(path).map_err(Error::file(path))?.len();
        let mut chd = open_chd(path, None)?;
        Ok(ChdInfo {
            header: chd.header().clone(),
            file_size,
            metadata: chd.metadata_refs().try_into()?,
            hunk_usage: HunkStorage::count(&chd)?,
        })
    }
}

/// How hunks are stored in a CHD.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HunkStorage {
    /// The hunk is stored uncompressed.
    Uncompressed,
    /// The hunk is stored as a reference to another identical chunk in the CHD.
    CopyFromSelf,
    /// The hunk is stored as a reference to another identical chunk in the parent CHD.
    CopyFromParent,
    /// The hunk is stored in the hunk map. Only occurs in legacy CHDs.
    Mini,
    /// The hunk is stored in an unknown way.
    Unknown,
    /// The hunk is compressed with the given codec.
    Codec(CodecType),
}

impl HunkStorage {
    /// Count how the hunks of a CHD are stored, for [`ChdInfo::hunk_usage`].
    fn count<F: Read + Seek>(chd: &Chd<F>) -> Result<Vec<(HunkStorage, u64)>> {
        // Hunks compressed by each of the four codecs, then hunks stored in other ways.
        const UNCOMPRESSED: usize = 4;
        const SELF: usize = 5;
        const PARENT: usize = 6;
        const MINI: usize = 7;
        const UNKNOWN: usize = 8;

        let mut counts = [0u64; 9];
        for entry in chd.map().iter() {
            let index = match entry {
                MapEntry::V5Compressed(entry) => match entry.hunk_type()? {
                    CompressionTypeV5::CompressionType0 => 0,
                    CompressionTypeV5::CompressionType1 => 1,
                    CompressionTypeV5::CompressionType2 => 2,
                    CompressionTypeV5::CompressionType3 => 3,
                    CompressionTypeV5::CompressionNone => UNCOMPRESSED,
                    CompressionTypeV5::CompressionSelf
                    | CompressionTypeV5::CompressionSelf0
                    | CompressionTypeV5::CompressionSelf1 => SELF,
                    CompressionTypeV5::CompressionParent
                    | CompressionTypeV5::CompressionParentSelf
                    | CompressionTypeV5::CompressionParent0
                    | CompressionTypeV5::CompressionParent1 => PARENT,
                    _ => UNKNOWN,
                },
                MapEntry::V5Uncompressed(_) => UNCOMPRESSED,
                MapEntry::LegacyEntry(entry) => match entry.hunk_type()? {
                    CompressionTypeLegacy::Invalid => continue,
                    CompressionTypeLegacy::Compressed => 0,
                    CompressionTypeLegacy::Uncompressed => UNCOMPRESSED,
                    CompressionTypeLegacy::Mini => MINI,
                    CompressionTypeLegacy::SelfHunk => SELF,
                    CompressionTypeLegacy::ParentHunk => PARENT,
                    CompressionTypeLegacy::ExternalCompressed => UNKNOWN,
                },
            };
            counts[index] += 1;
        }

        let codecs = chd.header().codecs();
        let storage = |index: usize| match index {
            UNCOMPRESSED => HunkStorage::Uncompressed,
            SELF => HunkStorage::CopyFromSelf,
            PARENT => HunkStorage::CopyFromParent,
            MINI => HunkStorage::Mini,
            UNKNOWN => HunkStorage::Unknown,
            index => codecs[index].map_or(HunkStorage::Unknown, HunkStorage::Codec),
        };
        Ok((UNCOMPRESSED..=UNKNOWN)
            .chain(0..UNCOMPRESSED)
            .filter(|&index| counts[index] > 0)
            .map(|index| (storage(index), counts[index]))
            .collect())
    }
}

/// Benchmark results
#[derive(Debug, Clone, Copy)]
pub struct Benchmark {
    /// The number of bytes read.
    pub bytes: u64,
    /// The number of hunks read.
    pub hunks: u32,
    /// How long reading took, including opening the CHD.
    pub time: Duration,
}

impl Benchmark {
    /// Read every hunk of the CHD at the path, timing how long it takes.
    pub fn run(path: &Path, parent: Option<&Path>, progress: Progress) -> Result<Benchmark> {
        let start = Instant::now();
        let mut chd = open_chd(path, parent)?;
        let total = chd.header().hunk_count() as u64;
        let mut hunk_buf = chd.get_hunksized_buffer();
        let mut cmp_buf = Vec::new();
        let mut hunks = chd.hunks();
        let mut result = Benchmark {
            bytes: 0,
            hunks: 0,
            time: Duration::ZERO,
        };
        while let Some(mut hunk) = hunks.next() {
            result.bytes += hunk.read_hunk_in(&mut cmp_buf, &mut hunk_buf)? as u64;
            result.hunks += 1;
            progress(result.hunks as u64, total);
        }
        result.time = start.elapsed();
        Ok(result)
    }
}
