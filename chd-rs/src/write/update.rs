//! Implementations to edit existing CHD files in place.
use super::metadata::{MetadataList, METADATA_FLAG_CHECKSUM};
use crate::error::{Error, Result};
use crate::header::Header;
use crate::metadata::{MetadataData, MetadataTag};
use crate::Chd;
use std::io::{Read, Seek, SeekFrom, Write};

impl<F: Read + Seek> Chd<F> {
    /// Computes the overall SHA1 of the CHD from the SHA1 of its data in the header and its
    /// checksummed metadata, to check against the SHA1 in the header.
    ///
    /// The SHA1 of V3 CHDs does not include metadata, so for V3 CHDs, this is the SHA1 of
    /// the data. Returns [`UnsupportedVersion`](Error::UnsupportedVersion) for CHDs older than V3.
    pub fn overall_sha1(&mut self) -> Result<[u8; 20]> {
        let raw_sha1 = self
            .header
            .raw_sha1()
            .or(self.header.sha1())
            .ok_or(Error::UnsupportedVersion)?;
        match self.header {
            Header::V3Header(_) => Ok(raw_sha1),
            _ => Ok(self.metadata_list()?.overall_sha1(raw_sha1)),
        }
    }

    /// Read every metadata entry of the CHD.
    fn metadata_list(&mut self) -> Result<MetadataList> {
        MetadataList::read(self)
    }
}

/// Editing CHD files in place.
impl<F: Read + Write + Seek> Chd<F> {
    /// Set the `index`-th metadata entry with the tag, or add a new entry if there is no
    /// such entry.
    ///
    /// If the tag is 0, or if the data is empty or 16MB or larger,
    /// returns [`InvalidParameter`](Error::InvalidParameter).
    pub fn set_metadata(
        &mut self,
        tag: impl MetadataTag,
        index: u32,
        data: impl MetadataData,
        checksum: bool,
    ) -> Result<()> {
        let flags = if checksum { METADATA_FLAG_CHECKSUM } else { 0 };
        self.edit_metadata(|metadata| {
            metadata.set(tag.metatag(), index, data.metadata_bytes(), flags)
        })
    }

    /// Add a new metadata entry after the existing entries.
    ///
    /// See [`set_metadata`](Chd::set_metadata).
    pub fn add_metadata(
        &mut self,
        tag: impl MetadataTag,
        data: impl MetadataData,
        checksum: bool,
    ) -> Result<()> {
        self.set_metadata(tag, u32::MAX, data, checksum)
    }

    /// Remove the `index`-th metadata entry with the tag.
    ///
    /// If there is no such entry is found, returns [`MetadataNotFound`](Error::MetadataNotFound).
    pub fn remove_metadata(&mut self, tag: impl MetadataTag, index: u32) -> Result<()> {
        self.edit_metadata(|metadata| metadata.remove(tag.metatag(), index))
    }

    /// Set the data SHA1 of the data, updating the overall SHA1 to match.
    ///
    /// For V3 CHDs, whose overall SHA1 calculation does not include metadata.
    pub fn set_raw_sha1(&mut self, raw_sha1: [u8; 20]) -> Result<()> {
        let meta_offset = self.header.meta_offset().ok_or(Error::UnsupportedVersion)?;
        let metadata = self.metadata_list()?;
        self.update_header(meta_offset, raw_sha1, &metadata)
    }

    fn edit_metadata(&mut self, edit: impl FnOnce(&mut MetadataList) -> Result<()>) -> Result<()> {
        let raw_sha1 = self
            .header
            .raw_sha1()
            .or(self.header.sha1())
            .ok_or(Error::UnsupportedVersion)?;

        let mut metadata = self.metadata_list()?;
        edit(&mut metadata)?;

        let eof = self.file.seek(SeekFrom::End(0))?;
        let (meta_offset, _) = metadata.commit(&mut self.file, eof)?;
        self.update_header(meta_offset, raw_sha1, &metadata)
    }

    /// Write the header with the metadata offset, and the SHA1s for the data and metadata.
    fn update_header(
        &mut self,
        meta_offset: u64,
        raw_sha1: [u8; 20],
        metadata: &MetadataList,
    ) -> Result<()> {
        // A CHD without a SHA1 of its data, such as an uncompressed CHD from chdman, has no
        // overall SHA1 either.
        let sha1 = if raw_sha1 == [0; 20] {
            [0; 20]
        } else {
            metadata.overall_sha1(raw_sha1)
        };
        match &mut self.header {
            Header::V3Header(header) => {
                header.meta_offset = meta_offset;
                header.sha1 = raw_sha1;
            }
            Header::V4Header(header) => {
                header.meta_offset = meta_offset;
                header.raw_sha1 = raw_sha1;
                header.sha1 = sha1;
            }
            Header::V5Header(header) => {
                header.meta_offset = meta_offset;
                header.raw_sha1 = raw_sha1;
                header.sha1 = sha1;
            }
            _ => return Err(Error::UnsupportedVersion),
        }

        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&self.header.to_bytes())?;
        self.file.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::header::{CodecType, Header};
    use crate::metadata::{Metadata, METADATA_HEADER_SIZE};
    use crate::write::{ChdWriteOptions, ChdWriter};
    use crate::{Chd, Error};
    use sha1::{Digest, Sha1};
    use std::io::{Cursor, Write};

