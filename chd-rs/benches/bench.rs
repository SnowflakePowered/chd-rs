use bencher::{benchmark_group, benchmark_main, Bencher};
use chd::read::ChdHunkBufReader;
use chd::ChdFile;
use std::env::args;
use std::fs::File;
use std::io::BufReader;

fn read_hunks_unbuf_bench(bench: &mut Bencher) {
    let mut f = BufReader::new(File::open(".testimages/Test.chd").expect(""));

    bench.iter(|| {
        let mut chd = ChdFile::open(&mut f, None).expect("file");
        let hunk_count = chd.header().hunk_count();
        let hunk_size = chd.header().hunk_size() as usize;
        let mut hunk_buf = vec![0u8; hunk_size];
        // 13439 breaks??
        // 13478 breaks now with decmp error.
        // for hunk_num in 13478..hunk_count {
        let mut cmp_buf = Vec::new();
        let mut bytes = 0;
        for hunk_num in 0..hunk_count {
            let mut hunk = chd.hunk(hunk_num).expect("could not acquire hunk");
            bytes += hunk
                .read_hunk_in(&mut cmp_buf, &mut hunk_buf)
                .expect(format!("could not read_hunk {}", hunk_num).as_str());
        }
        println!("total: {}", bytes);
    });
}

benchmark_group!(benches, read_hunks_unbuf_bench);
benchmark_main!(benches);
