use std::env;
use std::fs::File;
use std::io::{BufWriter, Write};

#[cfg(feature = "chd_core_file")]
fn build_chdcorefile() {
    use std::path::PathBuf;

    println!("cargo:rerun-if-changed=libchdcorefile/chdcorefile.h");
    let dst = if cfg!(target_os = "windows") {
        cmake::Config::new("libchdcorefile")
            .build_target("chdcorefile")
            .static_crt(true)
            .cxxflag("/MT")
            .cflag("/MT")
            .cxxflag("/NODEFAULTLIB:MSVCRT")
            .always_configure(true)
            .profile("Release")
            .very_verbose(true)
            .build()
    } else {
        cmake::Config::new("libchdcorefile")
            .build_target("chdcorefile")
            .profile("Release")
            .always_configure(true)
            .very_verbose(true)
            .build()
    };

    let mut lib_dst = PathBuf::from(format!("{}", dst.display()));
    lib_dst.push("build");
    if cfg!(target_os = "windows") {
        lib_dst.push("Release");
    }
    println!("cargo:rustc-link-search=native={}", lib_dst.display());
    println!("cargo:rustc-link-lib=static=chdcorefile");


    let bindings = bindgen::Builder::default()
        .header("libchdcorefile/chdcorefile.h")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks))
        .generate()
        .expect("Unable to generate bindings");

    // Write the bindings to the $OUT_DIR/bindings.rs file.
    let chdcorefile_src = File::create("src/chdcorefile_sys.rs")
        .expect("Unable to open file");

    bindings
        .write(Box::new(chdcorefile_src))
        .expect("Unable to write bindings to libchdcorefile.");
}
fn main() {
    #[cfg(feature = "chd_core_file")]
    if cfg!(feature = "chd_core_file") {
       build_chdcorefile();
    }

    let crate_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let mut buf = BufWriter::new(Vec::new());
    cbindgen::generate(crate_dir)
        .expect("Unable to generate bindings")
        .write(&mut buf);

    let bytes = buf.into_inner().expect("Unable to extract bytes");
    let string = String::from_utf8(bytes).expect("Unable to create string");
    let string = string.replace("CHD_ERROR_", "CHDERR_");
    File::create("chd.h")
        .expect("Unable to open file")
        .write_all(string.as_bytes())
        .expect("Unable to write bindings.")
}
