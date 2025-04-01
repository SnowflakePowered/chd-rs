#ifndef __CHD_H__
#define __CHD_H__

#pragma once

#include <stdarg.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>

/**
 * Open a CHD for reading.
 */
#define CHD_OPEN_READ 1

/**
 * Open a CHD for reading and writing. This mode is not supported and will always return an error
 * when passed into a constructor function such as [`chd_open`](crate::chd_open).
 */
#define CHD_OPEN_READWRITE 2

/**
 * The chunk size to read when pre-caching the underlying file stream into memory.
 */
#define PRECACHE_CHUNK_SIZE ((16 * 1024) * 1024)

#define CHD_MD5_BYTES 16

#define CHD_SHA1_BYTES 20

/**
 * An opaque type for an opened CHD file.
 */
typedef struct chd_file chd_file;

/**
 * libchdr-compatible CHD header struct.
 * This struct is ABI-compatible with [chd.h](https://github.com/rtissera/libchdr/blob/cdcb714235b9ff7d207b703260706a364282b063/include/libchdr/chd.h#L302)
 */
typedef struct chd_header {
  uint32_t length;
  uint32_t version;
  uint32_t flags;
  uint32_t compression[4];
  uint32_t hunkbytes;
  uint32_t totalhunks;
  uint64_t logicalbytes;
  uint64_t metaoffset;
  uint64_t mapoffset;
  uint8_t md5[CHD_MD5_BYTES];
  uint8_t parentmd5[CHD_MD5_BYTES];
  uint8_t sha1[CHD_SHA1_BYTES];
  uint8_t rawsha1[CHD_SHA1_BYTES];
  uint8_t parentsha1[CHD_SHA1_BYTES];
  uint32_t unitbytes;
  uint64_t unitcount;
  uint32_t hunkcount;
  uint32_t mapentrybytes;
  uint8_t *rawmap;
  uint32_t obsolete_cylinders;
  uint32_t obsolete_sectors;
  uint32_t obsolete_heads;
  uint32_t obsolete_hunksize;
} chd_header;

typedef void core_file;

