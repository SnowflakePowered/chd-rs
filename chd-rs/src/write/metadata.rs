//! Metadata encode handling.
use crate::error::{Error, Result};
use crate::metadata::{RawMetadataHeader, METADATA_HEADER_SIZE};
use crate::Chd;
use sha1::{Digest, Sha1};
use std::io::{Read, Seek, SeekFrom, Write};
use zerocopy::IntoBytes;

/// The metadata flag for entries that are included in the SHA1 of a CHD.
pub(super) const METADATA_FLAG_CHECKSUM: u8 = 0x01;

/// Metadata entries must be smaller than 16MB.
const METADATA_MAX_LENGTH: usize = 16 * 1024 * 1024;

/// A metadata entry that is written to a CHD file.
struct MetadataEntry {
    tag: u32,
    flags: u8,
    data: Vec<u8>,
    /// Where the entry was written, and how many bytes of data fit there.
    location: Option<(u64, usize)>,
}

/// The metadata entries of a CHD being written.
#[derive(Default)]
pub(super) struct MetadataList {
    entries: Vec<MetadataEntry>,
}

impl MetadataList {
    /// Read the entries of an existing CHD.
    pub(super) fn read<F: Read + Seek>(chd: &mut Chd<F>) -> Result<Self> {
        let refs = chd.metadata_refs().collect::<Result<Vec<_>>>()?;
        let entries = refs
            .iter()
            .map(|meta_ref| {
                let metadata = meta_ref.read(&mut chd.file)?;
                Ok(MetadataEntry {
                    tag: metadata.metatag,
                    flags: metadata.flags,
                    data: metadata.value,
                    location: Some((meta_ref.offset, meta_ref.length as usize)),
                })
            })
            .collect::<Result<_>>()?;
        Ok(MetadataList { entries })
    }

    /// The position of the `index`-th entry with the tag.
    fn find(&self, tag: u32, index: u32) -> Option<usize> {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.tag == tag)
            .nth(index as usize)
            .map(|(position, _)| position)
    }

    /// Set the `index`-th entry with the tag, or add a new entry if there is no such entry.
    pub(super) fn set(&mut self, tag: u32, index: u32, data: &[u8], flags: u8) -> Result<()> {
        // A tag of 0 is the wildcard for searching metadata.
        if tag == 0 || data.is_empty() || data.len() >= METADATA_MAX_LENGTH {
            return Err(Error::InvalidParameter);
        }

        match self.find(tag, index) {
            Some(position) => {
                let entry = &mut self.entries[position];
                entry.flags = flags;
                entry.data = data.to_vec();
            }
            None => self.entries.push(MetadataEntry {
                tag,
                flags,
                data: data.to_vec(),
                location: None,
            }),
        }
        Ok(())
    }

    /// Remove the `index`-th entry with the tag.
    pub(super) fn remove(&mut self, tag: u32, index: u32) -> Result<()> {
        let position = self.find(tag, index).ok_or(Error::MetadataNotFound)?;
        self.entries.remove(position);
        Ok(())
    }

    /// Write the entries that have not been written yet starting at `eof`.
    ///
    /// Returns the new end of file position.
    pub(super) fn write_unwritten<W: Write + Seek>(
        &mut self,
        file: &mut W,
        mut eof: u64,
    ) -> Result<u64> {
        for entry in self
            .entries
            .iter_mut()
            .filter(|entry| entry.location.is_none())
        {
            let header = RawMetadataHeader::new(entry.tag, entry.flags, entry.data.len() as u32, 0);
            file.seek(SeekFrom::Start(eof))?;
            file.write_all(header.as_bytes())?;
            file.write_all(&entry.data)?;
            entry.location = Some((eof, entry.data.len()));
            eof += (METADATA_HEADER_SIZE + entry.data.len()) as u64;
        }
        Ok(eof)
    }

    /// Compute the CHD overall SHA1.
    ///
    /// This is the hash of the SHA1 of the raw data, and the SHA1 of each
    /// checksummed metadata entry in sorted order.
    pub(super) fn overall_sha1(&self, raw_sha1: [u8; 20]) -> [u8; 20] {
        // https://github.com/mamedev/mame/blob/74b9fae5a3ca3c99828fe41da154e3c84951414e/src/lib/util/chd.cpp#L1839
        // Each entry is hashed as its tag followed by the SHA1 of its data, sorted by those bytes.
        let mut hashes: Vec<[u8; 24]> = self
            .entries
            .iter()
            .filter(|entry| entry.flags & METADATA_FLAG_CHECKSUM != 0)
            .map(|entry| {
                let mut hash = [0u8; 24];
                hash[..4].copy_from_slice(&entry.tag.to_be_bytes());
                hash[4..].copy_from_slice(&Sha1::digest(&entry.data));
                hash
            })
            .collect();
        hashes.sort_unstable();

        let mut sha1 = Sha1::new();
        sha1.update(raw_sha1);
        for hash in &hashes {
            sha1.update(hash);
        }
        sha1.finalize().into()
    }

    /// Write every entry and link them in order. New entries or those that don't fit
    /// where they were previously written get written starting at `eof`.
    ///
    /// Returns the offset of the first entry or 0 if there are none, and the new end of the file.
    pub(super) fn commit<W: Write + Seek>(&mut self, file: &mut W, eof: u64) -> Result<(u64, u64)> {
        for entry in &mut self.entries {
            if entry
                .location
                .is_some_and(|(_, capacity)| entry.data.len() > capacity)
            {
                entry.location = None;
            }
        }
        let eof = self.write_unwritten(file, eof)?;

        let offsets: Vec<u64> = self
            .entries
            .iter()
            .filter_map(|entry| entry.location.map(|(offset, _)| offset))
            .collect();
        for (position, entry) in self.entries.iter().enumerate() {
            let next = offsets.get(position + 1).copied().unwrap_or(0);
            let header =
                RawMetadataHeader::new(entry.tag, entry.flags, entry.data.len() as u32, next);
            file.seek(SeekFrom::Start(offsets[position]))?;
            file.write_all(header.as_bytes())?;
            file.write_all(&entry.data)?;
        }
        Ok((offsets.first().copied().unwrap_or(0), eof))
    }
}

