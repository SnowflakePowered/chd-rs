#![no_main]
use libfuzzer_sys::fuzz_target;
use std::io::Cursor;
use chd::ChdFile;

fuzz_target!(|data: &[u8]| {
    let cursor = Cursor::new(data);
    if let Ok(mut chd) = ChdFile::open(cursor, None) {
        let mut hunk_buf = chd.get_hunksized_buffer();
        let mut comp_buf = Vec::new();
        for (_hunk_num, mut hunk) in chd.hunks().enumerate() {
            hunk.read_hunk_in(&mut comp_buf, &mut hunk_buf)
            .expect("hunk could not be read");
        }
    }
});
