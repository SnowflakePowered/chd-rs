use crate::Result;
use num_traits::ToPrimitive;

#[cfg(feature = "verify_block_crc")]
use crate::Error;

#[allow(unused_imports)]
use crc::{Crc, CRC_16_IBM_3740, CRC_32_ISO_HDLC};

// CRC16 table in hashing.cpp indicates CRC16/CCITT, but constants
// are consistent with CRC16/CCITT-FALSE, which is CRC-16/IBM-3740
pub(crate) const CRC16: Crc<u16> = Crc::<u16>::new(&CRC_16_IBM_3740);

// The polynomial matches up (0x04c11db7 reflected = 0xedb88320), and
// checking with zlib crc32.c matches the check 0xcbf43926 for
// "12345678".
#[cfg(feature = "verify_block_crc")]
const CRC32: Crc<u32> = Crc::<u32>::new(&CRC_32_ISO_HDLC);

/// Crate-private trait for the implementation of a CHD-compatible CRC instance for
/// CRC bit widths.
pub(crate) trait ChdBlockChecksum {
    /// Checks the integrity of the decompressed data with the CHD hunk checksum for
    /// the given bit width using the CHD-native CRC instance for that bit width.
    ///
    /// If the `crc` provided is `None`, this function always returns `Ok`.
    ///
    /// This function should only be used to verify decompressed hunks. If the `verify_block_crc`
    /// feature is not enabled, this function always returns `Ok`.
    fn verify_block_checksum<C: ToPrimitive, R>(crc: Option<C>, buf: &[u8], result: R)
        -> Result<R>;
}

impl ChdBlockChecksum for Crc<u16> {
    #[inline(always)]
    #[allow(unused_variables)]
    fn verify_block_checksum<C: ToPrimitive, R>(
        crc: Option<C>,
        buf: &[u8],
        result: R,
    ) -> Result<R> {
        #[cfg(feature = "verify_block_crc")]
        match crc.and_then(|f| f.to_u16()) {
            Some(crc) if CRC16.checksum(buf) != crc => Err(Error::DecompressionError),
            _ => Ok(result),
        }

        #[cfg(not(feature = "verify_block_crc"))]
        Ok(result)
    }
}

impl ChdBlockChecksum for Crc<u32> {
    #[inline(always)]
    #[allow(unused_variables)]
    fn verify_block_checksum<C: ToPrimitive, R>(
        crc: Option<C>,
        buf: &[u8],
        result: R,
    ) -> Result<R> {
        #[cfg(feature = "verify_block_crc")]
        match crc.and_then(|f| f.to_u32()) {
            Some(crc) if CRC32.checksum(buf) != crc => Err(Error::DecompressionError),
            _ => Ok(result),
        }

        #[cfg(not(feature = "verify_block_crc"))]
        Ok(result)
    }
}
