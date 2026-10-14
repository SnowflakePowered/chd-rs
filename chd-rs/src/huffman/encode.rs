use super::{assign_canonical_codes, rle_full_bits, rle_num_bits, HuffmanError, HuffmanNode};
use crate::const_assert;
use bitstream_io::{BigEndian, BitWrite, BitWriter};
use std::cmp::Ordering;

/// Huffman byte encoder. The encoding parameters for this encoder are `NUM_CODES` = 256 and
/// `MAX_BITS` = 16
pub type Huffman8BitEncoder<'a> = HuffmanEncoder<'a, 256, 16>;

// Huffman encoder used to encode the RLE tokens of a Huffman encoded tree.
const SMALL_NUM_CODES: usize = 24;
type SmallHuffmanEncoder<'a> = HuffmanEncoder<'a, SMALL_NUM_CODES, 6>;

/// An RLE token of the code lengths in a Huffman encoded tree.
#[derive(Clone, Copy)]
enum LengthToken {
    /// A single code length.
    Literal(u8),
    /// The previous code length repeated `count + 2` more times.
    Repeat(u16),
}

impl LengthToken {
    /// The symbol encoded by the small tree. Literals are offset by one, with 0 marking a repeat.
    fn symbol(self) -> u32 {
        match self {
            LengthToken::Literal(length) => length as u32 + 1,
            LengthToken::Repeat(_) => 0,
        }
    }
}

/// CHD Huffman encoder.
///
/// `MAX_BITS` must be less than or equal to 24, and `NUM_CODES` must be at most `2 ** MAX_BITS`.
pub struct HuffmanEncoder<'a, const NUM_CODES: usize, const MAX_BITS: u8> {
    histogram: [u32; NUM_CODES],
    // Leaves are the first NUM_CODES nodes, followed by the internal nodes of the tree.
    nodes: Vec<HuffmanNode<'a>>,
    // Scratch list of node indices for building the tree.
    list: Vec<u16>,
}

