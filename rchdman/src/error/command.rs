//! The error type of the rchdman command line.
use super::Error;
use std::fmt::{Display, Formatter};
use std::io;

/// An error from the rchdman command line.
#[derive(Debug)]
#[non_exhaustive]
pub enum CommandError {
    /// A command failed.
    Command(Error),
    /// A metadata tag is not between 1 and 4 characters.
    InvalidTag(String),
    /// A codec is unknown, or can not be used to write a CHD.
    InvalidCodec(String),
    /// More than 4 codecs were given.
    TooManyCodecs,
    /// CHS values are not of the form `<cylinders>,<heads>,<sectors>`.
    InvalidChs(String),
    /// There is no hard disk template with the ID.
    InvalidTemplate(String),
    /// Writing the output of a command failed.
    Io(io::Error),
}

impl Display for CommandError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            CommandError::Command(error) => write!(f, "{error}"),
            CommandError::InvalidTag(tag) => write!(
                f,
                "Invalid tag '{tag}'; tag must be between 1 and 4 characters"
            ),
            CommandError::InvalidCodec(name) => write!(f, "Invalid compressor '{name}' specified"),
            CommandError::TooManyCodecs => {
                f.write_str("Too many compressors specified (maximum 4)")
            }
            CommandError::InvalidChs(chs) => write!(
                f,
                "Invalid CHS string '{chs}'; must be of the form <cylinders>,<heads>,<sectors>"
            ),
            CommandError::InvalidTemplate(id) => write!(f, "Template '{id}' is invalid"),
            CommandError::Io(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for CommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CommandError::Command(error) => Some(error),
            CommandError::Io(source) => Some(source),
            _ => None,
        }
    }
}

impl From<Error> for CommandError {
    fn from(error: Error) -> Self {
        CommandError::Command(error)
    }
}

impl From<io::Error> for CommandError {
    fn from(error: io::Error) -> Self {
        CommandError::Io(error)
    }
}
