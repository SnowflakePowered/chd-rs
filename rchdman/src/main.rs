use anyhow::anyhow;
use chd::chd::ChdFile;
use chd::header;
use chd::header::{ChdHeader, CodecType};
use clap::{Parser, Subcommand};
use num_traits::cast::FromPrimitive;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{Read, Seek};
use std::path::PathBuf;
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
}

fn get_file_version(chd: &ChdHeader) -> usize {
    match chd {
        ChdHeader::V1Header(_) => 1,
        ChdHeader::V2Header(_) => 2,
        ChdHeader::V3Header(_) => 3,
        ChdHeader::V4Header(_) => 4,
        ChdHeader::V5Header(_) => 5,
    }
}

fn print_hash(header: &ChdHeader) {
    match header {
        ChdHeader::V1Header(h) | ChdHeader::V2Header(h) => {
            println!("MD5:\t\t{}", hex::encode(h.md5));
            if header.has_parent() {
                println!("Parent MD5:\t{}", hex::encode(h.parent_md5));
            }
        }
        ChdHeader::V3Header(h) => {
            println!("MD5:\t\t{}", hex::encode(h.md5));
            if header.has_parent() {
                println!("Parent MD5:\t{}", hex::encode(h.parent_md5));
            }
            println!("SHA1:\t\t{}", hex::encode(h.sha1));
            if header.has_parent() {
                println!("Parent SHA1:\t{}", hex::encode(h.parent_sha1));
            }
        }
        ChdHeader::V4Header(h) => {
            println!("SHA1:\t\t{}", hex::encode(h.sha1));
            if header.has_parent() {
                println!("Parent SHA1:\t{}", hex::encode(h.parent_sha1));
            }
        }
        ChdHeader::V5Header(h) => {
            println!("SHA1:\t\t{}", hex::encode(h.sha1));
            println!("Data SHA1:\t{}", hex::encode(h.raw_sha1));
            if header.has_parent() {
                println!("Parent SHA1:\t{}", hex::encode(h.parent_sha1));
            }
        }
    }
}

fn print_compression(header: &ChdHeader) {
    print!("Compression:\t");
    if !header.is_compressed() {
        println!("none");
        return;
    }

    match header {
        ChdHeader::V1Header(h) | ChdHeader::V2Header(h) => {
            println!("{:?}", CodecType::from_u32(h.compression).unwrap());
        }
        ChdHeader::V3Header(h) => {
            println!("{:?}", CodecType::from_u32(h.compression).unwrap());
        }
        ChdHeader::V4Header(h) => {
            println!("{:?}", CodecType::from_u32(h.compression).unwrap());
        }
        ChdHeader::V5Header(h) => {
            for compression in h.compression {
                if compression == 0 {
                    break;
                }
                print!("{:?}, ", CodecType::from_u32(compression).unwrap());
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
    let res = parts.map(|e| char::from_u32(e));
    if res.iter().any(|f| f.is_none()) {
        return Err(anyhow!("unable to parse"));
    }
    Ok(res.map(Option::unwrap))
}

fn print_verbose<F: Seek + Read>(chd: &ChdFile<F>) -> anyhow::Result<()> {
    println!("     Hunks  Percent  Name");
    println!("----------  -------  ------------------------------------");

    for i in 0..chd.map().len() {
        let hunk = chd.map().get_entry(i).unwrap();
        // v5 only
        if !hunk.is_legacy() {
            if !hunk.is_compressed() {
                if hunk.block_offset()? == 0 {
                    // parent
                } else {
                    // none
                }
            }
        }
    }

    Ok(())
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match &cli.command {
        Commands::Info { input, verbose } => {
            let mut f = File::open(input)?;
            let fsize = f.metadata()?.len();
            let mut chd = ChdFile::open_stream(&mut f, None)?;
            println!("Input file:\t{}", input.display());
            println!("File Version:\t{}", get_file_version(chd.header()));
            println!(
                "Logical size:\t{} bytes",
                chd.header().logical_bytes().separate_with_commas()
            );
            println!(
                "Hunk Size:\t{} bytes",
                chd.header().hunk_bytes().separate_with_commas()
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

            if let Some(Ok(metadata)) = chd.metadata().map(|f| f.try_into_vec()) {
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
                    println!("{}", std::str::from_utf8(&meta.value).unwrap())
                }
            }

            // if verbose {
            //     print_verbose(&chd);
            // }
        }
    }
    Ok(())
}
