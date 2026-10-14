use crate::compression::{
    CodecImplementation, CompressionCodec, CompressionCodecType, DecompressResult,
};
use crate::const_assert;
use crate::error::{Error, Result};
use crate::header::CodecType;
use lzma_rust2::LzmaReader;
#[cfg(feature = "encode")]
use lzma_rust2::{EncodeMode, LzmaOptions, LzmaWriter, MfType};
#[cfg(feature = "encode")]
use std::io::Write;
use std::io::{Cursor, Read};

/// LZMA (lzma) codec.
///
/// ## Format Details
/// CHD compresses LZMA hunks without an xz stream header or end marker using
/// the default compression settings for LZMA 19.0. These settings are
///
/// * Literal Context Bits (`LC`): 3
/// * Literal Position Bits (`LP`): 0
/// * Position Bits (`PB`): 2
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
pub struct LzmaCodec<const LC: u32 = 3, const LP: u32 = 0, const PB: u32 = 2> {
    dict_size: u32,
}

impl<const LC: u32, const LP: u32, const PB: u32> CompressionCodec for LzmaCodec<LC, LP, PB> {}

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

impl<const LC: u32, const LP: u32, const PB: u32> CompressionCodecType for LzmaCodec<LC, LP, PB> {
    fn codec_type(&self) -> CodecType
    where
        Self: Sized,
    {
        CodecType::LzmaV5
    }
}

impl<const LC: u32, const LP: u32, const PB: u32> CodecImplementation for LzmaCodec<LC, LP, PB> {
    fn new(hunk_size: u32) -> Result<Self> {
        const_assert!(LC: u32, LP: u32, PB: u32 => LC <= 8 && LP <= 4 && PB <= 4);

        Ok(LzmaCodec {
            dict_size: get_lzma_dict_size(9, hunk_size),
        })
    }

    fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<DecompressResult> {
        let mut reader = LzmaReader::new(
            Cursor::new(input),
            output.len() as u64,
            LC,
            LP,
            PB,
            self.dict_size,
            None,
        )
        .map_err(|_| Error::DecompressionError)?;

        reader
            .read_exact(output)
            .map_err(|_| Error::DecompressionError)?;

        // Rewind back to only decoded bytes.
        let (input_cursor, unread) = reader.into_parts();
        let bytes_read = input_cursor.position() as usize - unread.len();
        Ok(DecompressResult::new(output.len(), bytes_read))
    }

    #[cfg(feature = "encode")]
    fn compress(&mut self, input: &[u8], output: &mut [u8]) -> Result<usize> {
        // MAME compresses with LzmaEncProps level 6, in
        // https://github.com/mamedev/mame/blob/74b9fae5a3ca3c99828fe41da154e3c84951414e/src/lib/util/chdcodec.cpp#L1312-L1318
        //
        // LzmaEncProps_Normalize
        // https://github.com/rtissera/libchdr/blob/cdcb714235b9ff7d207b703260706a364282b063/deps/lzma-19.00/src/LzmaEnc.c#L74-L78
        // normalizes level 6 to the normal (optimum) algorithm, fb = 32, a BT4 binary tree
        // match finder (btMode = 1, numHashBytes = 4), and mc = 32.
        let options = LzmaOptions {
            dict_size: self.dict_size,
            lc: LC,
            lp: LP,
            pb: PB,
            mode: EncodeMode::Normal,
            nice_len: 32,
            mf: MfType::Bt4,
            depth_limit: 32,
            preset_dict: None,
        };

        // Anything not smaller than the input is a failure, so don't bother writing more.
        let output_len = output.len().min(input.len());
        let output = &mut output[..output_len];
        let mut writer = LzmaWriter::new(output, &options, false, false, Some(input.len() as u64))
            .map_err(|_| Error::CompressionError)?;
        writer
            .write_all(input)
            .map_err(|_| Error::CompressionError)?;
        let remaining = writer.finish().map_err(|_| Error::CompressionError)?;

        let compressed_len = output_len - remaining.len();
        if compressed_len >= input.len() {
            return Err(Error::CompressionError);
        }
        Ok(compressed_len)
    }
}
