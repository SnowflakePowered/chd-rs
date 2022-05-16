mod header;

use chd::header::ChdHeader;
use chd::{ChdError, ChdFile};
use std::ffi::{CStr, CString};
use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::mem::MaybeUninit;
use std::os::raw::{c_char, c_int, c_void};
use std::path::Path;
use std::slice;
use crate::header::chd_header;

pub const CHD_OPEN_READ: i32 = 1;
pub const CHD_OPEN_READWRITE: i32 = 2;

pub trait SeekRead: Read + Seek {}
impl<R: Read + Seek> SeekRead for BufReader<R> {}

#[allow(non_camel_case_types)]
pub type chd_file = ChdFile<Box<dyn SeekRead>>;

pub use chd::ChdError as chd_error;
use chd::metadata::{ChdMetadata, ChdMetadataTag, KnownMetadata};

fn ffi_takeown_chd(chd: *mut chd_file) -> Box<ChdFile<Box<dyn SeekRead>>> {
    unsafe {
        Box::from_raw(chd)
    }
}

fn ffi_expose_chd(chd: Box<ChdFile<Box<dyn SeekRead>>>) -> *mut chd_file {
    Box::into_raw(chd)
}

fn ffi_open_chd(filename: *const c_char, parent: Option<Box<chd_file>>) -> Result<chd_file, chd_error> {
    let c_filename = unsafe { CStr::from_ptr(filename) };
    let filename = std::str::from_utf8(c_filename.to_bytes())
        .map(Path::new)
        .map_err(|_| chd_error::InvalidParameter)?;

    let file = File::open(filename)
        .map_err(|_| chd_error::FileNotFound)?;

    let bufread = Box::new(BufReader::new(file)) as Box<dyn SeekRead>;
    ChdFile::open(bufread, parent)
}

#[no_mangle]
pub extern "C" fn chd_open_file(
    filename: *const c_char,
    mode: c_int,
    parent: *mut chd_file,
    out: *mut *mut chd_file,
) -> chd_error {
    // we don't support READWRITE mode
    if mode == CHD_OPEN_READWRITE {
        return chd_error::FileNotWriteable
    }

    let parent = if parent.is_null() {
        None
    } else {
        Some(ffi_takeown_chd(parent))
    };

    let chd = match ffi_open_chd(filename, parent) {
        Ok(chd) => chd,
        Err(e) => return e,
    };

    unsafe { *out = ffi_expose_chd(Box::new(chd)) }
    chd_error::None
}

#[no_mangle]
/// Close a CHD file.
pub extern "C" fn chd_close(chd: *mut chd_file) {
    unsafe { drop(Box::from_raw(chd)) }
}

#[no_mangle]
/// Returns an error string for the corresponding CHD error.
pub extern "C" fn chd_error_string(err: chd_error) -> *const c_char {
    // SAFETY: This will leak, but this is much safer than
    // potentially allowing the C caller to corrupt internal state
    // by returning an internal pointer to an interned string.
    let err_string = unsafe { CString::new(err.to_string()).unwrap_unchecked() };
    err_string.into_raw()
}

fn ffi_chd_get_header(chd: &chd_file) -> chd_header {
    match chd.header() {
        ChdHeader::V5Header(_) => {
            header::get_v5_header(chd)
        }
        ChdHeader::V1Header(h) | ChdHeader::V2Header(h) => {
            h.into()
        }
        ChdHeader::V3Header(h) => h.into(),
        ChdHeader::V4Header(h) => h.into()
    }
}
#[no_mangle]
/// Returns a pointer to the extracted CHD header data.
pub extern "C" fn chd_get_header(chd: *const chd_file) -> *const chd_header {
    match unsafe { chd.as_ref() } {
        Some(chd) => {
            let header = ffi_chd_get_header(chd);
            Box::into_raw(Box::new(header))
        }
        None => std::ptr::null()
    }
}

