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
//!
//! AVHuff decompression is experimental and can be enabled with the `avhuff` feature.
#![forbid(unsafe_code)]

mod error;

mod cdrom;
mod compression;
mod huffman;
mod block_hash;
mod chdfile;

const fn make_tag(a: &[u8; 4]) -> u32 {
    return ((a[0] as u32) << 24) | ((a[1] as u32) << 16) | ((a[2] as u32) << 8) | (a[3] as u32);
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
pub mod metadata;
pub mod map;
pub mod read;

#[cfg(test)]
mod tests {
    use crate::ChdFile;
    use crate::metadata::ChdMetadata;
    use std::convert::TryInto;
    use std::fs::File;
    use std::io::{BufReader, Read, Write};
    use std::process::Termination;
    use bencher::Bencher;
    use crate::read::{ChdFileReader, ChdHunkBufReader};

    #[test]
    fn read_metas_test() {
        let mut f = File::open(".testimages/Test.chd").expect("");
        let mut chd = ChdFile::open(&mut f, None).expect("file");

        let metadatas: Vec<ChdMetadata> = chd.metadata().unwrap().try_into().expect("");
        let meta_datas: Vec<_> = metadatas
            .into_iter()
            .map(|s| String::from_utf8(s.value).unwrap())
            .collect();
        println!("{:?}", meta_datas);
    }

    #[test]
    fn read_hunks_test() {
        let mut f = BufReader::new(File::open(".testimages/Test.chd").expect(""));
        let mut chd = ChdFile::open(&mut f, None).expect("file");
        let hunk_count = chd.header().hunk_count();

        let mut hunk_buf = Vec::new();
        // 13439 breaks??
        // 13478 breaks now with decmp error.
        // for hunk_num in 13478..hunk_count {
        let mut cmp_buf = Vec::new();
        for hunk_num in 0..hunk_count {
            let mut hunk = chd.hunk(hunk_num).expect("could not acquire hunk");
            let read = ChdHunkBufReader::new_in(&mut hunk, &mut cmp_buf, hunk_buf)
                .expect(format!("could not read_hunk {}", hunk_num).as_str());
            hunk_buf = read.into_inner();
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
}
