use crate::huffman::HuffmanError;
use bitreader::BitReaderError;
use std::array::TryFromSliceError;
use std::error::Error;
use std::ffi::FromBytesWithNulError;
use std::fmt::Display;
use std::io::ErrorKind;
use std::str::Utf8Error;

/// Error types that may occur when reading a CHD file or hunk.
///
/// This type tries to be ABI-compatible with [libchdr](https://github.com/rtissera/libchdr/blob/6eeb6abc4adc094d489c8ba8cafdcff9ff61251b/include/libchdr/chd.h#L258),
/// given sane defaults in the C compiler. See [repr(C) in the Rustonomicon](https://doc.rust-lang.org/nomicon/other-reprs.html#reprc) for more details.
#[derive(Debug)]
#[repr(C)]
pub enum ChdError {
    /// No error.
    /// This is only used by the C API bindings.
    None,
    /// No drive interface.
    /// This is only for C-compatibility purposes and is otherwise unused.
    NoInterface,
    /// Unable to allocate the required size of buffer.
    OutOfMemory,
    /// The file is not a valid CHD file.
    InvalidFile,
    /// An invalid parameter was provided.
    InvalidParameter,
    /// The data is invalid.
    InvalidData,
    /// The file was not found.
    FileNotFound,
    /// This CHD requires a parent CHD that was not provided.
    RequiresParent,
    /// The provided file is not writable.
    /// Since chd-rs does not implement CHD creation, this is unused.
    FileNotWriteable,
    /// An error occurred when reading this CHD file.
    ReadError,
    /// An error occurred when writing this CHD file.
    /// Since chd-rs does not implement CHD creation, this is unused.
    WriteError,
    /// An error occurred when initializing a codec.
    CodecError,
    /// The provided parent CHD is invalid.
    InvalidParent,
    /// The request hunk is out of range for this CHD file.
    HunkOutOfRange,
    /// An error occurred when decompressing a hunk.
    DecompressionError,
    /// An error occurred when compressing a hunk.
    /// Since chd-rs does not implement CHD creation, this is unused.
    CompressionError,
    /// Could not create the file.
    /// Since chd-rs does not implement CHD creation, this is unused.
    CantCreateFile,
    /// Could not verify the CHD.
    /// This is only for C-compatibility purposes and is otherwise unused.
    CantVerify,
    /// The requested operation is not supported.
    /// This is only for C-compatibility purposes and is otherwise unused.
    NotSupported,
    /// The requested metadata was not found.
    /// This is only used by the C API bindings.
    MetadataNotFound,
    /// The metadata has an invalid size.
    /// This is only for C-compatibility purposes and is otherwise unused.
    InvalidMetadataSize,
    /// The CHD version of the provided file is not supported by this library.
    UnsupportedVersion,
    /// Unable to verify the CHD completely.
    /// This is only for C-compatibility purposes and is otherwise unused.
    VerifyIncomplete,
    /// The requested metadata is invalid.
    InvalidMetadata,
    /// The internal state of the decoder/encoder is invalid.
    /// This is only for C-compatibility purposes and is otherwise unused.
    InvalidState,
    /// An operation is already pending.
    /// This is only for C-compatibility purposes and is otherwise unused.
    OperationPending,
    /// No async operations are allowed.
    /// This is only for C-compatibility purposes and is otherwise unused.
    NoAsyncOperation,
    /// Decompressing the CHD requires a codec that is not supported.
    UnsupportedFormat,
    /// Unknown error.
    Unknown,
}

impl Error for ChdError {}

