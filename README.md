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
[claxon](https://crates.io/crates/claxon). While performance is not ignored, the focus
is on readability and correctness.

## Usage
Open a `ChdFile` with `ChdFile::open`, then iterate hunks from 0 to `chd.header().hunk_count()` to
read hunks.

The size of the destination buffer must be exactly `chd.header().hunk_size()` to decompress with
`hunk.read_hunk_in`, which takes the output slice and a buffer to hold compressed data.

```rust
fn main() -> Result<()> {
    let mut f = BufReader::new(File::open("image.chd")?;
    let mut chd = ChdFile::open(&mut f, None)?;
    let hunk_count = chd.header().hunk_count();
    let hunk_size = chd.header().hunk_size();
    
    // buffer to store decompressed hunks
    let mut out_buf = chd.get_hunksized_buffer();
    
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

### Lending Iterators
With `unstable_lending_iterators`, hunks and metadata can be slightly more ergonomically iterated over
albeit with a `while let` loop. This API is unstable until [Generalized Associated Types](https://github.com/rust-lang/rust/pull/96709)
and the `LendingIterator` trait is stabilized.


```toml
[dependencies]
chd = { version = "0.1", features = ["unstable_lending_iterators"] }
```

Then hunks can be iterated like so.

```rust
fn main() -> Result<()> {
    let mut f = BufReader::new(File::open("image.chd")?;
    let mut chd = ChdFile::open(&mut f, None)?;
    
    // buffer to store decompressed hunks
    let mut out_buf = chd.get_hunksized_buffer();
    
    // buffer for temporary compressed
    let mut temp_buf = Vec::new();
    let mut hunk_iter = chd.hunks();
    while let Some(mut hunk) = hunk_iter.next() {
        hunk.read_hunk_in(&mut temp_buf, &mut out_buf)?;
    }
}
```

A similar API exists for metadata in `ChdFile::metadata`.


### Verifying Hunk Checksums
By default, chd-rs does not verify the checksums of decompressed hunks for performance. The feature `verify_block_crc` should be enabled 
to verify hunk checksums.

```toml
[dependencies]
chd = { version = "0.1", features = ["verify_block_crc"] }
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
* CD LZMA (`CHD_CODEC_CD_LZMA`)
* CD Deflate (`CHD_CODEC_CD_ZLIB`)
* CD FLAC (`CHD_CODEC_CD_FLAC`)
* AV Huffman (`CHD_CODEC_AVHUFF`)

#### Codecs and Huffman API 
By default, the codecs and static Huffman implementations are not exposed as part of the public API, 
but can be enabled with the `codec_api` and `huffman_api` features respectively. These APIs are subject
to change but should be considered mostly stable. 

In particular the type signature for [`HuffmanDecoder`](https://github.com/SnowflakePowered/chd-rs/blob/e03e093021f1705d46fe6aaa8b32593489e55467/chd-rs/src/huffman.rs#L110)
is subject to change once [`generic_const_exprs`](https://github.com/rust-lang/rust/issues/76560) is stabilized.

## `libchdr` API
⚠️*The C API has not been heavily tested. Use at your own risk.* ⚠️

chd-rs provides a C API compatible with [chd.h](https://github.com/rtissera/libchdr/blob/6eeb6abc4adc094d489c8ba8cafdcff9ff61251b/include/libchdr/chd.h). 
ABI compatibility is detailed below but is untested when compiling as a dynamic library.

### `core_file*` support
The functions `chd_open_file`, and `chd_core_file` will not be available unless the feature `chd_core_file` is enabled. 

This is because `core_file*` is not an opaque pointer and is a C `FILE*` stream. This allows the underlying file pointer to be changed unsafely beneath 
the memory safety guarantees of chd-rs. We strongly encourage using `chd_open` instead of `chd_open_file`.  

If you need `core_file*` support, chd-capi should have the `chd_core_file` feature enabled, which will wrap 
`FILE*` to be usable in Rust with a lightweight wrapper in `libchdcorefile`. If the default implementation
is not suitable, you may need to implement `libchdcorefile` yourself. The `chd_core_file` feature requires
CMake and Clang to be installed.

### ABI compatibility

chd-rs makes the following ABI-compatibility guarantees compared to libchdr when compiled statically.
* `chd_error` is ABI and API-compatible with [chd.h](https://github.com/rtissera/libchdr/blob/cdcb714235b9ff7d207b703260706a364282b063/include/libchdr/chd.h#L258)
* `chd_header` is ABI and API-compatible [chd.h](https://github.com/rtissera/libchdr/blob/cdcb714235b9ff7d207b703260706a364282b063/include/libchdr/chd.h#L302)
* `chd_file *` is an opaque pointer. It is **not layout compatible** with [chd.c](https://github.com/rtissera/libchdr/blob/cdcb714235b9ff7d207b703260706a364282b063/src/libchdr_chd.c#L265)
* The layout of `core_file *` is user-defined when the `chd_core_file` feature is enabled.
* Freeing any pointer returned by chd-rs with `free` is undefined behaviour. A `chd_file *` pointer can be safely freed with `chd_close`.