use arrayvec::ArrayVec;
use chd::header::CodecType;
use clap::{Args, Parser, Subcommand};
use num_traits::cast::FromPrimitive;
use rchdman::cd::ExtractCdOptions;
use rchdman::chdman::{self, Benchmark, ChdInfo, HardDiskOptions, HunkStorage, Verification};
use rchdman::hd::{Template, TEMPLATES};
use rchdman::{CommandError, Error, InputSpan, Position, WriteOptions};
use std::ffi::OsStr;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;
use thousands::Separable;

type Result<T> = std::result::Result<T, CommandError>;

fn validate_file_exists(s: &OsStr) -> std::result::Result<PathBuf, std::io::Error> {
    let path = PathBuf::from(s);
    if path.exists() && path.is_file() {
        return Ok(path);
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "File not found or not a file.",
    ))
}

/// Parse a metadata tag, padding tags shorter than four characters with spaces.
fn parse_tag(tag: &str) -> Result<u32> {
    let bytes = tag.as_bytes();
    if bytes.is_empty() || bytes.len() > 4 {
        return Err(CommandError::InvalidTag(tag.to_owned()));
    }
    let mut padded = [b' '; 4];
    padded[..bytes.len()].copy_from_slice(bytes);
    Ok(u32::from_be_bytes(padded))
}

/// Parse a list of codecs, such as `none` or `cdlz,cdzl,cdfl`.
fn parse_compression(compression: &str) -> Result<ArrayVec<CodecType, 4>> {
    let mut codecs = ArrayVec::new();
    if compression == "none" {
        return Ok(codecs);
    }
    for name in compression.split(',') {
        let codec = parse_tag(name)
            .ok()
            .and_then(CodecType::from_u32)
            .filter(|codec| *codec != CodecType::None && !codec.is_legacy())
            .ok_or_else(|| CommandError::InvalidCodec(name.to_owned()))?;
        codecs
            .try_push(codec)
            .map_err(|_| CommandError::TooManyCodecs)?;
    }
    Ok(codecs)
}

/// The four characters of a metadata tag or codec, or `None` if they are not printable.
fn tag_to_string(tag: u32) -> Option<String> {
    let bytes = tag.to_be_bytes();
    bytes
        .iter()
        .all(|byte| byte.is_ascii_graphic() || *byte == b' ')
        .then(|| bytes.iter().map(|&byte| byte as char).collect())
}

/// The chdman name of a codec.
fn codec_name(codec: CodecType) -> &'static str {
    match codec {
        CodecType::None => "None",
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
        CodecType::ZstdV5 => "Zstandard",
        CodecType::ZstdCdV5 => "CD Zstandard",
    }
}

