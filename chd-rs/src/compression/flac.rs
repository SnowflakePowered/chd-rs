use std::convert::TryInto;
use std::io::{BufReader, Cursor, Read, Write};
use std::ops::Index;
use std::slice::SliceIndex;
use byteorder::{NativeEndian, WriteBytesExt};
use crate::compression::InternalCodec;
use crate::error::{ChdError, Result};
use claxon::frame::FrameReader;
use claxon::input::BufferedReader;
use crate::cdrom::{CD_FRAME_SIZE, CD_MAX_SECTOR_DATA};

const CHD_FLAC_HEADER_TEMPLATE: [u8; 0x2a] =
[
    0x66, 0x4C, 0x61, 0x43,                         /* +00: 'fLaC' stream header */
    0x80,                                           /* +04: metadata block type 0 (STREAMINFO), */
                                                    /*      flagged as last block */
    0x00, 0x00, 0x22,                               /* +05: metadata block length = 0x22 */
    0x00, 0x00,                                     /* +08: minimum block size */
    0x00, 0x00,                                     /* +0A: maximum block size */
    0x00, 0x00, 0x00,                               /* +0C: minimum frame size (0 == unknown) */
    0x00, 0x00, 0x00,                               /* +0F: maximum frame size (0 == unknown) */
    0x0A, 0xC4, 0x42, 0xF0, 0x00, 0x00, 0x00, 0x00, /* +12: sample rate (0x0ac44 == 44100), */
                                                    /*      numchannels (2), sample bits (16), */
                                                    /*      samples in stream (0 == unknown) */
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, /* +1A: MD5 signature (0 == none) */
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00  /* +2A: start of stream data */
];

/// Custom FLAC header matching CHD specification
pub(crate) struct ChdFlacHeader {
    header: [u8; 0x2a],
}

/// Linked reader struct that appends a custom FLAC header before the audio data.
pub(crate) struct ChdHeaderFlacBufRead<'a> {
    header: &'a [u8],
    inner: &'a [u8],
}

impl ChdFlacHeader {

    /// Create a FLAC header with the given parameters
    pub(crate) fn new(sample_rate: u32, channels: u8, block_size: u32) -> Self {
        let mut header = CHD_FLAC_HEADER_TEMPLATE.clone();

        // min/max blocksize
        // todo: confirm widening..
        // need to check if claxon is similar to libflac or drflac
        // https://github.com/rtissera/libchdr/blame/cdcb714235b9ff7d207b703260706a364282b063/src/libchdr_flac.c#L110
        // https://github.com/mamedev/mame/blob/master/src/lib/util/flac.cpp#L418
        header[0x0a] = (block_size >> 8) as u8;
        header[0x08] = (block_size >> 8) as u8;

        header[0x0b] = (block_size & 0xff) as u8;
        header[0x09] = (block_size & 0xff) as u8;

        header[0x12] = (sample_rate >> 12) as u8;
        header[0x13] = (sample_rate >> 4) as u8;

        header[0x14] = (sample_rate << 4) as u8 | ((channels - 1) << 1) as u8;

        ChdFlacHeader {
            header,
        }
    }

    /// Create a Read implementation that puts the FLAC header before the inner audio data.
    pub (crate) fn as_read<'a>(&'a mut self, buffer: &'a [u8]) -> ChdHeaderFlacBufRead<'a> {
        ChdHeaderFlacBufRead {
            header: &self.header,
            inner: buffer
        }
    }
}

impl <'a> Read for ChdHeaderFlacBufRead<'a> {
    fn read(&mut self, mut buf: &mut [u8]) -> std::io::Result<usize> {
        let mut bytes_read = 0;
        // read header first.
        if let Ok(read) = self.header.read(buf) {
            bytes_read += read;
        }

        // read from the inner data.
        if let Ok(read) = self.inner.read(&mut buf[bytes_read..]) {
            bytes_read += read;
        }
        Ok(bytes_read)
    }
}

struct FlacCodec;

#[cfg(target_endian = "big")]
const IS_LITTLE_ENDIAN: bool = false;

#[cfg(target_endian = "little")]
const IS_LITTLE_ENDIAN: bool = true;

/// Determine FLAC block size from 16-65535, and clamped to 2048 for sweet spot
const fn flac_optimal_size(bytes: u32) -> u32 {
    let mut hunkbytes = bytes / 4;
    while hunkbytes > 2048 {
        hunkbytes /= 2;
    }
    return hunkbytes;
}

impl InternalCodec for FlacCodec {
    fn is_lossy() -> bool {
        false
    }

    fn new(_hunkbytes: u32) -> Result<Self> {
        Ok(FlacCodec)
    }

    fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<u64> {
        // should do the equivalent of flac_decoder_decode_interleaved
        // https://github.com/rtissera/libchdr/blob/cdcb714235b9ff7d207b703260706a364282b063/src/libchdr_flac.c#L158
        let frames = input.len() / CD_FRAME_SIZE as usize;
        let num_samples = frames * CD_MAX_SECTOR_DATA as usize / 4;

        let mut samples_read = num_samples;
        let mut flac_header = ChdFlacHeader::new(44100, 2,
                                             flac_optimal_size(frames as u32 * CD_MAX_SECTOR_DATA));

        let mut flac_buf = BufferedReader::new(flac_header.as_read(input));
        let mut frame_read = FrameReader::new(flac_buf);
        let mut buf = Vec::new();

        // todo: fix this so we write up to the end of the output buffer
        // https://github.com/rtissera/libchdr/blob/cdcb714235b9ff7d207b703260706a364282b063/src/libchdr_flac.c
        // https://github.com/rtissera/libchdr/blob/cdcb714235b9ff7d207b703260706a364282b063/src/libchdr_chd.c
        // https://github.com/mamedev/mame/blob/master/src/lib/util/flac.cpp#L614
        let mut cursor = Cursor::new(output);
        // lt? lte?

        let mut bytes_written = 0;
        while samples_read <= num_samples {
            if let Ok(Some(block)) = frame_read.read_next_or_eof(buf) {
                // always the interleaved case.
                for sample in 0..block.len() {
                    for channel in 0..block.channels() {
                        let sample_data = block.sample(channel, sample) as u16;
                        cursor.write_i16::<NativeEndian>(sample_data as i16)?;
                        bytes_written += 2;
                    }
                    // += 2?
                    samples_read += 1;
                }
                buf = block.into_buffer();
            } else {
                return Err(ChdError::DecompressionError)
            }
        }

        Ok(bytes_written)
    }
}
