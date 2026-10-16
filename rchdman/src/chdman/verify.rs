//! `verify`.
use crate::common::{open_chd, open_writable, Progress};
use crate::error::{Error, Result};
use chd::iter::Lender;
use sha1::{Digest, Sha1};
use std::path::Path;

/// The SHA1s of a compressed CHD.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verification {
    /// The SHA1 of the data in the header, and as computed.
    pub raw_sha1: ([u8; 20], [u8; 20]),
    /// The overall SHA1 in the header, and as computed from the data and metadata.
    ///
    /// Only checked for V4 and newer CHDs, when the SHA1 of the data is correct.
    pub overall_sha1: Option<([u8; 20], [u8; 20])>,
}

impl Verification {
    /// Verify the SHA1s of the compressed CHD at the path, reading all of its data.
    ///
    /// Returns [`NothingToVerify`](Error::NothingToVerify) if the CHD is uncompressed or has
    /// no SHA1.
    pub fn verify(path: &Path, parent: Option<&Path>, progress: Progress) -> Result<Verification> {
        let mut chd = open_chd(path, parent)?;
        let header = chd.header().clone();
        if !header.is_compressed() {
            return Err(Error::NothingToVerify("CHD is uncompressed"));
        }
        let stored = header.raw_sha1().or(header.sha1());
        let Some(stored) = stored.filter(|sha1| *sha1 != [0; 20]) else {
            return Err(Error::NothingToVerify("CHD has no checksum"));
        };

        // The SHA1 only covers the logical size, not the padding of the last hunk.
        let logical_bytes = header.logical_bytes();
        let mut remaining = logical_bytes;
        let mut sha1 = Sha1::new();
        let mut hunk_buf = chd.get_hunksized_buffer();
        let mut cmp_buf = Vec::new();
        let mut hunks = chd.hunks();
        progress(0, logical_bytes);
        while let Some(mut hunk) = hunks.next() {
            hunk.read_hunk_in(&mut cmp_buf, &mut hunk_buf)?;
            let len = remaining.min(hunk_buf.len() as u64) as usize;
            sha1.update(&hunk_buf[..len]);
            remaining -= len as u64;
            progress(logical_bytes - remaining, logical_bytes);
        }
        let computed: [u8; 20] = sha1.finalize().into();

        // The overall SHA1 includes the SHA1 of the data, so it is only checked if that is right.
        let overall_sha1 = match header.raw_sha1() {
            Some(_) if stored == computed => {
                Some((header.sha1().unwrap_or_default(), chd.overall_sha1()?))
            }
            _ => None,
        };
        Ok(Verification {
            raw_sha1: (stored, computed),
            overall_sha1,
        })
    }

    /// Whether the SHA1s in the header are correct.
    pub fn is_valid(&self) -> bool {
        self.raw_sha1.0 == self.raw_sha1.1
            && self
                .overall_sha1
                .is_none_or(|(stored, computed)| stored == computed)
    }

    /// Write the computed SHA1s to the header of the CHD at the path.
    pub fn fix(&self, path: &Path) -> Result<()> {
        open_writable(path)?.set_raw_sha1(self.raw_sha1.1)?;
        Ok(())
    }
}