#[cfg(test)]
mod tests {
    use crate::metadata::{KnownMetadata, Metadata};
    use crate::write::{ChdWriteOptions, ChdWriter};
    use crate::{Chd, Error};
    use std::io::Cursor;

    const HUNK_BYTES: u32 = 4096;
    const TAG: u32 = crate::make_tag(b"TEST");

    fn writer() -> ChdWriter<Cursor<Vec<u8>>> {
        ChdWriter::new(
            Cursor::new(Vec::new()),
            ChdWriteOptions::new(HUNK_BYTES, 512),
        )
        .unwrap()
    }

    /// The metadata entries of a finished CHD, as (tag, flags, value).
    fn read(writer: ChdWriter<Cursor<Vec<u8>>>) -> (Vec<(u32, u8, Vec<u8>)>, u64) {
        let mut chd = Chd::open(writer.finish().unwrap(), None).unwrap();
        let meta_offset = chd.header().meta_offset().unwrap();
        let entries: Vec<Metadata> = chd.metadata_refs().try_into().unwrap();
        let entries = entries
            .into_iter()
            .map(|entry| (entry.metatag, entry.flags, entry.value))
            .collect();
        (entries, meta_offset)
    }

    #[test]
    fn metadata_before_hunks() {
        let mut writer = writer();
        writer
            .add_metadata(
                KnownMetadata::HardDisk,
                c"CYLS:1,HEADS:1,SECS:8,BPS:512",
                true,
            )
            .unwrap();
        writer
            .add_metadata(TAG, [1u8, 2, 3].as_slice(), false)
            .unwrap();
        writer.write_hunk(&[1; HUNK_BYTES as usize]).unwrap();

        let (entries, meta_offset) = read(writer);
        // Metadata directly follows the header, like chdman.
        assert_eq!(meta_offset, 124);
        assert_eq!(
            entries,
            vec![
                (
                    KnownMetadata::HardDisk as u32,
                    1,
                    b"CYLS:1,HEADS:1,SECS:8,BPS:512\0".to_vec()
                ),
                (TAG, 0, vec![1, 2, 3]),
            ]
        );
    }

