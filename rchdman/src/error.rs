//! The error type of rchdman.
use std::fmt::{Display, Formatter};
use std::io;
use std::path::PathBuf;

#[cfg(feature = "cli")]
mod command;
#[cfg(feature = "cli")]
pub use command::CommandError;

/// The result of an rchdman operation.
pub type Result<T> = std::result::Result<T, Error>;

/// An error from an rchdman operation.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// Reading or writing a CHD failed.
    Chd(chd::Error),
    /// Reading or writing a file failed.
    Io(io::Error),
    /// A file could not be opened, created, or read.
    File {
        /// The path of the file.
        path: PathBuf,
        /// Why the file could not be used.
        source: io::Error,
    },
    /// A CHD could not be opened.
    OpenChd {
        /// The path of the CHD.
        path: PathBuf,
        /// Why the CHD could not be opened.
        source: chd::Error,
    },
    /// An output file already exists, and overwriting it was not forced.
    OutputExists(PathBuf),
    /// An option is invalid, or can not be used with the other options.
    InvalidOption(String),
    /// The size of the data is not a whole number of units.
    NotDivisible {
        /// The size of the data.
        size: u64,
        /// The size of a unit, such as a sector.
        unit_bytes: u32,
    },
    /// The input ended before all of it was read.
    InputEnded,
    /// A table of contents, such as a cue sheet, is invalid.
    InvalidToc(String),
    /// The metadata of a CHD is invalid or missing.
    InvalidMetadata(String),
    /// There is no metadata entry with the tag and index.
    MetadataNotFound {
        /// The tag of the entry.
        tag: u32,
        /// The index of the entry among the entries with the tag.
        index: u32,
    },
    /// The CHD can not be verified, because it is uncompressed or has no SHA1.
    NothingToVerify(&'static str),
    /// The input uses something that chdman supports, but rchdman does not.
    Unsupported(&'static str),
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Chd(error) => write!(f, "{error}"),
            Error::Io(error) => write!(f, "{error}"),
            Error::File { path, source } => write!(f, "{source} ({})", path.display()),
            Error::OpenChd { path, source } => {
                write!(f, "Error opening CHD file ({}): {source}", path.display())
            }
            Error::OutputExists(path) => write!(
                f,
                "file already exists ({})\nUse --force (or -f) to force overwriting",
                path.display()
            ),
            Error::InvalidOption(message)
            | Error::InvalidToc(message)
            | Error::InvalidMetadata(message) => f.write_str(message),
            Error::NotDivisible { size, unit_bytes } => {
                write!(
                    f,
                    "Data size {size} is not divisible by unit size {unit_bytes}"
                )
            }
            Error::InputEnded => f.write_str("The input ended before all of it was read"),
            Error::MetadataNotFound { tag, index } => {
                let tag = String::from_utf8_lossy(&tag.to_be_bytes()).into_owned();
                write!(f, "Can't find metadata with tag '{tag}' and index {index}")
            }
            Error::NothingToVerify(reason) => write!(f, "No verification to be done; {reason}"),
            Error::Unsupported(what) => f.write_str(what),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Chd(error) | Error::OpenChd { source: error, .. } => Some(error),
            Error::Io(error) | Error::File { source: error, .. } => Some(error),
            _ => None,
        }
    }
}

impl From<chd::Error> for Error {
    fn from(error: chd::Error) -> Self {
        Error::Chd(error)
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Error::Io(error)
    }
}

impl Error {
    /// An invalid option.
    pub(crate) fn invalid(message: impl Into<String>) -> Error {
        Error::InvalidOption(message.into())
    }

    /// An error using a file.
    pub(crate) fn file(path: impl Into<PathBuf>) -> impl FnOnce(io::Error) -> Error {
        let path = path.into();
        move |source| Error::File { path, source }
    }
}
