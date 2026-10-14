use super::{assign_canonical_codes, rle_full_bits, rle_num_bits, HuffmanError, HuffmanNode};
use crate::const_assert;
use bitreader::{BitReader, BitReaderError};
use std::marker::PhantomData;

type LookupValue = u16;

// The 'default' encoding settings are NUM_BITS = 256, MAX_BITS = 16.
// I prefer to make explicit the parameters at type instantiation for
// clarity purposes.
/// Huffman byte decoder. The encoding parameters for this decoder are `NUM_BITS` = 256 and
/// `MAX_BITS` = 16
pub type Huffman8BitDecoder<'a> = HuffmanDecoder<'a, 256, 16, { lookup_len::<16>() }>;

/// Allocation free CHD huffman decoder.
///
/// `MAX_BITS` must be less than or equal to 24.
/// `LOOKUP_ARRAY_LEN` must be equal to `lookup_len::<MAX_BITS>()`,
/// which is `2 ** MAX_BITS`.
///
/// The Huffman lookup array is stack-allocated. If `MAX_BITS` is too large,
/// the stack may not be large enough to hold the `HuffmanDecoder`. To free up
/// stack space, the `HuffmanDecoder` should be boxed.
pub struct HuffmanDecoder<
    'a,
    const NUM_CODES: usize,
    const MAX_BITS: u8,
    // todo: [feature(generic_const_exprs)] will obsolete this.
    const LOOKUP_ARRAY_LEN: usize,
> {
    lookup_array: [LookupValue; LOOKUP_ARRAY_LEN],
    _phantom: PhantomData<HuffmanNode<'a>>,
}

/// Get the size of the lookup array for a given `MAX_BITS`
pub const fn lookup_len<const MAX_BITS: u8>() -> usize {
    1 << MAX_BITS
}

impl<'a, const NUM_CODES: usize, const MAX_BITS: u8, const LOOKUP_ARRAY_LEN: usize>
    HuffmanDecoder<'a, NUM_CODES, MAX_BITS, LOOKUP_ARRAY_LEN>
{
    const RLE_NUM_BITS: u8 = rle_num_bits(MAX_BITS);

    const RLE_FULL_BITS: u8 = rle_full_bits::<NUM_CODES>();

    fn new() -> Self {
        const_assert!(MAX_BITS: u8 => MAX_BITS <= 24u8);
        const_assert!(MAX_BITS: u8, LOOKUP_ARRAY_LEN: usize => LOOKUP_ARRAY_LEN == lookup_len::<MAX_BITS>());

        HuffmanDecoder {
            lookup_array: [0u16; LOOKUP_ARRAY_LEN],
            _phantom: PhantomData::default(),
        }
    }

    /// Import RLE encoded Huffman tree from the bit stream.
    pub fn from_tree_rle(reader: &mut BitReader<'_>) -> Result<Self, HuffmanError> {
        let mut huffnode_array = [HuffmanNode::default(); NUM_CODES];

        let mut curr_node = 0;
        while curr_node < NUM_CODES {
            let node_bits = reader.read_u8(Self::RLE_NUM_BITS)?;

            // 1 is an escape code
            if node_bits != 1 {
                huffnode_array[curr_node].num_bits = node_bits;
                curr_node += 1;
                continue;
            }

            let node_bits = reader.read_u8(Self::RLE_NUM_BITS)?;
            if node_bits == 1 {
                // Double 1 is just a 1
                huffnode_array[curr_node].num_bits = node_bits;
                curr_node += 1;
                continue;
            }

            let rep_count = reader.read_u8(Self::RLE_NUM_BITS)? + 3;
            for _ in 0..rep_count {
                huffnode_array[curr_node].num_bits = node_bits;
                curr_node += 1;
            }
        }

        if curr_node != NUM_CODES {
            return Err(HuffmanError::InvalidData);
        }

        let mut decoder = HuffmanDecoder::new();
        assign_canonical_codes(&mut huffnode_array, MAX_BITS)?;
        decoder.build_lookup_table(&huffnode_array);

        Ok(decoder)
    }

    /// Import a pre-encoded Huffman tree from the bitstream.
    pub fn from_huffman_tree(reader: &mut BitReader<'_>) -> Result<Self, HuffmanError> {
        // Parse the small tree
        let mut small_huf = HuffmanDecoder::<24, 6, { lookup_len::<6>() }>::new();
        let mut huffnode_array = [HuffmanNode::default(); 24];
        huffnode_array[0].num_bits = reader.read_u8(3)?;
        let start = reader.read_u8(3)? + 1;
        let mut count = 0;

        for idx in 1..24 {
            if idx < start || count == 7 {
                huffnode_array[idx as usize].num_bits = 0;
            } else {
                count = reader.read_u8(3)?;
                huffnode_array[idx as usize].num_bits = if count == 7 { 0 } else { count };
            }
        }

        assign_canonical_codes(&mut huffnode_array, 6)?;
        small_huf.build_lookup_table(&huffnode_array);

        // Process the rest of the data referring to the small tree.
        let mut new_huffman = Self::new();
        let mut huffnode_array = [HuffmanNode::default(); NUM_CODES];
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
                        huffnode_array[curr_node].num_bits = last as u8;
                        curr_node += 1;
                        count -= 1;
                    }
                }
                _ => {
                    last = value - 1;
                    huffnode_array[curr_node].num_bits = last as u8;
                    curr_node += 1;
                }
            }
        }

        if curr_node != NUM_CODES {
            return Err(HuffmanError::InvalidData);
        }

        assign_canonical_codes(&mut huffnode_array, MAX_BITS)?;
        new_huffman.build_lookup_table(&huffnode_array);

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

    const fn make_lookup(code: u16, bits: u8) -> LookupValue {
        ((code) << 5) | ((bits as u16) & 0x1f)
    }

    fn build_lookup_table(&mut self, huffnode_array: &[HuffmanNode<'a>; NUM_CODES]) {
        for (curr_code, node) in huffnode_array.iter().enumerate().take(NUM_CODES) {
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
