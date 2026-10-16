//! A Rust implementation of the CHDMAN utility commands, including
//! CD TOC and hard disk parsers.
pub mod cd;
pub mod chdman;
mod common;
mod error;
pub mod hd;

pub use common::{InputSpan, Position, Progress, WriteOptions};
#[cfg(feature = "cli")]
pub use error::CommandError;
pub use error::{Error, Result};
