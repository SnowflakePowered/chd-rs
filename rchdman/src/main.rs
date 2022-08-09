use anyhow::anyhow;
use chd::header::{CodecType, Header};
use chd::iter::LendingIterator;
use chd::map::{CompressionTypeLegacy, MapEntry, CompressionTypeV5};
use chd::Chd;
use clap::{Parser, Subcommand};
use num_traits::cast::FromPrimitive;
use sha1::{Digest, Sha1};
use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;
use thousands::Separable;

fn validate_file_exists(s: &OsStr) -> Result<PathBuf, std::io::Error> {
    let path = PathBuf::from(s);
    if path.exists() && path.is_file() {
        return Ok(path);
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "File not found or not a file.",
    ))
}

fn try_fourcc_to_u32(s: &str) -> anyhow::Result<u32> {
    const fn make_tag(a: &[u8; 4]) -> u32 {
        ((a[0] as u32) << 24) | ((a[1] as u32) << 16) | ((a[2] as u32) << 8) | (a[3] as u32)
    }

    let s = s.as_bytes();
    let tag = [
        s.get(0).map_or(b' ', |f| *f),
        s.get(1).map_or(b' ', |f| *f),
        s.get(2).map_or(b' ', |f| *f),
        s.get(3).map_or(b' ', |f| *f),
    ];

    Ok(make_tag(&tag))
}

#[derive(Parser)]
#[clap(author, version, about, long_about = None)]
#[clap(propagate_version = true)]
struct Cli {
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Displays information about a CHD
    Info {
        /// input file name
        #[clap(short, long, parse(try_from_os_str = validate_file_exists))]
        input: PathBuf,

        /// output additional information
        #[clap(short, long)]
        verbose: bool,
    },
    /// Benchmark chd-rs
    Benchmark {
        /// input file name
        #[clap(short, long, parse(try_from_os_str = validate_file_exists))]
        input: PathBuf,
    },
    /// Verifies the integrity of a CHD
    Verify {
        /// input file name
        #[clap(short, long, parse(try_from_os_str = validate_file_exists))]
        input: PathBuf,
        /// parent file name for input CHD
        #[clap(short = 'p', long, parse(try_from_os_str = validate_file_exists))]
        inputparent: Option<PathBuf>,
    },
    /// Dump metadata from the CHD to stdout or to a file
    Dumpmeta {
        /// input file name
        #[clap(short, long, parse(try_from_os_str = validate_file_exists))]
        input: PathBuf,
        /// output file name
        #[clap(short, long)]
        output: Option<PathBuf>,
        /// force overwriting an existing file
        #[clap(short, long)]
        force: bool,
        /// 4-character tag for metadata
        #[clap(short, long, parse(try_from_str = try_fourcc_to_u32))]
        tag: u32,
        #[clap(short = 'x', long, default_value = "0")]
        index: u32,
    },
    /// Extract raw file from a CHD input file
    Extractraw {
        /// output file name
        #[clap(short, long)]
        output: PathBuf,
        /// force overwriting an existing file
        #[clap(short, long)]
        force: bool,
        /// input file name
        #[clap(short, long, parse(try_from_os_str = validate_file_exists))]
        input: PathBuf,
        /// parent file name for input CHD
        #[clap(short = 'p', long, parse(try_from_os_str = validate_file_exists))]
        inputparent: Option<PathBuf>,
    },
}

