#ifndef __CHD_H__
#define __CHD_H__

#pragma once

#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

#define CHD_OPEN_READ 1

#define CHD_OPEN_READWRITE 2

#define CHD_MD5_BYTES 16

#define CHD_SHA1_BYTES 20

/**
 * Error types that may occur when reading a CHD file or hunk.
 *
 * This type tries to be ABI-compatible with [libchdr](https://github.com/rtissera/libchdr/blob/6eeb6abc4adc094d489c8ba8cafdcff9ff61251b/include/libchdr/chd.h#L258),
 * given sane defaults in the C compiler. See [repr(C) in the Rustonomicon](https://doc.rust-lang.org/nomicon/other-reprs.html#reprc) for more details.
 */
typedef enum chd_error {
  /**
   * No error.
   * This is only used by the C API bindings.
   */
  CHDERR_NONE,
  /**
   * No drive interface.
   * This is only for C-compatibility purposes and is otherwise unused.
   */
  CHDERR_NO_INTERFACE,
  /**
   * Unable to allocate the required size of buffer.
   */
  CHDERR_OUT_OF_MEMORY,
  /**
   * The file is not a valid CHD file.
   */
  CHDERR_INVALID_FILE,
  /**
   * An invalid parameter was provided.
   */
  CHDERR_INVALID_PARAMETER,
  /**
   * The data is invalid.
   */
  CHDERR_INVALID_DATA,
  /**
   * The file was not found.
   */
  CHDERR_FILE_NOT_FOUND,
  /**
   * This CHD requires a parent CHD that was not provided.
   */
  CHDERR_REQUIRES_PARENT,
  /**
   * The provided file is not writable.
   * Since chd-rs does not implement CHD creation, this is unused.
   */
  CHDERR_FILE_NOT_WRITEABLE,
  /**
   * An error occurred when reading this CHD file.
   */
  CHDERR_READ_ERROR,
  /**
   * An error occurred when writing this CHD file.
   * Since chd-rs does not implement CHD creation, this is unused.
   */
  CHDERR_WRITE_ERROR,
  /**
   * An error occurred when initializing a codec.
   */
  CHDERR_CODEC_ERROR,
  /**
   * The provided parent CHD is invalid.
   */
  CHDERR_INVALID_PARENT,
  /**
   * The request hunk is out of range for this CHD file.
   */
  CHDERR_HUNK_OUT_OF_RANGE,
  /**
   * An error occurred when decompressing a hunk.
   */
  CHDERR_DECOMPRESSION_ERROR,
  /**
   * An error occurred when compressing a hunk.
   * Since chd-rs does not implement CHD creation, this is unused.
   */
  CHDERR_COMPRESSION_ERROR,
  /**
   * Could not create the file.
   * Since chd-rs does not implement CHD creation, this is unused.
   */
  CHDERR_CANT_CREATE_FILE,
  /**
   * Could not verify the CHD.
   * This is only for C-compatibility purposes and is otherwise unused.
   */
  CHDERR_CANT_VERIFY,
  /**
   * The requested operation is not supported.
   * This is only for C-compatibility purposes and is otherwise unused.
   */
  CHDERR_NOT_SUPPORTED,
  /**
   * The requested metadata was not found.
   * This is only used by the C API bindings.
   */
  CHDERR_METADATA_NOT_FOUND,
  /**
   * The metadata has an invalid size.
   * This is only for C-compatibility purposes and is otherwise unused.
   */
  CHDERR_INVALID_METADATA_SIZE,
  /**
   * The CHD version of the provided file is not supported by this library.
   */
  CHDERR_UNSUPPORTED_VERSION,
  /**
   * Unable to verify the CHD completely.
   * This is only for C-compatibility purposes and is otherwise unused.
   */
  CHDERR_VERIFY_INCOMPLETE,
  /**
   * The requested metadata is invalid.
   */
  CHDERR_INVALID_METADATA,
  /**
   * The internal state of the decoder/encoder is invalid.
   * This is only for C-compatibility purposes and is otherwise unused.
   */
  CHDERR_INVALID_STATE,
  /**
   * An operation is already pending.
   * This is only for C-compatibility purposes and is otherwise unused.
   */
  CHDERR_OPERATION_PENDING,
  /**
   * No async operations are allowed.
   * This is only for C-compatibility purposes and is otherwise unused.
   */
  CHDERR_NO_ASYNC_OPERATION,
  /**
   * Decompressing the CHD requires a codec that is not supported.
   */
  CHDERR_UNSUPPORTED_FORMAT,
  /**
   * Unknown error.
   */
  CHDERR_UNKNOWN,
} chd_error;

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

chd_error chd_open(const char *filename, int mode, struct chd_file *parent, struct chd_file **out);

/**
 * Close a CHD file.
 */
void chd_close(struct chd_file *chd);

/**
 * Returns an error string for the corresponding CHD error.
 */
const char *chd_error_string(chd_error err);

/**
 * Returns a pointer to the extracted CHD header data.
 */
const struct chd_header *chd_get_header(const struct chd_file *chd);

/**
 * Read a single hunk from the CHD file.
 * The output buffer must be initialized and have a length
 * of exactly the hunk size, or it is undefined behaviour.
 */
chd_error chd_read(struct chd_file *chd, uint32_t hunknum, void *buffer);

/**
 * Get indexed metadata of the given search tag and index.
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
 * This function is not supported and always returns CHDERR_INVALID_PARAMETER.
 */
chd_error chd_codec_config(const struct chd_file *_chd, int32_t _param, void *_config);

/**
 * Read CHD header data from the file into the pointed struct.
 */
chd_error chd_read_header(const char *filename, struct chd_header *header);

/**
 * Returns the associated core_file.
 *
 * This method has different semantics than `chd_core_file` in libchdr.
 *
 * The input `chd_file*` will be dropped, and all prior references to
 * to the input `chd_file*` are rendered invalid, with the same semantics as `chd_close`.
 *
 * The provenance of the `chd_file*` is important to keep in mind.
 *
 * If the input `chd_file*` was opened with `chd_open`, the input `chd_file*` will be closed,
 * and the return value will be undefined. For now it is `NULL`, but this may change in the future.
 *
 * If the input `chd_file*` was opened with `chd_open_file` and the `chd_core_file` crate feature
 * is enabled, this method will return the same pointer as passed to `chd_input_file`, which may
 * be possible to cast to `FILE*` depending on the implementation of `libchdcorefile` that was
 * linked.
 */
core_file *chd_core_file(struct chd_file *chd);

/**
 * Open an existing CHD file from an opened `core_file` object.
 *
 * Ownership is taken of the `core_file*` object and should not be modified until
 * `chd_core_file` is called to retake ownership of the `core_file*`.
 */
chd_error chd_open_file(core_file *file, int mode, struct chd_file *parent, struct chd_file **out);

/**
 * Get the name of a particular codec.
 *
 * This method always returns the string "Unknown"
 */
const char *chd_get_codec_name(uint32_t _codec);

#ifdef __cplusplus
} // extern "C"
#endif // __cplusplus

#endif /* __CHD_H__ */
