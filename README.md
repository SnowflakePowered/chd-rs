# chd-rs

Pure Rust implementation of the CHD file format, drop-in compatible with libchdr.

## Drop-in Replacement
chd-rs provides a C API compatible with [chd.h](https://github.com/rtissera/libchdr/blob/6eeb6abc4adc094d489c8ba8cafdcff9ff61251b/include/libchdr/chd.h). 
It makes no guarantees of ABI compatibility, and if your project links dynamically with libchdr, the output library will not work. However, chd-rs provides 
a `CMakeLists.txt` that will link your project statically against `chd-rs`, and provides the exact same API as libchdr.

