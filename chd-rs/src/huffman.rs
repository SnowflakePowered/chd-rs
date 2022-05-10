use bitreader::{BitReader, BitReaderError};
use std::cmp::Ordering;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::marker::PhantomData;
use crate::const_assert;

type LookupValue = u16;

#[derive(Debug)]
pub enum HuffmanError {
    TooManyBits,
    InvalidData,
    InputBufferTooSmall,
    OutputBufferTooSmall,
    InternalInconsistency,
    TooManyContexts,
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
        }
    }
}

impl From<bitreader::BitReaderError> for HuffmanError {
    fn from(err: BitReaderError) -> Self {
        match err {
            BitReaderError::NotEnoughData {
                position: _,
                length: _,
                requested: _,
            } => HuffmanError::InputBufferTooSmall,
            BitReaderError::TooManyBitsForType {
                position: _,
                requested: _,
                allowed: _,
            } => HuffmanError::TooManyBits,
        }
    }
}

#[derive(Default, Clone, Copy)]
pub struct HuffmanNode<'a> {
    // Parent and count are needed for write but not for read only.
    #[cfg(feature = "write")]
    parent: usize,
    #[cfg(feature = "write")]
    count: u32,
    #[cfg(feature = "write")]
    histogram: Vec<u8>,
    weight: u32,
    bits: u32,
    num_bits: u8,
    _phantom: PhantomData<&'a HuffmanNode<'a>>, // want this phantomdata to ensure we can only compare nodes with the same lifetimes.
}

impl<'a> PartialEq for HuffmanNode<'a> {
    fn eq(&self, other: &Self) -> bool {
        self.bits == other.bits
    }
}

impl<'a> PartialOrd for HuffmanNode<'a> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        // todo check impl
        if self.weight != other.weight {
            //  node2->weight - node1->weight;
            return Some(other.weight.cmp(&self.weight));
        }
        if self.bits == other.bits {
            return None;
        }
        // (int)node1->bits - (int)node2->bits;
        return Some(self.bits.cmp(&other.bits));
    }
}

/// Allocation free CHD huffman decoder.
///
/// MAX_BITS must be less than or equal to 24.
/// LOOKUP_ARRAY_LEN must be equal to huffman::lookup_length::<MAX_BITS>().
pub struct HuffmanDecoder<'a, const NUM_CODES: usize, const MAX_BITS: u8, const LOOKUP_ARRAY_LEN: usize> {
    lookup_array: [LookupValue; LOOKUP_ARRAY_LEN],
    huffnode_array: [HuffmanNode<'a>; NUM_CODES],
}

/// Get the size of the lookup array for a given MAX_BITS
pub(crate) const fn lookup_length<const MAX_BITS: u8>() -> usize {
    1 << MAX_BITS
}

impl<'a, const NUM_CODES: usize, const MAX_BITS: u8, const LOOKUP_ARRAY_LEN: usize> HuffmanDecoder<'a, NUM_CODES, MAX_BITS, LOOKUP_ARRAY_LEN> {
    const NUM_BITS: u8 = match MAX_BITS {
        0..=7 => 3,  // < 8
        8..=15 => 4, // >= 8
        _ => 5,      // >= 16
    };

    fn new() -> Self {
        const_assert!(MAX_BITS: u8 => MAX_BITS <= 24u8);
        const_assert!(MAX_BITS: u8, LOOKUP_ARRAY_LEN: usize => LOOKUP_ARRAY_LEN == lookup_length::<MAX_BITS>());

        HuffmanDecoder {
            lookup_array: [0u16; LOOKUP_ARRAY_LEN],
            huffnode_array: [HuffmanNode::default(); NUM_CODES],
        }
    }

    pub fn from_tree_rle(
        reader: &mut BitReader<'_>,
    ) -> Result<Self, HuffmanError> {
        let mut decoder = HuffmanDecoder::new();

        let mut curr_node = 0;
        while curr_node < NUM_CODES {
            let node_bits = reader.read_u8(Self::NUM_BITS)?;

            // 1 is an escape code
            if node_bits != 1 {
                decoder.huffnode_array[curr_node].num_bits = node_bits;
                curr_node += 1;
                continue;
            }

            let node_bits = reader.read_u8(Self::NUM_BITS)?;
            if node_bits == 1 {
                // double 1 is just a 1
                decoder.huffnode_array[curr_node].num_bits = node_bits;
                curr_node += 1;
                continue;
            }

            let rep_count = reader.read_u8(Self::NUM_BITS)? + 3;
            for _ in 0..rep_count {
                decoder.huffnode_array[curr_node].num_bits = node_bits;
                curr_node += 1;
            }
        }

        if curr_node != NUM_CODES {
            return Err(HuffmanError::InvalidData);
        }

        decoder.assign_canonical_codes()?;
        decoder.build_lookup_table();

        Ok(decoder)
    }

    pub fn decode_one(&self, reader: &mut BitReader<'a>) -> Result<u32, HuffmanError> {
        let bits = reader.peek_u32(MAX_BITS)?;
        let lookup = self.lookup_array[bits as usize];
        reader.skip((lookup & 0x1f) as u64)?;
        Ok(lookup as u32 >> 5)
    }

    fn assign_canonical_codes(&mut self) -> Result<(), HuffmanError> {
        // todo: use iterators
        let mut curr_start = 0;

        // technically we need the histogram but not if we're read only
        let mut histogram = [0u32; 33];

        // Fill in histogram of bit lengths
        for curr_code in 0..NUM_CODES {
            let node = &self.huffnode_array[curr_code];
            if node.num_bits > MAX_BITS {
                return Err(HuffmanError::InternalInconsistency);
            }
            if node.num_bits <= 32 {
                histogram[node.num_bits as usize] += 1;
            }
        }

        // Determine starting code number of code lengths
        for code_len in (1..33).rev() {
            let next_start = (curr_start + histogram[code_len]) >> 1;
            if code_len != 1 && next_start * 2 != (curr_start + histogram[code_len]) {
                return Err(HuffmanError::InternalInconsistency);
            }
            histogram[code_len] = curr_start;
            curr_start = next_start
        }

        // Assign codes
        for curr_code in 0..NUM_CODES {
            let node = &mut self.huffnode_array[curr_code];
            if node.num_bits > 0 {
                node.bits = histogram[node.num_bits as usize];
                histogram[node.num_bits as usize] += 1;
            }
        }
        Ok(())
    }

    const fn make_lookup(code: u16, bits: u8) -> LookupValue {
        ((code) << 5) | ((bits as u16) & 0x1f)
    }

    fn build_lookup_table(&mut self) {
        for curr_code in 0..NUM_CODES {
            let node = &self.huffnode_array[curr_code];
            if node.num_bits > 0 {
                // Get entry
                let value = Self::make_lookup(curr_code as u16, node.num_bits);

                let shift = MAX_BITS - node.num_bits;
                let dest_idx = (node.bits << shift) as usize;
                let destend_idx = (((node.bits + 1) << shift) - 1) as usize;

                // fill matching entries
                for lookup in &mut self.lookup_array[dest_idx..=destend_idx] {
                    *lookup = value
                }
            }
        }
    }
}