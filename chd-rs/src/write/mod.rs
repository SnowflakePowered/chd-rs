//! Writing CHD files.
//!
//! ```no_run
//! use chd::write::{ChdWriteOptions, ChdWriter};
//! use std::fs::File;
//! use std::io;
//!
//! # fn main() -> chd::Result<()> {
//! let mut input = File::open("image.bin")?;
//! // Compressed like `chdman createraw -us 512`.
//! let options = ChdWriteOptions::raw(512);
//! let mut chd = ChdWriter::new(File::create("image.chd")?, options)?;
//! io::copy(&mut input, &mut chd)?;
//! chd.finish()?;
//! # Ok(())
//! # }
//! ```
use crate::cdrom::CD_FRAME_SIZE;
use crate::error::{Error, Result};
use crate::header::{CodecType, Header, HeaderV5, Version};
use crate::map::{CompressionTypeV5, Map, RawMapEntryV5};
use crate::metadata::{MetadataData, MetadataTag};
use crate::Chd;
use arrayvec::ArrayVec;
use compressed::{CompressionState, HunkKind};
use metadata::{MetadataList, METADATA_FLAG_CHECKSUM};
use sha1::{Digest, Sha1};
use std::io::{self, Read, Seek, SeekFrom, Write};

mod compressed;
mod metadata;
mod update;

pub use compressed::{CompressedHunk, HunkCompressor};

/// The size of a V5 header.
const HEADER_BYTES: u64 = 124;

/// The default codecs used for raw, hard disk, and DVD-ROM images.
const DEFAULT_CODECS: [CodecType; 4] = [
    CodecType::LzmaV5,
    CodecType::ZLibV5,
    CodecType::HuffV5,
    CodecType::FlacV5,
];

/// Options for creating a CHD file.
#[derive(Debug, Clone)]
pub struct ChdWriteOptions {
    hunk_bytes: u32,
    unit_bytes: u32,
    logical_bytes: Option<u64>,
    compression_codecs: ArrayVec<CodecType, 4>,
    hash_uncompressed: bool,
}

mod sealed {
    pub trait Sealed {}
}

/// The codecs to use when writing a CHD file.
///
/// Accepts up to four codecs. An empty slice (i.e. no codecs) indicates an uncompressed CHD.
pub trait ChdWriteCodecs: sealed::Sealed {
    #[doc(hidden)]
    fn into_array_vec(self) -> ArrayVec<CodecType, 4>;
}

macro_rules! impl_codecs {
    ($($len:literal),*) => {$(
        impl sealed::Sealed for &[CodecType; $len] {}

        impl ChdWriteCodecs for &[CodecType; $len] {
            fn into_array_vec(self) -> ArrayVec<CodecType, 4> {
                self.iter().copied().collect()
            }
        }
    )*};
}

impl_codecs!(0, 1, 2, 3, 4);

impl sealed::Sealed for &ArrayVec<CodecType, 4> {}

impl ChdWriteCodecs for &ArrayVec<CodecType, 4> {
    fn into_array_vec(self) -> ArrayVec<CodecType, 4> {
        self.clone()
    }
}

impl ChdWriteOptions {
    /// Options for a CHD with the given hunk and unit sizes in bytes.
    ///
    /// The hunk size must be a multiple of the unit size.
    pub fn new(hunk_bytes: u32, unit_bytes: u32) -> Self {
        ChdWriteOptions {
            hunk_bytes,
            unit_bytes,
            logical_bytes: None,
            compression_codecs: ArrayVec::new(),
            hash_uncompressed: false,
        }
    }

    /// Default write options used for raw images.
    ///
    /// The hunk size is the largest multiple of the unit size up to 4096 bytes, or the
    /// unit size if it is larger. Compressed with LZMA, Deflate, Huffman, and FLAC.
    pub fn raw(unit_bytes: u32) -> Self {
        let hunk_bytes = (4096 / unit_bytes.max(1) * unit_bytes).max(unit_bytes);
        ChdWriteOptions::new(hunk_bytes, unit_bytes).compression(&DEFAULT_CODECS)
    }

    /// Default write options used for hard disk images with 512 byte sectors.
    ///
    /// Hard disk images also need [`HardDisk`](crate::metadata::KnownMetadata::HardDisk) metadata to describe the disk geometry.
    /// Compressed with LZMA, Deflate, Huffman, and FLAC.
    pub fn hard_disk() -> Self {
        Self::raw(512)
    }

    /// Default write options used for CD-ROM images.
    ///
    /// Units are frames of a 2352 byte sector and 96 bytes of subcode, and hunks are 8
    /// frames. Compressed with the CD codecs for LZMA, Deflate, and FLAC.
    ///
    /// CD-ROM images also require track metadata, such as
    /// [`CdRomTrack2`](crate::metadata::KnownMetadata::CdRomTrack2).
    pub fn cd() -> Self {
        const FRAMES_PER_HUNK: u32 = 8;
        ChdWriteOptions::new(FRAMES_PER_HUNK * CD_FRAME_SIZE, CD_FRAME_SIZE).compression(&[
            CodecType::LzmaCdV5,
            CodecType::ZLibCdV5,
            CodecType::FlacCdV5,
        ])
    }

    /// Default write options used for DVD-ROM images.
    ///
    /// Units are 2048 byte sectors, and hunks are 2 sectors.
    /// Compressed with LZMA, Deflate, Huffman, and FLAC.
    pub fn dvd() -> Self {
        ChdWriteOptions::new(2 * 2048, 2048).compression(&DEFAULT_CODECS)
    }

    /// Default write options used for LaserDisc images .
    ///
    /// Units and hunks are a single frame of A/V Huffman data, which is
    /// `12 + channels * samples * 2 + width * height * 2` bytes, where `samples` is the most
    /// audio samples per channel in a frame. Compressed with the A/V Huffman codec.
    pub fn laserdisc(frame_bytes: u32) -> Self {
        ChdWriteOptions::new(frame_bytes, frame_bytes).compression(&[CodecType::AVHuffV5])
    }

    /// Set the hunk size in bytes, such as to change the hunk size of a preset.
    ///
    /// The hunk size must be a multiple of the unit size.
    pub fn hunk_bytes(mut self, hunk_bytes: u32) -> Self {
        self.hunk_bytes = hunk_bytes;
        self
    }

    /// The hunk and unit sizes in bytes.
    pub fn sizes(&self) -> (u32, u32) {
        (self.hunk_bytes, self.unit_bytes)
    }

    /// Set the logical size of the CHD in bytes ahead of time.
    ///
    /// For uncompressed CHDs, this places the map directly after the header.
    /// Otherwise, the map is written at the end of the file, and the logical size is the
    /// number of bytes written.
    ///
    /// If fewer bytes are written than the logical bytes set, the remaining length is filled with zeroes.
    pub fn logical_bytes(mut self, logical_bytes: u64) -> Self {
        self.logical_bytes = Some(logical_bytes);
        self
    }