    const TAG: u32 = crate::make_tag(b"TEST");
    const OTHER: u32 = crate::make_tag(b"OTHR");

    fn data() -> Vec<u8> {
        (0..3 * 4096 + 100).map(|i| (i % 251) as u8).collect()
    }

    /// A compressed V5 CHD with the given metadata entries.
    fn v5(metadata: &[(u32, &[u8], bool)]) -> Vec<u8> {
        let options = ChdWriteOptions::new(4096, 512).compression(&[CodecType::ZLibV5]);
        let mut writer = ChdWriter::new(Cursor::new(Vec::new()), options).unwrap();
        for &(tag, value, checksum) in metadata {
            writer.add_metadata(tag, value, checksum).unwrap();
        }
        writer.write_all(&data()).unwrap();
        writer.finish().unwrap().into_inner()
    }

    /// A V3 or V4 CHD with two hunks stored in the map, and no metadata.
    fn legacy(version: u32) -> Vec<u8> {
        let mut raw = Vec::new();
        raw.extend_from_slice(b"MComprHD");
        raw.extend_from_slice(&if version == 3 { 120u32 } else { 108 }.to_be_bytes());
        raw.extend_from_slice(&version.to_be_bytes());
        for value in [0u32, 1, 2] {
            // flags, zlib, total hunks
            raw.extend_from_slice(&value.to_be_bytes());
        }
        raw.extend_from_slice(&(2u64 * 4096).to_be_bytes()); // logical bytes
        raw.extend_from_slice(&0u64.to_be_bytes()); // metadata offset
        if version == 3 {
            raw.extend([0; 32]); // MD5 and parent MD5
        }
        raw.extend_from_slice(&4096u32.to_be_bytes()); // hunk bytes
        raw.extend([0x55; 20]); // SHA1
        raw.extend([0; 20]); // parent SHA1
        if version == 4 {
            raw.extend([0x66; 20]); // raw SHA1
        }

        // Hunks stored in the map entry itself, then the end of the map.
        for hunk in 0..2u64 {
            raw.extend_from_slice(&hunk.to_be_bytes()); // data
            raw.extend_from_slice(&0u32.to_be_bytes()); // CRC32
            raw.extend_from_slice(&[0, 8, 0, 3]); // length, and the mini type
        }
        raw.extend_from_slice(b"EndOfListCookie\0");
        raw
    }

    fn open(chd: Vec<u8>) -> Chd<Cursor<Vec<u8>>> {
        Chd::open(Cursor::new(chd), None).unwrap()
    }

    fn into_bytes(chd: Chd<Cursor<Vec<u8>>>) -> Vec<u8> {
        chd.into_inner().0.into_inner()
    }

    fn metadata(chd: &mut Chd<Cursor<Vec<u8>>>) -> Vec<(u32, Vec<u8>, u8)> {
        let metadata: Vec<Metadata> = chd.metadata_refs().try_into().unwrap();
        metadata
            .into_iter()
            .map(|meta| (meta.metatag, meta.value, meta.flags))
            .collect()
    }

    /// The overall SHA1 of a CHD with the given checksummed entries, computed independently.
    fn overall_sha1(raw_sha1: [u8; 20], metadata: &[(u32, &[u8])]) -> [u8; 20] {
        let mut hashes: Vec<Vec<u8>> = metadata
            .iter()
            .map(|(tag, value)| [&tag.to_be_bytes()[..], &Sha1::digest(value)[..]].concat())
            .collect();
        hashes.sort();
        let mut sha1 = Sha1::new();
        sha1.update(raw_sha1);
        for hash in hashes {
            sha1.update(hash);
        }
        sha1.finalize().into()
    }

    #[test]
    fn edits_match_writing_the_metadata() {
        let mut chd = open(v5(&[(TAG, b"one", true), (OTHER, b"unchecked", false)]));
        chd.add_metadata(TAG, b"two".as_slice(), true).unwrap();
        chd.set_metadata(TAG, 0, b"a longer first".as_slice(), true)
            .unwrap();
        chd.remove_metadata(OTHER, 0).unwrap();
        chd.add_metadata(OTHER, b"checked".as_slice(), true)
            .unwrap();

        let mut edited = open(into_bytes(chd));
        assert_eq!(
            metadata(&mut edited),
            [
                (TAG, b"a longer first".to_vec(), 1),
                (TAG, b"two".to_vec(), 1),
                (OTHER, b"checked".to_vec(), 1),
            ]
        );

        // The same SHA1s as writing the metadata in the first place.
        let written = open(v5(&[
            (TAG, b"a longer first", true),
            (TAG, b"two", true),
            (OTHER, b"checked", true),
        ]));
        assert_eq!(edited.header().sha1(), written.header().sha1());
        assert_eq!(edited.header().raw_sha1(), written.header().raw_sha1());
    }

