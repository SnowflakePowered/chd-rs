use crate::cdrom::{CD_FRAME_SIZE, CD_MAX_SECTOR_DATA, CD_SYNC_HEADER};
use crate::compression::codecs::*;
use crate::compression::ecc::ErrorCorrectedSector;
use crate::compression::CodecImplementation;
use crate::Error;
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

// 8 CD frames, the default chdman hunk size for CDs.
const HUNK_SIZE: usize = 8 * CD_FRAME_SIZE as usize;

fn random_bytes(len: usize, seed: u64) -> Vec<u8> {
    let mut data = vec![0u8; len];
    StdRng::seed_from_u64(seed).fill(&mut data[..]);
    data
}

fn text() -> Vec<u8> {
    b"The quick brown fox jumps over the lazy dog. "
        .iter()
        .copied()
        .cycle()
        .take(HUNK_SIZE)
        .collect()
}

/// A stereo tone as 16-bit PCM with the given byte order.
fn audio(big_endian: bool) -> Vec<u8> {
    (0..HUNK_SIZE / 4)
        .flat_map(|i| {
            let t = i as f64 / 44100.0;
            let left = ((t * 440.0 * std::f64::consts::TAU).sin() * 12000.0) as i16;
            let right = ((t * 660.0 * std::f64::consts::TAU).sin() * 8000.0) as i16;
            [left, right]
        })
        .flat_map(|sample| {
            if big_endian {
                sample.to_be_bytes()
            } else {
                sample.to_le_bytes()
            }
        })
        .collect()
}

/// Mode 1 CD frames with a valid sync header and ECC, so the ECC can be stripped.
fn cd_frames(subcode_seed: Option<u64>) -> Vec<u8> {
    let mut hunk = vec![0u8; HUNK_SIZE];
    let text = text();
    for (frame_num, frame) in hunk.chunks_exact_mut(CD_FRAME_SIZE as usize).enumerate() {
        let (sector, subcode) = frame.split_at_mut(CD_MAX_SECTOR_DATA as usize);
        sector[..12].copy_from_slice(&CD_SYNC_HEADER);
        // MSF address and mode 1.
        sector[12..16].copy_from_slice(&[0x00, 0x02, frame_num as u8, 0x01]);
        sector[16..2064].copy_from_slice(&text[frame_num..][..2048]);
        let mut sector = <&mut [u8; CD_MAX_SECTOR_DATA as usize]>::try_from(sector).unwrap();
        sector.generate_ecc();
        assert!(sector.verify_ecc());

        if let Some(seed) = subcode_seed {
            subcode.copy_from_slice(&random_bytes(subcode.len(), seed + frame_num as u64));
        }
    }
    hunk
}

/// Compress and decompress the input, returning the compressed length.
fn roundtrip<C: CodecImplementation>(name: &str, input: &[u8]) -> Result<usize, Error> {
    let mut codec = C::new(input.len() as u32).expect(name);
    let mut compressed = vec![0u8; input.len()];
    let compressed_len = codec.compress(input, &mut compressed)?;
    assert!(
        compressed_len < input.len(),
        "{name}: not smaller than input"
    );

    let mut output = vec![0u8; input.len()];
    let result = codec
        .decompress(&compressed[..compressed_len], &mut output)
        .unwrap_or_else(|e| panic!("{name}: decompression failed: {e:?}"));
    assert_eq!(result.total_out(), input.len(), "{name}");
    assert!(input == output, "{name}: roundtrip mismatch");
    Ok(compressed_len)
}

fn assert_roundtrip<C: CodecImplementation>(name: &str, input: &[u8]) {
    roundtrip::<C>(name, input).unwrap_or_else(|e| panic!("{name}: compression failed: {e:?}"));
}

fn assert_incompressible<C: CodecImplementation>(name: &str, input: &[u8]) {
    assert!(
        matches!(roundtrip::<C>(name, input), Err(Error::CompressionError)),
        "{name}: data should not compress"
    );
}