impl<'a, const NUM_CODES: usize, const MAX_BITS: u8> Default
    for HuffmanEncoder<'a, NUM_CODES, MAX_BITS>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<'a, const NUM_CODES: usize, const MAX_BITS: u8> HuffmanEncoder<'a, NUM_CODES, MAX_BITS> {
    const RLE_NUM_BITS: u8 = rle_num_bits(MAX_BITS);

    const RLE_FULL_BITS: u8 = rle_full_bits::<NUM_CODES>();

    /// Create a new encoder.
    pub fn new() -> Self {
        const_assert!(MAX_BITS: u8 => MAX_BITS <= 24u8);
        const_assert!(NUM_CODES: usize, MAX_BITS: u8 => NUM_CODES <= 1usize << MAX_BITS);
        // Node indices must fit in the u16 parent index.
        const_assert!(NUM_CODES: usize => NUM_CODES * 2 <= u16::MAX as usize + 1);

        HuffmanEncoder {
            histogram: [0u32; NUM_CODES],
            nodes: vec![HuffmanNode::default(); NUM_CODES * 2],
            list: Vec::with_capacity(NUM_CODES * 2),
        }
    }

    /// Compute the Huffman tree for the data to be encoded.
    ///
    /// Returns [`HuffmanError::ValueOutOfRange`] if any value is not less than `NUM_CODES`.
    pub fn try_compute_tree(
        &mut self,
        data: impl IntoIterator<Item = u32>,
    ) -> Result<(), HuffmanError> {
        self.histogram.fill(0);
        for value in data {
            *self
                .histogram
                .get_mut(value as usize)
                .ok_or(HuffmanError::ValueOutOfRange)? += 1;
        }
        let data_count: u32 = self.histogram.iter().sum();

        // Binary search for the largest weight scale that keeps codes within MAX_BITS.
        let mut lower_weight = 0;
        let mut upper_weight = data_count.saturating_mul(2);
        loop {
            let curr_weight = lower_weight + (upper_weight - lower_weight) / 2;
            if self.build_tree(data_count, curr_weight) <= MAX_BITS {
                lower_weight = curr_weight;

                // Early out if it worked with the raw weights, or if we're done searching.
                if curr_weight == data_count || (upper_weight - lower_weight) <= 1 {
                    break;
                }
            } else {
                upper_weight = curr_weight;
            }
        }

        assign_canonical_codes(&mut self.nodes[..NUM_CODES], MAX_BITS)
    }

    /// Build a Huffman tree with the histogram scaled to `total_weight`. Returns the
    /// longest code length.
    fn build_tree(&mut self, total_data: u32, total_weight: u32) -> u8 {
        let nodes = &mut self.nodes;
        let list = &mut self.list;

        // Make a list of all non-zero leaves.
        list.clear();
        for (code, (node, &count)) in nodes.iter_mut().zip(&self.histogram).enumerate() {
            *node = HuffmanNode::default();
            if count == 0 {
                continue;
            }

            list.push(code as u16);
            node.bits = code as u32;
            // Scale the weight by the current effective length, ensuring we don't go to 0.
            node.weight = ((count as u64 * total_weight as u64 / total_data as u64) as u32).max(1);
        }

        // Sort the list by weight, largest weight first. Leaves have unique codes,
        // so the order is total.
        list.sort_unstable_by(|&a, &b| {
            nodes[a as usize]
                .partial_cmp(&nodes[b as usize])
                .unwrap_or(Ordering::Equal)
        });

        // Build the tree by repeatedly joining the two lightest nodes, until only the root is left.
        for new_index in NUM_CODES as u16.. {
            let (Some(node1), Some(node0)) = (list.pop(), list.pop()) else {
                break;
            };

            let weight = nodes[node0 as usize].weight + nodes[node1 as usize].weight;
            nodes[new_index as usize] = HuffmanNode {
                weight,
                ..HuffmanNode::default()
            };
            nodes[node0 as usize].parent = Some(new_index);
            nodes[node1 as usize].parent = Some(new_index);

            // The list is sorted heaviest first, so insert before the first lighter node.
            let position = list.partition_point(|&item| nodes[item as usize].weight >= weight);
            list.insert(position, new_index);
        }

        // Compute the number of bits in each code from its depth in the tree.
        let mut max_bits = 0;
        for code in 0..NUM_CODES {
            let num_bits = if nodes[code].weight > 0 {
                let depth = std::iter::successors(nodes[code].parent, |&parent| {
                    nodes[parent as usize].parent
                })
                .count();
                // A lone code still needs a bit. With u32 weights the depth can not
                // exceed 48, so it always fits.
                depth.max(1) as u8
            } else {
                0
            };

            nodes[code].num_bits = num_bits;
            nodes[code].bits = 0;
            max_bits = max_bits.max(num_bits);
        }
        max_bits
    }

    /// Export the Huffman tree as an RLE encoded tree.
    ///
    /// The resulting bit stream is readable by
    /// [`HuffmanDecoder::from_tree_rle`](crate::huffman::HuffmanDecoder::from_tree_rle).
    pub fn export_tree_rle<W: BitWrite>(&self, writer: &mut W) -> Result<(), HuffmanError> {
        let num_bits = Self::RLE_NUM_BITS as u32;
        let max_repeats = (1 << num_bits) - 1;

        for (length, mut repeat_count) in self.code_length_runs() {
            while repeat_count > 0 {
                if length == 1 {
                    // 1 is an escape code, so it is written twice.
                    writer.write_var(num_bits, 1u8)?;
                    writer.write_var(num_bits, 1u8)?;
                    repeat_count -= 1;
                } else if repeat_count <= 2 {
                    // Two or fewer in a row are written raw.
                    writer.write_var(num_bits, length)?;
                    repeat_count -= 1;
                } else {
                    // Otherwise write a triple using 1 as the escape code.
                    let repeats = (repeat_count - 3).min(max_repeats);
                    writer.write_var(num_bits, 1u8)?;
                    writer.write_var(num_bits, length)?;
                    writer.write_var(num_bits, repeats as u8)?;
                    repeat_count -= repeats + 3;
                }
            }
        }
        Ok(())
    }

    /// Runs of equal code lengths, as `(length, count)`.
    fn code_length_runs(&self) -> impl Iterator<Item = (u8, usize)> + '_ {
        self.nodes[..NUM_CODES]
            .chunk_by(|a, b| a.num_bits == b.num_bits)
            .map(|run| (run[0].num_bits, run.len()))
    }

    /// Export the Huffman tree as a Huffman encoded tree.
    ///
    /// The resulting bit stream is readable by
    /// [`HuffmanDecoder::from_huffman_tree`](crate::huffman::HuffmanDecoder::from_huffman_tree).
    pub fn export_huffman_tree<W: BitWrite>(&self, writer: &mut W) -> Result<(), HuffmanError> {
        // First RLE compress the lengths of all the nodes. Each run is written as its length,
        // followed by the rest of the run as either another literal or a repeat token.
        let tokens: Vec<LengthToken> = self
            .code_length_runs()
            .flat_map(|(length, count)| {
                let rest = match count - 1 {
                    0 => None,
                    1 => Some(LengthToken::Literal(length)),
                    repeats => Some(LengthToken::Repeat(repeats as u16 - 2)),
                };
                std::iter::once(LengthToken::Literal(length)).chain(rest)
            })
            .collect();

        // Use a small Huffman encoder to create a tree for the RLE tokens.
        let mut small_huff = SmallHuffmanEncoder::new();
        small_huff.try_compute_tree(tokens.iter().map(|token| token.symbol()))?;

        // Determine the first and last non-zero lengths of the literal symbols,
        // clamping the first to be 8 at a maximum.
        let literals = &small_huff.nodes[1..SMALL_NUM_CODES];
        let first_non_zero = literals
            .iter()
            .position(|node| node.num_bits != 0)
            .map_or(31, |index| index + 1)
            .min(8);
        let last_non_zero = literals
            .iter()
            .rposition(|node| node.num_bits != 0)
            .map_or(0, |index| index + 1);

        // Output the lengths of each small tree node, starting with the RLE token (0),
        // followed by the first non-zero value, followed by the data terminated by a 7.
        //
        // The terminator is written even if the last node is non-zero, which
        // only happens if the length of a code can be 22 or more.
        writer.write::<3, u8>(small_huff.nodes[0].num_bits)?;
        writer.write::<3, u8>(first_non_zero as u8 - 1)?;
        for index in first_non_zero..=last_non_zero {
            writer.write::<3, u8>(small_huff.nodes[index].num_bits)?;
        }
        writer.write::<3, u8>(7)?;

        // Now encode the RLE tokens, with the count following each repeat.
        for token in tokens {
            small_huff.encode_one(writer, token.symbol())?;
            if let LengthToken::Repeat(count) = token {
                if count < 7 {
                    writer.write::<3, u16>(count)?;
                } else {
                    writer.write::<3, u16>(7)?;
                    writer.write_var(Self::RLE_FULL_BITS as u32, count - 7)?;
                }
            }
        }

        Ok(())
    }

    /// Encode a single value.
    ///
    /// The value must already be in the histogram that the tree was computed from,
    /// otherwise nothing will be written.
    pub fn encode_one<W: BitWrite>(&self, writer: &mut W, data: u32) -> Result<(), HuffmanError> {
        let node = &self.nodes[data as usize];
        debug_assert!(node.num_bits > 0, "value {} has no Huffman code", data);
        writer.write_var(node.num_bits as u32, node.bits)?;
        Ok(())
    }
}

