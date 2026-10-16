//! A Rust implementation of MAME's chdman.
//!
//! The commands of chdman are in [`chdman`], and the formats of CD-ROM and hard disk images
//! are in [`cd`] and [`hd`].
//!
//! LaserDisc images (`createld` and `extractld`), GD-ROM images, and Nero, cdrdao, or
//! `WAVE` inputs for CD-ROM images are not supported.
pub mod cd;
pub mod chdman;
mod common;
mod error;
pub mod hd;

pub use common::{InputSpan, Position, Progress, WriteOptions};
#[cfg(feature = "cli")]
pub use error::CommandError;
pub use error::{Error, Result};