    #[test]
    fn entries_that_grow_keep_their_place() {
        let mut chd = open(v5(&[
            (TAG, b"one", true),
            (TAG, b"two", true),
            (TAG, b"three", true),
        ]));
        let len = chd.inner().get_ref().len();
        chd.set_metadata(TAG, 1, b"two, but longer".as_slice(), true)
            .unwrap();
        chd.set_metadata(TAG, 2, b"3".as_slice(), false).unwrap();

        // Only the entry that grew is appended.
        let chd = into_bytes(chd);
        assert_eq!(chd.len(), len + METADATA_HEADER_SIZE + 15);
        assert_eq!(
            metadata(&mut open(chd)),
            [
                (TAG, b"one".to_vec(), 1),
                (TAG, b"two, but longer".to_vec(), 1),
                (TAG, b"3".to_vec(), 0),
            ]
        );
    }

    #[test]
    fn remove_every_entry() {
        let mut chd = open(v5(&[(TAG, b"one", true), (OTHER, b"two", true)]));
        chd.remove_metadata(TAG, 0).unwrap();
        chd.remove_metadata(OTHER, 0).unwrap();
        assert!(matches!(
            chd.remove_metadata(TAG, 0),
            Err(Error::MetadataNotFound)
        ));
        assert_eq!(chd.header().meta_offset(), Some(0));

        let raw_sha1 = chd.header().raw_sha1().unwrap();
        let mut edited = open(into_bytes(chd));
        assert!(metadata(&mut edited).is_empty());
        assert_eq!(edited.header().sha1(), Some(overall_sha1(raw_sha1, &[])));
    }

    #[test]
    fn invalid_metadata() {
        let mut chd = open(v5(&[]));
        assert!(matches!(
            chd.add_metadata(0u32, b"zero tag".as_slice(), true),
            Err(Error::InvalidParameter)
        ));
        assert!(matches!(
            chd.add_metadata(TAG, b"".as_slice(), true),
            Err(Error::InvalidParameter)
        ));
    }

    #[test]
    fn set_raw_sha1() {
        let original = v5(&[(TAG, b"one", true)]);
        let mut chd = open(original.clone());
        let raw_sha1 = chd.header().raw_sha1().unwrap();
        assert_eq!(chd.overall_sha1().unwrap(), chd.header().sha1().unwrap());

        chd.set_raw_sha1([0x12; 20]).unwrap();
        let mut broken = open(into_bytes(chd));
        assert_eq!(broken.header().raw_sha1(), Some([0x12; 20]));
        assert_eq!(
            broken.header().sha1(),
            Some(overall_sha1([0x12; 20], &[(TAG, b"one")]))
        );

        // Fixing it again restores the original file.
        broken.set_raw_sha1(raw_sha1).unwrap();
        assert!(into_bytes(broken) == original);
    }

    #[test]
    fn legacy_versions() {
        for version in [3, 4] {
            let mut chd = open(legacy(version));
            chd.add_metadata(TAG, b"one".as_slice(), true).unwrap();
            chd.add_metadata(OTHER, b"two".as_slice(), false).unwrap();
            chd.set_metadata(TAG, 0, b"one, but longer".as_slice(), true)
                .unwrap();

            let mut edited = open(into_bytes(chd));
            assert_eq!(
                metadata(&mut edited),
                [
                    (TAG, b"one, but longer".to_vec(), 1),
                    (OTHER, b"two".to_vec(), 0)
                ],
                "version {version}"
            );

            match edited.header().clone() {
                // The SHA1 of V3 CHDs does not include metadata.
                Header::V3Header(header) => {
                    assert_eq!(header.sha1, [0x55; 20]);
                    assert_eq!(edited.overall_sha1().unwrap(), [0x55; 20]);
                    edited.set_raw_sha1([0x77; 20]).unwrap();
                    assert_eq!(edited.header().sha1(), Some([0x77; 20]));
                }
                Header::V4Header(header) => {
                    let expected = overall_sha1([0x66; 20], &[(TAG, b"one, but longer")]);
                    assert_eq!(header.raw_sha1, [0x66; 20]);
                    assert_eq!(header.sha1, expected);
                    assert_eq!(edited.overall_sha1().unwrap(), expected);
                }
                _ => panic!("version {version}"),
            }
        }
    }
}