    /// Set the compression codecs to use when writing the output CHD.
    ///
    /// To disable compression, pass in the empty slice. This function
    /// accepts up to 4 compression codecs in a slice.
    pub fn compression(mut self, compression: impl ChdWriteCodecs) -> Self {
        self.compression_codecs = compression.into_array_vec();
        self
    }

    /// Enable or disable calculation of SHA1 hashes for uncompressed CHDs.
    /// chdman does not hash uncompressed CHDs, but this can be enabled
    /// here. The output CHDs are still valid but will not match the reference
    /// implementation.
    ///
    /// Only affects uncompressed CHDs.
    pub fn hash_uncompressed(mut self, hash_uncompressed: bool) -> Self {
        self.hash_uncompressed = hash_uncompressed;
        self
    }
}

/// Writes a CHD file.
///
/// Data is written either a hunk at a time with [`write_hunk`](ChdWriter::write_hunk), or as
/// a stream of bytes with [`Write`]. Hunks can also be compressed ahead of time, such as on
/// multiple threads, with a [`HunkCompressor`] from [`compressor`](ChdWriter::compressor),
/// then written in order with [`write_compressed_hunk`](ChdWriter::write_compressed_hunk).
///
/// The CHD must be completed with [`finish`](ChdWriter::finish),
/// otherwise the file is left incomplete.
pub struct ChdWriter<W: Write + Seek> {
    inner: W,
    options: ChdWriteOptions,
    /// The map entry of each hunk written so far.
    entries: Vec<RawMapEntryV5>,
    /// The number of hunks, if the logical size was set.
    hunk_count: Option<u32>,
    /// The bytes of the current hunk, if it is only partially written.
    buffer: Vec<u8>,
    /// The number of bytes written so far.
    written: u64,
    /// The end of the file.
    eof: u64,
    metadata: MetadataList,
    /// The hunks stored so far, shared with compressors.
    state: CompressionState,
    /// The compressor for hunks written with [`write_hunk`](ChdWriter::write_hunk) and [`Write`].
    compressor: HunkCompressor,
    /// The SHA1 of the logical data written so far, if the CHD is hashed.
    raw_sha1: Option<Sha1>,
    /// The number of logical bytes hashed so far.
    hashed: u64,
    /// The SHA1 of the parent CHD, or zeroes if there is no parent.
    parent_sha1: [u8; 20],
    /// The number of bytes of hunk data stored in the file so far.
    stored: u64,
    /// Whether writing failed part way through, leaving the file inconsistent.
    failed: bool,
}

impl<W: Write + Seek> ChdWriter<W> {
    /// Creates a writer for a CHD V5 file.
    ///
    /// Returns [`InvalidParameter`](Error::InvalidParameter) on the following conditions:
    /// - If the hunk size is not a non-zero multiple of the unit size.
    /// - If there would be too many hunks.
    /// - If a codec is not supported by CHD v5.
    ///
    /// If a codec does not support the hunk size, returns [`CodecError`](Error::CodecError).
    pub fn new(inner: W, options: ChdWriteOptions) -> Result<Self> {
        Self::create(inner, options, CompressionState::default(), [0; 20])
    }

    /// Creates a writer for a CHD V5 file that refers to data in a parent CHD.
    ///
    /// The parent will be read in full before writing the new CHD.
    ///
    /// Returns [`InvalidParameter`](Error::InvalidParameter) under the same conditions as
    /// [`new`](ChdWriter::new), as well as the following conditions:
    /// - If the CHD is uncompressed.
    /// - If the hunk or unit sizes are not those of the parent.
    /// - If the parent is older than CHD V3, which has no SHA1.
    pub fn with_parent<F: Read + Seek>(
        inner: W,
        options: ChdWriteOptions,
        parent: &mut Chd<F>,
    ) -> Result<Self> {
        let header = parent.header();
        let Some(parent_sha1) = header.sha1() else {
            return Err(Error::InvalidParameter);
        };
        if options.compression_codecs.is_empty()
            || options.hunk_bytes != header.hunk_size()
            || options.unit_bytes != header.unit_bytes()
            || options.unit_bytes == 0
            || !options.hunk_bytes.is_multiple_of(options.unit_bytes)
        {
            return Err(Error::InvalidParameter);
        }
        let state = CompressionState::with_parent(parent, options.unit_bytes)?;
        Self::create(inner, options, state, parent_sha1)
    }

    fn create(
        inner: W,
        options: ChdWriteOptions,
        state: CompressionState,
        parent_sha1: [u8; 20],
    ) -> Result<Self> {
        if options.unit_bytes == 0
            || options.hunk_bytes == 0
            || !options.hunk_bytes.is_multiple_of(options.unit_bytes)
        {
            return Err(Error::InvalidParameter);
        }

        let hunk_count = options
            .logical_bytes
            .map(|logical_bytes| {
                u32::try_from(logical_bytes.div_ceil(options.hunk_bytes as u64))
                    .map_err(|_| Error::InvalidParameter)
            })
            .transpose()?;

        let compressor = HunkCompressor::new(
            &options.compression_codecs,
            options.hunk_bytes,
            state.clone(),
        )?;
        let compressed = !options.compression_codecs.is_empty();
        let raw_sha1 = (compressed || options.hash_uncompressed).then(Sha1::new);

        let mut writer = ChdWriter {
            inner,
            options,
            entries: Vec::new(),
            hunk_count,
            buffer: Vec::new(),
            written: 0,
            eof: 0,
            metadata: MetadataList::default(),
            state,
            compressor,
            raw_sha1,
            hashed: 0,
            parent_sha1,
            stored: 0,
            failed: false,
        };

        // Write the header first.
        writer.inner.seek(SeekFrom::Start(0))?;
        let logical_bytes = writer.options.logical_bytes.unwrap_or(0);
        let header = writer.header(logical_bytes, 0, 0, [0; 20], [0; 20]);
        writer.inner.write_all(&header.to_bytes())?;
        writer.eof = HEADER_BYTES;

        // The map of an uncompressed CHD is preallocated after the header if we know its size.
        // The map of a compressed CHD is always written at the end.
        if let (Some(hunk_count), false) = (hunk_count, compressed) {
            let map_bytes = hunk_count as u64 * 4;
            io::copy(&mut io::repeat(0).take(map_bytes), &mut writer.inner)?;
            writer.eof += map_bytes;
        }
        Ok(writer)
    }

