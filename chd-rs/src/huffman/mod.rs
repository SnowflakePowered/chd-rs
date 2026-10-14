#![cfg_attr(docsrs, doc(cfg(huffman_api)))]
//! Implementation of the MAME CHD Huffman Decoder and Encoder.
//!
//! For format descriptions, see [huffman.cpp](https://github.com/mamedev/mame/blob/master/src/lib/util/huffman.cpp).
use bitreader::BitReaderError;
use std::cmp::Ordering;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::marker::PhantomData;

mod decode;
#[cfg(feature = "huff_write")]
mod encode;

pub use decode::{lookup_len, Huffman8BitDecoder, HuffmanDecoder};

#[cfg(feature = "huff_write")]
pub use encode::{Huffman8BitEncoder, HuffmanEncoder};

/// Error type for Huffman decoding.
#[derive(Debug)]
pub enum HuffmanError {
    /// Too many bits used to initialize the decoder or read a value.
    TooManyBits,
    /// The provided bitstream does not contain a valid MAME static Huffman tree.
    InvalidData,
    /// The input buffer is too small to read another Huffman code from.
    InputBufferTooSmall,
    /// The output buffer is too small to write to.
    OutputBufferTooSmall,
    /// There is an internal inconsistency within the Huffman tree.
    InternalInconsistency,
    /// The tree has too many contexts.
    TooManyContexts,
    /// A value to be encoded is not less than the number of codes.
    ValueOutOfRange,
}

impl Error for HuffmanError {}

impl Display for HuffmanError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            HuffmanError::TooManyBits => f.write_str("Too many bits"),
            HuffmanError::InvalidData => f.write_str("Invalid data"),
            HuffmanError::InputBufferTooSmall => f.write_str("Input buffer too small"),
            HuffmanError::OutputBufferTooSmall => f.write_str("Output buffer too small"),
            HuffmanError::InternalInconsistency => f.write_str("Internal inconsistency"),
            HuffmanError::TooManyContexts => f.write_str("Too many contexts"),
            HuffmanError::ValueOutOfRange => f.write_str("Value out of range"),
        }
    }
}

impl From<BitReaderError> for HuffmanError {
    fn from(err: BitReaderError) -> Self {
        match err {
            BitReaderError::NotEnoughData { .. } => HuffmanError::InputBufferTooSmall,
            BitReaderError::TooManyBitsForType { .. } => HuffmanError::TooManyBits,
        }
    }
}

#[cfg(feature = "huff_write")]
impl From<std::io::Error> for HuffmanError {
    fn from(err: std::io::Error) -> Self {
        match err.kind() {
            std::io::ErrorKind::WriteZero => HuffmanError::OutputBufferTooSmall,
            // bitstream-io rejects values that do not fit in the requested bits.
            std::io::ErrorKind::InvalidInput => HuffmanError::TooManyBits,
            _ => HuffmanError::InternalInconsistency,
        }
    }
}

#[derive(Default, Clone, Copy)]
struct HuffmanNode<'a> {
    // The parent is needed for write but not for read only.
    // It is an index into the encoder's node array.
    parent: Option<u16>,
    weight: u32,
    bits: u32,
    num_bits: u8,
    // Huffman nodes are parameterized over a lifetime to allow comparisons only for
    // nodes belonging to the same decoder.
    _phantom: PhantomData<&'a HuffmanNode<'a>>,
}

impl<'a> PartialEq for HuffmanNode<'a> {
    fn eq(&self, other: &Self) -> bool {
        self.bits == other.bits
    }
}

impl<'a> PartialOrd for HuffmanNode<'a> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        // Matches huffman_context_base::tree_node_compare: heaviest first, then by code.
        if self.weight != other.weight {
            //  node2->weight - node1->weight;
            return Some(other.weight.cmp(&self.weight));
        }
        if self.bits == other.bits {
            return None;
        }
        // (int)node1->bits - (int)node2->bits;
        Some(self.bits.cmp(&other.bits))
    }
}

/// Get the number of bits used to encode each value of an RLE encoded Huffman tree.
const fn rle_num_bits(max_bits: u8) -> u8 {
    match max_bits {
        0..=7 => 3,  // < 8
        8..=15 => 4, // >= 8
        _ => 5,      // >= 16
    }
}

/// Get the number of bits used to decode a Huffman tree
/// from a Huffman-encoded bitstream.
const fn rle_full_bits<const NUM_CODES: usize>() -> u8 {
    let mut temp = NUM_CODES - 9;
    let mut full_bits = 0;
    while temp != 0 {
        temp >>= 1;
        full_bits += 1;
    }
    full_bits
}

/// Assign canonical codes to the nodes based on the number of bits of each.
fn assign_canonical_codes(nodes: &mut [HuffmanNode<'_>], max_bits: u8) -> Result<(), HuffmanError> {
    let mut curr_start = 0;

    // The histogram of bit lengths is only needed while assigning codes.
    let mut histogram = [0u32; 33];

    // Fill in histogram of bit lengths.
    for node in nodes.iter() {
        if node.num_bits > max_bits {
            return Err(HuffmanError::InternalInconsistency);
        }
        if node.num_bits <= 32 {
            histogram[node.num_bits as usize] += 1;
        }
    }

    // Determine starting code number of code lengths.
    for code_len in (1..33).rev() {
        let next_start = (curr_start + histogram[code_len]) >> 1;
        if code_len != 1 && next_start * 2 != (curr_start + histogram[code_len]) {
            return Err(HuffmanError::InternalInconsistency);
        }
        histogram[code_len] = curr_start;
        curr_start = next_start
    }

    // Assign codes.
    for node in nodes.iter_mut() {
        if node.num_bits > 0 {
            node.bits = histogram[node.num_bits as usize];
            histogram[node.num_bits as usize] += 1;
        }
    }
    Ok(())
}
