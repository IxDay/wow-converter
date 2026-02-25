use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;

use clap::Parser;
use converter::mpq;

#[derive(Parser)]
#[command(about = "List M2 and WMO files from MPQ archives")]
struct Cli {
    /// Directory containing MPQ archives, or a single .mpq file
    #[arg(short, long)]
    data: PathBuf,

    /// Output file (default: stdout)
    #[arg(short, long)]
    output: Option<PathBuf>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    let mut archives = mpq::open_archives(&cli.data)?;

    let mut seen = HashSet::new();
    let mut names: Vec<String> = Vec::new();

    for archive in &mut archives {
        let entries = archive.list()?;
        for entry in &entries {
            let lower = entry.name.to_ascii_lowercase();

            let is_m2 = lower.ends_with(".m2");
            let is_wmo = lower.ends_with(".wmo");
            if !is_m2 && !is_wmo {
                continue;
            }

            // Extract filename only (after last \ or /)
            let filename = match lower.rfind(|c| c == '\\' || c == '/') {
                Some(pos) => &lower[pos + 1..],
                None => &lower,
            };

            // Skip WMO group files: stem ends with _NNN (3 digits)
            if is_wmo {
                let stem = &filename[..filename.len() - 4]; // strip ".wmo"
                if stem.len() >= 4 {
                    let bytes = stem.as_bytes();
                    let len = bytes.len();
                    if bytes[len - 4] == b'_'
                        && bytes[len - 3].is_ascii_digit()
                        && bytes[len - 2].is_ascii_digit()
                        && bytes[len - 1].is_ascii_digit()
                    {
                        continue;
                    }
                }
            }

            if seen.insert(filename.to_string()) {
                // Get the original-case filename
                let original_filename = match entry.name.rfind(|c: char| c == '\\' || c == '/') {
                    Some(pos) => &entry.name[pos + 1..],
                    None => &entry.name,
                };
                names.push(original_filename.to_string());
            }
        }
    }

    names.sort_unstable();

    let mut out: Box<dyn Write> = match &cli.output {
        Some(path) => Box::new(std::fs::File::create(path)?),
        None => Box::new(std::io::stdout().lock()),
    };

    for name in &names {
        writeln!(out, "{}", name)?;
    }

    Ok(())
}
