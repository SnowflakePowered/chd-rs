use std::io::{Cursor, Read, Seek, SeekFrom};
use bitreader::BitReader;
use crate::error::ChdError;
use crate::header::{CodecType, HeaderV5};
use byteorder::{ReadBytesExt, BigEndian};
use crate::huffman::HuffmanDecoder;

pub fn read_v5<F: Read + Seek>(header: &HeaderV5, mut file: F) -> Result<Vec<u8>, ChdError>{
    let map_size = header.hunk_count as usize * header.map_entry_bytes as usize;
    let mut raw_map = vec![0u8; map_size];

    // todo: proper traits for each inner header struct
    // uncompressed
    if header.compression[0] == CodecType::None as u32 {
        file.seek(SeekFrom::Start(header.map_offset))?;
        file.read_exact(&mut raw_map[..])?;
        return Ok(raw_map);
    }

    // read compressed params
    file.seek(SeekFrom::Start(header.map_offset))?;

    let map_bytes = file.read_u32::<BigEndian>()?;
    let first_offs = file.read_u48::<BigEndian>()?;
    let map_crc = file.read_u16::<BigEndian>()?;
    let length_bits = file.read_u8()?;
    let self_bits = file.read_u8()?;
    let parent_bits = file.read_u8()?;

    // read map
    let mut compressed: Vec<u8> = vec![0u8; map_bytes as usize];
    file.seek(SeekFrom::Start(header.map_offset + 16))?;
    file.read_exact(&mut compressed[..])?;

    let mut bitstream = BitReader::new(&compressed[..]);
    let mut decoder = HuffmanDecoder::from_tree_rle(16, 8, &mut bitstream)?;

    let mut rep_count = 0;
    let mut last_cmp = 0;
    for hunk in 0..header.hunk_count as usize {
        let map_slice = &mut raw_map[(hunk*12)..((hunk+1) * 12)];
        if rep_count > 0 {
            map_slice[0] = last_cmp;
            rep_count -= 1;
        } else {
            let val = decoder.decode_one(&mut bitstream)? as u8;
            // todo: make this proper enum
            match val {
                7 => { // COMPRESSION_RLE_SMALL
                    map_slice[0] = last_cmp;
                    rep_count = 2 + decoder.decode_one(&mut bitstream)?;
                },
                8 => {// COMPRESSION_RLE_LARGE
                    map_slice[0] = last_cmp;
                    rep_count = 2 + 16 + (decoder.decode_one(&mut bitstream)? << 4);
                    rep_count += decoder.decode_one(&mut bitstream)?;
                }
                _ => {
                    map_slice[0] = val;
                    last_cmp = val;
                }
            }
        }
    }

    // iterate hunks
    // todo: https://github.com/rtissera/libchdr/blob/cdcb714235b9ff7d207b703260706a364282b063/src/libchdr_chd.c#L1247
    // need to make shit proper enum first!

    Ok(raw_map)
}