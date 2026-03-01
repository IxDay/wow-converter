use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};

use wow_mpq::Archive;

/// Per-file pool of opened file descriptors.
struct FilePool {
    archives: Mutex<Vec<Archive>>,
    available: Condvar,
}

impl FilePool {
    fn acquire(&self) -> Archive {
        let mut pool = self.archives.lock().unwrap();
        loop {
            if let Some(archive) = pool.pop() {
                return archive;
            }
            pool = self.available.wait(pool).unwrap();
        }
    }

    fn release(&self, archive: Archive) {
        self.archives.lock().unwrap().push(archive);
        self.available.notify_one();
    }
}

/// A pool of pre-opened MPQ archives for concurrent access.
/// Each archive file gets its own sub-pool of `depth` file descriptors,
/// so threads only hold one archive at a time and don't block each other
/// across different MPQ files.
pub struct ArchivePool {
    /// One sub-pool per MPQ file, in priority order (sorted by filename).
    file_pools: Vec<FilePool>,
}

impl ArchivePool {
    /// Create a pool opening each archive `depth` times, all in parallel.
    pub fn new(data_path: &Path, depth: usize) -> Result<Self, Box<dyn std::error::Error>> {
        let mpq_paths = collect_mpq_paths(data_path)?;
        let n_files = mpq_paths.len();

        // Open all (n_files × depth) archives in parallel
        let all_archives: Vec<(usize, Archive)> = std::thread::scope(|s| {
            let handles: Vec<_> = mpq_paths
                .iter()
                .enumerate()
                .flat_map(|(file_idx, path)| {
                    (0..depth).map(move |_| {
                        s.spawn(move || {
                            let archive = Archive::open(path).map_err(|e| e.to_string())?;
                            Ok::<_, String>((file_idx, archive))
                        })
                    })
                })
                .collect();

            let mut results = Vec::with_capacity(handles.len());
            for h in handles {
                results.push(h.join().expect("pool thread panicked")?);
            }
            Ok::<_, String>(results)
        })?;

        // Group by file index into sub-pools
        let mut pools: Vec<Vec<Archive>> = (0..n_files).map(|_| Vec::with_capacity(depth)).collect();
        for (file_idx, archive) in all_archives {
            pools[file_idx].push(archive);
        }

        let file_pools: Vec<FilePool> = pools
            .into_iter()
            .map(|archives| FilePool {
                archives: Mutex::new(archives),
                available: Condvar::new(),
            })
            .collect();

        println!(
            "Archive pool: {} files x {} slots",
            n_files, depth
        );

        Ok(Self { file_pools })
    }

    /// Try reading a file from the pooled archives, trying each MPQ in order.
    pub fn read_file(&self, name: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        for file_pool in &self.file_pools {
            let mut archive = file_pool.acquire();
            let result = archive.read_file(name);
            file_pool.release(archive);
            if let Ok(data) = result {
                return Ok(data);
            }
        }
        Err(format!("File '{}' not found in any MPQ archive", name).into())
    }
}

/// Collect sorted MPQ file paths from a directory or single file.
fn collect_mpq_paths(path: &Path) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
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
    Ok(mpq_paths)
}

/// Open MPQ archives from a directory (all `.mpq` files) or a single `.mpq` file.
pub fn open_archives(path: &Path) -> Result<Vec<Archive>, Box<dyn std::error::Error>> {
    let mpq_paths = collect_mpq_paths(path)?;
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

/// Read the (listfile) from each archive and return all filenames.
/// Much faster than `archive.list()` which does a hash lookup per file.
pub fn list_files(archives: &mut [Archive]) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut all_names = Vec::new();
    for archive in archives.iter_mut() {
        match archive.read_file("(listfile)") {
            Ok(data) => {
                let text = String::from_utf8_lossy(&data);
                for line in text.lines() {
                    let line = line.trim();
                    if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
                        continue;
                    }
                    let name = match line.find(';') {
                        Some(pos) => line[..pos].trim(),
                        None => line,
                    };
                    if !name.is_empty() {
                        all_names.push(name.to_string());
                    }
                }
            }
            Err(_) => continue,
        }
    }
    Ok(all_names)
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
