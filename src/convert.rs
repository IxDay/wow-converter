use wow_gltf::{m2, mpq, wmo};

use std::path::PathBuf;

use clap::Parser;

#[derive(Parser)]
#[command(about = "Convert M2/WMO models from MPQ archives to glTF")]
struct Cli {
    /// Model name or path (M2 or WMO) to find in MPQ archives
    model_file: String,

    /// Output path (default: <model_name>.glb in CWD)
    #[arg(short, long)]
    output: Option<String>,

    /// Directory containing MPQ archives (default: current directory)
    #[arg(short, long)]
    data: Option<PathBuf>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    let cwd = std::env::current_dir()?;
    let mpq_dir = cli.data.as_deref().unwrap_or(&cwd);
    let mut archives = mpq::open_archives(mpq_dir)?;

    // Auto-detect format by extension, or try both if no extension given
    let query_lower = cli.model_file.to_lowercase();
    let (archive_path, is_wmo) = if query_lower.ends_with(".wmo") {
        (mpq::find_file(&mut archives, &cli.model_file, ".wmo")?, true)
    } else if query_lower.ends_with(".m2") {
        (mpq::find_file(&mut archives, &cli.model_file, ".m2")?, false)
    } else {
        // No extension — try M2 first, then WMO
        if let Ok(path) = mpq::find_file(&mut archives, &cli.model_file, ".m2") {
            (path, false)
        } else {
            (mpq::find_file(&mut archives, &cli.model_file, ".wmo")?, true)
        }
    };
    println!("Found: {}", archive_path);

    // Derive output path
    let stem = archive_path
        .rsplit(&['\\', '/'][..])
        .next()
        .unwrap_or(&archive_path);
    let stem = stem
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(stem);

    let output_path = match &cli.output {
        Some(out) => {
            let path = PathBuf::from(out);
            if path.extension().is_none() {
                path.with_extension("glb")
            } else {
                path
            }
        }
        None => PathBuf::from(format!("{}.glb", stem)),
    };

    drop(archives);

    let parallelism = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let pool = mpq::ArchivePool::new(mpq_dir, parallelism)?;

    if is_wmo {
        wmo::export_wmo(&pool, &archive_path, &output_path)?;
    } else {
        m2::export_m2(&pool, &archive_path, &output_path)?;
    }

    Ok(())
}
