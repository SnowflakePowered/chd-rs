use crate::header::CodecType;
use crate::error::{Result, ChdError};
use std::io::{Read, Seek, Write};
use flate2::{Decompress, FlushDecompress};
use xz2::stream::LzmaOptions;
use crate::cdrom::{CD_FRAME_SIZE, CD_MAX_SECTOR_DATA, CD_MAX_SUBCODE_DATA};

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
    engine: xz2::stream::Stream,
}

impl CompressionCodec for LzmaCodec {
    fn codec_type() -> CodecType {
        // LZMA codec is internal only.
        CodecType::None
    }

    fn is_lossy() -> bool {
        todo!()
    }

    fn new(hunk_size: u32) -> Result<Self> {
        // LzmaEnc.c LzmaEncProps_Normalize
        fn get_lzma_dict_size(level: u32, reduce_size: u32) -> u32
        {
            let mut dict_size: u32 = if level <= 5 {
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
                        dict_size = (2u32 << i);
                        break;
                    }
                    if reduce_size <= (3u32 << i) {
                        dict_size = (3u32 << i);
                        break;
                    }
                }
            }

            dict_size
        }

        let mut options = LzmaOptions::new_preset(9).map_err(|_| ChdError::CodecError)?;
        options.dict_size(get_lzma_dict_size(9, hunk_size));

        Ok(LzmaCodec {
            // todo: may have to go much more low level and use raw_decoder
            engine: xz2::stream::Stream::new_lzma_decoder(64).map_err(|_| ChdError::CodecError)?
        })
    }

    fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<u64> {
        todo!();
    }
}

struct CdLzCodec {
    engine: LzmaCodec,
    sub_engine: ZlibCodec,
}

impl CompressionCodec for CdLzCodec {
    fn codec_type() -> CodecType {
        // LZMA codec is
        CodecType::LzmaCdV5
    }

    fn is_lossy() -> bool {
        false
    }

    fn new(hunk_size: u32) -> Result<Self> {
        if hunk_size % CD_FRAME_SIZE != 0 {
            return Err(ChdError::CodecError)
        }

        Ok(CdLzCodec {
            engine: LzmaCodec::new((hunk_size / CD_FRAME_SIZE) * CD_MAX_SECTOR_DATA)?,
            sub_engine: ZlibCodec::new((hunk_size / CD_FRAME_SIZE) * CD_MAX_SUBCODE_DATA)?,
        })
    }

    fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<u64> {
        todo!()
    }
}