fn info(input: &PathBuf, verbose: bool) -> anyhow::Result<()> {
    fn get_file_version(chd: &Header) -> usize {
        match chd {
            Header::V1Header(_) => 1,
            Header::V2Header(_) => 2,
            Header::V3Header(_) => 3,
            Header::V4Header(_) => 4,
            Header::V5Header(_) => 5,
        }
    }

    fn print_hash(header: &Header) {
        match header {
            Header::V1Header(h) | Header::V2Header(h) => {
                println!("MD5:\t\t{}", hex::encode(h.md5));
                if header.has_parent() {
                    println!("Parent MD5:\t{}", hex::encode(h.parent_md5));
                }
            }
            Header::V3Header(h) => {
                println!("MD5:\t\t{}", hex::encode(h.md5));
                if header.has_parent() {
                    println!("Parent MD5:\t{}", hex::encode(h.parent_md5));
                }
                println!("SHA1:\t\t{}", hex::encode(h.sha1));
                if header.has_parent() {
                    println!("Parent SHA1:\t{}", hex::encode(h.parent_sha1));
                }
            }
            Header::V4Header(h) => {
                println!("SHA1:\t\t{}", hex::encode(h.sha1));
                if header.has_parent() {
                    println!("Parent SHA1:\t{}", hex::encode(h.parent_sha1));
                }
            }
            Header::V5Header(h) => {
                println!("SHA1:\t\t{}", hex::encode(h.sha1));
                println!("Data SHA1:\t{}", hex::encode(h.raw_sha1));
                if header.has_parent() {
                    println!("Parent SHA1:\t{}", hex::encode(h.parent_sha1));
                }
            }
        }
    }

    fn codec_name(ty: CodecType) -> &'static str {
        match ty {
            CodecType::None => "Copy from self",
            CodecType::Zlib => "Legacy zlib (Deflate)",
            CodecType::ZlibPlus => "Legacy zlib+ (Deflate)",
            CodecType::AV => "Legacy A/V",
            CodecType::ZLibV5 => "Deflate",
            CodecType::ZLibCdV5 => "CD Deflate",
            CodecType::LzmaCdV5 => "CD LZMA",
            CodecType::FlacCdV5 => "CD FLAC",
            CodecType::FlacV5 => "FLAC",
            CodecType::LzmaV5 => "LZMA",
            CodecType::AVHuffV5 => "A/V Huffman",
            CodecType::HuffV5 => "Huffman",
        }
    }

    fn print_compression(header: &Header) {
        fn to_chdman_compression_name(ty: CodecType) -> &'static str {
            match ty {
                CodecType::None => "none",
                CodecType::Zlib => "Legacy zlib (Deflate)",
                CodecType::ZlibPlus => "Legacy zlib+ (Deflate)",
                CodecType::AV => "Legacy av (AV)",
                CodecType::ZLibV5 => "zlib (Deflate)",
                CodecType::ZLibCdV5 => "cdzl (CD Deflate)",
                CodecType::LzmaCdV5 => "cdlz (CD LZMA)",
                CodecType::FlacCdV5 => "cdfl (CD FLAC)",
                CodecType::FlacV5 => "flac (FLAC)",
                CodecType::LzmaV5 => "lzma (LZMA)",
                CodecType::AVHuffV5 => "avhu (A/V Huffman)",
                CodecType::HuffV5 => "huff (Huffman)",
            }
        }

        print!("Compression:\t");
        if !header.is_compressed() {
            println!("none");
            return;
        }

        match header {
            Header::V1Header(h) | Header::V2Header(h) => {
                println!(
                    "{}",
                    to_chdman_compression_name(CodecType::from_u32(h.compression).unwrap())
                );
            }
            Header::V3Header(h) => {
                println!(
                    "{}",
                    to_chdman_compression_name(CodecType::from_u32(h.compression).unwrap())
                );
            }
            Header::V4Header(h) => {
                println!(
                    "{}",
                    to_chdman_compression_name(CodecType::from_u32(h.compression).unwrap())
                );
            }
            Header::V5Header(h) => {
                for compression in h.compression {
                    if compression == 0 {
                        break;
                    }
                    print!(
                        "{}, ",
                        to_chdman_compression_name(CodecType::from_u32(compression).unwrap())
                    );
                }
                println!();
            }
        }
    }

    fn to_fourcc(fourcc: u32) -> anyhow::Result<[char; 4]> {
        let parts = [
            (fourcc >> 24) & 0xff,
            (fourcc >> 16) & 0xff,
            (fourcc >> 8) & 0xff,
            fourcc & 0xff,
        ];
        let res = parts.map(char::from_u32);
        if res.iter().any(|f| f.is_none()) {
            return Err(anyhow!("unable to parse"));
        }
        Ok(res.map(Option::unwrap))
    }

    fn print_verbose<F: Seek + Read>(chd: &Chd<F>) -> anyhow::Result<()> {
        // can only have 4 comptypes.
        // first four is for the four comp types.
        // next four is NONE, SELF, PARENT, MINI, UNKNOWN
        let mut hunk_count = [0u64; 9];

        let num_hunks = chd.map().len();
        println!();
        println!("     Hunks  Percent  Name");
        println!("----------  -------  ------------------------------------");

        for i in 0..num_hunks {
            let hunk = chd.map().get_entry(i).unwrap();
            match hunk {
                MapEntry::V5Compressed(c) => match c.hunk_type()? {
                    CompressionTypeV5::CompressionType0 => {
                        hunk_count[0] += 1;
                    }
                    CompressionTypeV5::CompressionType1 => {
                        hunk_count[1] += 1;
                    }
                    CompressionTypeV5::CompressionType2 => {
                        hunk_count[2] += 1;
                    }
                    CompressionTypeV5::CompressionType3 => {
                        hunk_count[3] += 1;
                    }
                    CompressionTypeV5::CompressionNone => {
                        hunk_count[4] += 1;
                    }
                    CompressionTypeV5::CompressionSelf
                    | CompressionTypeV5::CompressionSelf0
                    | CompressionTypeV5::CompressionSelf1 => {
                        hunk_count[5] += 1;
                    }
                    CompressionTypeV5::CompressionParent
                    | CompressionTypeV5::CompressionParentSelf
                    | CompressionTypeV5::CompressionParent0
                    | CompressionTypeV5::CompressionParent1 => {}
                    _ => {
                        hunk_count[6] += 1;
                    }
                },
                MapEntry::V5Uncompressed(_) => {
                    hunk_count[4] += 1;
                }
                MapEntry::LegacyEntry(c) => {
                    match c.hunk_type()? {
                        CompressionTypeLegacy::Invalid => {}
                        CompressionTypeLegacy::Compressed => {
                            hunk_count[0] += 1;
                        }
                        CompressionTypeLegacy::Uncompressed => {
                            hunk_count[4] += 1;
                        }
                        CompressionTypeLegacy::Mini => {
                            hunk_count[7] += 1;
                        }
                        CompressionTypeLegacy::SelfHunk => {
                            hunk_count[5] += 1;
                        }
                        CompressionTypeLegacy::ParentHunk => {
                            hunk_count[6] += 1;
                        }
                        CompressionTypeLegacy::ExternalCompressed => {
                            // not sure this is valid.
                            hunk_count[8] += 1;
                        }
                    }
                }
            }
        }

        let results: Vec<(u64, f64, &'static str)> = hunk_count
            .iter()
            .enumerate()
            .map(|(i, count)| {
                let percent = *count as f64 / num_hunks as f64;
                let name = match i {
                    4 => "Uncompressed",
                    5 => "Copy from self",
                    6 => "Copy from parent",
                    7 => "Legacy 8-byte mini",
                    8 => "Unknown",
                    i => codec_name(
                        CodecType::from_u32(match chd.header() {
                            Header::V1Header(h) => h.compression,
                            Header::V2Header(h) => h.compression,
                            Header::V3Header(h) => h.compression,
                            Header::V4Header(h) => h.compression,
                            Header::V5Header(h) => h.compression[i],
                        })
                        .unwrap(),
                    ),
                };
                (*count, percent, name)
            })
            .collect();

        for (count, percent, name) in &results[4..] {
            if *count == 0u64 {
                continue;
            }
            println!(
                "{:>10}   {:>5.1}%  {:<40}",
                count.separate_with_commas(),
                100f64 * percent,
                name
            );
        }

        for (count, percent, name) in &results[..4] {
            if *count == 0u64 {
                continue;
            }
            println!(
                "{:>10}   {:>5.1}%  {:<40}",
                count.separate_with_commas(),
                100f64 * percent,
                name
            );
        }

        Ok(())
    }

    println!("\nchd-rs - rchdman info");
    let mut f = File::open(input)?;
    let fsize = f.metadata()?.len();
    let mut chd = Chd::open(&mut f, None)?;
    println!("Input file:\t{}", input.display());
    println!("File Version:\t{}", get_file_version(chd.header()));
    println!(
        "Logical size:\t{} bytes",
        chd.header().logical_bytes().separate_with_commas()
    );
    println!(
        "Hunk Size:\t{} bytes",
        chd.header().hunk_size().separate_with_commas()
    );
    println!(
        "Total Hunks:\t{}",
        chd.header().hunk_count().separate_with_commas()
    );
    println!(
        "Unit Size:\t{} bytes",
        chd.header().unit_bytes().separate_with_commas()
    );
    println!(
        "Total Units:\t{}",
        chd.header().unit_count().separate_with_commas()
    );
    print_compression(chd.header());
    println!("CHD size:\t{} bytes", fsize.separate_with_commas());

    if chd.header().is_compressed() {
        println!(
            "Ratio:\t\t{:.1}%",
            100.0 * fsize as f64 / chd.header().logical_bytes() as f64
        );
    }

    // hash
    print_hash(chd.header());

    if let Ok(metadata) = chd.metadata_refs().try_into_vec() {
        for meta in metadata {
            let tag = to_fourcc(meta.metatag);
            if let Ok(tag) = tag {
                println!(
                    "Metadata:\tTag='{}'  Index={}  Length={} bytes",
                    tag.iter().collect::<String>(),
                    meta.index,
                    meta.length
                );
            } else {
                println!(
                    "Metadata:\tTag={:0x}  Index={}  Length={} bytes",
                    meta.metatag, meta.index, meta.length
                );
            }
            print!("              \t");
            println!(
                "{}",
                meta.value
                    .iter()
                    .map(|u| {
                        if u.is_ascii_alphanumeric()
                            || u.is_ascii_whitespace()
                            || u.is_ascii_punctuation()
                        {
                            *u as char
                        } else {
                            '.'
                        }
                    })
                    .collect::<String>()
            );
        }
    }

    if verbose {
        print_verbose(&chd)?;
    }

    Ok(())
}

