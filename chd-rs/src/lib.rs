#![cfg_attr(docsrs, feature(doc_cfg, doc_cfg_hide))]
#![forbid(unsafe_code)]
#![warn(missing_docs)]
//! An implementation of the MAME CHD (Compressed Hunks of Data) format in pure Safe Rust, with support
//! for CHD V1-5.
//!
//! ## Supported Compression Codecs
//! chd-rs supports the following compression codecs.
//!
//! * None
//! * Zlib/Zlib+/Zlib V5
//! * CDZL (CD Zlib)
//! * CDLZ (CD LZMA)
//! * CDFL (CD FLAC)
//! * FLAC (Raw FLAC)
//! * LZMA (Raw LZMA)
//! * Huff (MAME Static Huffman)
//! * AVHU (AV Huffman)
//!
//! ## Iterating over hunks
//! Because [`ChdHunk`](crate::ChdHunk) keeps a mutable reference to its owning
//! [`ChdFile`](crate::ChdFile), direct iteration of hunks is not possible without
//! Generic Associated Types. Instead, the hunk indices should be iterated over.
//!
//! ```rust
//! use std::fs::File;
//! use std::io::BufReader;
//! use chd::ChdFile;
//!
//! let mut f = BufReader::new(File::open("file.chd")?);
//! let mut chd = ChdFile::open(&mut f, None)?;
//! let hunk_count = chd.header().hunk_count();
//! let hunk_size = chd.header().hunk_size();
//!
//! // buffer to store uncompressed hunk data must be the same length as the hunk size.
//! let mut hunk_buf = chd.get_hunksized_buffer();
//! // buffer to store compressed data.
//! let mut cmp_buf = Vec::new();
//!
//! for hunk_num in 0..hunk_count {
//!     let mut hunk = chd.hunk(hunk_num)?;
//!     hunk.read_hunk_in(&mut cmp_buf, &mut hunk_buf)?;
//! }
//! ```
//!
//! ## Iterating over metadata
//! Metadata in a CHD file consists of a list of entries that contain offsets to the
//! byte data of the metadata contents in the CHD file. The individual metadata entries
//! can be iterated directly, but a reference to the source stream has to be provided to
//! read the data.
//! ```rust
//! use std::fs::File;
//! use std::io::BufReader;
//! use chd::ChdFile;
//!
//! let mut f = BufReader::new(File::open("file.chd")?);
//! let mut chd = ChdFile::open(&mut f, None)?;
//! let entries = chd.metadata_refs()?;
//! for entry in entries {
//!     let metadata = entry.read(&mut f)?;
//! }
//!```
//! chd-rs provides a helper to retrieve all metadata content at once for convenience.
//! ```rust
//! use std::fs::File;
//! use std::io::BufReader;
//! use chd::ChdFile;
//! use chd::metadata::ChdMetadata;
//!
//! let mut f = BufReader::new(File::open("file.chd")?);
//! let mut chd = ChdFile::open(&mut f, None)?;
//! let entries = chd.metadata_refs()?;
//! let metadatas: Vec<ChdMetadata> = entries.try_into()?;
//!```
mod error;

mod block_hash;
mod cdrom;
mod chdfile;
mod compression;

#[cfg(feature = "huffman_api")]
pub mod huffman;

#[cfg(not(feature = "huffman_api"))]
mod huffman;

#[cfg(feature = "codec_api")]
/// Implementations of decompression codecs used in MAME CHD.
///
/// Each codec may have restrictions on the hunk size, lengths and contents
/// of the buffer. If [`decompress`](crate::codecs::CodecImplementation::decompress) is called
/// with buffers that do not satisfy the constraints, it may return [`CompressionError`](crate::ChdError),
/// or panic, especially if the output buffer does not satisfy length requirements.
///
/// Because codecs are allowed to be used outside of a hunk-sized granularity, such as in
/// CD-ROM wrapped codecs that use Deflate to decompress subcode data, the codec implementations
/// do not check the length of the output buffer against the hunk size. It is up to the caller
/// of [`decompress`](crate::codecs::CodecImplementation::decompress) to uphold length invariants.
#[cfg_attr(docsrs, doc(cfg(codec_api)))]
pub mod codecs {
    pub use crate::compression::codecs::*;
    pub use crate::compression::{
        CodecImplementation, CompressionCodec, CompressionCodecType, DecompressResult,
    };
}

const fn make_tag(a: &[u8; 4]) -> u32 {
    ((a[0] as u32) << 24) | ((a[1] as u32) << 16) | ((a[2] as u32) << 8) | (a[3] as u32)
}

macro_rules! const_assert {
    ($($list:ident : $ty:ty),* => $expr:expr) => {{
        struct Assert<$(const $list: $ty,)*>;
        impl<$(const $list: $ty,)*> Assert<$($list,)*> {
            const OK: u8 = 0 - !($expr) as u8;
        }
        Assert::<$($list,)*>::OK
    }};
    ($expr:expr) => {
        const OK: u8 = 0 - !($expr) as u8;
    };
}

pub(crate) use const_assert;

pub use chdfile::{ChdFile, ChdHunk};
pub use error::{ChdError, Result};
pub mod header;
pub mod map;
pub mod metadata;
pub mod read;

#[cfg(feature = "unstable_lending_iterators")]
#[cfg_attr(docsrs, doc(cfg(unstable_lending_iterators)))]
pub mod iter;

