/* automatically generated by rust-bindgen 0.60.1 */

pub type size_t = ::std::os::raw::c_ulonglong;
pub type wchar_t = ::std::os::raw::c_ushort;
pub type max_align_t = f64;
pub type core_file = ::std::os::raw::c_void;
extern "C" {
    pub fn core_fread(
        file: *mut core_file,
        buffer: *mut ::std::os::raw::c_void,
        size: size_t,
    ) -> size_t;
}
extern "C" {
    pub fn core_fseek(
        file: *mut core_file,
        offset: size_t,
        origin: ::std::os::raw::c_int,
    ) -> ::std::os::raw::c_int;
}
extern "C" {
    pub fn core_fopen(filename: *const ::std::os::raw::c_char) -> *mut core_file;
}
extern "C" {
    pub fn core_fclose(file: *mut core_file);
}
