use std::marker::PhantomData;
use std::cmp::Ordering;
use std::error::Error;
use std::fmt::{Display, Formatter};
use bitreader::{BitReaderError, BitReader};

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
            HuffmanError::InputBufferTooSmall =>  f.write_str("Input buffer too small"),
            HuffmanError::OutputBufferTooSmall =>  f.write_str("Output buffer too small"),
            HuffmanError::InternalInconsistency =>  f.write_str("Internal inconsistency"),
            HuffmanError::TooManyContexts => f.write_str("Too many contexts"),
        }
    }
}

impl From<bitreader::BitReaderError> for HuffmanError {
    fn from(err: BitReaderError) -> Self {
        match err {
            BitReaderError::NotEnoughData { position: _, length: _, requested: _}=> HuffmanError::InputBufferTooSmall,
            BitReaderError::TooManyBitsForType { position: _, requested: _, allowed: _ } => HuffmanError::TooManyBits,
        }
    }
}
#[derive(Default, Clone)]
pub struct HuffmanNode<'a> {
    parent: usize,
    count: u32,
    weight: u32,
    bits: u32,
    num_bits: u8,
    _phantom: PhantomData<&'a HuffmanNode<'a>> // want this phantomdata to ensure we can only compare nodes with the same lifetimes.
}

impl <'a> PartialEq for HuffmanNode<'a> {
    fn eq(&self, other: &Self) -> bool {
        self.bits == other.bits
    }
}

impl <'a> PartialOrd for HuffmanNode<'a> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        // todo check impl
        if self.weight != other.weight {
            //  node2->weight - node1->weight;
            return Some(other.weight.cmp(&self.weight))
        }
        if self.bits == other.bits {
            return None
        }
        // (int)node1->bits - (int)node2->bits;
        return Some(self.bits.cmp(&other.bits))
    }
}

pub struct HuffmanDecoder<'a> {
    num_codes: usize,
    max_bits: u8,
    lookup_array: Vec<LookupValue>,
    huffnode_array: Vec<HuffmanNode<'a>>,
    histogram: Vec<u32>,
}

impl <'a> HuffmanDecoder<'a> {
    fn new(num_codes: usize, max_bits: u8) -> Result<HuffmanDecoder<'a>, HuffmanError> {
        // todo: limit to 24
        if max_bits > 24 {
            return Err(HuffmanError::TooManyBits)
        }

        Ok(HuffmanDecoder {
            num_codes,
            max_bits,
            lookup_array: vec![0u16; 1 << max_bits],
            huffnode_array: vec![HuffmanNode::default(); num_codes],
            histogram: Vec::new()
        })
    }

    pub fn from_tree_rle(num_codes: usize, max_bits: u8, reader: &mut BitReader<'_>)
            -> Result<HuffmanDecoder<'a>, HuffmanError> {
        let mut decoder = HuffmanDecoder::new(num_codes, max_bits)?;
        let num_bits = match max_bits {
            0..= 7 => 3, // < 8
            8..= 15 => 4, // >= 8
            _ => 5, // >= 16
        };

        let mut curr_node = 0;
        while curr_node < num_codes {

            let node_bits = reader.read_u8(num_bits)?;

            // 1 is an escape code
            if node_bits != 1 {
                decoder.huffnode_array[curr_node].num_bits = node_bits;
                curr_node += 1;
                continue;
            }

            let node_bits =  reader.read_u8(num_bits)?;
            if node_bits == 1 {
                // double 1 is just a 1
                decoder.huffnode_array[curr_node].num_bits = node_bits;
                curr_node += 1;
                continue;
            }

            let rep_count = reader.read_u8(num_bits)? + 3;
            for _ in 0..rep_count {
                decoder.huffnode_array[curr_node].num_bits = node_bits;
                curr_node += 1 ;
            }
        }

        if curr_node != decoder.num_codes {
            return Err(HuffmanError::InvalidData)
        }

        decoder.assign_canonical_codes()?;
        decoder.build_lookup_table();

        Ok(decoder)
    }

    pub fn decode_one(&self, reader: &mut BitReader<'a>) -> Result<u32, HuffmanError> {
        let bits = reader.peek_u32(self.max_bits)?;
        let lookup = self.lookup_array[bits as usize];
        reader.skip((lookup & 0x1f) as u64)?;
        Ok(lookup as u32 >> 5)
    }

    fn assign_canonical_codes(&mut self) -> Result<(), HuffmanError> {
        // todo: use iterators
        let mut curr_start = 0;
        let mut histogram = [0u32; 33];

        // Fill in histogram of bit lengths
        for curr_code in 0..self.num_codes {
            let node = &self.huffnode_array[curr_code];
            if node.num_bits > self.max_bits {
                return Err(HuffmanError::InternalInconsistency)
            }
            if node.num_bits <= 32 {
                histogram[node.num_bits as usize] += 1;
            }
        }

        // Determine starting code number of code lengths
        for code_len in (1..33).rev() {
            let next_start = (curr_start + histogram[code_len]) >> 1;
            if code_len != 1 && next_start * 2 != (curr_start + histogram[code_len]) {
                return Err(HuffmanError::InternalInconsistency)
            }
            histogram[code_len] = curr_start;
            curr_start = next_start
        }

        // Assign codes
        for curr_code in 0..self.num_codes {
            let node = &mut self.huffnode_array[curr_code];
            if node.num_bits > 0 {
                node.bits = histogram[node.num_bits as usize];
                histogram[node.num_bits as usize] += 1;
            }
        }
        Ok(())
    }

    const fn make_lookup(code: u16, bits: u8) -> LookupValue {
        (((code) << 5) | ((bits as u16) & 0x1f))
    }

    fn build_lookup_table(&mut self) {
        for curr_code in 0..self.num_codes {
            let node = &self.huffnode_array[curr_code];
            if node.num_bits > 0 {
                // Get entry
                let value = HuffmanDecoder::make_lookup(curr_code as u16, node.num_bits);

                let shift = self.max_bits - node.num_bits;
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