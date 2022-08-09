use crate::huffman::HuffmanError;
use bitreader::BitReaderError;
use std::array::TryFromSliceError;
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
pub enum Error {
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

impl std::error::Error for Error {}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::None => f.write_str("no error"),
            Error::NoInterface => f.write_str("no drive interface"),
            Error::OutOfMemory => f.write_str("out of memory"),
            Error::InvalidFile => f.write_str("invalid file"),
            Error::InvalidParameter => f.write_str("invalid parameter"),
            Error::InvalidData => f.write_str("invalid data"),
            Error::FileNotFound => f.write_str("file not found"),
            Error::RequiresParent => f.write_str("requires parent"),
            Error::FileNotWriteable => f.write_str("file not writeable"),
            Error::ReadError => f.write_str("read error"),
            Error::WriteError => f.write_str("write error"),
            Error::CodecError => f.write_str("codec error"),
            Error::InvalidParent => f.write_str("invalid parent"),
            Error::HunkOutOfRange => f.write_str("hunk out of range"),
            Error::DecompressionError => f.write_str("decompression error"),
            Error::CompressionError => f.write_str("compression error"),
            Error::CantCreateFile => f.write_str("can't create file"),
            Error::CantVerify => f.write_str("can't verify file"),
            Error::NotSupported => f.write_str("operation not supported"),
            Error::MetadataNotFound => f.write_str("can't find metadata"),
            Error::InvalidMetadataSize => f.write_str("invalid metadata size"),
            Error::UnsupportedVersion => f.write_str("unsupported CHD version"),
            Error::VerifyIncomplete => f.write_str("incomplete verify"),
            Error::InvalidMetadata => f.write_str("invalid metadata"),
            Error::InvalidState => f.write_str("invalid state"),
            Error::OperationPending => f.write_str("operation pending"),
            Error::NoAsyncOperation => f.write_str("no async operation in progress"),
            Error::UnsupportedFormat => f.write_str("unsupported format"),
            Error::Unknown => f.write_str("undocumented error"),
        }
    }
}

impl From<TryFromSliceError> for Error {
    fn from(_: TryFromSliceError) -> Self {
        Error::InvalidFile
    }
}

impl From<BitReaderError> for Error {
    fn from(_: BitReaderError) -> Self {
        Error::ReadError
    }
}

impl From<FromBytesWithNulError> for Error {
    fn from(_: FromBytesWithNulError) -> Self {
        Error::InvalidData
    }
}

impl From<Utf8Error> for Error {
    fn from(_: Utf8Error) -> Self {
        Error::InvalidData
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        match err.kind() {
            ErrorKind::NotFound => Error::FileNotFound,
            ErrorKind::PermissionDenied => Error::NotSupported,
            ErrorKind::ConnectionRefused => Error::Unknown,
            ErrorKind::ConnectionReset => Error::Unknown,
            ErrorKind::ConnectionAborted => Error::Unknown,
            ErrorKind::NotConnected => Error::Unknown,
            ErrorKind::AddrInUse => Error::Unknown,
            ErrorKind::AddrNotAvailable => Error::Unknown,
            ErrorKind::BrokenPipe => Error::Unknown,
            ErrorKind::AlreadyExists => Error::CantCreateFile,
            ErrorKind::WouldBlock => Error::Unknown,
            ErrorKind::InvalidInput => Error::InvalidParameter,
            ErrorKind::InvalidData => Error::InvalidData,
            ErrorKind::TimedOut => Error::Unknown,
            ErrorKind::WriteZero => Error::WriteError,
            ErrorKind::Interrupted => Error::Unknown,
            ErrorKind::Other => Error::Unknown,
            ErrorKind::UnexpectedEof => Error::ReadError,
            ErrorKind::Unsupported => Error::NotSupported,
            ErrorKind::OutOfMemory => Error::OutOfMemory,
            _ => Error::Unknown,
        }
    }
}

impl From<HuffmanError> for Error {
    fn from(_e: HuffmanError) -> Self {
        Error::DecompressionError
    }
}

impl From<Error> for std::io::Error {
    fn from(e: Error) -> Self {
        std::io::Error::new(ErrorKind::Other, e)
    }
}

/// Result type for chd-rs.
pub type Result<T> = std::result::Result<T, Error>;
