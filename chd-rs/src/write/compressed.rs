//! Hunk compression utilities for CHD encoding.
use crate::compression::CompressionCodec;
use crate::error::{Error, Result};
use crate::header::CodecType;
use crate::write::ChdWriteCodecs;
use crate::Chd;
use arrayvec::ArrayVec;
use parking_lot::RwLock;
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::io::{Read, Seek};
use std::sync::Arc;

/// Hash key for an uncompressed hunk.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct HunkHash {
    pub sha1: [u8; 20],
    pub crc16: u16,
}

impl HunkHash {
    /// Hash a hunk. The SHA1 is left as zeroes if it is not needed.
    pub(crate) fn new(hunk: &[u8], with_sha1: bool) -> Self {
        HunkHash {
            sha1: if with_sha1 {
                Sha1::digest(hunk).into()
            } else {
                [0; 20]
            },
            crc16: crate::block_hash::CRC16.checksum(hunk),
        }
    }
}

/// Opaque compression state for a CHD writer.
///
/// Internally, this stores an index of previously compressed hunks
/// to handle self-compression, and an index of the parent CHD.
#[derive(Clone, Default)]
pub(crate) struct CompressionState {
    hunks: Arc<RwLock<HashMap<HunkHash, u32>>>,
    /// The first unit of the parent CHD that starts each hunk-sized run of units.
    parent: Arc<HashMap<HunkHash, u64>>,
}

impl CompressionState {
    /// The state for a CHD with a parent CHD.
    ///
    /// Every run of units in the parent the size of a hunk is hashed, like chdman, so a
    /// hunk can refer to data in the parent that is not aligned to the hunks of the parent.
    /// The parent must have the given hunk size.
    pub fn with_parent<F: Read + Seek>(parent: &mut Chd<F>, unit_bytes: u32) -> Result<Self> {
        let hunk_bytes = parent.header().hunk_size() as usize;
        let hunk_count = parent.header().hunk_count();
        let units_per_hunk = hunk_bytes / unit_bytes as usize;

        // Two hunks at a time, for the runs of units that start in the first hunk and end
        // in the second.
        let mut units = HashMap::new();
        let mut buffer = vec![0u8; 2 * hunk_bytes];
        let mut compressed = Vec::new();
        if hunk_count > 0 {
            parent
                .hunk(0)?
                .read_hunk_in(&mut compressed, &mut buffer[..hunk_bytes])?;
        }
        for hunk_num in 0..hunk_count {
            let runs = if hunk_num + 1 < hunk_count {
                parent
                    .hunk(hunk_num + 1)?
                    .read_hunk_in(&mut compressed, &mut buffer[hunk_bytes..])?;
                units_per_hunk
            } else {
                1
            };
            for run in 0..runs {
                let start = run * unit_bytes as usize;
                let hash = HunkHash::new(&buffer[start..start + hunk_bytes], true);
                let unit = hunk_num as u64 * units_per_hunk as u64 + run as u64;
                units.entry(hash).or_insert(unit);
            }
            buffer.copy_within(hunk_bytes.., 0);
        }

        Ok(CompressionState {
            hunks: Default::default(),
            parent: Arc::new(units),
        })
    }

    /// Find the index of the previously compressed hunk with the given hash
    pub fn find(&self, hash: &HunkHash) -> Option<u32> {
        self.hunks.read().get(hash).copied()
    }

    /// Find the unit of the parent CHD that starts a run of units with the given hash.
    pub fn find_parent(&self, hash: &HunkHash) -> Option<u64> {
        self.parent.get(hash).copied()
    }

    /// Mark a compressed hunk as previously written.
    pub fn insert(&self, hash: HunkHash, hunk_num: u32) {
        self.hunks.write().entry(hash).or_insert(hunk_num);
    }

    /// Whether this is the same state, shared by the same writer.
    pub fn is_same(&self, other: &CompressionState) -> bool {
        Arc::ptr_eq(&self.hunks, &other.hunks)
    }
}

/// The method that a hunk is stored in a CHD file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum HunkKind {
    /// The hunk is compressed with the codec at this index in the header.
    Compressed(u8),
    /// The hunk is uncompressed.
    Uncompressed,
    /// The hunk is identical to one previously written to the CHD, or to data in the parent CHD.
    Duplicate,
}

