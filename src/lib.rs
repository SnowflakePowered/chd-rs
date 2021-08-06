
mod header;
mod error;
mod metadata;
mod cdrom;
mod chd;
mod compression;

const fn make_tag(a: &[u8; 4]) -> u32 {
    return ((a[0] as u32) << 24) | ((a[1] as u32) << 16) | ((a[2] as u32) << 8) | (a[3] as u32)
}

#[cfg(test)]
mod tests {
    use std::fs::{File};
    use crate::header;
    use crate::metadata;
    use std::convert::TryInto;
    use std::borrow::Borrow;
    use crate::header::ChdHeader;
    use crate::chd::ChdFile;

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
        let mut chd = ChdFile::try_from_file(&mut f, None).expect("file");
        let res = chd.header();

        let meta_datas: Vec<_> = chd.metadata().unwrap().into_iter()
            .map(|s| unsafe { String::from_utf8_unchecked(s.value ) })
                .collect();
        println!("debug");
    }
}