impl Display for ChdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChdError::None => f.write_str("no error"),
            ChdError::NoInterface => f.write_str("no drive interface"),
            ChdError::OutOfMemory => f.write_str("out of memory"),
            ChdError::InvalidFile => f.write_str("invalid file"),
            ChdError::InvalidParameter => f.write_str("invalid parameter"),
            ChdError::InvalidData => f.write_str("invalid data"),
            ChdError::FileNotFound => f.write_str("file not found"),
            ChdError::RequiresParent => f.write_str("requires parent"),
            ChdError::FileNotWriteable => f.write_str("file not writeable"),
            ChdError::ReadError => f.write_str("read error"),
            ChdError::WriteError => f.write_str("write error"),
            ChdError::CodecError => f.write_str("codec error"),
            ChdError::InvalidParent => f.write_str("invalid parent"),
            ChdError::HunkOutOfRange => f.write_str("hunk out of range"),
            ChdError::DecompressionError => f.write_str("decompression error"),
            ChdError::CompressionError => f.write_str("compression error"),
            ChdError::CantCreateFile => f.write_str("can't create file"),
            ChdError::CantVerify => f.write_str("can't verify file"),
            ChdError::NotSupported => f.write_str("operation not supported"),
            ChdError::MetadataNotFound => f.write_str("can't find metadata"),
            ChdError::InvalidMetadataSize => f.write_str("invalid metadata size"),
            ChdError::UnsupportedVersion => f.write_str("unsupported CHD version"),
            ChdError::VerifyIncomplete => f.write_str("incomplete verify"),
            ChdError::InvalidMetadata => f.write_str("invalid metadata"),
            ChdError::InvalidState => f.write_str("invalid state"),
            ChdError::OperationPending => f.write_str("operation pending"),
            ChdError::NoAsyncOperation => f.write_str("no async operation in progress"),
            ChdError::UnsupportedFormat => f.write_str("unsupported format"),
            ChdError::Unknown => f.write_str("undocumented error"),
        }
    }
}

impl From<TryFromSliceError> for ChdError {
    fn from(_: TryFromSliceError) -> Self {
        return ChdError::InvalidFile;
    }
}

impl From<BitReaderError> for ChdError {
    fn from(_: BitReaderError) -> Self {
        return ChdError::ReadError;
    }
}

impl From<FromBytesWithNulError> for ChdError {
    fn from(_: FromBytesWithNulError) -> Self {
        return ChdError::InvalidData;
    }
}

impl From<Utf8Error> for ChdError {
    fn from(_: Utf8Error) -> Self {
        return ChdError::InvalidData;
    }
}

impl From<std::io::Error> for ChdError {
    fn from(err: std::io::Error) -> Self {
        match err.kind() {
            ErrorKind::NotFound => ChdError::FileNotFound,
            ErrorKind::PermissionDenied => ChdError::NotSupported,
            ErrorKind::ConnectionRefused => ChdError::Unknown,
            ErrorKind::ConnectionReset => ChdError::Unknown,
            ErrorKind::ConnectionAborted => ChdError::Unknown,
            ErrorKind::NotConnected => ChdError::Unknown,
            ErrorKind::AddrInUse => ChdError::Unknown,
            ErrorKind::AddrNotAvailable => ChdError::Unknown,
            ErrorKind::BrokenPipe => ChdError::Unknown,
            ErrorKind::AlreadyExists => ChdError::CantCreateFile,
            ErrorKind::WouldBlock => ChdError::Unknown,
            ErrorKind::InvalidInput => ChdError::InvalidParameter,
            ErrorKind::InvalidData => ChdError::InvalidData,
            ErrorKind::TimedOut => ChdError::Unknown,
            ErrorKind::WriteZero => ChdError::WriteError,
            ErrorKind::Interrupted => ChdError::Unknown,
            ErrorKind::Other => ChdError::Unknown,
            ErrorKind::UnexpectedEof => ChdError::ReadError,
            ErrorKind::Unsupported => ChdError::NotSupported,
            ErrorKind::OutOfMemory => ChdError::OutOfMemory,
            _ => ChdError::Unknown,
        }
    }
}

impl From<HuffmanError> for ChdError {
    fn from(_e: HuffmanError) -> Self {
        ChdError::DecompressionError
    }
}

impl From<ChdError> for std::io::Error {
    fn from(e: ChdError) -> Self {
        std::io::Error::new(ErrorKind::Other, e)
    }
}

/// Result type for chd-rs.
pub type Result<T> = std::result::Result<T, ChdError>;