macro_rules! codec_tests {
    ($($test:ident: $codec:ty,)*) => {$(
        #[test]
        fn $test() {
            let name = stringify!($codec);
            assert_roundtrip::<$codec>(name, &text());
            assert_roundtrip::<$codec>(name, &vec![0u8; HUNK_SIZE]);
            assert_roundtrip::<$codec>(name, &cd_frames(None));
            assert_incompressible::<$codec>(name, &random_bytes(HUNK_SIZE, 1));
        }
    )*};
}

codec_tests! {
    zlib_roundtrip: ZlibCodec,
    lzma_roundtrip: LzmaCodec,
    zstd_roundtrip: ZstdCodec,
    huff_roundtrip: HuffmanCodec,
    cdzl_roundtrip: CdZlibCodec,
    cdlz_roundtrip: CdLzmaCodec,
    cdzs_roundtrip: CdZstdCodec,
    cdfl_roundtrip: CdFlacCodec,
}

#[test]
fn audio_roundtrip() {
    let input = audio(true);
    assert_roundtrip::<ZlibCodec>("zlib", &input);
    assert_roundtrip::<LzmaCodec>("lzma", &input);
    // zrip does not entropy code literals without matches, so it can not compress the tone.
    #[cfg(feature = "fast_zstd")]
    assert_roundtrip::<ZstdCodec>("zstd", &input);
    #[cfg(not(feature = "fast_zstd"))]
    assert_incompressible::<ZstdCodec>("zstd", &input);
    assert_roundtrip::<HuffmanCodec>("huff", &input);
    assert_roundtrip::<CdFlacCodec>("cdfl", &input);

    // Treated as CD frames, the audio samples in the subcode area do not compress on
    // their own, which fails the whole hunk for the CD codecs other than cdfl.
    assert_incompressible::<CdZlibCodec>("cdzl", &input);
    assert_incompressible::<CdLzmaCodec>("cdlz", &input);
    assert_incompressible::<CdZstdCodec>("cdzs", &input);
}

#[test]
fn flac_roundtrip() {
    let name = "RawFlacCodec";
    for big_endian in [true, false] {
        let input = audio(big_endian);
        assert_roundtrip::<RawFlacCodec>(name, &input);

        // The byte order that matches the samples should compress better.
        let mut codec = RawFlacCodec::new(HUNK_SIZE as u32).unwrap();
        let mut compressed = vec![0u8; HUNK_SIZE];
        codec.compress(&input, &mut compressed).unwrap();
        assert_eq!(compressed[0], if big_endian { b'B' } else { b'L' });
    }
    assert_incompressible::<RawFlacCodec>(name, &random_bytes(HUNK_SIZE, 2));
}

#[test]
fn none_copies() {
    let input = random_bytes(HUNK_SIZE, 3);
    let mut codec = NoneCodec::new(HUNK_SIZE as u32).unwrap();
    let mut output = vec![0u8; HUNK_SIZE];
    assert_eq!(codec.compress(&input, &mut output).unwrap(), HUNK_SIZE);
    assert_eq!(input, output);
}

#[test]
fn cd_strips_ecc() {
    // Sectors with valid ECC compress much better than the same sectors with damaged ECC.
    let input = cd_frames(None);
    let mut damaged = input.clone();
    for frame in damaged.chunks_exact_mut(CD_FRAME_SIZE as usize) {
        frame[2100] ^= 0xFF;
    }

    let stripped = roundtrip::<CdLzmaCodec>("cdlz", &input).unwrap();
    let unstripped = roundtrip::<CdLzmaCodec>("cdlz damaged", &damaged).unwrap();
    assert!(
        stripped < unstripped,
        "{stripped} should be smaller than {unstripped}"
    );
}