/// A hunk after it has been compressed by [`HunkCompressor`].
pub struct CompressedHunk<'a> {
    /// The uncompressed hunk.
    pub(crate) hunk: &'a [u8],
    pub(crate) kind: HunkKind,
    /// The compressed hunk, or empty if the hunk is stored as is or is a duplicate.
    pub(crate) data: Vec<u8>,
    pub(crate) hash: HunkHash,
    /// The state of the writer the hunk was compressed for.
    pub(crate) state: CompressionState,
}

impl CompressedHunk<'_> {
    /// The length of the compressed hunk.
    ///
    /// For uncompressed hunks, this is the same as the input data. For duplicate hunks, this is 0.
    pub fn len(&self) -> usize {
        match self.kind {
            HunkKind::Compressed(_) => self.data.len(),
            HunkKind::Uncompressed => self.hunk.len(),
            HunkKind::Duplicate => 0,
        }
    }
}

/// Compresses hunks for encoding.
pub struct HunkCompressor {
    hunk_bytes: u32,
    codecs: ArrayVec<Box<dyn CompressionCodec>, 4>,
    state: CompressionState,
    /// Whether to look for duplicate hunks, which uncompressed CHDs can not store.
    /// Hunks are only hashed with SHA1 when looking for duplicates.
    find_duplicates: bool,
    best: Vec<u8>,
    scratch: Vec<u8>,
}

impl HunkCompressor {
    /// A compressor for the given codecs at their default levels.
    #[cfg(test)]
    pub(crate) fn new(
        codecs: impl ChdWriteCodecs,
        hunk_bytes: u32,
        state: CompressionState,
    ) -> Result<Self> {
        Self::with_levels(codecs, [None; 4], hunk_bytes, state)
    }

    /// Create a compressor for the given codecs. Without codecs, every hunk is stored as is.
    ///
    /// Each codec compresses at the level in the same position, or its default level if
    /// that is `None`.
    ///
    /// Returns [`InvalidParameter`](Error::InvalidParameter) if a codec is not a V5 codec
    /// or a level is not valid for its codec, or [`CodecError`](Error::CodecError) if a
    /// codec does not support the hunk size.
    pub(crate) fn with_levels(
        codecs: impl ChdWriteCodecs,
        levels: [Option<i32>; 4],
        hunk_bytes: u32,
        state: CompressionState,
    ) -> Result<Self> {
        let codecs = codecs
            .into_array_vec()
            .iter()
            .zip(levels)
            .map(|(codec, level)| match codec {
                CodecType::None => Err(Error::InvalidParameter),
                codec if codec.is_legacy() => Err(Error::InvalidParameter),
                codec => codec.init_with_level(hunk_bytes, level),
            })
            .collect::<Result<ArrayVec<_, 4>>>()?;

        let find_duplicates = !codecs.is_empty();
        Ok(HunkCompressor {
            hunk_bytes,
            codecs,
            state,
            find_duplicates,
            best: vec![0; hunk_bytes as usize],
            scratch: vec![0; hunk_bytes as usize],
        })
    }