#[cfg(test)]
mod tests {
    use crate::metadata::ChdMetadata;
    use crate::read::{ChdFileReader, ChdHunkBufReader};
    use crate::ChdFile;
    use std::convert::TryInto;
    use std::fs::File;
    use std::io::{BufReader, Read, Write};

    #[cfg(feature = "unstable_lending_iterators")]
    use crate::iter::LendingIterator;

    #[test]
    fn read_metas_test() {
        let mut f = File::open(".testimages/Test.chd").expect("");
        let mut chd = ChdFile::open(&mut f, None).expect("file");

        let metadatas: Vec<ChdMetadata> = chd.metadata_refs().try_into().expect("");
        let meta_datas: Vec<_> = metadatas
            .into_iter()
            .map(|s| String::from_utf8(s.value).unwrap())
            .collect();
        println!("{:?}", meta_datas);
    }

    #[test]
    fn read_hunk_buffer_test() {
        let mut f = BufReader::new(File::open(".testimages/cliffhgr.chd").expect(""));
        let mut chd = ChdFile::open(&mut f, None).expect("file");
        let hunk_count = chd.header().hunk_count();

        let mut hunk_buf = Vec::new();
        let mut cmp_buf = Vec::new();
        for hunk_num in 0..hunk_count {
            let mut hunk = chd.hunk(hunk_num).expect("could not acquire hunk");
            let read = ChdHunkBufReader::new_in(&mut hunk, &mut cmp_buf, hunk_buf)
                .expect(format!("could not read_hunk {}", hunk_num).as_str());
            hunk_buf = read.into_inner();
        }
    }

    #[test]
    fn read_hunk_test() {
        let mut f = BufReader::new(File::open(".testimages/cliffhgr.chd").expect(""));
        let mut chd = ChdFile::open(&mut f, None).expect("file");
        let hunk_count = chd.header().hunk_count();

        let mut hunk_buf = chd.get_hunksized_buffer();
        let mut cmp_buf = Vec::new();
        for hunk_num in 0..hunk_count {
            let mut hunk = chd.hunk(hunk_num).expect("could not acquire hunk");
            hunk.read_hunk_in(&mut cmp_buf, &mut hunk_buf)
                .expect(format!("could not read_hunk {}", hunk_num).as_str());
            println!("Read hunk {}", hunk_num);
        }
    }

    #[test]
    fn read_file_test() {
        let mut f = BufReader::new(File::open(".testimages/Test.chd").expect(""));
        let chd = ChdFile::open(&mut f, None).expect("file");
        let mut read = ChdFileReader::new(chd);

        let mut buf = Vec::new(); // this is really bad..
        read.read_to_end(&mut buf).expect("can read to end");
        let mut f_out = File::create(".testimages/out.bin").expect("");
        f_out.write_all(&buf).expect("did not write")
    }

    #[test]
    #[cfg(feature = "unsound_owning_iterators")]
    fn hunk_iter_test() {
        let f_bytes = include_bytes!("../.testimages/mocapbj_a29a02.chd");
        let mut f_cursor = Cursor::new(f_bytes);
        // let mut f = BufReader::new(File::open(".testimages/mocapbj_a29a02.chd").expect(""));
        let mut chd = ChdFile::open(&mut f_cursor, None).expect("file");
        let mut hunk_buf = chd.get_hunksized_buffer();
        let mut comp_buf = Vec::new();
        for (_hunk_num, mut hunk) in chd.hunks().skip(7838).enumerate() {
            hunk.read_hunk_in(&mut comp_buf, &mut hunk_buf)
                .expect("hunk could not be read");
        }
    }

    #[test]
    #[cfg(feature = "unstable_lending_iterators")]
    fn hunk_iter_lending_test() {
        let mut f = BufReader::new(File::open(".testimages/cliffhgr.chd").expect(""));
        // let mut f = BufReader::new(File::open(".testimages/cliffhgr.chd").expect(""));

        let mut chd = ChdFile::open(&mut f, None).expect("file");
        let mut hunk_buf = chd.get_hunksized_buffer();
        let mut comp_buf = Vec::new();
        let mut hunks = chd.hunks();
        let mut hunk_num = 0;
        while let Some(mut hunk) = hunks.next() {
            hunk.read_hunk_in(&mut comp_buf, &mut hunk_buf)
                .expect(&*format!("hunk {} could not be read", hunk_num));
            hunk_num += 1;
        }
    }

    #[test]
    #[cfg(feature = "unstable_lending_iterators")]
    fn metadata_iter_lending_test() {
        let mut f = BufReader::new(File::open(".testimages/Test.chd").expect(""));
        let mut chd = ChdFile::open(&mut f, None).expect("file");
        let mut metas = chd.metadata();
        while let Some(mut meta) = metas.next() {
            let contents = meta.read().expect("metadata entry could not be read");
            println!("{:?}", String::from_utf8(contents.value));
        }
    }

    #[test]
    #[cfg(feature = "unsound_owning_iterators")]
    fn metadata_iter_test() {
        let mut f = BufReader::new(File::open(".testimages/Test.chd").expect(""));
        let mut chd = ChdFile::open(&mut f, None).expect("file");
        for mut meta in chd.metadata().expect("metadata could not be read") {
            let contents = meta.read().expect("metadata entry could not be read");
            println!("{:?}", String::from_utf8(contents.value));
        }
    }
}
