# chd-rs

Reimplementation of the CHD file format in pure Safe Rust, drop-in compatible with libchdr.

[![Latest Version](https://img.shields.io/crates/v/chd.svg)](https://crates.io/crates/chd) [![Docs](https://docs.rs/chd/badge.svg)](https://docs.rs/chd) ![License](https://img.shields.io/crates/l/chd)

## Usage

```rust
fn main() -> Result<()> {
    let mut f = BufReader::new(File::open("image.chd")?;
    let mut chd = ChdFile::open_stream(&mut f, None)?;
    let hunk_count = chd.header().hunk_count();
    let hunk_size = chd.header().hunk_bytes();
    
    // buffer to store decompressed hunks
    let mut hunk_buf = vec![0u8; hunk_size as usize];
    
    // buffer for temporary compressed
    let mut cmp_buf = Vec::new();
    for hunk_num in 0..hunk_count {
        let mut hunk = chd.hunk(hunk_num)?;
        hunk.read_hunk_in(&mut cmp_buf, &mut hunk_buf)?;
    }
}
```

### Verify Block CRC
By default, chd-rs does not verify the checksums of decompressed hunks. The feature `verify_block_crc` should be enabled to verify hunk checksums.

```toml
[dependencies]
chd = { features = ["verify_block_crc"] }
```

## `libchdr` API (WIP)
chd-rs provides a C API compatible with [chd.h](https://github.com/rtissera/libchdr/blob/6eeb6abc4adc094d489c8ba8cafdcff9ff61251b/include/libchdr/chd.h). 
It makes no guarantees of ABI compatibility, and if your project links dynamically with libchdr, the output library will not work. However, chd-rs provides 
a `CMakeLists.txt` that will link your project statically against `chd-rs`, and provides mostly the exact same API as libchdr.

### `core_file*` support
The functions `chd_open_file`, and `chd_core_file` will not be available unless the feature `unsafe_c_file_streams` is enabled. 

This is because `core_file*` is not an opaque pointer and is a C `FILE*` stream. This allows the underlying file pointer to be changed unsafely beneath 
the memory safety guarantees of chd-rs. We strongly encourage using `chd_open` instead of `chd_open_file`.  

If you need `core_file*` support, chd-capi should have the `unsafe_c_file_streams` feature enabled. This will attempt to use C `FILE*` streams as the underlying
stream for a `chd_file*`. This is highly unsafe and not supported on all platforms.

