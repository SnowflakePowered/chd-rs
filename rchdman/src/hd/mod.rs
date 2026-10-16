//! Hard disk geometry and metadata.
use crate::error::{Error, Result};
use std::fmt::{Display, Formatter};

mod image;

pub use image::{HardDisk, HardDiskWriter};

/// The size of an IDE sector in bytes.
pub(crate) const IDE_SECTOR_SIZE: u32 = 512;

/// The geometry of a hard disk, stored in its `GDDD` metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    /// The number of cylinders.
    pub cylinders: u32,
    /// The number of heads.
    pub heads: u32,
    /// The number of sectors per track.
    pub sectors: u32,
    /// The size of a sector in bytes.
    pub sector_bytes: u32,
}

impl Geometry {
    /// The total number of sectors.
    pub fn total_sectors(&self) -> u64 {
        self.cylinders as u64 * self.heads as u64 * self.sectors as u64
    }

    /// The size of the hard disk in bytes.
    pub fn logical_bytes(&self) -> u64 {
        self.total_sectors() * self.sector_bytes as u64
    }

    /// Guess the geometry of a hard disk of the given size.
    ///
    /// Picks the most sectors per track (up to 63), then the most heads (up to 16), that
    /// exactly fit the number of sectors, rounding up sectors otherwise.
    pub(crate) fn guess(bytes: u64, sector_bytes: u32) -> Result<Geometry> {
        if bytes == 0 {
            return Err(Error::invalid(
                "Can't guess CHS values because there is no input file",
            ));
        }
        let (total, sectors, heads) = (bytes / sector_bytes as u64..)
            .flat_map(|total| (2..=63u64).rev().map(move |sectors| (total, sectors)))
            .flat_map(|(total, sectors)| {
                (2..=16u64).rev().map(move |heads| (total, sectors, heads))
            })
            .find(|(total, sectors, heads)| total % (sectors * heads) == 0)
            .expect("a multiple of 4 sectors is 2 heads of 2 sectors");
        Ok(Geometry {
            cylinders: u32::try_from(total / (sectors * heads))
                .map_err(|_| Error::invalid("Hard disk is too large"))?,
            heads: heads as u32,
            sectors: sectors as u32,
            sector_bytes,
        })
    }

    /// Parse the geometry from `GDDD` metadata.
    pub(crate) fn from_metadata(metadata: &[u8]) -> Result<Geometry> {
        let text = String::from_utf8_lossy(metadata);
        let text = text.trim_end_matches('\0');
        let mut values = [None; 4];
        for field in text.split(',') {
            let (key, value) = field.split_once(':').unwrap_or((field, ""));
            let index = match key {
                "CYLS" => 0,
                "HEADS" => 1,
                "SECS" => 2,
                "BPS" => 3,
                _ => continue,
            };
            values[index] = value.trim().parse::<u32>().ok();
        }
        match values {
            [Some(cylinders), Some(heads), Some(sectors), Some(sector_bytes)] => Ok(Geometry {
                cylinders,
                heads,
                sectors,
                sector_bytes,
            }),
            _ => Err(Error::InvalidMetadata(format!(
                "Error parsing hard disk metadata '{text}'"
            ))),
        }
    }

    /// The geometry in an IDE identify block, or `None` if the drive is too large for CHS
    /// addressing.
    pub(crate) fn from_ident(ident: &[u8], sector_bytes: u32) -> Result<Option<Geometry>> {
        if ident.len() < 14 {
            return Err(Error::invalid("Ident file is invalid (too short)"));
        }
        let word = |offset: usize| u16::from_le_bytes([ident[offset], ident[offset + 1]]) as u32;
        let geometry = Geometry {
            cylinders: word(2),
            heads: word(6),
            sectors: word(12),
            sector_bytes,
        };
        // chdman ignores the CHS values of drives of 8GB or more.
        Ok((geometry.total_sectors() < 16_514_064).then_some(geometry))
    }

    /// The `GDDD` metadata for the geometry, without the terminating NUL.
    pub(crate) fn metadata(&self) -> String {
        format!(
            "CYLS:{},HEADS:{},SECS:{},BPS:{}",
            self.cylinders, self.heads, self.sectors, self.sector_bytes
        )
    }
}

impl Display for Geometry {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{},{},{} ({} bytes per sector)",
            self.cylinders, self.heads, self.sectors, self.sector_bytes
        )
    }
}

/// A hard disk model with a known geometry.
#[derive(Debug, Clone, Copy)]
pub struct Template {
    /// The manufacturer.
    pub manufacturer: &'static str,
    /// The model.
    pub model: &'static str,
    /// The geometry.
    pub geometry: Geometry,
}

impl Template {
    const fn new(
        manufacturer: &'static str,
        model: &'static str,
        cylinders: u32,
        heads: u32,
        sectors: u32,
        sector_bytes: u32,
    ) -> Template {
        Template {
            manufacturer,
            model,
            geometry: Geometry {
                cylinders,
                heads,
                sectors,
                sector_bytes,
            },
        }
    }
}

/// The hard disk templates of chdman, by ID.
pub const TEMPLATES: &[Template] = &[
    Template::new("Conner", "CFA170A", 332, 16, 63, 512),
    Template::new("Rodime", "R0201", 321, 2, 16, 512),
    Template::new("Rodime", "R0202", 321, 4, 16, 512),
    Template::new("Rodime", "R0203", 321, 6, 16, 512),
    Template::new("Rodime", "R0204", 321, 8, 16, 512),
    Template::new("Seagate", "ST-213", 615, 2, 17, 512),
    Template::new("Seagate", "ST-225", 615, 4, 17, 512),
    Template::new("Seagate", "ST-251", 820, 6, 17, 512),
    Template::new("Seagate", "ST-3600N", 1877, 7, 76, 512),
    Template::new("Maxtor", "LXT-213S", 1314, 7, 53, 512),
    Template::new("Maxtor", "LXT-340S", 1574, 7, 70, 512),
    Template::new("Maxtor", "MXT-540SL", 2466, 7, 87, 512),
    Template::new("Micropolis", "1528", 2094, 15, 83, 512),
    Template::new("Quantum", "Fireball CR 4.3 AT", 14848, 9, 63, 512),
    Template::new("Quantum", "Fireball CR 6.4 AT", 13328, 15, 63, 512),
    Template::new("Quantum", "Fireball CR 8.4 AT", 16383, 16, 63, 512),
    Template::new("Quantum", "Fireball CR 13.0 AT", 25228, 16, 63, 512),
];

#[cfg(test)]
mod tests {
    use super::Geometry;

    #[test]
    fn guess_like_chdman() {
        // 163MB, the size of the first template.
        let geometry = Geometry::guess(332 * 16 * 63 * 512, 512).unwrap();
        assert_eq!(
            (geometry.cylinders, geometry.heads, geometry.sectors),
            (332, 16, 63)
        );
        // A prime number of sectors is rounded up.
        let geometry = Geometry::guess(13 * 512, 512).unwrap();
        assert_eq!(geometry.total_sectors(), 14);
    }

    #[test]
    fn metadata_roundtrip() {
        let geometry = Geometry {
            cylinders: 332,
            heads: 16,
            sectors: 63,
            sector_bytes: 512,
        };
        assert_eq!(geometry.metadata(), "CYLS:332,HEADS:16,SECS:63,BPS:512");
        assert_eq!(
            Geometry::from_metadata(b"CYLS:332,HEADS:16,SECS:63,BPS:512\0").unwrap(),
            geometry
        );
    }
}
