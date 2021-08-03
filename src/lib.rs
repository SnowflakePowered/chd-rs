mod header;
mod error;
mod metadata;
mod cdrom;

const fn make_tag(a: &[u8; 4]) -> u32 {
    return ((a[0] as u32) << 24) | ((a[1] as u32) << 16) | ((a[2] as u32) << 8) | (a[3] as u32)
}

#[cfg(test)]
mod tests {
    use std::fs::{File};
    use crate::header;
    use crate::metadata;

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
        let res = header::read_header(&mut f).expect("parse failed");
        let meta = metadata::MetadataIter::new(&mut f, res.meta_offset);
        let metas: Vec<_> = meta.collect();
        let meta_datas: Vec<_> = metas.iter().flat_map(|e| e.read(&mut f))
            .map(|s| unsafe { String::from_utf8_unchecked(s) })
                .collect();
        println!("debug");
    }
}
