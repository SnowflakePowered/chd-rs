use std::io::{Cursor, Read, Seek, SeekFrom};
use bitreader::BitReader;
use crate::error::ChdError;
use crate::header::{CodecType, HeaderV5, V5CompressionType};
use byteorder::{ReadBytesExt, BigEndian, WriteBytesExt};
use num_traits::{FromPrimitive, ToPrimitive};
use crc16::{CCITT_FALSE, State};
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
            match V5CompressionType::from_u8(val).ok_or(ChdError::DecompressionError)? {
                V5CompressionType::CompressionRleSmall => { // COMPRESSION_RLE_SMALL
                    map_slice[0] = last_cmp;
                    rep_count = 2 + decoder.decode_one(&mut bitstream)?;
                },
                V5CompressionType::CompressionRleLarge => {// COMPRESSION_RLE_LARGE
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

    let mut curr_off = first_offs;
    let mut last_self = 0;
    let mut last_parent = 0;

    for hunk in 0..header.hunk_count as usize {
        let map_slice = &mut raw_map[(hunk*12)..((hunk+1) * 12)];
        let mut off = curr_off;
        let mut len: u32 = 0;
        let mut crc: u16 = 0;

        match V5CompressionType::from_u8(map_slice[0]).ok_or(ChdError::DecompressionError)? {
            V5CompressionType::CompressionType0 | V5CompressionType::CompressionType1
                | V5CompressionType::CompressionType2 | V5CompressionType::CompressionType3 => {
                len = bitstream.read_u32(length_bits)?;
                curr_off += len as u64;
                crc = bitstream.read_u16(16)?;
            }
            V5CompressionType::CompressionNone => {
                len = header.hunk_bytes;
                curr_off += len as u64;
                crc = bitstream.read_u16(16)?;
            }
            V5CompressionType::CompressionSelf => {
                off = bitstream.read_u64(self_bits)?;
                last_self = off;
            }
            V5CompressionType::CompressionParent => {
                off = bitstream.read_u64(parent_bits)?;
                last_parent = off;
            }

            // pseudo types
            V5CompressionType::CompressionSelf1 => last_self += 1,
            V5CompressionType::CompressionSelf0 => {
                map_slice[0] = V5CompressionType::CompressionSelf.to_u8()
                    .ok_or(ChdError::DecompressionError)?;
                off = last_self;
            }
            V5CompressionType::CompressionParentSelf => {
                map_slice[0] = V5CompressionType::CompressionParent.to_u8()
                    .ok_or(ChdError::DecompressionError)?;
                off = ((hunk * header.hunk_bytes as usize) / header.unit_bytes as usize) as u64;
                last_parent = off;
            }
            V5CompressionType::CompressionParent1 => last_parent += (header.hunk_bytes / header.unit_bytes) as u64,
            V5CompressionType::CompressionParent0 => {
                map_slice[0] = V5CompressionType::CompressionParent.to_u8()
                    .ok_or(ChdError::DecompressionError)?;
                off = last_parent;
            }
            _ => {
                return Err(ChdError::DecompressionError)
            }
        }

        let mut cursor = Cursor::new(map_slice);
        cursor.seek(SeekFrom::Start(1))?;
        cursor.write_u24::<BigEndian>(len)?;
        cursor.write_u48::<BigEndian>(off)?;
        cursor.write_u16::<BigEndian>(crc)?;
    }

    if State::<CCITT_FALSE>::calculate(&raw_map[0..header.hunk_count as usize * 12]) != map_crc {
        return Err(ChdError::DecompressionError)
    }

    Ok(raw_map)
}
