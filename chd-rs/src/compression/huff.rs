use crate::compression::{
    CodecImplementation, CompressionCodec, CompressionCodecType, DecompressResult,
};
use crate::error::Result;
use crate::header::CodecType;
use crate::huffman::Huffman8BitDecoder;
use bitreader::BitReader;

/// MAME 8-bit Huffman (huff) codec.
///
/// ## Format Details
/// The Huffman codec uses a Huffman-encoded Huffman tree with the
/// the default Huffman settings of
/// * `NUM_CODES`: 256
/// * `MAX_BITS`: 16
///
/// The last decoded code from the input buffer may not contain enough bits for a full
/// byte is reconstructed by shifting zero-bits in from the right. See the source code for
/// [huffman.rs](https://github.com/SnowflakePowered/chd-rs/blob/575cc2330b0c6eb444e8773068295510147ffa6b/chd-rs/src/huffman.rs#L242)
/// for more details.
/// ## Buffer Restrictions
/// Each compressed Huffman hunk decompresses to a hunk-sized chunk.
/// The input buffer must contain exactly enough data to fill the output buffer
/// when decompressed.
pub struct HuffmanCodec {
    #[cfg(feature = "encode")]
    encoder: crate::huffman::Huffman8BitEncoder<'static>,
}

impl CodecImplementation for HuffmanCodec {
    fn new(_: u32) -> Result<Self> {
        Ok(HuffmanCodec {
            #[cfg(feature = "encode")]
            encoder: crate::huffman::Huffman8BitEncoder::new(),
        })
    }

    fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<DecompressResult> {
        let mut bit_reader = BitReader::new(input);
        let decoder = Huffman8BitDecoder::from_huffman_tree(&mut bit_reader)?;

        for i in output.iter_mut() {
            *i = decoder.decode_one(&mut bit_reader)? as u8;
        }

        Ok(DecompressResult::new(
            output.len(),
            ((input.len() * 8) - bit_reader.remaining() as usize) / 8,
        ))
    }

    #[cfg(feature = "encode")]
    fn compress(&mut self, input: &[u8], output: &mut [u8]) -> Result<usize> {
        let output_len = output.len().min(input.len());
        let mut remaining = &mut output[..output_len];
        self.encoder
            .encode(input, &mut remaining)
            .map_err(|_| crate::Error::CompressionError)?;

        let compressed_len = output_len - remaining.len();
        if compressed_len >= input.len() {
            return Err(crate::Error::CompressionError);
        }
        Ok(compressed_len)
    }
}

impl CompressionCodecType for HuffmanCodec {
    fn codec_type(&self) -> CodecType {
        CodecType::HuffV5
    }
}

impl CompressionCodec for HuffmanCodec {}
