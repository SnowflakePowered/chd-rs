# chd-rs
[![Latest Version](https://img.shields.io/crates/v/chd.svg)](https://crates.io/crates/chd) [![Docs](https://docs.rs/chd/badge.svg)](https://docs.rs/chd) ![License](https://img.shields.io/crates/l/chd)
[![Minimum Supported Rust Version 1.59](https://img.shields.io/badge/rust-1.59%2B-orange.svg)](https://github.com/rust-lang/rust/blob/master/RELEASES.md#version-1590-2022-02-24)


Reimplementation of the CHD file format in pure Safe Rust, drop-in compatible with libchdr.

chd-rs aims to be a memory-safe, well documented, and clean from-scratch implementation of CHD, verifiable against 
[chd.cpp](https://github.com/mamedev/mame/blob/master/src/lib/util/chd.cpp) while being easier to read and use as
documentation to implement the format natively in other languages. It is standalone and can be built with just
a Rust compiler, without the need for a full C/C++ toolchain. 

Performance is competitive but a little slower than libchdr in benchmarks from using more immature (but fully correct) 
pure Rust implementations of compression codecs. Deflate (zlib) compression is backed by [flate2](https://crates.io/crates/flate2), 
LZMA is backed by [lzma-rs](https://crates.io/crates/lzma-rs) (modified slightly to allow 
[headerless decoding of LZMA chunks](https://crates.io/crates/lzma-rs-headerless)), and FLAC decompression is backed by
[claxon](https://crates.io/crates/claxon). While performance is not ignored (only chd-rs uses an allocation-free Huffman decoder!), the focus
is on readability and correctness.

## Usage
Open a `ChdFile` with `ChdFile::open_stream`, then iterate hunks from 0 to `chd.header().hunk_count()` to
read hunks.

The size of the destination buffer must be exactly `chd.header().hunk_size()` to decompress with
`hunk.read_hunk_in`, which takes the output slice and a buffer to hold compressed data.
```rust
fn main() -> Result<()> {
    let mut f = BufReader::new(File::open("image.chd")?;
    let mut chd = ChdFile::open_stream(&mut f, None)?;
    let hunk_count = chd.header().hunk_count();
    let hunk_size = chd.header().hunk_size();
    
    // buffer to store decompressed hunks
    let mut out_buf = vec![0u8; hunk_size as usize];
    
    // buffer for temporary compressed
    let mut temp_buf = Vec::new();
    for hunk_num in 0..hunk_count {
        let mut hunk = chd.hunk(hunk_num)?;
        hunk.read_hunk_in(&mut temp_buf, &mut out_buf)?;
    }
}
```

For more ergonomic but slower usage, [`chd::read`](https://github.com/SnowflakePowered/chd-rs/blob/master/chd-rs/src/read.rs) provides buffered adapters that implement `Read` and `Seek` at the
hunk level. A buffered adapter at the file level is also available.  

### Verify Block CRC
By default, chd-rs does not verify the checksums of decompressed hunks. The feature `verify_block_crc` should be enabled 
to verify hunk checksums.

```toml
[dependencies]
chd = { version  "0.0.4", features = ["verify_block_crc"] }
```

### Supported Codecs
chd-rs supports the following compression codecs, with wider coverage than libchdr. For implementation details,
see the [`chd::compression`](https://github.com/SnowflakePowered/chd-rs/tree/master/chd-rs/src/compression) module.

#### V1-4 Codecs
⚠️*V1-4 support has not been as rigorously tested as V5 support.* ⚠️
* None (`CHDCOMPRESSION_NONE`)
* Zlib (`CHDCOMPRESSION_ZLIB`)
* Zlib+ (`CHDCOMPRESSION_ZLIB`)

#### V5 Codecs
* None (`CHD_CODEC_NONE`)
* LZMA (`CHD_CODEC_LZMA`)
* Deflate (`CHD_CODEC_ZLIB`)
* FLAC (`CHD_CODEC_FLAC`)
* Huffman (`CHD_CODEC_HUFF`)
* CD LZMA (`CHD_CODEC_CDLZ`)
* CD Deflate (`CHD_CODEC_CDZL`)
* CD FLAC (`CHD_CODEC_CDFL`)

#### AVHuff support
⚠️*AVHuff support is a work in progress and likely incorrect as of 0.0.4* ⚠️

Experimental, and probably incorrect [AV Huffman (AVHU)](https://github.com/SnowflakePowered/chd-rs/blob/master/chd-rs/src/compression/avhuff.rs)
support can be enabled with the `avhuff` feature. **Do not rely on its correctness as of 0.0.4**. The implementation of the AVHU codec
will be verified and cleaned up before the 0.1 release.

## `libchdr` API (WIP)
⚠️*The C API is incomplete and heavily work in progress as of 0.0.4.* ⚠️

chd-rs provides a C API compatible with [chd.h](https://github.com/rtissera/libchdr/blob/6eeb6abc4adc094d489c8ba8cafdcff9ff61251b/include/libchdr/chd.h). 
It makes no guarantees of ABI compatibility, and if your project links dynamically with libchdr, the output library will not work. However, chd-rs provides 
a `CMakeLists.txt` that will link your project statically against `chd-rs`, and provides mostly the exact same API as libchdr.

### `core_file*` support
The functions `chd_open_file`, and `chd_core_file` will not be available unless the feature `unsafe_c_file_streams` is enabled. 

This is because `core_file*` is not an opaque pointer and is a C `FILE*` stream. This allows the underlying file pointer to be changed unsafely beneath 
the memory safety guarantees of chd-rs. We strongly encourage using `chd_open` instead of `chd_open_file`.  

If you need `core_file*` support, chd-capi should have the `unsafe_c_file_streams` feature enabled. This will attempt to use C `FILE*` streams as the underlying
stream for a `chd_file*`. This is highly unsafe and not supported on all platforms.

### ABI compatibility

chd-rs makes no guarantees of ABI-compatibility unless otherwise documented, and will only provide source-level
compatibility with chd.h. Other APIs exposed by libchdr are also not provided.