    /// Create a compressor for hunks to write with
    /// [`write_compressed_hunk`](ChdWriter::write_compressed_hunk).
    ///
    /// The compressor uses the codecs of the CHD, and shares the hunks already written so
    /// duplicate hunks are not compressed again. If the CHD is uncompressed, the compressor
    /// stores every hunk as is.
    pub fn compressor(&self) -> Result<HunkCompressor> {
        HunkCompressor::new(
            &self.options.compression_codecs,
            self.options.hunk_bytes,
            self.state.clone(),
        )
    }

    /// Compresses and writes the next hunk, which must be exactly the hunk size.
    ///
    /// If only part of a hunk was written previously with [`Write`] or otherwise failed partway through,
    /// returns [`InvalidState`](Error::InvalidState).
    ///
    /// If the hunk is not exactly the hunk size, returns [`InvalidParameter`](Error::InvalidParameter).
    /// If the CHD is already as large as its logical size, returns [`HunkOutOfRange`](Error::HunkOutOfRange).
    pub fn write_hunk(&mut self, hunk: &[u8]) -> Result<()> {
        self.check_failed()?;
        if !self.buffer.is_empty() {
            return Err(Error::InvalidState);
        }
        let compressed = self.compressor.compress(hunk)?;
        self.write_compressed_hunk(compressed)
    }

    /// Write the next hunk, precompressed by a compressor from [`compressor`](ChdWriter::compressor).
    ///
    /// If only part of a hunk was written previously with [`Write`], or writing failed part way
    /// through before, returns [`InvalidState`](Error::InvalidState).
    /// If the hunk was compressed by a compressor of another writer, returns [`InvalidParameter`](Error::InvalidParameter).
    /// If the CHD is already as large as its logical size, returns [`HunkOutOfRange`](Error::HunkOutOfRange).
    pub fn write_compressed_hunk(&mut self, compressed: CompressedHunk<'_>) -> Result<()> {
        self.check_failed()?;
        if !self.buffer.is_empty() {
            return Err(Error::InvalidState);
        }
        if !compressed.state.is_same(&self.state) {
            return Err(Error::InvalidParameter);
        }

        let hunk = compressed.hunk;
        self.store_hunk(compressed)?;
        self.hash_logical(hunk);
        self.written += hunk.len() as u64;
        Ok(())
    }

    /// Set the `index`-th metadata entry with the tag, or add a new entry if there is no
    /// such entry. Entries with `checksum` set are included in the SHA1 of the CHD.
    ///
    /// Metadata set before the first hunk is written is placed before the hunks.
    /// Otherwise, it is placed after the hunks.
    ///
    /// Returns [`InvalidParameter`](Error::InvalidParameter) if the tag is 0, or if the
    /// data is empty or 16MB or larger.
    pub fn set_metadata(
        &mut self,
        tag: impl MetadataTag,
        index: u32,
        data: impl MetadataData,
        checksum: bool,
    ) -> Result<()> {
        let flags = if checksum { METADATA_FLAG_CHECKSUM } else { 0 };
        self.metadata
            .set(tag.metatag(), index, data.metadata_bytes(), flags)
    }

    /// Add a new metadata entry after the existing entries.
    ///
    /// See [`set_metadata`](ChdWriter::set_metadata).
    pub fn add_metadata(
        &mut self,
        tag: impl MetadataTag,
        data: impl MetadataData,
        checksum: bool,
    ) -> Result<()> {
        self.set_metadata(tag, u32::MAX, data, checksum)
    }

    /// Remove the `index`-th metadata entry with the tag.
    ///
    /// Returns [`MetadataNotFound`](Error::MetadataNotFound) if there is no such entry.
    pub fn remove_metadata(&mut self, tag: impl MetadataTag, index: u32) -> Result<()> {
        self.metadata.remove(tag.metatag(), index)
    }

    /// The number of bytes of data written to the stream.
    pub fn bytes_written(&self) -> u64 {
        self.written
    }

    /// The number of bytes of hunk data stored in the stream, after compression.
    ///
    /// Duplicate hunks in compressed CHDs, and hunks of zeroes in uncompressed CHDs, are
    /// not stored.
    ///
    /// The current compression ratio can be calculated as
    /// [`bytes_stored`](ChdWriter::bytes_stored) divided by [`bytes_written`](ChdWriter::bytes_written).
    pub fn bytes_stored(&self) -> u64 {
        self.stored
    }

    /// Finish writing the CHD, returning the inner writer.
    ///
    /// Returns [`InvalidState`](Error::InvalidState) if writing failed part way through before.
    pub fn finish(mut self) -> Result<W> {
        self.check_failed()?;
        let hunk_bytes = self.options.hunk_bytes as u64;

        // Pad the last hunk with zeroes. Its data was hashed as it was written.
        if !self.buffer.is_empty() {
            let mut hunk = std::mem::take(&mut self.buffer);
            hunk.resize(hunk_bytes as usize, 0);
            let compressed = self.compressor.compress(&hunk)?;
            self.store_hunk(compressed)?;
        }

        // Whole hunks may extend past the logical size, since the last hunk is padded.
        let logical_bytes = self.options.logical_bytes.unwrap_or(self.written);

        // Hunks that were never written are zeroes.
        let hunk_count = logical_bytes.div_ceil(hunk_bytes);
        let hunk_count = u32::try_from(hunk_count).map_err(|_| Error::InvalidData)?;
        if self.entries.len() < hunk_count as usize {
            let zeroes = vec![0u8; hunk_bytes as usize];
            while self.entries.len() < hunk_count as usize {
                let compressed = self.compressor.compress(&zeroes)?;
                self.store_hunk(compressed)?;
                self.hash_logical(&zeroes);
            }
        }

        // Write metadata that was set after the hunks or no longer fits, and link it all.
        let (meta_offset, eof) = self.metadata.commit(&mut self.inner, self.eof)?;
        self.eof = eof;

        let (raw_sha1, sha1) = match self.raw_sha1.take() {
            Some(raw_sha1) => {
                let raw_sha1 = raw_sha1.finalize().into();
                (raw_sha1, self.metadata.overall_sha1(raw_sha1))
            }
            None => ([0; 20], [0; 20]),
        };

        // The map of a compressed CHD is written at the end. The map of an uncompressed CHD
        // is either preallocated after the header, or written at the end.
        let (map_offset, map) = if self.is_compressed() {
            let entries = std::mem::take(&mut self.entries);
            (self.eof, Map::new_compressed_v5(entries))
        } else {
            let offsets: Vec<u32> = self
                .entries
                .iter()
                .map(|entry| (entry.offset() / hunk_bytes) as u32)
                .collect();
            let map_offset = if self.hunk_count.is_some() {
                HEADER_BYTES
            } else {
                self.eof
            };
            (
                map_offset,
                Map::new_uncompressed_v5(&offsets, hunk_bytes as u32),
            )
        };

        let header = self.header(logical_bytes, map_offset, meta_offset, raw_sha1, sha1);
        let map_bytes = map.to_bytes(&Header::V5Header(header.clone()))?;

        self.inner.seek(SeekFrom::Start(map_offset))?;
        self.inner.write_all(&map_bytes)?;
        self.inner.seek(SeekFrom::Start(0))?;
        self.inner.write_all(&header.to_bytes())?;
        self.inner.flush()?;
        Ok(self.inner)
    }