impl<'a> HuffmanEncoder<'a, 256, 16> {
    /// Encode a buffer of bytes with the CHD Huffman codec.
    ///
    /// The resulting buffer can be decoded with [`Huffman8BitDecoder::from_huffman_tree`](crate::huffman::Huffman8BitDecoder).
    pub fn encode<W: std::io::Write>(
        &mut self,
        input: &[u8],
        output: W,
    ) -> Result<(), HuffmanError> {
        self.try_compute_tree(input.iter().map(|&byte| byte as u32))?;

        let mut writer = BitWriter::endian(output, BigEndian);
        self.export_huffman_tree(&mut writer)?;
        for &byte in input {
            self.encode_one(&mut writer, byte as u32)?;
        }
        // Pad the final byte with zeroes, like MAME's bitstream_out::flush.
        writer.byte_align()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Huffman8BitEncoder, HuffmanEncoder, HuffmanError};
    use crate::compression::codecs::HuffmanCodec;
    use crate::compression::CodecImplementation;
    use crate::huffman::{lookup_len, Huffman8BitDecoder, HuffmanDecoder};
    use bitreader::BitReader;
    use bitstream_io::{BigEndian, BitWrite, BitWriter};
    use rand::rngs::StdRng;
    use rand::{RngExt, SeedableRng};

    // Seeded so failures are reproducible.
    fn random_bytes(len: usize, seed: u64) -> Vec<u8> {
        let mut data = vec![0u8; len];
        StdRng::seed_from_u64(seed).fill(&mut data[..]);
        data
    }

    // Symbol i appears fib(i + 1) times, which needs codes far longer than 16 bits
    // unless the encoder limits them.
    fn fibonacci_skewed() -> Vec<u8> {
        let (mut a, mut b) = (1usize, 1usize);
        let mut data = Vec::new();
        for symbol in 0..27u8 {
            data.resize(data.len() + a, symbol);
            (a, b) = (b, a + b);
        }
        data
    }

    fn test_inputs() -> Vec<(&'static str, Vec<u8>)> {
        let mut skewed = random_bytes(4096, 0x1234_5678);
        skewed.iter_mut().for_each(|b| *b &= 0x0F);
        skewed.resize(skewed.len() + 8192, 0xFF);

        vec![
            ("empty", Vec::new()),
            ("single symbol", vec![0x42; 4096]),
            (
                "two symbols",
                (0..4096)
                    .map(|i| if i % 3 == 0 { 7 } else { 200 })
                    .collect(),
            ),
            (
                "all symbols uniform",
                (0..=255u8).cycle().take(256 * 16).collect(),
            ),
            ("random", random_bytes(19584, 0xDEAD_BEEF)),
            ("skewed", skewed),
            ("fibonacci", fibonacci_skewed()),
        ]
    }

