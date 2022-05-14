/// Common logic for CD-ROM decompression codecs.
use crate::cdrom::{CD_FRAME_SIZE, CD_MAX_SECTOR_DATA, CD_MAX_SUBCODE_DATA, CD_SYNC_HEADER};
use crate::compression::ecc::ErrorCorrectedSector;
use crate::compression::lzma::LzmaCodec;
use crate::compression::zlib::ZlibCodec;
use crate::compression::{CompressionCodec, CompressionCodecType, DecompressResult, CodecImplementation};
use crate::error::{ChdError, Result};
use crate::header::CodecType;
use std::convert::TryFrom;

/// CD-ROM wrapper decompression codec (cdlz) that uses the [LZMA codec](crate::codecs::LzmaCodec)
/// for decompression of sector data and the [Deflate codec](crate::codecs::ZlibCodec) for
/// decompression of subcode data.
///
/// ## Format Details
/// CD-ROM compressed hunks have a layout with all compressed frame data in sequential order,
/// followed by compressed subcode data.
/// ```c
/// [Header, Frame0, Frame1, ..., FrameN, Subcode0, Subcode1, ..., SubcodeN]
/// ```
///
/// The slice of the input buffer from `Frame0` to Frame1` is a single LZMA compressed stream,
/// followed by the subcode data which is a single Deflate compressed stream.
///
/// The size of the header is determined by the number of 2448-byte sized frames that can fit
/// into a hunk-sized buffer and the length of such buffer. First, the number of ECC bytes
/// are calculated as `(frames + 7) / 8`. If the hunk size is less than 65536 (0x10000) bytes,
/// then the length of the compressed sector data is stored as a 2 byte big-endian integer,
/// otherwise the length is 3 bytes, stored after the number of ECC bytes in the header.
///
/// After decompression, the data is swizzled so that each frame is followed by its corresponding
/// subcode data.
/// ```c
/// [Frame0, Subcode0, Frame1, Subcode1, ..., FrameN, SubcodeN]
/// ```
/// After swizzling, the following CD sync header will be written to
/// the first 12 bytes of each frame.
/// ```
/// pub const CD_SYNC_HEADER: [u8; 12] = [
///     0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00,
/// ];
/// ```
/// The ECC data is then regenerated throughout the sector.
///
/// ## Buffer Restrictions
/// Each compressed CDLZ hunk decompresses to a hunk-sized chunk. The hunk size must be a multiple of
/// 2448, the size of each CD frame.
/// The input buffer must contain exactly enough data to fill the hunk-sized output buffer
/// when decompressed.
pub type CdLzCodec = CdCodec<LzmaCodec, ZlibCodec>;

/// CD-ROM wrapper decompression codec (cdzl) using the [Deflate codec](crate::codecs::ZlibCodec)
/// for decompression of sector data and the [Deflate codec](crate::codecs::ZlibCodec) for
/// decompression of subcode data.
///
/// ## Format Details
/// CD-ROM compressed hunks have a layout with a header, then all compressed frame data
/// in sequential order, followed by compressed subcode data.
/// ```c
/// [Header, Frame0, Frame1, ..., FrameN, Subcode0, Subcode1, ..., SubcodeN]
/// ```
///
/// The slice of the input buffer from `Frame0` to Frame1` is a single Deflate compressed stream,
/// followed by the subcode data which is a single Deflate compressed stream.
///
/// The size of the header is determined by the number of 2448-byte sized frames that can fit
/// into a hunk-sized buffer and the length of such buffer. First, the number of ECC bytes
/// are calculated as `(frames + 7) / 8`. If the hunk size is less than 65536 (0x10000) bytes,
/// then the length of the compressed sector data is stored as a 2 byte big-endian integer,
/// otherwise the length is 3 bytes, stored after the number of ECC bytes in the header.
///
/// After decompression, the data is swizzled so that each frame is followed by its corresponding
/// subcode data.
///
/// ```c
/// [Frame0, Subcode0, Frame1, Subcode1, ..., FrameN, SubcodeN]
/// ```
/// After swizzling, the following CD sync header will be written to
/// the first 12 bytes of each frame.
/// ```
/// pub const CD_SYNC_HEADER: [u8; 12] = [
///     0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00,
/// ];
/// ```
/// The ECC data is then regenerated throughout the sector.
///
/// ## Buffer Restrictions
/// Each compressed CDZL hunk decompresses to a hunk-sized chunk. The hunk size must be a multiple of
/// 2448, the size of each CD frame.
/// The input buffer must contain exactly enough data to fill the output buffer
/// when decompressed.
pub type CdZlCodec = CdCodec<ZlibCodec, ZlibCodec>;

impl CompressionCodecType for CdLzCodec {
    fn codec_type(&self) -> CodecType {
        CodecType::LzmaCdV5
    }
}