#ifdef __cplusplus
extern "C" {
#endif // __cplusplus

/**
 * Opens a CHD file by file name, with a layout-undefined backing file pointer owned by
 * the library.
 *
 * The result of passing an object created by this function into [`chd_core_file`](crate::chd_core_file)
 * is strictly undefined. Instead, all `chd_file*` pointers with provenance from `chd_open` should be
 * closed with [`chd_close`](crate::chd_close).
 *
 * # Safety
 * * `filename` is a valid, null-terminated **UTF-8** string.
 * * `parent` is either `NULL` or a valid pointer to a `chd_file` obtained from [`chd_open`](crate::chd_open), [`chd_open_file`](crate::chd_open_file), or [`chd_open_core_file`](crate::chd_open_core_file).
 * * `out` is aligned and can store a pointer to a `chd_file*`. On success, `out` will point to a valid `chd_file*`.
 * * After this function returns, `parent` is invalid and must not be used, otherwise it will be undefined behaviour. There is no way to retake ownership of `parent`.
 */
chd_error chd_open(const char *filename,
                   int mode,
                   struct chd_file *parent,
                   struct chd_file **out);

/**
 * Close a CHD file.
 *
 * # Safety
 * * `chd` is either `NULL` or a valid pointer to a `chd_file` obtained from [`chd_open`](crate::chd_open), [`chd_open_file`](crate::chd_open_file), or [`chd_open_core_file`](crate::chd_open_core_file).
 * * If `chd` is `NULL`, this does nothing.
 */
void chd_close(struct chd_file *chd);

/**
 * Returns an error string for the corresponding CHD error.
 *
 * # Safety
 * The returned string is leaked and the memory **should not and can not ever** be validly freed.
 * Attempting to free the returned pointer with `free` is **undefined behaviour**.
 */
const char *chd_error_string(chd_error err);

/**
 * Returns a pointer to the extracted CHD header data.
 * # Safety
 * * `chd` is either `NULL` or a valid pointer to a `chd_file` obtained from [`chd_open`](crate::chd_open), [`chd_open_file`](crate::chd_open_file), or [`chd_open_core_file`](crate::chd_open_core_file).
 * * If `chd` is `NULL`, returns `NULL`.
 * * The returned pointer is leaked and the memory **should not and can not ever** be validly freed. Attempting to free the returned pointer with `free` is **undefined behaviour**. A non-leaking variant is provided in [`chd_read_header`](crate::chd_read_header).
 */
const struct chd_header *chd_get_header(const struct chd_file *chd);

/**
 * Read a single hunk from the CHD file.
 *
 * # Safety
 * * `chd` is either `NULL` or a valid pointer to a `chd_file` obtained from [`chd_open`](crate::chd_open), [`chd_open_file`](crate::chd_open_file), or [`chd_open_core_file`](crate::chd_open_core_file).
 * * `buffer` must an aligned pointer to a block of initialized memory of exactly the hunk size for the input `chd_file*` that is valid for both reads and writes. This size can be found with [`chd_get_header`](crate::chd_get_header).
 * * If `chd` is `NULL`, returns `CHDERR_INVALID_PARAMETER`.
 */
chd_error chd_read(struct chd_file *chd,
                   uint32_t hunknum,
                   void *buffer);

/**
 * Get indexed metadata of the given search tag and index.
 *
 * # Safety
 * * `chd` is either `NULL` or a valid pointer to a `chd_file` obtained from [`chd_open`](crate::chd_open), [`chd_open_file`](crate::chd_open_file), or [`chd_open_core_file`](crate::chd_open_core_file).
 * * `output` must be an aligned pointer to a block of initialized memory of size exactly `output_len` that is valid for writes.
 * * `result_len` must be either NULL or an aligned pointer to a `uint32_t` that is valid for writes.
 * * `result_tag` must be either NULL or an aligned pointer to a `uint32_t` that is valid for writes.
 * * `result_flags` must be either NULL or an aligned pointer to a `uint8_t` that is valid for writes.
 * * If `chd` is `NULL`, returns `CHDERR_INVALID_PARAMETER`.
 */
chd_error chd_get_metadata(struct chd_file *chd,
                           uint32_t searchtag,
                           uint32_t searchindex,
                           void *output,
                           uint32_t output_len,
                           uint32_t *result_len,
                           uint32_t *result_tag,
                           uint8_t *result_flags);

/**
 * Set codec internal parameters.
 *
 * This function is not supported and always returns `CHDERR_INVALID_PARAMETER`.
 */
chd_error chd_codec_config(const struct chd_file *_chd, int32_t _param, void *_config);

/**
 * Read CHD header data from the file into the pointed struct.
 *
 * # Safety
 * * `filename` is a valid, null-terminated **UTF-8** string.
 * * `header` is either `NULL`, or an aligned pointer to a possibly uninitialized `chd_header` struct.
 * * If `header` is `NULL`, returns `CHDERR_INVALID_PARAMETER`
 */
chd_error chd_read_header(const char *filename,
                          struct chd_header *header);

/**
 * Read CHD header data from the file into the pointed struct.
 *
 * Ownership of the core_file is retained by the caller when calling this function.
 *
 * # Safety
 * * `filename` is a valid, null-terminated **UTF-8** string.
 * * `header` is either `NULL`, or an aligned pointer to a possibly uninitialized `chd_header` struct.
 * * If `header` is `NULL`, returns `CHDERR_INVALID_PARAMETER`
 */
chd_error chd_read_header_file(core_file *file,
                               struct chd_header *header);

/**
 * Read CHD header data from the file into the pointed struct.
 *
 * Ownership is retained by the caller when calling this function.
 *
 * # Safety
 * * `file` is a valid pointer to a `core_file` with respect to the implementation of libchdcorefile that was linked.
 * * `header` is either `NULL`, or an aligned pointer to a possibly uninitialized `chd_header` struct.
 */
chd_error chd_read_header_core_file(core_file *file,
                                    struct chd_header *header);

/**
 * Returns the associated `core_file*`.
 *
 * This method has different semantics than `chd_core_file` in libchdr.
 *
 * The input `chd_file*` will be dropped, and all prior references to
 * to the input `chd_file*` are rendered invalid, with the same semantics as `chd_close`.
 *
 * The provenance of the `chd_file*` is important to keep in mind.
 *
 * If the input `chd_file*` was opened with [`chd_open`](crate::chd_open), the input `chd_file*` will be closed,
 * and the return value should be considered undefined. For now it is `NULL`, but relying on this
 * behaviour is unstable and may change in the future.
 *
 * If the input `chd_file*` was opened with `chd_open_file` and the `chd_core_file` crate feature
 * is enabled, this method will return the same pointer as passed to `chd_input_file`, which may
 * be possible to cast to `FILE*` depending on the implementation of `libchdcorefile` that was
 * linked.
 *
 * # Safety
 * * `chd` is either `NULL` or a valid pointer to a `chd_file` obtained from [`chd_open`](crate::chd_open), [`chd_open_file`](crate::chd_open_file), or [`chd_open_core_file`](crate::chd_open_core_file).
 * * If `chd` is `NULL`, returns `NULL`.
 * * If `chd` has provenance from [`chd_open`](crate::chd_open), the returned pointer is undefined and must not be used.
 * * `chd` is **no longer valid** upon return of this function, and subsequent reuse of the `chd_file*` pointer is **undefined behaviour**.
 */
core_file *chd_core_file(struct chd_file *chd);

/**
 * Open an existing CHD file from an opened `core_file` object.
 *
 * Ownership is taken of the `core_file*` object and should not be modified until
 * `chd_core_file` is called to retake ownership of the `core_file*`.
 *
 * # Safety
 * * `file` is a valid pointer to a `core_file` with respect to the implementation of libchdcorefile that was linked.
 * * `parent` is either `NULL` or a valid pointer to a `chd_file` obtained from [`chd_open`](crate::chd_open), [`chd_open_file`](crate::chd_open_file), or [`chd_open_core_file`](crate::chd_open_core_file).
 * * `out` is aligned and can store a pointer to a `chd_file*`. On success, `out` will point to a valid `chd_file*`.
 * * Until the returned `chd_file*` in `out` is closed with [`chd_close`](crate::chd_close) or [`chd_core_file`](crate::chd_core_file), external mutation of `file` will result in undefined behaviour.
 * * After this function returns, `parent` is invalid and must not be used, otherwise it will be undefined behaviour. There is no way to retake ownership of `parent`.
 */
chd_error chd_open_file(core_file *file,
                        int mode,
                        struct chd_file *parent,
                        struct chd_file **out);

/**
 * Open an existing CHD file from an opened `core_file` object.
 *
 * Ownership is taken of the `core_file*` object and should not be modified until
 * `chd_core_file` is called to retake ownership of the `core_file*`.
 *
 * # Safety
 * * `file` is a valid pointer to a `core_file` with respect to the implementation of libchdcorefile that was linked.
 * * `parent` is either `NULL` or a valid pointer to a `chd_file` obtained from [`chd_open`](crate::chd_open), [`chd_open_file`](crate::chd_open_file), or [`chd_open_core_file`](crate::chd_open_core_file).
 * * `out` is aligned and can store a pointer to a `chd_file*`. On success, `out` will point to a valid `chd_file*`.
 * * Until the returned `chd_file*` in `out` is closed with [`chd_close`](crate::chd_close) or [`chd_core_file`](crate::chd_core_file), external mutation of `file` will result in undefined behaviour.
 * * After this function returns, `parent` is invalid and must not be used, otherwise it will be undefined behaviour. There is no way to retake ownership of `parent`.
 */
chd_error chd_open_core_file(core_file *file,
                             int mode,
                             struct chd_file *parent,
                             struct chd_file **out);

/**
 * Get the name of a particular codec.
 *
 * This method always returns the string "Unknown"
 */
const char *chd_get_codec_name(uint32_t _codec);

/**
 * Precache the underlying file into memory with an optional callback to report progress.
 *
 * The underlying stream of the input `chd_file` is swapped with a layout-undefined in-memory stream.
 *
 * If the provenance of the original `chd_file` is from [`chd_open`](crate::chd_open), then the underlying
 * stream is safely dropped.
 *
 * If instead the underlying stream is a `core_file` opened from [`chd_open_file`](crate::chd_open_file),
 * or [`chd_open_core_file`](crate::chd_open_core_file), then the same semantics of calling [`chd_core_file`](crate::chd_core_file)
 * applies, and ownership of the underlying stream is released to the caller.
 *
 * After precaching, the input `chd_file` no longer returns a valid inner stream when passed to [`chd_core_file`](crate::chd_core_file),
 * and should be treated as having the same provenance as being from [`chd_open`](crate::chd_open).
 *
 * # Safety
 * * `chd` is either `NULL` or a valid pointer to a `chd_file` obtained from [`chd_open`](crate::chd_open), [`chd_open_file`](crate::chd_open_file), or [`chd_open_core_file`](crate::chd_open_core_file).
 */
chd_error chd_precache_progress(struct chd_file *chd,
                                void (*progress)(size_t pos, size_t total, void *param),
                                void *param);

/**
 * Precache the underlying file into memory.
 *
 * The underlying stream of the input `chd_file` is swapped with a layout-undefined in-memory stream.
 *
 * If the provenance of the original `chd_file` is from [`chd_open`](crate::chd_open), then the underlying
 * stream is safely dropped.
 *
 * If instead the underlying stream is a `core_file` opened from [`chd_open_file`](crate::chd_open_file),
 * or [`chd_open_core_file`](crate::chd_open_core_file), then the same semantics of calling [`chd_core_file`](crate::chd_core_file)
 * applies, and ownership of the underlying stream is released to the caller.
 *
 * After precaching, the input `chd_file` no longer returns a valid inner stream when passed to [`chd_core_file`](crate::chd_core_file),
 * and should be treated as having the same provenance as being from [`chd_open`](crate::chd_open).
 *
 * # Safety
 * * `chd` is either `NULL` or a valid pointer to a `chd_file` obtained from [`chd_open`](crate::chd_open), [`chd_open_file`](crate::chd_open_file), or [`chd_open_core_file`](crate::chd_open_core_file).
 */
chd_error chd_precache(struct chd_file *chd);

#ifdef __cplusplus
} // extern "C"
#endif // __cplusplus

#endif /* __CHD_H__ */
