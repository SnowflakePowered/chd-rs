/// Implementation of the MAME CHD Huffman Decoder.
///
/// For format descriptions, see [huffman.cpp](https://github.com/mamedev/mame/blob/master/src/lib/util/huffman.cpp).
use crate::const_assert;
use bitreader::{BitReader, BitReaderError};
use std::cmp::Ordering;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::marker::PhantomData;

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

impl From<BitReaderError> for HuffmanError {
    fn from(err: BitReaderError) -> Self {
        match err {
            BitReaderError::NotEnoughData { .. } => HuffmanError::InputBufferTooSmall,
            BitReaderError::TooManyBitsForType { .. } => HuffmanError::TooManyBits,
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
        // todo: check impl with MAME
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
pub struct HuffmanDecoder<
    'a,
    const NUM_CODES: usize,
    const MAX_BITS: u8,
    // todo: [feature(generic_const_exprs)] will obsolete this.
    const LOOKUP_ARRAY_LEN: usize,
> {
    lookup_array: [LookupValue; LOOKUP_ARRAY_LEN],
    huffnode_array: [HuffmanNode<'a>; NUM_CODES],
}

/// Get the size of the lookup array for a given MAX_BITS
pub(crate) const fn lookup_length<const MAX_BITS: u8>() -> usize {
    1 << MAX_BITS
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

impl<'a, const NUM_CODES: usize, const MAX_BITS: u8, const LOOKUP_ARRAY_LEN: usize>
    HuffmanDecoder<'a, NUM_CODES, MAX_BITS, LOOKUP_ARRAY_LEN>
{
    const RLE_NUM_BITS: u8 = match MAX_BITS {
        0..=7 => 3,  // < 8
        8..=15 => 4, // >= 8
        _ => 5,      // >= 16
    };

    const RLE_FULL_BITS: u8 = rle_full_bits::<NUM_CODES>();

    fn new() -> Self {
        const_assert!(MAX_BITS: u8 => MAX_BITS <= 24u8);
        const_assert!(MAX_BITS: u8, LOOKUP_ARRAY_LEN: usize => LOOKUP_ARRAY_LEN == lookup_length::<MAX_BITS>());

        HuffmanDecoder {
            lookup_array: [0u16; LOOKUP_ARRAY_LEN],
            huffnode_array: [HuffmanNode::default(); NUM_CODES],
        }
    }

    /// Import RLE encoded Huffman tree from the bit stream.
    pub fn from_tree_rle(reader: &mut BitReader<'_>) -> Result<Self, HuffmanError> {
        let mut decoder = HuffmanDecoder::new();

        let mut curr_node = 0;
        while curr_node < NUM_CODES {
            let node_bits = reader.read_u8(Self::RLE_NUM_BITS)?;

            // 1 is an escape code
            if node_bits != 1 {
                decoder.huffnode_array[curr_node].num_bits = node_bits;
                curr_node += 1;
                continue;
            }

            let node_bits = reader.read_u8(Self::RLE_NUM_BITS)?;
            if node_bits == 1 {
                // Double 1 is just a 1
                decoder.huffnode_array[curr_node].num_bits = node_bits;
                curr_node += 1;
                continue;
            }

            let rep_count = reader.read_u8(Self::RLE_NUM_BITS)? + 3;
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

    /// Import pre-encoded Huffman tree from the bitstream.
    pub fn from_huffman_tree(reader: &mut BitReader<'_>) -> Result<Self, HuffmanError> {
        // Parse the small tree
        let mut small_huf = HuffmanDecoder::<24, 6, { lookup_length::<6>() }>::new();

        small_huf.huffnode_array[0].num_bits = reader.read_u8(3)?;
        let start = reader.read_u8(3)? + 1;
        let mut count = 0;

        for idx in 1..24 {
            if idx < start || count == 7 {
                small_huf.huffnode_array[idx as usize].num_bits = 0;
            } else {
                count = reader.read_u8(3)?;
                small_huf.huffnode_array[idx as usize].num_bits =
                    if count == 7 { 0 } else { count };
            }
        }

        small_huf.assign_canonical_codes()?;
        small_huf.build_lookup_table();

        // Process the rest of the data referring to the small tree.
        let mut new_huffman = Self::new();
        let mut last: u32 = 0;
        let mut curr_node = 0;
        while curr_node < NUM_CODES {
            let value = small_huf.decode_one(reader)?;
            match value {
                0 => {
                    let mut count = reader.read_u32(3)? + 2;
                    if count == 7 + 2 {
                        count += reader.read_u32(Self::RLE_FULL_BITS)?;
                    }
                    while count != 0 && curr_node < NUM_CODES {
                        new_huffman.huffnode_array[curr_node].num_bits = last as u8;
                        curr_node += 1;
                        count -= 1;
                    }
                }
                _ => {
                    last = value - 1;
                    new_huffman.huffnode_array[curr_node].num_bits = last as u8;
                    curr_node += 1;
                }
            }
        }

        if curr_node != NUM_CODES {
            return Err(HuffmanError::InvalidData);
        }

        new_huffman.assign_canonical_codes()?;
        new_huffman.build_lookup_table();

        Ok(new_huffman)
    }

    /// Decode a single code from the Huffman bitstream
    pub fn decode_one(&self, reader: &mut BitReader<'a>) -> Result<u32, HuffmanError> {
        // The MAME bitstream.h shifts in zeroes when there are less than MAX_BITS
        // left in the bitstream. We have to explicitly handle this case since
        // bitreader will error on not enough data.
        let bits = match reader.peek_u32(MAX_BITS) {
            Ok(r) => Ok(r),
            Err(e @ BitReaderError::TooManyBitsForType { .. }) => Err(e),
            Err(e @ BitReaderError::NotEnoughData { length: 0, .. }) => Err(e),
            Err(BitReaderError::NotEnoughData {
                length: remainder, ..
            }) => Ok(reader.peek_u32(remainder as u8)? << (MAX_BITS - remainder as u8)),
        }?;
        let lookup = self.lookup_array[bits as usize];
        reader.skip((lookup & 0x1f) as u64)?;
        Ok(lookup as u32 >> 5)
    }

    fn assign_canonical_codes(&mut self) -> Result<(), HuffmanError> {
        let mut curr_start = 0;

        // Since we're read-only we don't need to keep the histogram around
        // once we're done here.
        let mut histogram = [0u32; 33];

        // Fill in histogram of bit lengths.
        for curr_code in 0..NUM_CODES {
            let node = &self.huffnode_array[curr_code];
            if node.num_bits > MAX_BITS {
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

                // Fill matching entries
                for lookup in &mut self.lookup_array[dest_idx..=destend_idx] {
                    *lookup = value
                }
            }
        }
    }
}
