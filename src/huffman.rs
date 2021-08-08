use std::cell::Cell;
use std::rc::Rc;
use bitreader::BitReader;
use std::marker::PhantomData;
use std::cmp::Ordering;

type LookupValue = u16;

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
    max_bits: u16,
    rle_remaining: i32,
    prev_data: Cell<u8>,
    lookup_array: Vec<LookupValue>,
    huffnode_array: Vec<HuffmanNode<'a>>,
    histogram: Vec<u32>,
}

impl <'a> HuffmanDecoder<'a> {
    pub fn new(num_codes: usize, max_bits: u16) -> HuffmanDecoder<'a> {
        // todo: limit to 24
        let mut ret = HuffmanDecoder {
            num_codes,
            max_bits,
            rle_remaining: 0,
            prev_data: Cell::new(0),
            lookup_array: vec![0u16; 1 << max_bits],
            huffnode_array: vec![HuffmanNode::default(); num_codes],
            histogram: Vec::new()
        };
        ret
    }

    pub fn from_tree_rle(num_codes: usize, max_bits: u16, mut reader: BitReader<'_>) -> HuffmanDecoder<'a> {
        let mut decoder = HuffmanDecoder::new(num_codes, max_bits);
        let num_bits = match max_bits {
            0..= 7 => 3, // < 8
            8..= 15 => 4, // >= 8
            _ => 5, // >= 16
        };

        let mut curr_node = 0;
        while curr_node < num_codes {
            let node_bits = reader.read_u8(num_bits).unwrap(); // todo result

            // 1 is an escape code
            if node_bits != 1 {
                decoder.huffnode_array[curr_node].num_bits = node_bits;
                curr_node += 1;
                continue;
            }

            let node_bits =  reader.read_u8(num_bits).unwrap(); // todo result
            if node_bits == 1 {
                // double 1 is just a 1
                decoder.huffnode_array[curr_node].num_bits = node_bits;
                curr_node += 1;
                continue;
            }

            let rep_count = reader.read_u8(num_bits).unwrap() + 3; // todo result
            for _ in 0..rep_count {
                decoder.huffnode_array[curr_node].num_bits = node_bits;
                curr_node += 1 ;
            }
        }

        if curr_node != decoder.num_codes {
            // invalid data
            todo!();
        }

        decoder.assign_canonical_codes();
        todo!();
        decoder
    }

    fn assign_canonical_codes(&mut self) -> () {

    }
}