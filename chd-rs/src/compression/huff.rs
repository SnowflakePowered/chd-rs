use crate::compression::{CompressionCodec, CompressionCodecType, DecompressLength, InternalCodec};
use crate::error::Result;
use crate::header::CodecType;
use crate::huffman;
use crate::huffman::HuffmanDecoder;
use bitreader::BitReader;

// The 'default' encoding settings are NUM_BITS = 256, MAX_BITS = 16.
// I prefer to make explicit the parameters at type instantiation for
// clarity purposes.
type Huffman8BitDecoder<'a> = HuffmanDecoder<'a, 256, 16, { huffman::lookup_length::<16>() }>;

/// MAME Huffman Codec.
pub struct HuffmanCodec;
impl InternalCodec for HuffmanCodec {
    fn is_lossy(&self) -> bool {
        false
    }

    fn new(_: u32) -> Result<Self> {
        Ok(HuffmanCodec)
    }

    fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<DecompressLength> {
        let mut bit_reader = BitReader::new(input);
        let decoder = Huffman8BitDecoder::from_huffman_tree(&mut bit_reader)?;

        for i in 0..output.len() {
            output[i] = decoder.decode_one(&mut bit_reader)? as u8;
        }

        Ok(DecompressLength::new(
            output.len(),
            ((input.len() * 8) - bit_reader.remaining() as usize) / 8,
        ))
    }
}

impl CompressionCodecType for HuffmanCodec {
    fn codec_type(&self) -> CodecType {
        CodecType::HuffV5
    }
}

impl CompressionCodec for HuffmanCodec {}