impl CompressionCodecType for CdZlCodec {
    fn codec_type(&self) -> CodecType {
        CodecType::ZLibCdV5
    }
}

impl CompressionCodec for CdZlCodec {}
impl CompressionCodec for CdLzCodec {}

// unstable(adt_const_params): const TYPE: CodecType, but marker traits bring us
// most of the way.
/// CD-ROM codec wrapper.
pub struct CdCodec<Engine: CodecImplementation, SubEngine: CodecImplementation> {
    engine: Engine,
    sub_engine: SubEngine,
    buffer: Vec<u8>,
}

impl<Engine: CodecImplementation, SubEngine: CodecImplementation> CodecImplementation
    for CdCodec<Engine, SubEngine>
{
    fn is_lossy(&self) -> bool {
        self.engine.is_lossy() && self.sub_engine.is_lossy()
    }

    fn new(hunk_size: u32) -> Result<Self> {
        if hunk_size % CD_FRAME_SIZE != 0 {
            return Err(ChdError::CodecError);
        }

        let buffer = vec![0u8; hunk_size as usize];
        Ok(CdCodec {
            engine: Engine::new((hunk_size / CD_FRAME_SIZE) * CD_MAX_SECTOR_DATA)?,
            sub_engine: SubEngine::new((hunk_size / CD_FRAME_SIZE) * CD_MAX_SUBCODE_DATA)?,
            buffer,
        })
    }

    fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<DecompressResult> {
        // https://github.com/rtissera/libchdr/blob/cdcb714235b9ff7d207b703260706a364282b063/src/libchdr_chd.c#L647
        let frames = output.len() / CD_FRAME_SIZE as usize;
        let complen_bytes = if output.len() < 65536 { 2 } else { 3 };
        let ecc_bytes = (frames + 7) / 8;
        let header_bytes = ecc_bytes + complen_bytes;

        // Extract compressed length of base
        let mut sector_compressed_len: u32 =
            (input[ecc_bytes + 0] as u32) << 8 | input[ecc_bytes + 1] as u32;
        if complen_bytes > 2 {
            sector_compressed_len = sector_compressed_len << 8 | input[ecc_bytes + 2] as u32;
        }

        // decode frame data
        let frame_res = self.engine.decompress(
            &input[header_bytes..][..sector_compressed_len as usize],
            &mut self.buffer[..frames * CD_MAX_SECTOR_DATA as usize],
        )?;

        #[cfg(feature = "want_subcode")]
        let sub_res = self.sub_engine.decompress(
            &input[header_bytes + sector_compressed_len as usize..],
            &mut self.buffer[frames * CD_MAX_SECTOR_DATA as usize..][..frames * CD_MAX_SUBCODE_DATA as usize],
        )?;

        #[cfg(not(feature = "want_subcode"))]
        let sub_res = DecompressResult::default();

        // Decompressed data has layout
        // [Frame0, Frame1, ..., FrameN, Subcode0, Subcode1, ..., SubcodeN]
        // We need to reassemble the data to be
        // [Frame0, Subcode0, Frame1, Subcode1, ..., FrameN, SubcodeN]

        // Reassemble frame data to expected layout.
        for (frame_num, chunk) in self.buffer[..frames * CD_MAX_SECTOR_DATA as usize]
            .chunks_exact(CD_MAX_SECTOR_DATA as usize)
            .enumerate()
        {
            output[frame_num * CD_FRAME_SIZE as usize..][..CD_MAX_SECTOR_DATA as usize]
                .copy_from_slice(chunk);
        }

        // Reassemble subcode data to expected layout.
        #[cfg(feature = "want_subcode")]
        for (frame_num, chunk) in self.buffer[frames * CD_MAX_SECTOR_DATA as usize..]
            .chunks_exact(CD_MAX_SUBCODE_DATA as usize)
            .enumerate()
        {
            output[frame_num * CD_FRAME_SIZE as usize + CD_MAX_SECTOR_DATA as usize..]
                [..CD_MAX_SUBCODE_DATA as usize]
                .copy_from_slice(chunk);
        }

        // Recreate ECC data
        #[cfg(feature = "want_raw_data_sector")]
        for frame_num in 0..frames {
            let mut sector = <&mut [u8; CD_MAX_SECTOR_DATA as usize]>::try_from(
                &mut output[frame_num * CD_FRAME_SIZE as usize..][..CD_MAX_SECTOR_DATA as usize],
            )?;
            if (input[frame_num / 8] & (1 << (frame_num % 8))) != 0 {
                sector[0..12].copy_from_slice(&CD_SYNC_HEADER);
                sector.generate_ecc();
            }
        }

        Ok(frame_res + sub_res)
    }
}