fn benchmark(p: impl AsRef<Path>) -> anyhow::Result<()> {
    println!("\nchd-rs - rchdman benchmark");
    let mut f = BufReader::new(File::open(p)?);

    let start = Instant::now();
    let mut chd = Chd::open(&mut f, None)?;
    let mut hunk_buf = chd.get_hunksized_buffer();
    let mut cmp_buf = Vec::new();
    let hunk_iter = chd.hunks();
    let mut bytes = 0;
    let mut hunk_num = 0;

    hunk_iter.for_each(|mut hunk| {
        bytes += hunk
            .read_hunk_in(&mut cmp_buf, &mut hunk_buf)
            .unwrap_or_else(|_| panic!("could not read_hunk {}", hunk_num));
        hunk_num += 1;
    });

    let time = Instant::now().saturating_duration_since(start);
    println!(
        "Read {} bytes ({} hunks) in {} seconds",
        bytes,
        hunk_num,
        time.as_secs_f64()
    );
    println!(
        "Rate is {} MB/s",
        (bytes / (1024 * 1024)) as f64 / time.as_secs_f64()
    );

    Ok(())
}

fn verify(input: impl AsRef<Path>, inputparent: Option<impl AsRef<Path>>) -> anyhow::Result<()> {
    println!("\nchd-rs - rchdman verify");
    let f = BufReader::new(File::open(input)?);

    let p = if let Some(parent) = inputparent {
        let f = BufReader::new(File::open(parent)?);
        let parent_chd = Chd::open(f, None)?;
        Some(Box::new(parent_chd))
    } else {
        None
    };

    let mut chd = Chd::open(f, p)?;

    let header = chd.header();
    if !header.is_compressed() {
        return Err(anyhow!("No verification to be done; CHD is uncompressed"));
    }

    let raw_sha1 = match header {
        Header::V3Header(h) => h.sha1,
        Header::V4Header(h) => h.raw_sha1,
        Header::V5Header(h) => h.raw_sha1,
        _ => return Err(anyhow!("No verification to be done; CHD has no checksum")),
    };

    let mut hasher = Sha1::new();
    let mut out_buf = chd.get_hunksized_buffer();
    let mut hunk_iter = chd.hunks();
    let mut comp_buffer = Vec::new();
    while let Some(mut hunk) = hunk_iter.next() {
        hunk.read_hunk_in(&mut comp_buffer, &mut out_buf)?;
        hasher.update(&out_buf);
    }
    let raw_result = hasher.finalize();

    if raw_result[..] == raw_sha1[..] {
        println!("Raw SHA1 verification successful!");
    } else {
        eprintln!(
            "Error: Raw SHA1 in header = {}\n              actual SHA1 = {}\n",
            hex::encode(raw_sha1),
            hex::encode(raw_result)
        );
    }

    // todo: full verification
    Ok(())
}

