//! Hard disk CHDs.
use super::Geometry;
use crate::common::{NewMetadata, NoParent, ParallelWriter, WriteOptions};
use crate::error::{Error, Result};
use chd::metadata::{KnownMetadata, Metadata};
use chd::read::ChdReader;
use chd::write::ChdWriteOptions;
use chd::Chd;
use std::io::{self, Read, Seek, SeekFrom, Write};

/// A hard disk in a CHD.
///
/// ```no_run
/// use chd::Chd;
/// use rchdman::hd::HardDisk;
/// use std::fs::File;
/// use std::io::{self, BufReader};
///
/// # fn main() -> rchdman::Result<()> {
/// let chd = Chd::open(BufReader::new(File::open("disk.chd")?), None)?;
/// let mut disk = HardDisk::new(chd)?;
/// println!("{}", disk.geometry());
/// io::copy(&mut disk, &mut File::create("disk.img")?)?;
/// # Ok(())
/// # }
/// ```
pub struct HardDisk<F: Read + Seek> {
    reader: ChdReader<F>,
    geometry: Geometry,
    ident: Option<Vec<u8>>,
}

impl<F: Read + Seek> HardDisk<F> {
    /// The hard disk in a CHD, reading its geometry and ident from its metadata.
    pub fn new(mut chd: Chd<F>) -> Result<Self> {
        let metadata: Vec<Metadata> = chd.metadata_refs().try_into()?;
        let find = |tag: KnownMetadata| {
            metadata
                .iter()
                .find(|entry| entry.metatag == tag as u32 && entry.index == 0)
        };
        let geometry = find(KnownMetadata::HardDisk).ok_or_else(|| {
            Error::InvalidMetadata("Unable to find hard disk metadata in the CHD".into())
        })?;
        Ok(HardDisk {
            geometry: Geometry::from_metadata(&geometry.value)?,
            ident: find(KnownMetadata::HardDiskIdent).map(|entry| entry.value.clone()),
            reader: ChdReader::new(chd),
        })
    }

    /// The geometry of the hard disk.
    pub fn geometry(&self) -> Geometry {
        self.geometry
    }

    /// The IDE identify block of the hard disk.
    pub fn ident(&self) -> Option<&[u8]> {
        self.ident.as_deref()
    }
}

impl<F: Read + Seek> Read for HardDisk<F> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.reader.read(buf)
    }
}

impl<F: Read + Seek> Seek for HardDisk<F> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.reader.seek(pos)
    }
}

/// Writes a hard disk to a CHD.
///
/// The data written fills the start of the hard disk, and the rest is zeroes.
///
/// ```no_run
/// use rchdman::hd::{Geometry, HardDiskWriter};
/// use rchdman::WriteOptions;
/// use std::fs::File;
/// use std::io::{self, BufWriter};
///
/// # fn main() -> rchdman::Result<()> {
/// let geometry = Geometry { cylinders: 332, heads: 16, sectors: 63, sector_bytes: 512 };
/// let file = BufWriter::new(File::create("disk.chd")?);
/// let mut disk = HardDiskWriter::new(file, geometry, None, &WriteOptions::default())?;
/// io::copy(&mut File::open("disk.img")?, &mut disk)?;
/// disk.finish()?;
/// # Ok(())
/// # }
/// ```
pub struct HardDiskWriter<W: Write + Seek> {
    inner: ParallelWriter<W>,
}

impl<W: Write + Seek> HardDiskWriter<W> {
    /// A writer of a hard disk CHD with the given geometry, and IDE identify block.
    pub fn new(
        inner: W,
        geometry: Geometry,
        ident: Option<&[u8]>,
        options: &WriteOptions,
    ) -> Result<Self> {
        Self::create(inner, geometry, ident, options, None::<&mut NoParent>)
    }

    /// A writer of a hard disk CHD that refers to data in a parent CHD.
    pub fn with_parent<F: Read + Seek>(
        inner: W,
        geometry: Geometry,
        ident: Option<&[u8]>,
        options: &WriteOptions,
        parent: &mut Chd<F>,
    ) -> Result<Self> {
        Self::create(inner, geometry, ident, options, Some(parent))
    }

    fn create<F: Read + Seek>(
        inner: W,
        geometry: Geometry,
        ident: Option<&[u8]>,
        options: &WriteOptions,
        parent: Option<&mut Chd<F>>,
    ) -> Result<Self> {
        let preset = options.resolve(
            ChdWriteOptions::raw(geometry.sector_bytes).logical_bytes(geometry.logical_bytes()),
            parent.as_ref().map(|parent| parent.header()),
        )?;
        let mut metadata = vec![NewMetadata::text(
            KnownMetadata::HardDisk as u32,
            &geometry.metadata(),
        )];
        metadata.extend(ident.map(|ident| NewMetadata {
            tag: KnownMetadata::HardDiskIdent as u32,
            value: ident.to_vec(),
            checksum: true,
        }));
        Ok(HardDiskWriter {
            inner: options.create(inner, preset, parent, metadata)?,
        })
    }

    /// Finish the CHD, filling the rest of the hard disk with zeroes.
    pub fn finish(self) -> Result<W> {
        self.inner.finish()
    }
}

impl<W: Write + Seek> Write for HardDiskWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}
