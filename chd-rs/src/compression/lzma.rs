use std::io::Cursor;
use lzma_rs::decode::lzma::LzmaParams;
use lzma_rs::lzma_decompress_with_params;
use crate::compression::{BlockCodec, DecompressLength, InternalCodec};
use crate::error::{Result, ChdError};

pub struct LzmaCodec {
    params: LzmaParams,
}

impl BlockCodec for LzmaCodec {}

// LzmaEnc.c LzmaEncProps_Normalize
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

impl InternalCodec for LzmaCodec {
    fn is_lossy(&self) -> bool {
        false
    }

    fn new(hunk_size: u32) -> Result<Self> {

        // LZMA 19.0 uses lc = 3, lp = 0, pb = 2
        let params = LzmaParams::new(3, 0, 2,
                                     get_lzma_dict_size(9, hunk_size),
                                     None);

        Ok(LzmaCodec {
            params
        })
    }

    // not sure if this works but
    fn decompress(&mut self, input: &[u8], mut output: &mut [u8]) -> Result<DecompressLength> {
        let mut read = Cursor::new(input);
        let len = output.len();
        if let Ok(_) = lzma_decompress_with_params(&mut read, &mut output, None,
                                                   self.params.with_size( len as u64)) {


            Ok(DecompressLength::new(len, read.position() as usize))
        } else {
            Err(ChdError::DecompressionError)
        }
    }
}