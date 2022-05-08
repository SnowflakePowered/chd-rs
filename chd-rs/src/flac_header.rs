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
    header: [u8; CHD_FLAC_HEADER_TEMPLATE.len()],
}

/// Linked reader struct that appends a custom FLAC header before the audio data.
pub(crate) struct ChdHeaderFlacBufRead<'a> {
    header: &'a [u8],
    inner: &'a [u8],
}

impl ChdFlacHeader {

    pub const fn len() -> usize {
        CHD_FLAC_HEADER_TEMPLATE.len()
    }

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
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
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