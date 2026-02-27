use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::Router;
use clap::Parser;
use wow_mpq::Archive;

use wow_gltf::{m2, mpq, wmo};

#[derive(Parser)]
#[command(about = "Serve WoW models as glTF over HTTP")]
struct Cli {
    /// Directory containing MPQ archives, or a single .mpq file
    #[arg(short, long)]
    data: PathBuf,

    /// Cache directory for converted .glb files
    #[arg(short, long, default_value = "glb")]
    cache: PathBuf,

    /// Port to listen on
    #[arg(short, long, default_value_t = 8080)]
    port: u16,
}

struct AppState {
    archives: Mutex<Vec<Archive>>,
    file_index: HashMap<String, String>,
    model_list: String,
    cache_dir: PathBuf,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    let mut archives = mpq::open_archives(&cli.data)?;

    // Build file index: lowercase filename -> full archive path
    let all_files = mpq::list_files(&mut archives)?;
    let mut file_index: HashMap<String, String> = HashMap::new();
    let mut seen = HashSet::new();
    let mut names: Vec<String> = Vec::new();

    for entry in &all_files {
        let lower = entry.to_ascii_lowercase();

        let is_m2 = lower.ends_with(".m2");
        let is_wmo = lower.ends_with(".wmo");
        if !is_m2 && !is_wmo {
            continue;
        }

        // Extract filename only
        let filename_lower = match lower.rfind(|c| c == '\\' || c == '/') {
            Some(pos) => &lower[pos + 1..],
            None => &lower,
        };

        // Skip WMO group files
        if is_wmo {
            let stem = &filename_lower[..filename_lower.len() - 4];
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

        // First occurrence wins for the index; map filename -> full archive path
        file_index
            .entry(filename_lower.to_string())
            .or_insert_with(|| entry.clone());

        if seen.insert(filename_lower.to_string()) {
            let original_filename = match entry.rfind(|c: char| c == '\\' || c == '/') {
                Some(pos) => &entry[pos + 1..],
                None => entry.as_str(),
            };
            names.push(original_filename.to_string());
        }
    }

    names.sort_unstable();
    let model_list = names.join("\n");
    println!("Indexed {} models", names.len());

    std::fs::create_dir_all(&cli.cache)?;

    let state = Arc::new(AppState {
        archives: Mutex::new(archives),
        file_index,
        model_list,
        cache_dir: cli.cache,
    });

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/models", get(models_handler))
        .route("/models/{*path}", get(model_handler))
        .with_state(state);

    let addr = format!("0.0.0.0:{}", cli.port);
    println!("Listening on http://localhost:{}", cli.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn index_handler() -> Html<&'static str> {
    Html(include_str!("../static/index.html"))
}

async fn models_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/plain")], state.model_list.clone())
}

async fn model_handler(
    State(state): State<Arc<AppState>>,
    Path(path): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    let lower = path.to_ascii_lowercase();

    let is_m2 = lower.ends_with(".m2");
    let is_wmo = lower.ends_with(".wmo");
    if !is_m2 && !is_wmo {
        return Err(StatusCode::NOT_FOUND);
    }

    let archive_path = state
        .file_index
        .get(&lower)
        .ok_or(StatusCode::NOT_FOUND)?
        .clone();

    // Compute cache path: strip extension, add .glb
    let stem = lower
        .strip_suffix(".m2")
        .or_else(|| lower.strip_suffix(".wmo"))
        .unwrap_or(&lower)
        .to_string();
    let cache_path = state.cache_dir.join(format!("{}.glb", stem));

    // Check cache first (no lock needed)
    if cache_path.exists() {
        let data = tokio::fs::read(&cache_path)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        return Ok(([(header::CONTENT_TYPE, "model/gltf-binary")], data));
    }

    // Convert inside spawn_blocking (library is not Send-safe)
    let cache_dir = state.cache_dir.clone();
    let cache_path_clone = cache_path.clone();
    let state_clone = Arc::clone(&state);

    let result = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let mut archives = state_clone.archives.lock().map_err(|e| e.to_string())?;

        // Double-check cache (another request may have created it)
        if cache_path_clone.exists() {
            return Ok(());
        }

        // Write to temp file, then rename for atomicity
        let temp_path = cache_dir.join(format!(".tmp_{}", stem));

        let export_result = if is_m2 {
            m2::export_m2(&mut archives, &archive_path, &temp_path)
        } else {
            wmo::export_wmo(&mut archives, &archive_path, &temp_path)
        };
        export_result.map_err(|e| e.to_string())?;

        std::fs::rename(&temp_path, &cache_path_clone).map_err(|e| e.to_string())?;
        Ok(())
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if let Err(e) = result {
        eprintln!("Export error for {}: {}", path, e);
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    let data = tokio::fs::read(&cache_path)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(([(header::CONTENT_TYPE, "model/gltf-binary")], data))
}