    /// Whether the CHD is compressed.
    fn is_compressed(&self) -> bool {
        !self.options.compression_codecs.is_empty()
    }

    /// Add logical data to the raw SHA1, up to the logical size of the CHD.
    fn hash_logical(&mut self, data: &[u8]) {
        if let Some(raw_sha1) = &mut self.raw_sha1 {
            let remaining = self
                .options
                .logical_bytes
                .map_or(u64::MAX, |logical_bytes| {
                    logical_bytes.saturating_sub(self.hashed)
                });
            let len = (data.len() as u64).min(remaining) as usize;
            raw_sha1.update(&data[..len]);
            self.hashed += len as u64;
        }
    }

    /// Store a compressed hunk in the file, and add it to the map.
    fn store_hunk(&mut self, compressed: CompressedHunk<'_>) -> Result<()> {
        let hunk_num = self.entries.len();
        if self
            .hunk_count
            .is_some_and(|hunk_count| hunk_num >= hunk_count as usize)
        {
            return Err(Error::HunkOutOfRange);
        }
        let hunk_num = u32::try_from(hunk_num).map_err(|_| Error::HunkOutOfRange)?;

        // A failure from here on can leave the file and map inconsistent.
        let result = self.store_hunk_unchecked(hunk_num, compressed);
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn store_hunk_unchecked(
        &mut self,
        hunk_num: u32,
        compressed: CompressedHunk<'_>,
    ) -> Result<()> {
        // Metadata set before the first hunk is placed before the hunks, like chdman.
        if hunk_num == 0 {
            self.eof = self.metadata.write_unwritten(&mut self.inner, self.eof)?;
        }

        let entry = if self.is_compressed() {
            self.store_compressed(hunk_num, compressed)?
        } else {
            self.store_uncompressed(compressed.hunk, compressed.hash.crc16)?
        };
        self.entries.push(entry);
        Ok(())
    }

    /// Returns [`InvalidState`](Error::InvalidState) if writing failed part way through before.
    fn check_failed(&self) -> Result<()> {
        if self.failed {
            return Err(Error::InvalidState);
        }
        Ok(())
    }

    /// Store a hunk of a compressed CHD, appending its data unaligned like,
    /// or referring to an identical hunk.
    fn store_compressed(
        &mut self,
        hunk_num: u32,
        compressed: CompressedHunk<'_>,
    ) -> Result<RawMapEntryV5> {
        // The writer decides whether a hunk is a duplicate, since the compressor may not
        // have seen a hunk stored after it started compressing. Like chdman, hunks in the
        // CHD itself are preferred over the parent.
        if let Some(other) = self.state.find(&compressed.hash) {
            return Ok(RawMapEntryV5::new(
                CompressionTypeV5::CompressionSelf as u8,
                0,
                other as u64,
                0,
            ));
        }
        if let Some(unit) = self.state.find_parent(&compressed.hash) {
            return Ok(RawMapEntryV5::new(
                CompressionTypeV5::CompressionParent as u8,
                0,
                unit,
                0,
            ));
        }

        let (compression, data) = match compressed.kind {
            HunkKind::Compressed(index) => (index, compressed.data.as_slice()),
            HunkKind::Uncompressed => (CompressionTypeV5::CompressionNone as u8, compressed.hunk),
            // Every hunk a compressor finds is also found by the writer.
            HunkKind::Duplicate => return Err(Error::InvalidData),
        };

        let offset = self.eof;
        self.inner.seek(SeekFrom::Start(offset))?;
        self.inner.write_all(data)?;
        self.eof += data.len() as u64;
        self.stored += data.len() as u64;
        self.state.insert(compressed.hash, hunk_num);
        Ok(RawMapEntryV5::new(
            compression,
            data.len() as u32,
            offset,
            compressed.hash.crc16,
        ))
    }

    /// Store a hunk of an uncompressed CHD.
    fn store_uncompressed(&mut self, hunk: &[u8], crc16: u16) -> Result<RawMapEntryV5> {
        let hunk_bytes = self.options.hunk_bytes as u64;
        let none = CompressionTypeV5::CompressionNone as u8;

        // Hunks of zeroes are not stored, and have an offset of 0.
        if hunk.iter().all(|&byte| byte == 0) {
            return Ok(RawMapEntryV5::new(none, 0, 0, crc16));
        }

        // Otherwise, append the hunk aligned to the hunk size, padding with zeroes.
        let offset = self.eof.next_multiple_of(hunk_bytes);
        u32::try_from(offset / hunk_bytes).map_err(|_| Error::InvalidData)?;
        self.inner.seek(SeekFrom::Start(self.eof))?;
        io::copy(&mut io::repeat(0).take(offset - self.eof), &mut self.inner)?;
        self.inner.write_all(hunk)?;
        self.eof = offset + hunk_bytes;
        self.stored += hunk_bytes;
        Ok(RawMapEntryV5::new(none, hunk_bytes as u32, offset, crc16))
    }

    /// The header for a CHD of the given logical size.
    fn header(
        &self,
        logical_bytes: u64,
        map_offset: u64,
        meta_offset: u64,
        raw_sha1: [u8; 20],
        sha1: [u8; 20],
    ) -> HeaderV5 {
        let ChdWriteOptions {
            hunk_bytes,
            unit_bytes,
            ..
        } = self.options;

        let mut compression = [0u32; 4];
        for (slot, codec) in compression.iter_mut().zip(&self.options.compression_codecs) {
            *slot = *codec as u32;
        }

        HeaderV5 {
            version: Version::ChdV5,
            length: HEADER_BYTES as u32,
            compression,
            logical_bytes,
            map_offset,
            meta_offset,
            hunk_bytes,
            unit_bytes,
            sha1,
            parent_sha1: self.parent_sha1,
            raw_sha1,
            unit_count: logical_bytes.div_ceil(unit_bytes as u64),
            hunk_count: logical_bytes.div_ceil(hunk_bytes as u64) as u32,
            map_entry_bytes: if self.is_compressed() { 12 } else { 4 },
        }
    }
}

impl<W: Write + Seek> Write for ChdWriter<W> {
    /// Writes up to the logical size of the CHD, if it was set.
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.check_failed()?;
        let hunk_bytes = self.options.hunk_bytes as usize;
        let remaining = self
            .options
            .logical_bytes
            .map_or(u64::MAX, |logical_bytes| {
                logical_bytes.saturating_sub(self.written)
            });
        let len = buf
            .len()
            .min(hunk_bytes - self.buffer.len())
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        self.buffer.extend_from_slice(&buf[..len]);
        self.hash_logical(&buf[..len]);
        self.written += len as u64;

