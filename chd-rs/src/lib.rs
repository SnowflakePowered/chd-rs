
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
    use std::borrow::Borrow;
    use std::io::Read;
    use crate::header::ChdHeader;
    use crate::chd::ChdFile;
    use crate::metadata::ChdMetadata;
    use crate::compression::flac::ChdFlacHeader;

    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }

    fn make_tag_test() {
        assert_eq!(2 + 2, 4);
    }

    #[test]
    fn test() {
        let mut f = File::open(".testimages/Test.chd").expect("");
        let mut chd = ChdFile::open_stream(&mut f, None).expect("file");
        let res = chd.header();

        let metadatas: Vec<ChdMetadata> = chd.metadata().unwrap().try_into().expect("");

        let meta_datas: Vec<_> = metadatas.into_iter()
            .map(|s| unsafe { String::from_utf8_unchecked(s.value ) })
                .collect();
        println!("{:?}", meta_datas);

        println!("{}, {}", chd.map().len(), chd.header().hunk_count());

        for i in 0..chd.header().hunk_count() as usize {
            if let Some(map) = chd.map().get_entry(i) {

            }
        }
    }

    #[test]
    fn flac_buf_test() {
        let b = [1, 2, 3, 4];
        let mut fb = ChdFlacHeader::new(44100, 2, 2352);
        let mut n = vec![0u8; 100];
        fb.as_read(&b).read(&mut n).expect("read");
        assert_eq!(&n[0x2a..][..4], &b);
    }
}