    /// Compress a hunk with codec that returns the smallest result.
    ///
    /// The provided buffer must be exactly the hunk size, otherwise returns [`InvalidParameter`](Error::InvalidParameter).
    pub fn compress<'a>(&mut self, hunk: &'a [u8]) -> Result<CompressedHunk<'a>> {
        if hunk.len() != self.hunk_bytes as usize {
            return Err(Error::InvalidParameter);
        }

        let hash = HunkHash::new(hunk, self.find_duplicates);
        if self.find_duplicates
            && (self.state.find(&hash).is_some() || self.state.find_parent(&hash).is_some())
        {
            return Ok(CompressedHunk {
                hunk,
                kind: HunkKind::Duplicate,
                data: Vec::new(),
                hash,
                state: self.state.clone(),
            });
        }

        // The hunk itself is the result to beat. Each codec only gets room for a result
        // strictly smaller than the best so far so a codec that can not beat it fails early.
        let mut best_codec = None;
        let mut best_len = hunk.len();
        for (index, codec) in (0u8..).zip(&mut self.codecs) {
            let limit = best_len.saturating_sub(1);
            if let Ok(len) = codec.compress(hunk, &mut self.scratch[..limit]) {
                best_codec = Some(index);
                best_len = len;
                std::mem::swap(&mut self.best, &mut self.scratch);
            }
        }

        // The writer already has the hunk, so hunks stored as is carry no data.
        let (kind, data) = match best_codec {
            Some(index) => (HunkKind::Compressed(index), self.best[..best_len].to_vec()),
            None => (HunkKind::Uncompressed, Vec::new()),
        };
        Ok(CompressedHunk {
            hunk,
            kind,
            data,
            hash,
            state: self.state.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{CompressionState, HunkCompressor, HunkHash, HunkKind};
    use crate::cdrom::CD_FRAME_SIZE;
    use crate::header::CodecType;
    use crate::Error;
    use rand::rngs::StdRng;
    use rand::{RngExt, SeedableRng};

    const HUNK_BYTES: u32 = 8 * CD_FRAME_SIZE;

    fn random_bytes(len: usize, seed: u64) -> Vec<u8> {
        let mut data = vec![0u8; len];
        StdRng::seed_from_u64(seed).fill(&mut data[..]);
        data
    }

    fn text() -> Vec<u8> {
        b"The quick brown fox jumps over the lazy dog. "
            .iter()
            .copied()
            .cycle()
            .take(HUNK_BYTES as usize)
            .collect()
    }

    /// Decompress a hunk with the codec it was compressed with.
    fn decompress(codecs: &[CodecType], index: u8, data: &[u8]) -> Vec<u8> {
        let mut codec = codecs[index as usize].init(HUNK_BYTES).unwrap();
        let mut output = vec![0u8; HUNK_BYTES as usize];
        codec.decompress(data, &mut output).unwrap();
        output
    }

    #[test]
    fn picks_smallest_codec() {
        let codecs = [CodecType::ZLibV5, CodecType::LzmaV5, CodecType::HuffV5];
        let mut compressor =
            HunkCompressor::new(&codecs, HUNK_BYTES, CompressionState::default()).unwrap();
        let input = text();
        let hunk = compressor.compress(&input).unwrap();

        // Compare with each codec on its own.
        let sizes: Vec<Option<usize>> = codecs
            .iter()
            .map(|codec| {
                let mut output = vec![0u8; HUNK_BYTES as usize];
                codec
                    .init(HUNK_BYTES)
                    .unwrap()
                    .compress(&input, &mut output)
                    .ok()
            })
            .collect();
        let smallest = sizes.iter().flatten().min().copied().unwrap();
        // The first codec with the smallest size wins ties.
        let first_smallest = sizes
            .iter()
            .position(|&size| size == Some(smallest))
            .unwrap();

        assert_eq!(hunk.kind, HunkKind::Compressed(first_smallest as u8));
        assert_eq!(hunk.data.len(), smallest);
        assert_eq!(decompress(&codecs, first_smallest as u8, &hunk.data), input);
    }

    #[test]
    fn incompressible_hunks_are_uncompressed() {
        let codecs = [CodecType::ZLibV5, CodecType::LzmaV5];
        let mut compressor =
            HunkCompressor::new(&codecs, HUNK_BYTES, CompressionState::default()).unwrap();
        let input = random_bytes(HUNK_BYTES as usize, 1);
        let hunk = compressor.compress(&input).unwrap();
        assert_eq!(hunk.kind, HunkKind::Uncompressed);
        assert!(hunk.data.is_empty());
    }

    #[test]
    fn cd_codecs() {
        let codecs = [
            CodecType::LzmaCdV5,
            CodecType::ZLibCdV5,
            CodecType::FlacCdV5,
        ];
        let mut compressor =
            HunkCompressor::new(&codecs, HUNK_BYTES, CompressionState::default()).unwrap();
        let input = text();
        let hunk = compressor.compress(&input).unwrap();
        let HunkKind::Compressed(index) = hunk.kind else {
            panic!("not compressed");
        };
        assert_eq!(decompress(&codecs, index, &hunk.data), input);
        assert_eq!(hunk.hash.crc16, crate::block_hash::CRC16.checksum(&input));
    }

    #[test]
    fn duplicates_are_not_compressed() {
        let state = CompressionState::default();
        let mut compressor =
            HunkCompressor::new(&[CodecType::ZLibV5], HUNK_BYTES, state.clone()).unwrap();
        let input = text();

        let first = compressor.compress(&input).unwrap();
        assert_eq!(first.kind, HunkKind::Compressed(0));

        // Once the writer has stored the hunk, the same hunk is a duplicate.
        state.insert(first.hash, 0);
        let second = compressor.compress(&input).unwrap();
        assert_eq!(second.kind, HunkKind::Duplicate);
        assert!(second.data.is_empty());
        assert!(second.hash == HunkHash::new(&input, true));
    }

    #[test]
    fn no_codecs() {
        let mut compressor =
            HunkCompressor::new(&[], HUNK_BYTES, CompressionState::default()).unwrap();
        let input = text();
        assert_eq!(
            compressor.compress(&input).unwrap().kind,
            HunkKind::Uncompressed
        );
    }

    #[test]
    fn invalid_codecs() {
        for codec in [CodecType::None, CodecType::Zlib, CodecType::AV] {
            assert!(matches!(
                HunkCompressor::new(&[codec], HUNK_BYTES, CompressionState::default()),
                Err(Error::InvalidParameter)
            ));
        }
        // CD codecs need a hunk size that is a multiple of the frame size.
        assert!(matches!(
            HunkCompressor::new(&[CodecType::LzmaCdV5], 4096, CompressionState::default()),
            Err(Error::CodecError)
        ));
    }

    #[test]
    fn wrong_hunk_size() {
        let mut compressor = HunkCompressor::new(
            &[CodecType::ZLibV5],
            HUNK_BYTES,
            CompressionState::default(),
        )
        .unwrap();
        assert!(matches!(
            compressor.compress(&[0u8; 16]),
            Err(Error::InvalidParameter)
        ));
    }

    /// Correlated stereo 16-bit PCM, which FLAC compresses better with mid-side coding.
    fn audio() -> Vec<u8> {
        (0..HUNK_BYTES as usize / 4)
            .flat_map(|n| {
                let left = ((n as f64 / 20.0).sin() * 8000.0) as i16;
                let right = left / 2 + ((n as f64 / 7.0).sin() * 500.0) as i16;
                [left.to_be_bytes(), right.to_be_bytes()]
            })
            .flatten()
            .collect()
    }

    /// Text of random words, which compresses to different sizes at different levels.
    fn words() -> Vec<u8> {
        const WORDS: [&str; 8] = [
            "chd ", "hunk ", "frame ", "sector ", "map ", "lzma ", "zlib ", "flac ",
        ];
        let mut rng = StdRng::seed_from_u64(2);
        let mut data = Vec::new();
        while data.len() < HUNK_BYTES as usize {
            data.extend_from_slice(WORDS[rng.random_range(0..WORDS.len())].as_bytes());
        }
        data.truncate(HUNK_BYTES as usize);
        data
    }

    /// Each codec with levels, the level chdman compresses with, and levels to test.
    fn leveled_codecs() -> Vec<(CodecType, i32, Vec<i32>)> {
        #[cfg(feature = "fast_zstd")]
        let (zstd_default, zstd_levels) = (
            zstd_safe::max_c_level(),
            vec![
                zstd_safe::min_c_level(),
                -1,
                0,
                1,
                3,
                19,
                zstd_safe::max_c_level(),
            ],
        );
        #[cfg(not(feature = "fast_zstd"))]
        let (zstd_default, zstd_levels) = (1, vec![1]);
        let zlib_levels: Vec<i32> = (0..=9).collect();
        let lzma_levels: Vec<i32> = (0..=9).collect();
        let flac_levels: Vec<i32> = (0..=8).collect();
        vec![
            (CodecType::ZLibV5, 9, zlib_levels.clone()),
            (CodecType::ZLibCdV5, 9, zlib_levels),
            (CodecType::LzmaV5, 6, lzma_levels.clone()),
            (CodecType::LzmaCdV5, 6, lzma_levels),
            (CodecType::ZstdV5, zstd_default, zstd_levels.clone()),
            (CodecType::ZstdCdV5, zstd_default, zstd_levels),
            (CodecType::FlacV5, 8, flac_levels.clone()),
            (CodecType::FlacCdV5, 8, flac_levels),
        ]
    }

    fn compress_with(codec: CodecType, level: Option<i32>, input: &[u8]) -> Option<Vec<u8>> {
        let mut output = vec![0u8; HUNK_BYTES as usize];
        let len = codec
            .init_with_level(HUNK_BYTES, level)
            .unwrap()
            .compress(input, &mut output)
            .ok()?;
        output.truncate(len);
        Some(output)
    }

    #[test]
    fn default_levels_are_chdman_levels() {
        for (codec, chdman_level, _) in leveled_codecs() {
            for input in [text(), words(), audio()] {
                assert_eq!(
                    compress_with(codec, None, &input),
                    compress_with(codec, Some(chdman_level), &input),
                    "{:?}",
                    codec as u32
                );
            }
        }
    }

    #[test]
    fn levels_round_trip_and_change_output() {
        for (codec, _, levels) in leveled_codecs() {
            let input = match codec {
                CodecType::FlacV5 | CodecType::FlacCdV5 => audio(),
                _ => words(),
            };
            let mut sizes = Vec::new();
            for level in levels.iter().copied() {
                // Stored Deflate is larger than the input.
                let Some(compressed) = compress_with(codec, Some(level), &input) else {
                    sizes.push(None);
                    continue;
                };
                let mut output = vec![0u8; HUNK_BYTES as usize];
                codec
                    .init(HUNK_BYTES)
                    .unwrap()
                    .decompress(&compressed, &mut output)
                    .unwrap();
                assert!(output == input, "{:?} level {level}", codec as u32);
                sizes.push(Some(compressed.len()));
            }
            sizes.sort();
            sizes.dedup();
            assert!(
                levels.len() == 1 || sizes.len() > 1,
                "{:?} compressed every level to {sizes:?}",
                codec as u32
            );
        }
    }

    #[test]
    fn invalid_levels() {
        let mut invalid = vec![
            (CodecType::ZLibV5, -1),
            (CodecType::ZLibV5, 10),
            (CodecType::ZLibCdV5, 10),
            (CodecType::LzmaV5, -1),
            (CodecType::LzmaV5, 10),
            (CodecType::LzmaCdV5, 10),
            (CodecType::FlacV5, -1),
            (CodecType::FlacV5, 9),
            (CodecType::FlacCdV5, 9),
            (CodecType::HuffV5, 0),
            (CodecType::AVHuffV5, 0),
        ];
        #[cfg(feature = "fast_zstd")]
        invalid.extend([
            (CodecType::ZstdV5, zstd_safe::max_c_level() + 1),
            (CodecType::ZstdCdV5, zstd_safe::min_c_level() - 1),
        ]);
        for (codec, level) in invalid {
            assert!(
                matches!(
                    HunkCompressor::with_levels(
                        &[codec],
                        [Some(level), None, None, None],
                        HUNK_BYTES,
                        CompressionState::default()
                    ),
                    Err(Error::InvalidParameter)
                ),
                "{:?} level {level}",
                codec as u32
            );
        }
    }

    #[test]
    fn levels_apply_to_codecs_in_order() {
        let codecs = [CodecType::ZLibV5, CodecType::LzmaV5];
        let input = text();
        let mut compressor = HunkCompressor::with_levels(
            &codecs,
            [Some(0), None, None, None],
            HUNK_BYTES,
            CompressionState::default(),
        )
        .unwrap();
        let hunk = compressor.compress(&input).unwrap();
        // Stored Deflate never wins, so LZMA does.
        assert_eq!(hunk.kind, HunkKind::Compressed(1));
        assert!(matches!(
            HunkCompressor::with_levels(
                &codecs,
                [None, Some(10), None, None],
                HUNK_BYTES,
                CompressionState::default()
            ),
            Err(Error::InvalidParameter)
        ));
    }
}