        if self.buffer.len() == hunk_bytes {
            let hunk = std::mem::take(&mut self.buffer);
            let compressed = self.compressor.compress(&hunk)?;
            self.store_hunk(compressed)?;
            self.buffer = hunk;
            self.buffer.clear();
        }
        Ok(len)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::{ChdWriteOptions, ChdWriter};
    use crate::map::MapEntry;
    use crate::{Chd, Error};
    use std::io::{Cursor, Write};

    const HUNK_BYTES: u32 = 4096;
    const UNIT_BYTES: u32 = 512;

    /// Data with zero hunks, a repeated hunk, and a partial last hunk.
    fn data() -> Vec<u8> {
        let mut data = Vec::new();
        for hunk in 0..6u8 {
            let fill = if hunk == 1 || hunk == 4 { 0 } else { hunk + 1 };
            data.extend(std::iter::repeat_n(fill, HUNK_BYTES as usize));
        }
        data.extend(std::iter::repeat_n(9u8, 1000));
        data
    }

    fn write(options: ChdWriteOptions, data: &[u8]) -> Vec<u8> {
        let mut writer = ChdWriter::new(Cursor::new(Vec::new()), options).unwrap();
        // Write in uneven pieces to exercise buffering.
        for chunk in data.chunks(1500) {
            writer.write_all(chunk).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    /// Read all the hunks of a CHD, returning the data and the map offsets.
    fn read(chd: Vec<u8>) -> (Vec<u8>, u64, Vec<u64>) {
        let mut chd = Chd::open(Cursor::new(chd), None).unwrap();
        let logical_bytes = chd.header().logical_bytes();
        let offsets = chd
            .map()
            .iter()
            .map(|entry| match entry {
                MapEntry::V5Uncompressed(entry) => entry.block_offset(),
                _ => panic!("not an uncompressed V5 map"),
            })
            .collect();

        let mut data = Vec::new();
        let mut hunk = chd.get_hunksized_buffer();
        let mut compressed = Vec::new();
        for hunk_num in 0..chd.header().hunk_count() {
            chd.hunk(hunk_num)
                .unwrap()
                .read_hunk_in(&mut compressed, &mut hunk)
                .unwrap();
            data.extend_from_slice(&hunk);
        }
        (data, logical_bytes, offsets)
    }

    #[test]
    fn roundtrip() {
        let data = data();
        for options in [
            ChdWriteOptions::new(HUNK_BYTES, UNIT_BYTES),
            ChdWriteOptions::new(HUNK_BYTES, UNIT_BYTES).logical_bytes(data.len() as u64),
        ] {
            let preallocated = options.logical_bytes.is_some();
            let (read, logical_bytes, offsets) = read(write(options, &data));
            assert_eq!(logical_bytes, data.len() as u64);
            assert_eq!(&read[..data.len()], &data[..]);
            assert!(read[data.len()..].iter().all(|&byte| byte == 0));

            // Zero hunks are not stored, and stored hunks are aligned to the hunk size.
            assert_eq!(offsets.len(), 7);
            assert_eq!((offsets[1], offsets[4]), (0, 0));
            assert!(offsets
                .iter()
                .all(|&offset| offset % HUNK_BYTES as u64 == 0));
            if preallocated {
                // The map follows the header, so the first hunk is aligned after it.
                assert_eq!(offsets[0], HUNK_BYTES as u64);
            }
        }
    }

    #[test]
    fn unwritten_hunks_are_zero() {
        let data = vec![7u8; HUNK_BYTES as usize];
        let options =
            ChdWriteOptions::new(HUNK_BYTES, UNIT_BYTES).logical_bytes(4 * HUNK_BYTES as u64);
        let (read, logical_bytes, offsets) = read(write(options, &data));
        assert_eq!(logical_bytes, 4 * HUNK_BYTES as u64);
        assert_eq!(&read[..data.len()], &data[..]);
        assert!(read[data.len()..].iter().all(|&byte| byte == 0));
        assert_eq!(&offsets[1..], &[0, 0, 0]);
    }

    #[test]
    fn write_hunks() {
        let data = data();
        let mut writer = ChdWriter::new(
            Cursor::new(Vec::new()),
            ChdWriteOptions::new(HUNK_BYTES, UNIT_BYTES),
        )
        .unwrap();
        for hunk in data.chunks_exact(HUNK_BYTES as usize) {
            writer.write_hunk(hunk).unwrap();
        }
        let (read, logical_bytes, _) = read(writer.finish().unwrap().into_inner());
        assert_eq!(logical_bytes, 6 * HUNK_BYTES as u64);
        assert_eq!(&read[..], &data[..6 * HUNK_BYTES as usize]);
    }

    #[test]
    fn invalid_options() {
        for (hunk_bytes, unit_bytes) in [(0, 512), (4096, 0), (4096, 1000)] {
            assert!(matches!(
                ChdWriter::new(
                    Cursor::new(Vec::new()),
                    ChdWriteOptions::new(hunk_bytes, unit_bytes)
                ),
                Err(Error::InvalidParameter)
            ));
        }
    }

    #[test]
    fn invalid_writes() {
        let options = ChdWriteOptions::new(HUNK_BYTES, UNIT_BYTES);
        let mut writer = ChdWriter::new(Cursor::new(Vec::new()), options.clone()).unwrap();
        assert!(matches!(
            writer.write_hunk(&[0; 100]),
            Err(Error::InvalidParameter)
        ));

        // Hunks can not be written in the middle of a partially written hunk.
        writer.write_all(&[1; 100]).unwrap();
        assert!(matches!(
            writer.write_hunk(&[0; HUNK_BYTES as usize]),
            Err(Error::InvalidState)
        ));

        // No more than the logical size can be written.
        let mut writer = ChdWriter::new(
            Cursor::new(Vec::new()),
            options.logical_bytes(HUNK_BYTES as u64),
        )
        .unwrap();
        writer.write_hunk(&[1; HUNK_BYTES as usize]).unwrap();
        assert!(matches!(
            writer.write_hunk(&[1; HUNK_BYTES as usize]),
            Err(Error::HunkOutOfRange)
        ));
    }
}

#[cfg(test)]
mod compressed_tests {
    use super::{ChdWriteOptions, ChdWriter};
    use crate::header::{CodecType, Header};
    use crate::map::{CompressionTypeV5, MapEntry};
    use crate::{Chd, Error};
    use sha1::{Digest, Sha1};
    use std::io::{Cursor, Write};

    const HUNK_BYTES: u32 = 4096;
    const UNIT_BYTES: u32 = 512;
    const TAG: u32 = crate::make_tag(b"TEST");

    fn options() -> ChdWriteOptions {
        ChdWriteOptions::new(HUNK_BYTES, UNIT_BYTES)
            .compression(&[CodecType::ZLibV5, CodecType::LzmaV5])
    }

    fn text_hunk(seed: u8) -> Vec<u8> {
        format!("hunk {seed} says hello. ")
            .into_bytes()
            .into_iter()
            .cycle()
            .take(HUNK_BYTES as usize)
            .collect()
    }

    fn random_hunk(seed: u64) -> Vec<u8> {
        use rand::rngs::StdRng;
        use rand::{RngExt, SeedableRng};
        let mut hunk = vec![0u8; HUNK_BYTES as usize];
        StdRng::seed_from_u64(seed).fill(&mut hunk[..]);
        hunk
    }

    /// Text, zeroes, a repeat of the first hunk, more zeroes, random data, and a partial hunk.
    fn data() -> Vec<u8> {
        let zeroes = vec![0u8; HUNK_BYTES as usize];
        let mut data = Vec::new();
        for hunk in [
            text_hunk(1),
            zeroes.clone(),
            text_hunk(1),
            zeroes,
            random_hunk(7),
        ] {
            data.extend(hunk);
        }
        data.extend(std::iter::repeat_n(5u8, 1000));
        data
    }

    struct ReadChd {
        data: Vec<u8>,
        header: crate::header::HeaderV5,
        /// The compression type and offset of each map entry.
        entries: Vec<(u8, u64)>,
    }

    fn read(chd: Vec<u8>) -> ReadChd {
        let mut chd = Chd::open(Cursor::new(chd), None).unwrap();
        let Header::V5Header(header) = chd.header().clone() else {
            panic!("not a V5 header");
        };
        let entries = chd
            .map()
            .iter()
            .map(|entry| match entry {
                MapEntry::V5Compressed(entry) => (
                    num_traits::ToPrimitive::to_u8(&entry.hunk_type().unwrap()).unwrap(),
                    entry.block_offset(),
                ),
                _ => panic!("not a compressed V5 map"),
            })
            .collect();

        let mut data = Vec::new();
        let mut hunk = chd.get_hunksized_buffer();
        for hunk_num in 0..chd.header().hunk_count() {
            chd.hunk(hunk_num)
                .unwrap()
                .read_hunk_in(&mut Vec::new(), &mut hunk)
                .unwrap();
            data.extend_from_slice(&hunk);
        }
        ReadChd {
            data,
            header,
            entries,
        }
    }

    /// The SHA1 of a CHD with one checksummed metadata entry.
    fn overall_sha1(raw_sha1: [u8; 20], tag: u32, value: &[u8]) -> [u8; 20] {
        let mut sha1 = Sha1::new();
        sha1.update(raw_sha1);
        sha1.update(tag.to_be_bytes());
        sha1.update(Sha1::digest(value));
        sha1.finalize().into()
    }

    fn write_hunks(options: ChdWriteOptions, data: &[u8]) -> Vec<u8> {
        let mut writer = ChdWriter::new(Cursor::new(Vec::new()), options).unwrap();
        for hunk in data.chunks_exact(HUNK_BYTES as usize) {
            writer.write_hunk(hunk).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn write_compressed_hunks(options: ChdWriteOptions, data: &[u8]) -> Vec<u8> {
        let mut writer = ChdWriter::new(Cursor::new(Vec::new()), options).unwrap();
        let mut compressor = writer.compressor().unwrap();
        for hunk in data.chunks_exact(HUNK_BYTES as usize) {
            let compressed = compressor.compress(hunk).unwrap();
            writer.write_compressed_hunk(compressed).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn compressed_roundtrip() {
        let data = data();
        let mut writer = ChdWriter::new(Cursor::new(Vec::new()), options()).unwrap();
        writer
            .add_metadata(TAG, b"checked".as_slice(), true)
            .unwrap();
        writer
            .add_metadata(TAG, b"unchecked".as_slice(), false)
            .unwrap();
        for chunk in data.chunks(1500) {
            writer.write_all(chunk).unwrap();
        }
        let read = read(writer.finish().unwrap().into_inner());

        assert_eq!(read.header.logical_bytes, data.len() as u64);
        assert_eq!(&read.data[..data.len()], &data[..]);
        assert!(read.data[data.len()..].iter().all(|&byte| byte == 0));

        // The raw SHA1 covers the logical data, and the SHA1 the checksummed metadata.
        let raw_sha1: [u8; 20] = Sha1::digest(&data).into();
        assert_eq!(read.header.raw_sha1, raw_sha1);
        assert_eq!(read.header.sha1, overall_sha1(raw_sha1, TAG, b"checked"));
        assert_eq!(read.header.compression[0], CodecType::ZLibV5 as u32);
        assert_eq!(read.header.compression[2], 0);

        // Repeats refer to the first copy, and random data is stored as is.
        let self_ref = CompressionTypeV5::CompressionSelf as u8;
        let none = CompressionTypeV5::CompressionNone as u8;
        assert_eq!(read.entries[2], (self_ref, 0));
        assert_eq!(read.entries[3], (self_ref, 1));
        assert_eq!(read.entries[4].0, none);
        for hunk in [0, 1, 5] {
            assert!(read.entries[hunk].0 <= 1, "hunk {hunk} is not compressed");
        }
    }

    #[test]
    fn unwritten_hunks_refer_to_the_first_zero_hunk() {
        let options = options().logical_bytes(5 * HUNK_BYTES as u64);
        let mut writer = ChdWriter::new(Cursor::new(Vec::new()), options).unwrap();
        writer.write_hunk(&text_hunk(2)).unwrap();
        let read = read(writer.finish().unwrap().into_inner());

        let mut expected = text_hunk(2);
        expected.resize(5 * HUNK_BYTES as usize, 0);
        assert_eq!(read.data, expected);
        let raw_sha1: [u8; 20] = Sha1::digest(&expected).into();
        assert_eq!(read.header.raw_sha1, raw_sha1);

        let self_ref = CompressionTypeV5::CompressionSelf as u8;
        assert_eq!(&read.entries[2..], &[(self_ref, 1); 3]);
    }

    #[test]
    fn write_compressed_hunks_matches_write_hunk() {
        let data = data();
        assert_eq!(
            write_compressed_hunks(options(), &data),
            write_hunks(options(), &data)
        );
    }

    #[test]
    fn duplicates_compressed_before_the_original_is_written() {
        let mut writer = ChdWriter::new(Cursor::new(Vec::new()), options()).unwrap();
        let mut compressor = writer.compressor().unwrap();
        let hunk = text_hunk(3);

        // Both are compressed before either is written, so the compressor does not know the
        // second is a duplicate, but the writer does.
        let first = compressor.compress(&hunk).unwrap();
        let second = compressor.compress(&hunk).unwrap();
        writer.write_compressed_hunk(first).unwrap();
        writer.write_compressed_hunk(second).unwrap();

        let read = read(writer.finish().unwrap().into_inner());
        assert_eq!(
            read.entries[1],
            (CompressionTypeV5::CompressionSelf as u8, 0)
        );
    }

    #[test]
    fn compressed_hunks_from_another_writer() {
        let mut writer = ChdWriter::new(Cursor::new(Vec::new()), options()).unwrap();
        let other = ChdWriter::new(Cursor::new(Vec::new()), options()).unwrap();
        let hunk = text_hunk(4);
        let compressed = other.compressor().unwrap().compress(&hunk).unwrap();
        assert!(matches!(
            writer.write_compressed_hunk(compressed),
            Err(Error::InvalidParameter)
        ));
    }

    #[test]
    fn hash_uncompressed() {
        let data = data();
        for hash_uncompressed in [false, true] {
            let options =
                ChdWriteOptions::new(HUNK_BYTES, UNIT_BYTES).hash_uncompressed(hash_uncompressed);
            let mut writer = ChdWriter::new(Cursor::new(Vec::new()), options).unwrap();
            writer
                .add_metadata(TAG, b"checked".as_slice(), true)
                .unwrap();
            writer.write_all(&data).unwrap();
            let chd = writer.finish().unwrap().into_inner();
            let chd = Chd::open(Cursor::new(chd), None).unwrap();
            let Header::V5Header(header) = chd.header() else {
                panic!("not a V5 header");
            };

            if hash_uncompressed {
                let raw_sha1: [u8; 20] = Sha1::digest(&data).into();
                assert_eq!(header.raw_sha1, raw_sha1);
                assert_eq!(header.sha1, overall_sha1(raw_sha1, TAG, b"checked"));
            } else {
                assert_eq!((header.raw_sha1, header.sha1), ([0; 20], [0; 20]));
            }
        }
    }

    #[test]
    fn uncompressed_through_compressor() {
        // The same path works for uncompressed CHDs, storing every hunk as is.
        let data = data();
        let options = ChdWriteOptions::new(HUNK_BYTES, UNIT_BYTES);
        assert_eq!(
            write_compressed_hunks(options.clone(), &data),
            write_hunks(options, &data)
        );
    }
}

#[cfg(test)]
mod logical_size_tests {
    use super::{ChdWriteOptions, ChdWriter};
    use crate::header::CodecType;
    use crate::Chd;
    use std::io::{Cursor, ErrorKind, Write};

    const HUNK_BYTES: u32 = 4096;

    #[test]
    fn whole_hunks_can_pad_past_the_logical_size() {
        for codecs in [&[][..], &[CodecType::ZLibV5][..]] {
            let mut options = ChdWriteOptions::new(HUNK_BYTES, 512).logical_bytes(5000);
            if !codecs.is_empty() {
                options = options.compression(&[CodecType::ZLibV5]);
            }
            let mut writer = ChdWriter::new(Cursor::new(Vec::new()), options).unwrap();
            writer.write_hunk(&[1; HUNK_BYTES as usize]).unwrap();
            writer.write_hunk(&[2; HUNK_BYTES as usize]).unwrap();
            let chd = Chd::open(writer.finish().unwrap(), None).unwrap();
            assert_eq!(chd.header().logical_bytes(), 5000);
        }
    }

    #[test]
    fn write_stops_at_the_logical_size() {
        let options = ChdWriteOptions::new(HUNK_BYTES, 512).logical_bytes(5000);
        let mut writer = ChdWriter::new(Cursor::new(Vec::new()), options).unwrap();
        let error = writer.write_all(&[1; 6000]).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::WriteZero);
        let chd = Chd::open(writer.finish().unwrap(), None).unwrap();
        assert_eq!(chd.header().logical_bytes(), 5000);
    }
}

#[cfg(test)]
mod failure_tests {
    use super::{ChdWriteOptions, ChdWriter};
    use crate::header::CodecType;
    use crate::{Chd, Error};
    use std::cell::Cell;
    use std::io::{self, Cursor, Seek, SeekFrom, Write};
    use std::rc::Rc;

    const HUNK_BYTES: u32 = 4096;

    /// A writer that fails while `fail` is set.
    struct FailingWriter {
        inner: Cursor<Vec<u8>>,
        fail: Rc<Cell<bool>>,
    }

    impl Write for FailingWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.fail.get() {
                return Err(io::Error::other("failed"));
            }
            self.inner.write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.inner.flush()
        }
    }

    impl Seek for FailingWriter {
        fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
            self.inner.seek(pos)
        }
    }

    fn failing_writer(codec: Option<CodecType>) -> (ChdWriter<FailingWriter>, Rc<Cell<bool>>) {
        let fail = Rc::new(Cell::new(false));
        let inner = FailingWriter {
            inner: Cursor::new(Vec::new()),
            fail: fail.clone(),
        };
        let mut options = ChdWriteOptions::new(HUNK_BYTES, 512);
        if let Some(codec) = codec {
            options = options.compression(&[codec]);
        }
        (ChdWriter::new(inner, options).unwrap(), fail)
    }

    #[test]
    fn io_errors_poison_the_writer() {
        for codec in [None, Some(CodecType::ZLibV5)] {
            let (mut writer, fail) = failing_writer(codec);
            writer.write_hunk(&[1; HUNK_BYTES as usize]).unwrap();

            fail.set(true);
            let error = writer.write_hunk(&[2; HUNK_BYTES as usize]).unwrap_err();
            assert!(!matches!(error, Error::InvalidState));

            // Even once the inner writer works again, the file may be inconsistent.
            fail.set(false);
            assert!(matches!(
                writer.write_hunk(&[3; HUNK_BYTES as usize]),
                Err(Error::InvalidState)
            ));
            let mut compressor = writer.compressor().unwrap();
            let compressed = compressor.compress(&[3; HUNK_BYTES as usize]).unwrap();
            assert!(matches!(
                writer.write_compressed_hunk(compressed),
                Err(Error::InvalidState)
            ));
            assert!(writer.write(&[3; 10]).is_err());
            assert!(matches!(writer.finish(), Err(Error::InvalidState)));
        }
    }

    #[test]
    fn invalid_hunks_do_not_poison_the_writer() {
        let (mut writer, _) = failing_writer(Some(CodecType::ZLibV5));
        let (other, _) = failing_writer(Some(CodecType::ZLibV5));

        assert!(matches!(
            writer.write_hunk(&[1; 10]),
            Err(Error::InvalidParameter)
        ));
        let compressed = other
            .compressor()
            .unwrap()
            .compress(&[1; HUNK_BYTES as usize])
            .unwrap();
        assert!(matches!(
            writer.write_compressed_hunk(compressed),
            Err(Error::InvalidParameter)
        ));

        // A partial hunk written with Write blocks whole hunks until it is completed.
        writer.write_all(&[1; 10]).unwrap();
        assert!(matches!(
            writer.write_hunk(&[1; HUNK_BYTES as usize]),
            Err(Error::InvalidState)
        ));
        writer.write_all(&[1; HUNK_BYTES as usize - 10]).unwrap();
        writer.write_hunk(&[2; HUNK_BYTES as usize]).unwrap();

        let chd = Chd::open(writer.finish().unwrap().inner, None).unwrap();
        assert_eq!(chd.header().logical_bytes(), 2 * HUNK_BYTES as u64);
    }

    #[test]
    fn progress() {
        let hunk = |byte| vec![byte; HUNK_BYTES as usize];

        // Uncompressed CHDs store every hunk but zeroes.
        let (mut writer, _) = failing_writer(None);
        for byte in [1, 0, 1] {
            writer.write_hunk(&hunk(byte)).unwrap();
        }
        writer.write_all(&[1; 100]).unwrap();
        assert_eq!(writer.bytes_written(), 3 * HUNK_BYTES as u64 + 100);
        assert_eq!(writer.bytes_stored(), 2 * HUNK_BYTES as u64);

        // Compressed CHDs store compressed hunks, but not duplicates.
        let (mut writer, _) = failing_writer(Some(CodecType::ZLibV5));
        let mut compressor = writer.compressor().unwrap();
        let mut stored = 0;
        for byte in [1, 2, 1] {
            let hunk = hunk(byte);
            let compressed = compressor.compress(&hunk).unwrap();
            stored += compressed.len() as u64;
            writer.write_compressed_hunk(compressed).unwrap();
        }
        assert_eq!(writer.bytes_written(), 3 * HUNK_BYTES as u64);
        assert_eq!(writer.bytes_stored(), stored);
        assert!(stored > 0 && stored < 2 * HUNK_BYTES as u64);
    }
}

#[cfg(test)]
mod parent_tests {
    use super::{ChdWriteOptions, ChdWriter};
    use crate::header::{CodecType, Header};
    use crate::map::{CompressionTypeV5, MapEntry};
    use crate::read::ChdReader;
    use crate::{Chd, Error};
    use rand::rngs::StdRng;
    use rand::{RngExt, SeedableRng};
    use std::io::{Cursor, Write};

    const HUNK_BYTES: u32 = 4096;
    const UNIT_BYTES: u32 = 512;

    fn options() -> ChdWriteOptions {
        ChdWriteOptions::new(HUNK_BYTES, UNIT_BYTES).compression(&[CodecType::ZLibV5])
    }

    fn random(len: usize, seed: u64) -> Vec<u8> {
        let mut data = vec![0u8; len];
        StdRng::seed_from_u64(seed).fill(&mut data[..]);
        data
    }

    fn parent(data: &[u8]) -> Chd<Cursor<Vec<u8>>> {
        let mut writer = ChdWriter::new(Cursor::new(Vec::new()), options()).unwrap();
        writer.write_all(data).unwrap();
        Chd::open(writer.finish().unwrap(), None).unwrap()
    }

    /// The compression type and offset of each map entry.
    fn entries(chd: &Chd<Cursor<Vec<u8>>>) -> Vec<(u8, u64)> {
        chd.map()
            .iter()
            .map(|entry| match entry {
                MapEntry::V5Compressed(entry) => (
                    num_traits::ToPrimitive::to_u8(&entry.hunk_type().unwrap()).unwrap(),
                    entry.block_offset(),
                ),
                _ => panic!("not a compressed V5 map"),
            })
            .collect()
    }

    #[test]
    fn hunks_refer_to_the_parent() {
        let hunk = HUNK_BYTES as usize;
        let parent_data = random(6 * hunk, 1);
        let mut parent = parent(&parent_data);
        let parent_sha1 = parent.header().sha1().unwrap();

        // Aligned and unaligned runs of the parent, new data, and a repeat of the new data.
        let unit = UNIT_BYTES as usize;
        let new = random(hunk, 2);
        let mut data = Vec::new();
        data.extend_from_slice(&parent_data[..hunk]);
        data.extend_from_slice(&new);
        data.extend_from_slice(&parent_data[3 * hunk..4 * hunk]);
        data.extend_from_slice(&parent_data[unit..unit + hunk]);
        data.extend_from_slice(&parent_data[4 * hunk + 3 * unit..5 * hunk + 3 * unit]);
        data.extend_from_slice(&new);
        data.extend_from_slice(&parent_data[5 * hunk..]);

        let mut writer =
            ChdWriter::with_parent(Cursor::new(Vec::new()), options(), &mut parent).unwrap();
        writer.write_all(&data).unwrap();
        let child = Chd::open(writer.finish().unwrap(), Some(Box::new(parent))).unwrap();

        let units_per_hunk = (HUNK_BYTES / UNIT_BYTES) as u64;
        let parent_ref = CompressionTypeV5::CompressionParent as u8;
        let self_ref = CompressionTypeV5::CompressionSelf as u8;
        let entries = entries(&child);
        assert_eq!(entries[0], (parent_ref, 0));
        assert!(entries[1].0 != parent_ref && entries[1].0 != self_ref);
        assert_eq!(entries[2], (parent_ref, 3 * units_per_hunk));
        assert_eq!(entries[3], (parent_ref, 1));
        assert_eq!(entries[4], (parent_ref, 4 * units_per_hunk + 3));
        assert_eq!(entries[5], (self_ref, 1));
        assert_eq!(entries[6], (parent_ref, 5 * units_per_hunk));

        let Header::V5Header(header) = child.header().clone() else {
            panic!("not a V5 header");
        };
        assert_eq!(header.parent_sha1, parent_sha1);
        assert_ne!(parent_sha1, [0; 20]);

        let mut read = Vec::new();
        std::io::Read::read_to_end(&mut ChdReader::new(child), &mut read).unwrap();
        assert!(read == data);
    }

    #[test]
    fn parent_must_match() {
        let mut parent = parent(&random(2 * HUNK_BYTES as usize, 1));
        for options in [
            ChdWriteOptions::new(HUNK_BYTES, UNIT_BYTES),
            ChdWriteOptions::new(2 * HUNK_BYTES, UNIT_BYTES).compression(&[CodecType::ZLibV5]),
            ChdWriteOptions::new(HUNK_BYTES, 2 * UNIT_BYTES).compression(&[CodecType::ZLibV5]),
        ] {
            assert!(matches!(
                ChdWriter::with_parent(Cursor::new(Vec::new()), options, &mut parent),
                Err(Error::InvalidParameter)
            ));
        }
    }
}