#[no_mangle]
/// Read a single hunk from the CHD file.
/// The output buffer must be initialized and have a length
/// of exactly the hunk size, or it is undefined behaviour.
pub extern "C" fn chd_read(chd: *mut chd_file, hunknum: u32, buffer: *mut c_void) -> chd_error {
    match unsafe { chd.as_mut() } {
        None => chd_error::InvalidParameter,
        Some(chd) => {
            let hunk = chd.hunk(hunknum);
            if let Ok(mut hunk) = hunk {
                let size = hunk.len();
                let mut comp_buf = Vec::new();
                // SAFETY: The output buffer *must* be initialized and
                // have a length of exactly the hunk size.
                let output: &mut [u8] = unsafe {
                    slice::from_raw_parts_mut(buffer as *mut u8, size)
                };
                let result =
                    hunk.read_hunk_in(&mut comp_buf, output);
                match result {
                    Ok(_) => chd_error::None,
                    Err(e) => e
                }
            } else {
                chd_error::HunkOutOfRange
            }
        }
    }
}

fn find_metadata(chd: &mut chd_file, search_tag: u32, mut search_index: u32) -> Result<ChdMetadata, ChdError>{
    for mut entry
        in chd.metadata().ok_or(ChdError::MetadataNotFound)? {
        if entry.metatag() == search_tag || entry.metatag() == KnownMetadata::Wildcard.metatag() {
            if search_index == 0 {
                return Ok(entry.read()?)
            }
            search_index -= 1;
        }
    }
    Err(ChdError::MetadataNotFound)
}
#[no_mangle]
/// Get indexed metadata of the given search tag and index.
pub extern "C" fn chd_get_metadata(
    chd: *mut chd_file,
    searchtag: u32,
    searchindex: u32,
    output: *mut c_void,
    output_len: u32,
    result_len: *mut u32,
    result_tag: *mut u32,
    result_flags: *mut u8,
) -> chd_error {
    match unsafe { chd.as_mut() } {
        Some(chd) => {
            let entry = find_metadata(chd, searchtag, searchindex);
            match (entry, searchtag) {
                (Ok(meta), _) => {
                    unsafe {
                        let output_len = std::cmp::min(output_len, meta.value.len() as u32);
                        std::ptr::copy_nonoverlapping(meta.value.as_ptr() as *const c_void,
                        output, output_len as usize);

                        if !result_tag.is_null() {
                            result_tag.write(meta.metatag)
                        }
                        if !result_len.is_null() {
                            result_len.write(meta.length)
                        }
                        if !result_flags.is_null() {
                            result_flags.write(meta.flags)
                        }
                    }
                    chd_error::None
                }
                (Err(_), tag) => unsafe {
                    if (tag == KnownMetadata::HardDisk.metatag() || tag == KnownMetadata::Wildcard.metatag())
                        && searchindex == 0 {
                        let header = chd.header();
                        if let ChdHeader::V1Header(header) = header {
                            let fake_meta = format!("CYLS:{},HEADS:{},SECS:{},BPS:{}",
                                header.cylinders,
                                header.heads,
                                header.sectors,
                                header.hunk_bytes / header.hunk_size
                            );
                            let cstring = CString::from_vec_unchecked(fake_meta.into_bytes());
                            let bytes = cstring.into_bytes_with_nul();
                            let len = bytes.len();
                            let output_len = std::cmp::min(output_len, len as u32);

                            std::ptr::copy_nonoverlapping(bytes.as_ptr() as *const c_void,
                                                          output, output_len as usize);
                            if !result_tag.is_null() {
                                result_tag.write(KnownMetadata::HardDisk.metatag())
                            }
                            if !result_len.is_null() {
                                result_len.write(len as u32)
                            }
                            return chd_error::None
                        }
                    }
                    chd_error::MetadataNotFound
                }
            }
        }
        None => chd_error::InvalidParameter
    }
}

#[no_mangle]
/// Set codec internal parameters.
///
/// This function is not supported and always returns CHDERR_INVALID_PARAMETER.
pub extern "C" fn chd_codec_config(
    _chd: *const chd_file,
    _param: i32,
    _config: *mut c_void,
) -> chd_error {
    chd_error::InvalidParameter
}

#[no_mangle]
/// Read CHD header data from the file into the pointed struct.
pub extern "C" fn chd_read_header(filename: *const c_char, header: *mut MaybeUninit<chd_header>) -> chd_error {
    let chd = ffi_open_chd(filename, None);
    match chd {
        Ok(chd) => {
            let chd_header = ffi_chd_get_header(&chd);
            match unsafe { header.as_mut() } {
                None => ChdError::InvalidParameter,
                Some(header) => {
                    header.write(chd_header);
                    ChdError::None
                }
            }
        }
        Err(e) => e
    }
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