    fn assert_max_bits<const NUM_CODES: usize, const MAX_BITS: u8>(
        encoder: &HuffmanEncoder<'_, NUM_CODES, MAX_BITS>,
    ) {
        let longest = encoder.nodes[..NUM_CODES]
            .iter()
            .map(|node| node.num_bits)
            .max()
            .unwrap();
        assert!(
            longest <= MAX_BITS,
            "code length {} exceeds {}",
            longest,
            MAX_BITS
        );
    }

    #[test]
    fn huffman_tree_roundtrip() {
        for (name, input) in test_inputs() {
            let mut encoder = Huffman8BitEncoder::new();
            let mut output = Vec::new();
            encoder.encode(&input, &mut output).expect(name);
            assert_max_bits(&encoder);

            let mut reader = BitReader::new(&output);
            let decoder = Huffman8BitDecoder::from_huffman_tree(&mut reader).expect(name);
            for (i, &expected) in input.iter().enumerate() {
                let value = decoder.decode_one(&mut reader).expect(name);
                assert_eq!(value, expected as u32, "{}: mismatch at {}", name, i);
            }
        }
    }

    #[test]
    fn huff_codec_roundtrip() {
        let hunk_size = 19584;
        let mut codec = HuffmanCodec::new(hunk_size).unwrap();
        for (name, input) in test_inputs() {
            let mut input = input;
            input.resize(hunk_size as usize, 0);

            let mut encoder = Huffman8BitEncoder::new();
            let mut compressed = Vec::new();
            encoder.encode(&input, &mut compressed).expect(name);

            let mut output = vec![0u8; hunk_size as usize];
            codec.decompress(&compressed, &mut output).expect(name);
            assert_eq!(input, output, "{}", name);
        }
    }

    fn rle_roundtrip<const NUM_CODES: usize, const MAX_BITS: u8, const LOOKUP_LEN: usize>(
        name: &str,
        input: &[u32],
    ) {
        let mut encoder = HuffmanEncoder::<'_, NUM_CODES, MAX_BITS>::new();
        encoder.try_compute_tree(input.iter().copied()).expect(name);
        assert_max_bits(&encoder);

        let mut output = Vec::new();
        let mut writer = BitWriter::endian(&mut output, BigEndian);
        encoder.export_tree_rle(&mut writer).expect(name);
        for &value in input {
            encoder.encode_one(&mut writer, value).expect(name);
        }
        writer.byte_align().expect(name);

        let mut reader = BitReader::new(&output);
        let decoder =
            HuffmanDecoder::<'_, NUM_CODES, MAX_BITS, LOOKUP_LEN>::from_tree_rle(&mut reader)
                .expect(name);
        for (i, &expected) in input.iter().enumerate() {
            let value = decoder.decode_one(&mut reader).expect(name);
            assert_eq!(value, expected, "{}: mismatch at {}", name, i);
        }
    }

    #[test]
    fn value_out_of_range() {
        let mut encoder = HuffmanEncoder::<'_, 16, 8>::new();
        assert!(matches!(
            encoder.try_compute_tree([0, 15, 16]),
            Err(HuffmanError::ValueOutOfRange)
        ));
    }

    #[test]
    fn rle_tree_roundtrip() {
        // The V5 compressed map uses a 16 code, 8 bit tree.
        let map_like: Vec<u32> = (0..2000u32)
            .map(|i| match i % 10 {
                0..=6 => 0,
                7 => 1,
                8 => 4 + (i / 10) % 3,
                _ => 15,
            })
            .collect();
        rle_roundtrip::<16, 8, { lookup_len::<8>() }>("map like", &map_like);

        // A dominant symbol gets a 1 bit code, exercising the escape code.
        let mut dominant = vec![3u32; 1000];
        dominant.extend(0..16);
        rle_roundtrip::<16, 8, { lookup_len::<8>() }>("dominant", &dominant);

        // Long runs of equal lengths exercise the repeat count.
        let uniform: Vec<u32> = (0..256u32).cycle().take(256 * 4).collect();
        rle_roundtrip::<256, 16, { lookup_len::<16>() }>("uniform", &uniform);

        let random: Vec<u32> = random_bytes(8192, 0xC0FF_EE00)
            .into_iter()
            .map(u32::from)
            .collect();
        rle_roundtrip::<256, 16, { lookup_len::<16>() }>("random", &random);

        rle_roundtrip::<16, 8, { lookup_len::<8>() }>("single", &[5u32; 64]);
    }
}
