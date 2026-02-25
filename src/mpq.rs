use std::path::{Path, PathBuf};

use wow_mpq::Archive;

/// Open MPQ archives from a directory (all `.mpq` files) or a single `.mpq` file.
pub fn open_archives(path: &Path) -> Result<Vec<Archive>, Box<dyn std::error::Error>> {
    if path.is_file() {
        println!("Opened 1 MPQ archive");
        return Ok(vec![Archive::open(path)?]);
    }

    let mut mpq_paths: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(path)? {
        let path = entry?.path();
        if path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("mpq"))
            == Some(true)
        {
            mpq_paths.push(path);
        }
    }
    mpq_paths.sort();

    if mpq_paths.is_empty() {
        return Err(format!("No .mpq files found in '{}'", path.display()).into());
    }

    let mut archives: Vec<Archive> = Vec::new();
    for path in &mpq_paths {
        archives.push(Archive::open(path)?);
    }
    println!("Opened {} MPQ archive(s)", archives.len());
    Ok(archives)
}

/// Try reading a file from each archive in order, returning the first success.
pub fn read_file(
    archives: &mut [Archive],
    name: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    for archive in archives.iter_mut() {
        if let Ok(data) = archive.read_file(name) {
            return Ok(data);
        }
    }
    Err(format!("File '{}' not found in any MPQ archive", name).into())
}

/// Search all archives for a file matching `query` with the given `extension`.
/// For `.wmo`, skips group files (e.g. `building_000.wmo`).
pub fn find_file(
    archives: &mut [Archive],
    query: &str,
    extension: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let query_normalized = if query.to_lowercase().ends_with(extension) {
        query.to_string()
    } else {
        format!("{}{}", query, extension)
    };
    let query_lower = query_normalized.to_lowercase().replace('/', "\\");
    let is_full_path = query_normalized.contains('\\') || query_normalized.contains('/');

    for archive in archives.iter_mut() {
        let entries = archive.list()?;
        for entry in &entries {
            let entry_lower = entry.name.to_lowercase().replace('/', "\\");

            // For .wmo searches, skip group files like building_000.wmo
            if extension == ".wmo" && is_wmo_group_file(&entry_lower) {
                continue;
            }

            if is_full_path {
                if entry_lower == query_lower {
                    return Ok(entry.name.clone());
                }
            } else {
                let filename = entry_lower.rsplit('\\').next().unwrap_or(&entry_lower);
                let query_filename = query_lower.rsplit('\\').next().unwrap_or(&query_lower);
                if filename == query_filename {
                    return Ok(entry.name.clone());
                }
            }
        }
    }

    Err(format!(
        "File '{}' not found in any MPQ archive",
        query_normalized
    )
    .into())
}

/// Returns true if the path looks like a WMO group file (e.g. `foo_000.wmo`).
fn is_wmo_group_file(path_lower: &str) -> bool {
    let stem = path_lower.strip_suffix(".wmo").unwrap_or(path_lower);
    // Group files end with _NNN (3 digits)
    if let Some(tail) = stem.rsplit('_').next() {
        tail.len() == 3 && tail.chars().all(|c| c.is_ascii_digit())
    } else {
        false
    }
}
