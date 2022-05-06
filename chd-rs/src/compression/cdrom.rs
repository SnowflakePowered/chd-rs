use std::convert::TryFrom;
use crate::cdrom::{CD_FRAME_SIZE, CD_MAX_SECTOR_DATA, CD_MAX_SUBCODE_DATA, CD_SYNC_HEADER};
use crate::compression::{BlockCodec, CompressionCodec, CompressionCodecType, DecompressLength, InternalCodec};
use crate::compression::lzma::LzmaCodec;
use crate::compression::zlib::ZlibCodec;
use crate::error::{Result, ChdError};
use crate::header::CodecType;
use crate::compression::ecc::ErrorCorrectedSector;

pub type CdLzCodec = CdBlockCodec<LzmaCodec, ZlibCodec>;
pub type CdZlCodec = CdBlockCodec<ZlibCodec, ZlibCodec>;

impl CompressionCodecType for CdLzCodec {
    fn codec_type() -> CodecType {
        CodecType::LzmaCdV5
    }
}

impl CompressionCodecType for CdZlCodec {
    fn codec_type() -> CodecType {
        CodecType::ZLibCdV5
    }
}

impl CompressionCodec for CdZlCodec {}
impl CompressionCodec for CdLzCodec {}

// unstable(adt_const_params): const TYPE: CodecType
pub struct CdBlockCodec<Engine: BlockCodec, SubEngine: BlockCodec> {
    engine: Engine,
    sub_engine: SubEngine,
    buffer: Vec<u8>
}

impl <Engine: BlockCodec, SubEngine: BlockCodec> InternalCodec for CdBlockCodec<Engine, SubEngine> {
    fn is_lossy() -> bool {
        Engine::is_lossy() && SubEngine::is_lossy()
    }

    fn new(hunk_size: u32) -> Result<Self> {
        if hunk_size % CD_FRAME_SIZE != 0 {
            return Err(ChdError::CodecError)
        }

        let buffer = vec![0u8; hunk_size as usize];
        Ok(CdBlockCodec {
            engine: Engine::new((hunk_size / CD_FRAME_SIZE) * CD_MAX_SECTOR_DATA)?,
            sub_engine: SubEngine::new((hunk_size / CD_FRAME_SIZE) * CD_MAX_SUBCODE_DATA)?,
            buffer
        })
    }

    fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<DecompressLength> {
        // https://github.com/rtissera/libchdr/blob/cdcb714235b9ff7d207b703260706a364282b063/src/libchdr_chd.c#L647
        let frames = output.len() / CD_FRAME_SIZE as usize;
        let complen_bytes = if output.len() < 65536 { 2 } else { 3 };
        let ecc_bytes = (frames + 7) / 8;
        let header_bytes = ecc_bytes + complen_bytes;

        // extract compressed length of base
        let mut complen_base = (input[ecc_bytes + 0].checked_shl(8).unwrap_or(0)) | input[ecc_bytes] + 1;
        if complen_base > 2 {
            complen_base = complen_base.checked_shl(8).unwrap_or(0) | input[ecc_bytes + 2];
        }

        // decode frame data
        let frame_res = self.engine.decompress(&input[header_bytes..][..complen_base as usize],
                               &mut self.buffer[..frames * CD_MAX_SECTOR_DATA as usize])?;

        // WANT_SUBCODE
        let sub_res = self.sub_engine.decompress(&input[header_bytes + complen_base as usize..],
                                   &mut self.buffer[frames * CD_MAX_SECTOR_DATA as usize..][..CD_MAX_SUBCODE_DATA as usize])?;


        // reassemble data
        for frame_num in 0..frames {
            output[frame_num * CD_FRAME_SIZE as usize..][..CD_MAX_SECTOR_DATA as usize]
                .copy_from_slice(&self.buffer[frame_num * CD_MAX_SECTOR_DATA as usize..][..CD_MAX_SECTOR_DATA as usize]);

            // WANT_SUBCODE
            output[frame_num * CD_FRAME_SIZE as usize + CD_MAX_SECTOR_DATA as usize..][..CD_MAX_SUBCODE_DATA as usize]
                .copy_from_slice(&self.buffer[frames * CD_MAX_SECTOR_DATA as usize + frame_num * CD_FRAME_SIZE as usize..][..CD_MAX_SUBCODE_DATA as usize]);

            // WANT_RAW_DATA_SECTOR

            // this may be a bit overkill..
            let mut sector_slice = <&mut [u8; CD_MAX_SECTOR_DATA as usize]>
            ::try_from(&mut output[frame_num * CD_FRAME_SIZE as usize..][..CD_MAX_SECTOR_DATA as usize])?;
            if (input[frame_num / 8] & (1 << (frame_num % 8))) != 0 {
                sector_slice[0..12].copy_from_slice(&CD_SYNC_HEADER);
                sector_slice.generate_ecc();
            }
        }
        Ok(frame_res + sub_res)
    }
}
