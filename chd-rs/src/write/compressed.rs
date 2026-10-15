//! Hunk compression utilities for CHD encoding.
use crate::compression::CompressionCodec;
use crate::error::{Error, Result};
use crate::header::CodecType;
use crate::write::ChdWriteCodecChoice;
use arrayvec::ArrayVec;
use parking_lot::RwLock;
use sha1::{Digest, Sha1};
use std::collections::HashMap;
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
/// to handle self-compression.
#[derive(Clone, Default)]
pub(crate) struct CompressionState {
    hunks: Arc<RwLock<HashMap<HunkHash, u32>>>,
}

impl CompressionState {
    /// Find the index of the previously compressed hunk with the given hash
    pub fn find(&self, hash: &HunkHash) -> Option<u32> {
        self.hunks.read().get(hash).copied()
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
    /// The hunk is identical to one previously written to the CHD.
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
    /// Create a compressor for the given codecs. Without codecs, every hunk is stored as is.
    ///
    /// Returns [`InvalidParameter`](Error::InvalidParameter) if a codec is not a V5 codec,
    /// or [`CodecError`](Error::CodecError) if a codec does not support the hunk size.
    pub(crate) fn new(
        codecs: impl ChdWriteCodecChoice,
        hunk_bytes: u32,
        state: CompressionState,
    ) -> Result<Self> {
        let codecs = codecs
            .into_array_vec()
            .iter()
            .map(|codec| match codec {
                CodecType::None => Err(Error::InvalidParameter),
                codec if codec.is_legacy() => Err(Error::InvalidParameter),
                codec => codec.init(hunk_bytes),
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
        if self.find_duplicates && self.state.find(&hash).is_some() {
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
}