fn dumpmeta(
    input: impl AsRef<Path>,
    output: Option<&PathBuf>,
    force: bool,
    tag: u32,
    index: u32,
) -> anyhow::Result<()> {
    println!("\nchd-rs - rchdman dumpmeta");

    let mut f = BufReader::new(File::open(input)?);
    let mut chd = Chd::open(&mut f, None)?;

    let metas = chd.metadata_refs().try_into_vec()?;
    let tag = metas
        .iter()
        .find(|p| p.metatag == tag && p.index == index)
        .ok_or_else(|| anyhow!("Error reading metadata: can't find metadata"))?;

    if let Some(output) = output {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(!force)
            .create(true)
            .truncate(true)
            .open(output)?;
        file.write_all(&*tag.value)?;
        println!("File ({}) written, {} bytes", output.display(), tag.length)
    } else {
        println!("{}", String::from_utf8_lossy(&*tag.value));
    }
    Ok(())
}

fn extractraw(
    input: &PathBuf,
    inputparent: Option<impl AsRef<Path>>,
    output: &PathBuf,
    force: bool,
) -> anyhow::Result<()> {
    println!("\nchd-rs - rchdman extractraw");
    let mut output_file = BufWriter::new(
        OpenOptions::new()
            .write(true)
            .create_new(!force)
            .create(true)
            .truncate(true)
            .open(output)?,
    );

    println!("Output File:  {}", output.display());
    println!("Input CHD:    {}", input.display());

    let f = BufReader::new(File::open(input)?);

    let p = if let Some(parent) = inputparent {
        let f = BufReader::new(File::open(parent)?);
        let parent_chd = Chd::open(f, None)?;
        Some(Box::new(parent_chd))
    } else {
        None
    };

    let mut chd = Chd::open(f, p)?;
    let mut cmp_buf = Vec::new();
    let mut out_buf = chd.get_hunksized_buffer();
    let mut hunk_iter = chd.hunks();
    while let Some(mut hunk) = hunk_iter.next() {
        hunk.read_hunk_in(&mut cmp_buf, &mut out_buf)?;
        output_file.write_all(&out_buf)?;
    }
    println!("Extraction complete");
    output_file.flush()?;
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match &cli.command {
        Commands::Info { input, verbose } => info(input, *verbose)?,
        Commands::Benchmark { input } => benchmark(input)?,
        Commands::Verify { input, inputparent } => verify(input, inputparent.as_deref())?,
        Commands::Dumpmeta {
            input,
            output,
            force,
            tag,
            index,
        } => dumpmeta(input, output.as_ref(), *force, *tag, *index)?,
        Commands::Extractraw {
            input,
            inputparent,
            force,
            output,
        } => extractraw(input, inputparent.as_deref(), output, *force)?,
    }
    Ok(())
}
