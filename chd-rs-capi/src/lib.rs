use std::ffi::CStr;
use std::fmt::Debug;
use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::os::raw::{c_char, c_int, c_void};
use std::panic::catch_unwind;
use std::path::Path;
use chd::{ChdError, ChdFile};
use chd::header::ChdHeader;

trait ReadAndSeek: Read + Seek {}

impl <R: Read + Seek> ReadAndSeek for BufReader<R> {}

#[allow(non_camel_case_types)]
pub type chd_file = ChdFile<Box<dyn ReadAndSeek>>;

#[no_mangle]
extern "C" fn chd_open_file(filename: *const c_char, _mode: c_int,
                            parent: *mut chd_file,
                            out: *mut *mut chd_file) -> ChdError {

    let c_filename = unsafe { CStr::from_ptr(filename) };
    let filename = if let Ok(s) = std::str::from_utf8(c_filename.to_bytes()) {
        Path::new(s)
    } else {
        return ChdError::InvalidParameter
    };
    let file = if let Ok(file) = File::open(filename) {
        file
    } else {
        return ChdError::FileNotFound
    };
    let bufread =
        Box::new(BufReader::new(file)) as Box<dyn ReadAndSeek>;
    let parent = if parent.is_null() {
        None
    } else {
        Some(unsafe { Box::from_raw(parent) })
    };
    let chd =
        if let Ok(chd) = ChdFile::open(bufread, parent) {
            chd
        } else {
            return ChdError::FileNotFound;
        };
    unsafe {
        *out = Box::into_raw(Box::new(chd))
    }
    return ChdError::None;

}

#[no_mangle]
extern "C" fn chd_close(chd: *mut chd_file) {
    unsafe {
        drop(Box::from_raw(chd))
    }
}

#[no_mangle]
extern "C" fn chd_error_string(err: ChdError) -> *const c_char {
    todo!()
}


#[no_mangle]
extern "C" fn chd_get_header(chd: *const chd_file) -> *const ChdHeader {
    todo!()
}

#[no_mangle]
extern "C" fn chd_read_header(filename: *const c_char, header: *const ChdHeader) -> ChdError {
    todo!()
}

#[no_mangle]
extern "C" fn chd_read(chd: *const chd_file, hunknum: u32, buffer: *mut c_void) -> ChdError {
    todo!()
}

#[no_mangle]
extern "C" fn chd_get_metadata(chd: *const chd_file, searchtag: u32, searchindex: u32,
                               output: *mut c_void, output_len: u32, result_len: *mut u32,
                               result_tag: *mut u32, result_flags: *mut u8) -> ChdError {
    todo!()
}

#[no_mangle]
extern "C" fn chd_codec_config(_chd: *const chd_file, _param: i32, _config: *mut c_void) -> ChdError {
    return ChdError::InvalidParameter;
}

#[cfg(test)]
mod tests {
    use std::assert_eq;

    #[test]
    fn it_works() {
        let result = 2 + 2;
        assert_eq!(result, 4);
    }
}