#[test]
fn cd_incompressible_subcode() {
    // Like MAME, the subcode must compress on its own for the CD codecs, but only the
    // total has to be smaller for cdfl.
    let input = cd_frames(Some(4));
    assert!(matches!(
        roundtrip::<CdZlibCodec>("cdzl", &input),
        Err(Error::CompressionError)
    ));
    roundtrip::<CdFlacCodec>("cdfl", &input).unwrap();
}

/// A raw A/V frame with metadata, `channels` channels of tones, and a YUY2 video frame,
/// padded with zeroes to `hunk_size`.
fn chav_frame(channels: u8, samples: u16, width: u16, height: u16, hunk_size: usize) -> Vec<u8> {
    let mut frame = Vec::with_capacity(hunk_size);
    let metadata = b"META";
    frame.extend_from_slice(b"chav");
    frame.push(metadata.len() as u8);
    frame.push(channels);
    frame.extend_from_slice(&samples.to_be_bytes());
    frame.extend_from_slice(&width.to_be_bytes());
    frame.extend_from_slice(&height.to_be_bytes());
    frame.extend_from_slice(metadata);

    for channel in 0..channels {
        let frequency = 440.0 * (channel as f64 + 1.0);
        for i in 0..samples {
            let t = i as f64 / 48000.0;
            let sample = ((t * frequency * std::f64::consts::TAU).sin() * 10000.0) as i16;
            frame.extend_from_slice(&sample.to_be_bytes());
        }
    }

    let noise = random_bytes(width as usize * height as usize * 2, 5);
    for y in 0..height as usize {
        for x in 0..width as usize {
            let (luma, chroma) = match y % 4 {
                // A gradient.
                0 => ((x * 255 / width as usize) as u8, 128u8),
                // Flat to the end of the row, which maximizes the RLE count.
                1 => (16, 128),
                // Short runs that do not reach the end of the row.
                2 => (((x / 12) * 40) as u8, ((x / 20) * 30) as u8),
                // Noise, for literal deltas.
                _ => {
                    let index = 2 * (y * width as usize + x);
                    (noise[index], noise[index + 1])
                }
            };
            frame.push(luma);
            frame.push(chroma);
        }
    }

    assert!(frame.len() <= hunk_size);
    frame.resize(hunk_size, 0);
    frame
}

#[test]
fn avhuff_roundtrip() {
    // 720x480 is too big for a quick test, but keeps the shape of a LaserDisc frame.
    let input = chav_frame(2, 1602, 160, 48, 48 * 1024);
    let compressed_len = roundtrip::<AVHuffCodec>("avhu", &input).unwrap();
    assert!(compressed_len < input.len() / 2, "{compressed_len}");

    // Video only.
    assert_roundtrip::<AVHuffCodec>("avhu video", &chav_frame(0, 0, 64, 16, 4096));
}

#[test]
fn avhuff_invalid_input() {
    let mut codec = AVHuffCodec::new(4096).unwrap();
    let mut output = vec![0u8; 4096];

    // Short frames must be padded with zeroes.
    let mut input = chav_frame(1, 64, 32, 8, 4096);
    *input.last_mut().unwrap() = 1;
    assert!(matches!(
        codec.compress(&input, &mut output),
        Err(Error::InvalidData)
    ));

    // Not an A/V frame.
    assert!(matches!(
        codec.compress(&text()[..4096], &mut output),
        Err(Error::InvalidData)
    ));
}

#[test]
fn avhuff_incompressible() {
    // A frame of noise does not compress, without padding to make up for it.
    let video_len = 64 * 16 * 2;
    let mut input = chav_frame(0, 0, 64, 16, 16 + video_len);
    input[16..].copy_from_slice(&random_bytes(video_len, 6));
    assert_incompressible::<AVHuffCodec>("avhu noise", &input);
}

#[test]
fn small_output_buffer() {
    let input = text();
    let mut codec = <LzmaCodec>::new(HUNK_SIZE as u32).unwrap();
    let mut output = vec![0u8; 16];
    assert!(matches!(
        codec.compress(&input, &mut output),
        Err(Error::CompressionError)
    ));
}
