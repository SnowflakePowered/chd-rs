mod ecc;

use std::convert::TryFrom;
use crate::header::CodecType;
use crate::error::{ChdError, Result};
use std::io::{BufReader, Write};
use flate2::{Decompress, FlushDecompress};
use lzma_rs::decode::lzma::LzmaParams;
use lzma_rs::lzma_decompress_with_params;
use crate::compression::ecc::ErrorCorrectedSector;


use crate::cdrom::{CD_FRAME_SIZE, CD_MAX_SECTOR_DATA, CD_MAX_SUBCODE_DATA, CD_SYNC_HEADER};

trait CompressionCodec {
    fn codec_type() -> CodecType;
    fn is_lossy() -> bool;
    fn new(hunk_bytes: u32) -> Result<Self> where Self: Sized;
    fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<u64>;
}

struct NoneCodec;
impl CompressionCodec for NoneCodec {
    fn codec_type() -> CodecType {
        CodecType::None
    }

    fn is_lossy() -> bool {
        false
    }

    fn new(_: u32) -> Result<Self> {
        Ok(NoneCodec)
    }

    fn decompress(&mut self, input: &[u8], mut output: &mut [u8]) -> Result<u64> {
        Ok(output.write(input)? as u64)
    }
}

struct ZlibCodec {
    engine: flate2::Decompress,
}
impl CompressionCodec for ZlibCodec {
    fn codec_type() -> CodecType {
        CodecType::Zlib
    }

    fn is_lossy() -> bool {
        false
    }

    fn new(_: u32) ->  Result<Self> {
        Ok(ZlibCodec {
            engine: Decompress::new(false)
        })
    }

    fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<u64> {
        self.engine.reset(false);
        let status = self.engine.decompress(input, output, FlushDecompress::Finish)
            .map_err(|_| ChdError::DecompressionError)?;

        if status == flate2::Status::BufError {
            return Err(ChdError::CompressionError);
        }

        let total_out = self.engine.total_out();
        if self.engine.total_out() != output.len() as u64 {
            return Err(ChdError::DecompressionError);
        }
        return Ok(total_out);
    }
}

struct LzmaCodec {
    params: LzmaParams,
}

impl CompressionCodec for LzmaCodec {
    fn codec_type() -> CodecType {
        // LZMA codec is internal only.
        CodecType::None
    }

    fn is_lossy() -> bool {
        false
    }

    fn new(hunk_size: u32) -> Result<Self> {
        // LzmaEnc.c LzmaEncProps_Normalize
        fn get_lzma_dict_size(level: u32, reduce_size: u32) -> u32
        {
            let mut dict_size = if level <= 5 {
                1 << (level * 2 + 14)
            } else if level <= 7 {
                1 << 25
            } else {
                1 << 26
            };

            // this does the same thing as LzmaEnc.c when determining dict_size
            if dict_size > reduce_size {
                for i in 11..=30 {
                    if reduce_size <= (2u32 << i) {
                        dict_size = 2u32 << i;
                        break;
                    }
                    if reduce_size <= (3u32 << i) {
                        dict_size = 3u32 << i;
                        break;
                    }
                }
            }

            dict_size
        }

        // LZMA 19.0 uses lc = 3, lp = 0, pb = 2
        let params = LzmaParams::new(3, 0, 2,
                                     get_lzma_dict_size(9, hunk_size),
                                                            None);

        Ok(LzmaCodec {
            params
        })
    }

    // not sure if this works but
    fn decompress(&mut self, input: &[u8], mut output: &mut [u8]) -> Result<u64> {
        let mut read = BufReader::new(input);
        let len = output.len();
        if let Ok(_) = lzma_decompress_with_params(&mut read, &mut output, None,
                                                   self.params.with_size( len as u64)) {
            Ok( len as u64)
        } else {
            Err(ChdError::DecompressionError)
        }
    }
}

struct CdLzCodec {
    engine: LzmaCodec,
    sub_engine: ZlibCodec,
    buffer: Vec<u8>,
}

impl CompressionCodec for CdLzCodec {
    fn codec_type() -> CodecType {
        CodecType::LzmaCdV5
    }

    fn is_lossy() -> bool {
        false
    }

    fn new(hunk_size: u32) -> Result<Self> {
        if hunk_size % CD_FRAME_SIZE != 0 {
            return Err(ChdError::CodecError)
        }

        let mut buffer = vec![0u8; hunk_size as usize];
        Ok(CdLzCodec {
            engine: LzmaCodec::new((hunk_size / CD_FRAME_SIZE) * CD_MAX_SECTOR_DATA)?,
            sub_engine: ZlibCodec::new((hunk_size / CD_FRAME_SIZE) * CD_MAX_SUBCODE_DATA)?,
            buffer
        })
    }

    fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<u64> {
        // https://github.com/rtissera/libchdr/blob/cdcb714235b9ff7d207b703260706a364282b063/src/libchdr_chd.c#L647
        let frames = input.len() / CD_FRAME_SIZE as usize;
        let complen_bytes = if output.len() < 65536 { 2 } else { 3 };
        let ecc_bytes = (frames + 7) / 8;
        let header_bytes = ecc_bytes + complen_bytes;

        let mut complen_base = (input[ecc_bytes + 0].checked_shl(8).unwrap_or(0)) | input[ecc_bytes] + 1;
        if complen_base > 2 {
            complen_base = complen_base.checked_shl(8).unwrap_or(0) | input[ecc_bytes + 2];
        }

        // decode frame data
        self.engine.decompress(&input[header_bytes..][..complen_base as usize],
                               &mut self.buffer[..frames * CD_MAX_SECTOR_DATA as usize])?;

        // WANT_SUBCODE
        self.sub_engine.decompress(&input[header_bytes + complen_base as usize..],
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
        Ok(0)
    }
}

