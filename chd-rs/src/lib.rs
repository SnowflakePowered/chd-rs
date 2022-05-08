#![forbid(unsafe_code)]

pub mod header;
pub mod error;
pub mod metadata;
pub mod chd;

mod cdrom;
mod compression;
mod huffman;
mod map;

const fn make_tag(a: &[u8; 4]) -> u32 {
    return ((a[0] as u32) << 24) | ((a[1] as u32) << 16) | ((a[2] as u32) << 8) | (a[3] as u32)
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use crate::header;
    use crate::metadata;
    use std::convert::TryInto;
    use std::io::Read;
    use crate::header::ChdHeader;
    use crate::chd::ChdFile;
    use crate::metadata::ChdMetadata;

    #[test]
    fn read_metas_test() {
        let mut f = File::open(".testimages/Test.chd").expect("");
        let mut chd = ChdFile::open_stream(&mut f, None).expect("file");
        let res = chd.header();

        let metadatas: Vec<ChdMetadata> = chd.metadata().unwrap().try_into().expect("");
        let meta_datas: Vec<_> = metadatas.into_iter()
            .map(|s| String::from_utf8(s.value ).unwrap() )
                .collect();
        println!("{:?}", meta_datas);

    }

    #[test]
    fn read_hunks_test() {
        let mut f = File::open(".testimages/Test.chd").expect("");
        let mut chd = ChdFile::open_stream(&mut f, None).expect("file");
        let hunk_size = chd.header().hunk_bytes();
        let hunk_count = chd.header().hunk_count();

        let mut hunk_buf = vec![0u8; hunk_size as usize];
        // 13439 breaks??
        // 13478 breaks now with decmp error.
        // for hunk_num in 13478..hunk_count {
        for hunk_num in 13478..hunk_count {
            let mut hunk = chd.hunk(hunk_num).expect("could not acquire hunk");
            hunk.read_hunk(&mut hunk_buf).expect(format!("could not read_hunk {}", hunk_num).as_str());
        }
    }
}
