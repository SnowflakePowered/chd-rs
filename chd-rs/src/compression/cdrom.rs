/// Common logic for CD-ROM decompression codecs.
use crate::cdrom::{CD_FRAME_SIZE, CD_MAX_SECTOR_DATA, CD_MAX_SUBCODE_DATA, CD_SYNC_HEADER};
use crate::compression::ecc::ErrorCorrectedSector;
use crate::compression::lzma::LzmaCodec;
use crate::compression::zlib::ZlibCodec;
use crate::compression::{CompressionCodec, CompressionCodecType, DecompressLength, InternalCodec};
use crate::error::{ChdError, Result};
use crate::header::CodecType;
use cfg_if::cfg_if;
use std::convert::TryFrom;

pub type CdLzCodec = CdBlockCodec<LzmaCodec, ZlibCodec>;
pub type CdZlCodec = CdBlockCodec<ZlibCodec, ZlibCodec>;

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
pub struct CdBlockCodec<Engine: InternalCodec, SubEngine: InternalCodec> {
    engine: Engine,
    sub_engine: SubEngine,
    buffer: Vec<u8>,
}

impl<Engine: InternalCodec, SubEngine: InternalCodec> InternalCodec
    for CdBlockCodec<Engine, SubEngine>
{
    fn is_lossy(&self) -> bool {
        self.engine.is_lossy() && self.sub_engine.is_lossy()
    }

    fn new(hunk_size: u32) -> Result<Self> {
        if hunk_size % CD_FRAME_SIZE != 0 {
            return Err(ChdError::CodecError);
        }

        let buffer = vec![0u8; hunk_size as usize];
        Ok(CdBlockCodec {
            engine: Engine::new((hunk_size / CD_FRAME_SIZE) * CD_MAX_SECTOR_DATA)?,
            sub_engine: SubEngine::new((hunk_size / CD_FRAME_SIZE) * CD_MAX_SUBCODE_DATA)?,
            buffer,
        })
    }

    fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<DecompressLength> {
        // https://github.com/rtissera/libchdr/blob/cdcb714235b9ff7d207b703260706a364282b063/src/libchdr_chd.c#L647
        let frames = output.len() / CD_FRAME_SIZE as usize;
        let complen_bytes = if output.len() < 65536 { 2 } else { 3 };
        let ecc_bytes = (frames + 7) / 8;
        let header_bytes = ecc_bytes + complen_bytes;

        // Extract compressed length of base
        let mut complen_base: u32 =
            (input[ecc_bytes + 0] as u32) << 8 | input[ecc_bytes + 1] as u32;
        if complen_bytes > 2 {
            complen_base = complen_base << 8 | input[ecc_bytes + 2] as u32;
        }

        // decode frame data
        let frame_res = self.engine.decompress(
            &input[header_bytes..][..complen_base as usize],
            &mut self.buffer[..frames * CD_MAX_SECTOR_DATA as usize],
        )?;

        cfg_if! {
            if #[cfg(feature = "want_subcode")] {
                let sub_res = self.sub_engine.decompress(
                    &input[header_bytes + complen_base as usize..],
                    &mut self.buffer[frames * CD_MAX_SECTOR_DATA as usize..][..frames * CD_MAX_SUBCODE_DATA as usize],
                )?;
            } else {
                let sub_res = DecompressLength::default();
            }
        }

        // Decompressed FLAC data has layout
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
            let mut sector_slice = <&mut [u8; CD_MAX_SECTOR_DATA as usize]>::try_from(
                &mut output[frame_num * CD_FRAME_SIZE as usize..][..CD_MAX_SECTOR_DATA as usize],
            )?;
            if (input[frame_num / 8] & (1 << (frame_num % 8))) != 0 {
                sector_slice[0..12].copy_from_slice(&CD_SYNC_HEADER);
                sector_slice.generate_ecc();
            }
        }

        Ok(frame_res + sub_res)
    }
}
