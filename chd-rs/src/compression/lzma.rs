use crate::compression::{
    CodecImplementation, CompressionCodec, CompressionCodecType, DecompressResult,
};
use crate::error::{ChdError, Result};
use crate::header::CodecType;
use lzma_rs::decompress::raw::{LzmaDecoder, LzmaParams, LzmaProperties};
use std::io::Cursor;

/// LZMA (lzma) decompression codec.
///
/// ## Format Details
/// CHD compresses LZMA hunks without an xz stream header using
/// the default compression settings for LZMA 19.0. These settings are
///
/// * Literal Context Bits (`lc`): 3
/// * Literal Position Bits (`lp`): 0
/// * Position Bits (`pb`): 2
///
/// The dictionary size is determined via the following algorithm with a level of 9, and a
/// reduction size of hunk size.
///
/// ```rust
/// fn get_lzma_dict_size(level: u32, reduce_size: u32) -> u32 {
///     let mut dict_size = if level <= 5 {
///         1 << (level * 2 + 14)
///     } else if level <= 7 {
///         1 << 25
///     } else {
///        1 << 26
///     };
///
///     // This does the same thing as LzmaEnc.c when determining dict_size
///     if dict_size > reduce_size {
///         for i in 11..=30 {
///             if reduce_size <= (2u32 << i) {
///                 dict_size = 2u32 << i;
///                 break;
///             }
///             if reduce_size <= (3u32 << i) {
///                 dict_size = 3u32 << i;
///                 break;
///             }
///         }
///     }
///     dict_size
/// }
/// ```
///
/// ## Buffer Restrictions
/// Each compressed LZMA hunk decompresses to a hunk-sized chunk.
/// The input buffer must contain exactly enough data to fill the output buffer
/// when decompressed.
pub struct LzmaCodec {
    // The LZMA codec for CHD uses raw LZMA chunks without a stream header. The result
    // is that the chunks are encoded with the defaults used in LZMA 19.0.
    // These defaults are lc = 3, lp = 0, pb = 2.
    engine: LzmaDecoder,
}

impl CompressionCodec for LzmaCodec {}

/// MAME/libchdr uses an ancient LZMA 19.00.
///
/// To match the proper dictionary size, we copy the algorithm from
/// [`LzmaEnc::LzmaEncProps_Normalize`](https://github.com/rtissera/libchdr/blob/cdcb714235b9ff7d207b703260706a364282b063/deps/lzma-19.00/src/LzmaEnc.c#L52).
fn get_lzma_dict_size(level: u32, reduce_size: u32) -> u32 {
    let mut dict_size = if level <= 5 {
        1 << (level * 2 + 14)
    } else if level <= 7 {
        1 << 25
    } else {
        1 << 26
    };

    // this does the same thing as LzmaEnc.c when determining dict_size
    if dict_size > reduce_size {
        // might be worth converting this to a while loop for const
        // depends if we can const-propagate hunk_size.
        // todo: #[feature(const_inherent_unchecked_arith)
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

impl CompressionCodecType for LzmaCodec {
    fn codec_type(&self) -> CodecType
    where
        Self: Sized,
    {
        CodecType::LzmaV5
    }
}

impl CodecImplementation for LzmaCodec {
    fn new(hunk_size: u32) -> Result<Self> {
        Ok(LzmaCodec {
            engine: LzmaDecoder::new(
                LzmaParams::new(
                    LzmaProperties {
                        lc: 3,
                        lp: 0,
                        pb: 2,
                    },
                    get_lzma_dict_size(9, hunk_size),
                    None,
                ),
                None,
            )
            .map_err(|_| ChdError::DecompressionError)?,
        })
    }

    fn decompress(&mut self, input: &[u8], mut output: &mut [u8]) -> Result<DecompressResult> {
        let mut read = Cursor::new(input);
        let len = output.len();
        self.engine.reset(Some(Some(len as u64)));
        self.engine
            .decompress(&mut read, &mut output)
            .map_err(|_| ChdError::DecompressionError)?;
        Ok(DecompressResult::new(len, read.position() as usize))
    }
}