/// The chdman name for a list of codecs, such as `cdlz (CD LZMA), cdzl (CD Deflate)`.
fn compression_string(codecs: &[CodecType]) -> String {
    if codecs.is_empty() {
        return "none".to_owned();
    }
    codecs
        .iter()
        .map(|&codec| {
            let tag = tag_to_string(codec as u32).unwrap_or_default();
            format!("{tag} ({})", codec_name(codec))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Parse the ID of a hard disk template, as listed by `listtemplates`.
fn parse_template(id: &str) -> Result<Template> {
    id.parse::<usize>()
        .ok()
        .and_then(|id| TEMPLATES.get(id).copied())
        .ok_or_else(|| CommandError::InvalidTemplate(id.to_owned()))
}

fn parse_chs(s: &str) -> Result<(u32, u32, u32)> {
    let values: Option<Vec<u32>> = s
        .split(',')
        .map(|value| value.trim().parse().ok())
        .collect();
    match values.as_deref() {
        Some(&[cylinders, heads, sectors]) => Ok((cylinders, heads, sectors)),
        _ => Err(CommandError::InvalidChs(s.to_owned())),
    }
}

/// The abbreviations of chdman options that are more than one letter.
const ABBREVIATIONS: &[(&str, &str)] = &[
    ("-ip", "--inputparent"),
    ("-op", "--outputparent"),
    ("-ob", "--outputbin"),
    ("-sb", "--splitbin"),
    ("-isb", "--inputstartbyte"),
    ("-ish", "--inputstarthunk"),
    ("-ib", "--inputbytes"),
    ("-ih", "--inputhunks"),
    ("-hs", "--hunksize"),
    ("-us", "--unitsize"),
    ("-np", "--numprocessors"),
    ("-tp", "--template"),
    ("-id", "--ident"),
    ("-chs", "--chs"),
    ("-ss", "--sectorsize"),
    ("-ix", "--index"),
    ("-vt", "--valuetext"),
    ("-vf", "--valuefile"),
    ("-nocs", "--nochecksum"),
];

#[derive(Parser)]
#[clap(author, version, about, long_about = None)]
#[clap(propagate_version = true)]
struct Cli {
    #[clap(subcommand)]
    command: Commands,
}

/// Which part of the input to use.
#[derive(Args, Debug, Default)]
struct SpanArgs {
    /// starting byte offset within the input
    #[clap(long, conflicts_with = "inputstarthunk")]
    inputstartbyte: Option<u64>,
    /// starting hunk offset within the input
    #[clap(long)]
    inputstarthunk: Option<u64>,
    /// effective length of input in bytes
    #[clap(long, conflicts_with = "inputhunks")]
    inputbytes: Option<u64>,
    /// effective length of input in hunks
    #[clap(long)]
    inputhunks: Option<u64>,
}

impl SpanArgs {
    fn span(&self) -> InputSpan {
        InputSpan {
            start: self
                .inputstartbyte
                .map(Position::Bytes)
                .or(self.inputstarthunk.map(Position::Hunks)),
            length: self
                .inputbytes
                .map(Position::Bytes)
                .or(self.inputhunks.map(Position::Hunks)),
        }
    }
}

/// Options for the output CHD.
#[derive(Args, Debug)]
struct OutputArgs {
    /// output file name
    #[clap(short, long)]
    output: PathBuf,
    /// parent file name for output CHD
    #[clap(long, parse(try_from_os_str = validate_file_exists))]
    outputparent: Option<PathBuf>,
    /// force overwriting an existing file
    #[clap(short, long)]
    force: bool,
    /// size of each hunk, in bytes
    #[clap(long)]
    hunksize: Option<u32>,
    /// which compression codecs to use (up to 4), or none
    #[clap(short, long, parse(try_from_str = parse_compression))]
    compression: Option<ArrayVec<CodecType, 4>>,
    /// limit the number of processors to use during compression
    #[clap(long)]
    numprocessors: Option<usize>,
}

impl OutputArgs {
    fn options(&self) -> WriteOptions {
        let mut options = WriteOptions::default().threads(self.numprocessors.unwrap_or(0));
        if let Some(codecs) = &self.compression {
            options = options.compression(codecs);
        }
        if let Some(hunk_bytes) = self.hunksize {
            options = options.hunk_bytes(hunk_bytes);
        }
        options
    }
}

/// The input CHD and its parent.
#[derive(Args, Debug)]
struct InputArgs {
    /// input file name
    #[clap(short, long, parse(try_from_os_str = validate_file_exists))]
    input: PathBuf,
    /// parent file name for input CHD
    #[clap(short = 'p', long, parse(try_from_os_str = validate_file_exists))]
    inputparent: Option<PathBuf>,
}

/// A metadata entry of the input CHD.
#[derive(Args, Debug)]
struct MetaArgs {
    /// input file name
    #[clap(short, long, parse(try_from_os_str = validate_file_exists))]
    input: PathBuf,
    /// 4-character tag for metadata
    #[clap(short, long, parse(try_from_str = parse_tag))]
    tag: u32,
    /// indexed instance of this metadata tag
    #[clap(long, default_value = "0")]
    index: u32,
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
    /// Verifies the integrity of a CHD
    Verify {
        #[clap(flatten)]
        input: InputArgs,
        /// fix the SHA-1 if it is incorrect
        #[clap(short, long)]
        fix: bool,
    },
    /// Create a raw CHD from the input file
    Createraw {
        /// input file name
        #[clap(short, long, parse(try_from_os_str = validate_file_exists))]
        input: PathBuf,
        #[clap(flatten)]
        output: OutputArgs,
        #[clap(flatten)]
        span: SpanArgs,
        /// size of each unit, in bytes
        #[clap(long)]
        unitsize: Option<u32>,
    },
    /// Create a hard disk CHD from the input file
    Createhd {
        /// input file name
        #[clap(short, long, parse(try_from_os_str = validate_file_exists))]
        input: Option<PathBuf>,
        #[clap(flatten)]
        output: OutputArgs,
        #[clap(flatten)]
        span: SpanArgs,
        /// use hard disk template (see listtemplates)
        #[clap(long, parse(try_from_str = parse_template), conflicts_with_all = &["chs", "sectorsize"])]
        template: Option<Template>,
        /// name of ident file to provide CHS information
        #[clap(long, parse(try_from_os_str = validate_file_exists))]
        ident: Option<PathBuf>,
        /// specifies CHS values directly
        #[clap(long, parse(try_from_str = parse_chs))]
        chs: Option<(u32, u32, u32)>,
        /// size of the output file
        #[clap(short, long)]
        size: Option<u64>,
        /// size of each hard disk sector
        #[clap(long)]
        sectorsize: Option<u32>,
    },
    /// Create a CD CHD from the input file
    Createcd {
        /// input file name (.cue, .iso)
        #[clap(short, long, parse(try_from_os_str = validate_file_exists))]
        input: PathBuf,
        #[clap(flatten)]
        output: OutputArgs,
    },
    /// Create a DVD CHD from the input file
    Createdvd {
        /// input file name
        #[clap(short, long, parse(try_from_os_str = validate_file_exists))]
        input: PathBuf,
        #[clap(flatten)]
        output: OutputArgs,
        #[clap(flatten)]
        span: SpanArgs,
    },
    /// Extract raw file from a CHD input file
    Extractraw(ExtractRawArgs),
    /// Extract raw hard disk file from a CHD input file
    Extracthd(ExtractRawArgs),
    /// Extract DVD file from a CHD input file
    Extractdvd(ExtractRawArgs),
    /// Extract CD file from a CHD input file
    Extractcd {
        /// output file name (.cue for a cue sheet, otherwise a cdrdao table of contents)
        #[clap(short, long)]
        output: PathBuf,
        /// output file name for binary data
        #[clap(long)]
        outputbin: Option<PathBuf>,
        /// output one binary file per track (%t in the bin name is the track number)
        #[clap(long)]
        splitbin: bool,
        /// force overwriting an existing file
        #[clap(short, long)]
        force: bool,
        #[clap(flatten)]
        input: InputArgs,
    },
    /// Copy data from one CHD to another of the same type
    Copy {
        #[clap(flatten)]
        input: InputArgs,
        #[clap(flatten)]
        output: OutputArgs,
        #[clap(flatten)]
        span: SpanArgs,
    },
    /// Add metadata to the CHD
    Addmeta {
        #[clap(flatten)]
        meta: MetaArgs,
        /// text for the metadata
        #[clap(
            long,
            conflicts_with = "valuefile",
            required_unless_present = "valuefile"
        )]
        valuetext: Option<String>,
        /// file containing data to add
        #[clap(long, parse(try_from_os_str = validate_file_exists))]
        valuefile: Option<PathBuf>,
        /// do not include this metadata information in the overall SHA-1
        #[clap(long)]
        nochecksum: bool,
    },
    /// Remove metadata from the CHD
    Delmeta(MetaArgs),
    /// Dump metadata from the CHD to stdout or to a file
    Dumpmeta {
        #[clap(flatten)]
        meta: MetaArgs,
        /// output file name
        #[clap(short, long)]
        output: Option<PathBuf>,
        /// force overwriting an existing file
        #[clap(short, long)]
        force: bool,
    },
    /// List hard disk templates
    Listtemplates,
    /// Benchmark chd-rs
    Benchmark(InputArgs),
}

#[derive(Args)]
struct ExtractRawArgs {
    /// output file name
    #[clap(short, long)]
    output: PathBuf,
    /// force overwriting an existing file
    #[clap(short, long)]
    force: bool,
    #[clap(flatten)]
    input: InputArgs,
    #[clap(flatten)]
    span: SpanArgs,
}

/// Prints progress at most a few times a second.
fn progress(verb: &'static str) -> impl FnMut(u64, u64) {
    let mut last = None::<Instant>;
    move |done, total| {
        let finished = done >= total;
        if !finished && last.is_some_and(|last| last.elapsed().as_millis() < 250) {
            return;
        }
        last = Some(Instant::now());
        let percent = if total == 0 {
            100.0
        } else {
            100.0 * done as f64 / total as f64
        };
        eprint!("{verb}, {percent:.1}% complete... \r");
        if finished {
            eprintln!();
        }
        let _ = std::io::stderr().flush();
    }
}

fn info(input: &Path, verbose: bool) -> Result<()> {
    let info = ChdInfo::open(input)?;
    let header = &info.header;
    println!("Input file:   {}", input.display());
    println!("File Version: {}", header.version() as u32);
    let sizes = [
        ("Logical size", header.logical_bytes(), " bytes"),
        ("Hunk Size", header.hunk_size().into(), " bytes"),
        ("Total Hunks", header.hunk_count().into(), ""),
        ("Unit Size", header.unit_bytes().into(), " bytes"),
        ("Total Units", header.unit_count(), ""),
    ];
    for (name, size, unit) in sizes {
        let size = size.separate_with_commas();
        println!("{:<14}{size}{unit}", format!("{name}:"));
    }

    let codecs: Vec<CodecType> = header.codecs().into_iter().flatten().collect();
    println!("Compression:  {}", compression_string(&codecs));
    println!(
        "CHD size:     {} bytes",
        info.file_size.separate_with_commas()
    );
    if header.is_compressed() {
        println!(
            "Ratio:        {:.1}%",
            100.0 * info.file_size as f64 / header.logical_bytes() as f64
        );
    }

    let parent = |hash: Option<String>| hash.filter(|_| header.has_parent());
    let hashes = [
        ("MD5", header.md5().map(hex::encode)),
        ("Parent MD5", parent(header.parent_md5().map(hex::encode))),
        ("SHA1", header.sha1().map(hex::encode)),
        ("Data SHA1", header.raw_sha1().map(hex::encode)),
        ("Parent SHA1", parent(header.parent_sha1().map(hex::encode))),
    ];
    for (name, hash) in hashes {
        if let Some(hash) = hash {
            println!("{:<14}{hash}", format!("{name}:"));
        }
    }

    for meta in &info.metadata {
        match tag_to_string(meta.metatag) {
            Some(tag) => println!(
                "Metadata:     Tag='{tag}'  Index={}  Length={} bytes",
                meta.index, meta.length
            ),
            None => println!(
                "Metadata:     Tag={:08x}  Index={}  Length={} bytes",
                meta.metatag, meta.index, meta.length
            ),
        }
        let text: String = meta
            .value
            .iter()
            .take(60)
            .map(|&byte| {
                if byte.is_ascii_graphic() || byte == b' ' {
                    byte as char
                } else {
                    '.'
                }
            })
            .collect();
        println!("              {text}");
    }

    if verbose {
        let total = header.hunk_count() as f64;
        println!();
        println!("     Hunks  Percent  Name");
        println!("----------  -------  ------------------------------------");
        for &(storage, count) in &info.hunk_usage {
            println!(
                "{:>10}   {:>5.1}%  {:<40}",
                count.separate_with_commas(),
                100.0 * count as f64 / total,
                match storage {
                    HunkStorage::Uncompressed => "Uncompressed",
                    HunkStorage::CopyFromSelf => "Copy from self",
                    HunkStorage::CopyFromParent => "Copy from parent",
                    HunkStorage::Mini => "Legacy 8-byte mini",
                    HunkStorage::Codec(codec) => codec_name(codec),
                    _ => "Unknown",
                }
            );
        }
    }
    Ok(())
}

/// Verify a CHD, returning whether it is valid or was fixed.
fn verify(input: &Path, parent: Option<&Path>, fix: bool) -> Result<bool> {
    let verification = Verification::verify(input, parent, &mut progress("Verifying"))?;
    let (stored, computed) = verification.raw_sha1;
    if stored == computed {
        println!("Raw SHA1 verification successful!");
        if let Some((stored, computed)) = verification.overall_sha1 {
            if stored == computed {
                println!("Overall SHA1 verification successful!");
            } else {
                eprintln!("Error: Overall SHA1 in header = {}", hex::encode(stored));
                eprintln!("                  actual SHA1 = {}", hex::encode(computed));
            }
        }
    } else {
        eprintln!("Error: Raw SHA1 in header = {}", hex::encode(stored));
        eprintln!("              actual SHA1 = {}", hex::encode(computed));
    }
    if verification.is_valid() {
        return Ok(true);
    }
    if fix {
        verification.fix(input)?;
        println!("SHA1 updated to correct value in input CHD");
    }
    Ok(fix)
}

fn print_output(output: &OutputArgs) {
    println!("Output CHD:   {}", output.output.display());
    if let Some(parent) = &output.outputparent {
        println!("Parent CHD:   {}", parent.display());
    }
    if let Some(codecs) = &output.compression {
        println!("Compression:  {}", compression_string(codecs));
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("Error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode> {
    let args = std::env::args_os().map(|arg| {
        ABBREVIATIONS
            .iter()
            .find(|(abbreviation, _)| arg == *abbreviation)
            .map_or(arg, |(_, long)| long.into())
    });
    let cli = Cli::parse_from(args);
    println!("rchdman - chd-rs CHD manager {}", env!("CARGO_PKG_VERSION"));

    match &cli.command {
        Commands::Info { input, verbose } => info(input, *verbose)?,
        Commands::Verify { input, fix } => {
            if !verify(&input.input, input.inputparent.as_deref(), *fix)? {
                return Ok(ExitCode::FAILURE);
            }
        }
        Commands::Createraw {
            input,
            output,
            span,
            unitsize,
        } => {
            let options = output.options();
            print_output(output);
            println!("Input file:   {}", input.display());
            chdman::create_raw(
                input,
                &output.output,
                output.outputparent.as_deref(),
                *unitsize,
                span.span(),
                &options,
                output.force,
                &mut progress("Compressing"),
            )?;
            println!("Compression complete");
        }
        Commands::Createhd {
            input,
            output,
            span,
            template,
            ident,
            chs,
            size,
            sectorsize,
        } => {
            let options = output.options();
            print_output(output);
            if let Some(input) = input {
                println!("Input file:   {}", input.display());
            }
            // A template is its CHS values and sector size.
            let geometry = template.map(|template| template.geometry);
            let hd = HardDiskOptions {
                chs: geometry
                    .map(|geometry| (geometry.cylinders, geometry.heads, geometry.sectors))
                    .or(*chs),
                sector_bytes: geometry
                    .map(|geometry| geometry.sector_bytes)
                    .or(*sectorsize),
                size: *size,
                ident: ident.clone(),
            };
            let geometry = chdman::create_hd(
                input.as_deref(),
                &output.output,
                output.outputparent.as_deref(),
                &hd,
                span.span(),
                &options,
                output.force,
                &mut progress("Compressing"),
            )?;
            println!("Cylinders:    {}", geometry.cylinders);
            println!("Heads:        {}", geometry.heads);
            println!("Sectors:      {}", geometry.sectors);
            println!("Bytes/sector: {}", geometry.sector_bytes);
            println!(
                "Logical size: {}",
                geometry.logical_bytes().separate_with_commas()
            );
        }
        Commands::Createcd { input, output } => {
            let options = output.options();
            print_output(output);
            println!("Input file:   {}", input.display());
            let toc = chdman::create_cd(
                input,
                &output.output,
                output.outputparent.as_deref(),
                &options,
                output.force,
                &mut progress("Compressing"),
            )?;
            println!("Input tracks: {}", toc.tracks.len());
            println!(
                "Logical size: {}",
                toc.logical_bytes().separate_with_commas()
            );
        }
        Commands::Createdvd {
            input,
            output,
            span,
        } => {
            let options = output.options();
            print_output(output);
            println!("Input file:   {}", input.display());
            chdman::create_dvd(
                input,
                &output.output,
                output.outputparent.as_deref(),
                span.span(),
                &options,
                output.force,
                &mut progress("Compressing"),
            )?;
            println!("Compression complete");
        }
        Commands::Extractraw(args) | Commands::Extracthd(args) | Commands::Extractdvd(args) => {
            println!("Output File:  {}", args.output.display());
            println!("Input CHD:    {}", args.input.input.display());
            chdman::extract_raw(
                &args.input.input,
                args.input.inputparent.as_deref(),
                &args.output,
                args.span.span(),
                args.force,
                &mut progress("Extracting"),
            )?;
            println!("Extraction complete");
        }
        Commands::Extractcd {
            output,
            outputbin,
            splitbin,
            force,
            input,
        } => {
            println!("Input CHD:    {}", input.input.display());
            println!("Output TOC:   {}", output.display());
            let options = ExtractCdOptions {
                output_bin: outputbin.clone(),
                split_bin: *splitbin,
            };
            let extracted = chdman::extract_cd(
                &input.input,
                input.inputparent.as_deref(),
                output,
                &options,
                *force,
                &mut progress("Extracting"),
            )?;
            for bin in &extracted.bin_files {
                println!("Output Data:  {}", bin.display());
            }
            for warning in &extracted.warnings {
                println!("Warning: {warning}");
            }
            println!("Extraction complete");
        }
        Commands::Copy {
            input,
            output,
            span,
        } => {
            let options = output.options();
            print_output(output);
            println!("Input CHD:    {}", input.input.display());
            chdman::copy(
                &input.input,
                input.inputparent.as_deref(),
                &output.output,
                output.outputparent.as_deref(),
                span.span(),
                &options,
                output.force,
                &mut progress("Compressing"),
            )?;
            println!("Compression complete");
        }
        Commands::Addmeta {
            meta,
            valuetext,
            valuefile,
            nochecksum,
        } => {
            // Text is stored with a terminating NUL.
            let value = match (valuetext, valuefile) {
                (Some(text), _) => {
                    let text = text
                        .strip_prefix('"')
                        .and_then(|text| text.strip_suffix('"'))
                        .unwrap_or(text);
                    let mut value = text.as_bytes().to_vec();
                    value.push(0);
                    value
                }
                (None, Some(file)) => std::fs::read(file).map_err(|source| Error::File {
                    path: file.clone(),
                    source,
                })?,
                (None, None) => unreachable!("clap requires --valuetext or --valuefile"),
            };
            chdman::add_metadata(&meta.input, meta.tag, meta.index, &value, !nochecksum)?;
            println!("Metadata added");
        }
        Commands::Delmeta(meta) => {
            chdman::remove_metadata(&meta.input, meta.tag, meta.index)?;
            println!("Metadata removed");
        }
        Commands::Dumpmeta {
            meta,
            output,
            force,
        } => {
            let metadata = chdman::dump_metadata(&meta.input, meta.tag, meta.index)?;
            match output {
                Some(output) => {
                    if !force && output.exists() {
                        return Err(Error::OutputExists(output.clone()).into());
                    }
                    std::fs::write(output, &metadata.value).map_err(|source| Error::File {
                        path: output.clone(),
                        source,
                    })?;
                    println!(
                        "File ({}) written, {} bytes",
                        output.display(),
                        metadata.value.len()
                    );
                }
                None => std::io::stdout().write_all(&metadata.value)?,
            }
        }
        Commands::Listtemplates => {
            println!();
            println!("ID  Manufacturer  Model           Cylinders  Heads  Sectors  Sector Size  Total Size");
            println!("------------------------------------------------------------------------------------");
            for (id, template) in TEMPLATES.iter().enumerate() {
                let geometry = template.geometry;
                println!(
                    "{id:2}  {:<12}  {:<14}  {:>9}  {:>5}  {:>7}  {:>11}  {:>7} MB",
                    template.manufacturer,
                    template.model,
                    geometry.cylinders,
                    geometry.heads,
                    geometry.sectors,
                    geometry.sector_bytes,
                    geometry.logical_bytes() / 1024 / 1024
                );
            }
        }
        Commands::Benchmark(input) => {
            let parent = input.inputparent.as_deref();
            let result = Benchmark::run(&input.input, parent, &mut progress("Reading"))?;
            println!(
                "Read {} bytes ({} hunks) in {} seconds",
                result.bytes,
                result.hunks,
                result.time.as_secs_f64()
            );
            println!(
                "Rate is {} MB/s",
                (result.bytes / (1024 * 1024)) as f64 / result.time.as_secs_f64()
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}