    #[test]
    fn metadata_after_hunks() {
        let mut writer = writer();
        writer.add_metadata(TAG, [1u8].as_slice(), true).unwrap();
        writer.write_hunk(&[1; HUNK_BYTES as usize]).unwrap();
        writer.add_metadata(TAG, [2u8].as_slice(), true).unwrap();

        let mut chd = Chd::open(writer.finish().unwrap(), None).unwrap();
        let mut hunk = chd.get_hunksized_buffer();
        chd.hunk(0)
            .unwrap()
            .read_hunk_in(&mut Vec::new(), &mut hunk)
            .unwrap();
        assert!(hunk.iter().all(|&byte| byte == 1));

        let entries: Vec<Metadata> = chd.metadata_refs().try_into().unwrap();
        let values: Vec<_> = entries.iter().map(|entry| entry.value.clone()).collect();
        assert_eq!(values, vec![vec![1], vec![2]]);
    }

    #[test]
    fn overwrite_metadata() {
        let mut writer = writer();
        for value in [b"first".as_slice(), b"second", b"third"] {
            writer.add_metadata(TAG, value, true).unwrap();
        }
        writer.write_hunk(&[1; HUNK_BYTES as usize]).unwrap();

        // Shrinks in place, grows past its space, and replaces before it was written.
        writer
            .set_metadata(TAG, 0, b"1st".as_slice(), true)
            .unwrap();
        writer
            .set_metadata(TAG, 1, b"a much longer second".as_slice(), false)
            .unwrap();
        writer
            .set_metadata(TAG, 3, b"fourth".as_slice(), true)
            .unwrap();
        writer
            .set_metadata(TAG, 3, b"4th".as_slice(), true)
            .unwrap();

        let (entries, _) = read(writer);
        let entries: Vec<_> = entries
            .into_iter()
            .map(|(_, flags, value)| (flags, value))
            .collect();
        // Entries keep their place in the list, so their indices do not change.
        assert_eq!(
            entries,
            vec![
                (1, b"1st".to_vec()),
                (0, b"a much longer second".to_vec()),
                (1, b"third".to_vec()),
                (1, b"4th".to_vec()),
            ]
        );
    }

    #[test]
    fn remove_metadata() {
        let mut writer = writer();
        writer.add_metadata(TAG, [1u8].as_slice(), true).unwrap();
        writer
            .add_metadata(KnownMetadata::HardDisk, [2u8].as_slice(), true)
            .unwrap();
        writer.add_metadata(TAG, [3u8].as_slice(), true).unwrap();
        writer.write_hunk(&[1; HUNK_BYTES as usize]).unwrap();

        writer.remove_metadata(TAG, 0).unwrap();
        assert!(matches!(
            writer.remove_metadata(TAG, 1),
            Err(Error::MetadataNotFound)
        ));

        let (entries, _) = read(writer);
        let values: Vec<_> = entries.into_iter().map(|(_, _, value)| value).collect();
        assert_eq!(values, vec![vec![2], vec![3]]);
    }

    #[test]
    fn remove_all_metadata() {
        let mut writer = writer();
        writer.add_metadata(TAG, [1u8].as_slice(), true).unwrap();
        writer.write_hunk(&[1; HUNK_BYTES as usize]).unwrap();
        writer.remove_metadata(TAG, 0).unwrap();

        let (entries, meta_offset) = read(writer);
        assert!(entries.is_empty());
        assert_eq!(meta_offset, 0);
    }

    #[test]
    fn invalid_metadata() {
        let mut writer = writer();
        assert!(matches!(
            writer.add_metadata(0u32, [1u8].as_slice(), true),
            Err(Error::InvalidParameter)
        ));
        assert!(matches!(
            writer.add_metadata(TAG, [].as_slice(), true),
            Err(Error::InvalidParameter)
        ));
        let too_large = vec![0u8; 16 * 1024 * 1024];
        assert!(matches!(
            writer.add_metadata(TAG, too_large.as_slice(), true),
            Err(Error::InvalidParameter)
        ));
    }
}
