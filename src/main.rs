use std::io::Cursor;
use std::path::PathBuf;

use clap::Parser;
use image::ImageFormat;
use wow_blp::convert::blp_to_image;
use wow_blp::parser::load_blp_from_buf;
use wow_m2::M2Model;
use wow_m2::SkinFile;
use wow_mpq::Archive;

use gltf::document::{Document, Node, Scene};
use gltf::material::{Image, Material, MimeType, Texture, TextureInfo};
use gltf::mesh::{Mesh, Mode, Primitive};

#[derive(Parser)]
#[command(about = "Convert M2 models from MPQ archives to glTF")]
struct Cli {
    /// M2 model name or path to find in MPQ archives
    m2_file: String,

    /// Output path (default: <model_name>.glb in CWD)
    #[arg(short, long)]
    output: Option<String>,

    /// Directory containing MPQ archives (default: current directory)
    #[arg(short, long)]
    data: Option<PathBuf>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    cmd_export(&cli.m2_file, &cli.output, cli.data.as_deref())
}

/// Search all archives for an M2 file matching the query.
/// Returns the original-case path from the archive's listfile.
fn find_m2_path(archives: &mut [Archive], query: &str) -> Result<String, Box<dyn std::error::Error>> {
    // Normalize query: ensure .m2 extension, lowercase for comparison
    let query_normalized = if query.to_lowercase().ends_with(".m2") {
        query.to_string()
    } else {
        format!("{}.m2", query)
    };
    let query_lower = query_normalized.to_lowercase().replace('/', "\\");

    let is_full_path = query_normalized.contains('\\') || query_normalized.contains('/');

    for archive in archives.iter_mut() {
        let entries = archive.list()?;
        for entry in &entries {
            let entry_lower = entry.name.to_lowercase().replace('/', "\\");
            if is_full_path {
                // Match full path
                if entry_lower == query_lower {
                    return Ok(entry.name.clone());
                }
            } else {
                // Match just the filename portion
                let filename = entry_lower.rsplit('\\').next().unwrap_or(&entry_lower);
                let query_filename = query_lower.rsplit('\\').next().unwrap_or(&query_lower);
                if filename == query_filename {
                    return Ok(entry.name.clone());
                }
            }
        }
    }

    Err(format!("M2 file '{}' not found in any MPQ archive", query).into())
}

/// Try reading a file from each archive in order, returning the first success.
fn read_file_from_archives(
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

fn cmd_export(m2_file: &str, output: &Option<String>, data: Option<&std::path::Path>) -> Result<(), Box<dyn std::error::Error>> {
    // 1. Scan data directory (or CWD) for *.mpq files and open each
    let mpq_dir = match data {
        Some(dir) => dir.to_path_buf(),
        None => std::env::current_dir()?,
    };
    let mut mpq_paths: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(&mpq_dir)? {
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
        return Err(format!("No .mpq files found in '{}'", mpq_dir.display()).into());
    }

    let mut archives: Vec<Archive> = Vec::new();
    for path in &mpq_paths {
        let archive = Archive::open(path)?;
        archives.push(archive);
    }
    println!("Opened {} MPQ archive(s)", archives.len());

    // 2. Find the M2 file in the archives
    let m2_archive_path = find_m2_path(&mut archives, m2_file)?;
    println!("Found: {}", m2_archive_path);

    // 3. Read and parse the M2
    let m2_data = read_file_from_archives(&mut archives, &m2_archive_path)?;
    let model = M2Model::parse(&mut Cursor::new(&m2_data))?;

    // 4. Construct skin path and read it
    let skin_archive_path = m2_archive_path
        .strip_suffix(".m2")
        .or_else(|| m2_archive_path.strip_suffix(".M2"))
        .unwrap_or(&m2_archive_path);
    let skin_archive_path = format!("{}00.skin", skin_archive_path);
    let skin_data = read_file_from_archives(&mut archives, &skin_archive_path)?;
    let skin = SkinFile::parse(&mut Cursor::new(&skin_data))?;
    let skin_indices = skin.indices();
    let skin_triangles = skin.triangles();
    let submeshes = skin.submeshes();

    // 5. Extract and convert textures (BLP -> PNG)
    let mut png_textures: Vec<(String, Vec<u8>)> = Vec::new();
    for tex in &model.textures {
        let raw_path = tex.filename.string.to_string_lossy();
        if raw_path.is_empty() {
            continue;
        }
        let blp_data = read_file_from_archives(&mut archives, &raw_path)?;
        let blp_image = load_blp_from_buf(&blp_data)?;
        let dynamic_image = blp_to_image(&blp_image, 0)?;
        let mut png_buf: Vec<u8> = Vec::new();
        dynamic_image.write_to(&mut Cursor::new(&mut png_buf), ImageFormat::Png)?;

        // Extract basename without extension for the material name
        let basename = raw_path
            .rsplit(&['\\', '/'][..])
            .next()
            .unwrap_or(&raw_path);
        let name = basename
            .strip_suffix(".blp")
            .or_else(|| basename.strip_suffix(".BLP"))
            .unwrap_or(basename)
            .to_string();

        println!("Converted texture: {} ({} bytes PNG)", basename, png_buf.len());
        png_textures.push((name, png_buf));
    }

    // 6. Build glTF

    // Extract vertex data
    let vertex_count = model.vertices.len();
    let mut positions: Vec<f32> = Vec::with_capacity(vertex_count * 3);
    let mut normals: Vec<f32> = Vec::with_capacity(vertex_count * 3);
    let mut uvs: Vec<f32> = Vec::with_capacity(vertex_count * 2);
    let mut uvs2: Vec<f32> = Vec::with_capacity(vertex_count * 2);
    let mut bone_indices: Vec<u8> = Vec::with_capacity(vertex_count * 4);
    let mut bone_weights: Vec<u8> = Vec::with_capacity(vertex_count * 4);

    for vertex in &model.vertices {
        // Convert M2 coordinate system (Z-up) to glTF (Y-up): (X, Y, Z) -> (X, Z, -Y)
        positions.push(vertex.position.x);
        positions.push(vertex.position.z);
        positions.push(-vertex.position.y);

        normals.push(vertex.normal.x);
        normals.push(vertex.normal.z);
        normals.push(-vertex.normal.y);

        uvs.push(vertex.tex_coords.x);
        uvs.push(vertex.tex_coords.y);

        if let Some(tc2) = vertex.tex_coords2 {
            uvs2.push(tc2.x);
            uvs2.push(tc2.y);
        } else {
            uvs2.push(0.0);
            uvs2.push(0.0);
        }

        for j in 0..4 {
            bone_indices.push(vertex.bone_indices[j]);
        }
        for j in 0..4 {
            bone_weights.push(vertex.bone_weights[j]);
        }
    }

    // Build triangle indices from skin data
    let mut all_indices: Vec<u16> = Vec::new();
    for submesh in submeshes {
        let tri_start = submesh.triangle_start as usize;
        let tri_count = submesh.triangle_count as usize;
        for i in 0..tri_count {
            let tri_idx = skin_triangles[tri_start + i] as usize;
            let vertex_idx = skin_indices[tri_idx];
            all_indices.push(vertex_idx);
        }
    }

    // Build material from first texture
    let material = if let Some((name, png_data)) = png_textures.first() {
        let image = Image::new(png_data.clone(), MimeType::Png);
        let texture = Texture::new(image);
        Material::builder()
            .name(name)
            .metallic_factor(0.0)
            .base_color_texture(TextureInfo::new(texture))
            .build()
    } else {
        Material::builder().name("default").metallic_factor(0.0).build()
    };

    // Build primitive
    let primitive = Primitive::builder()
        .mode(Mode::Triangles)
        .position(positions)
        .normal(normals)
        .tex_coord(uvs)
        .tex_coord(uvs2)
        .joints_u8(bone_indices)
        .weights_u8(bone_weights)
        .indices(all_indices)
        .material(material)
        .build();

    let mesh = Mesh::builder().primitive(primitive).build();

    // Derive stem from archive path for naming
    let m2_stem = m2_archive_path
        .rsplit(&['\\', '/'][..])
        .next()
        .unwrap_or(&m2_archive_path)
        .strip_suffix(".m2")
        .or_else(|| m2_archive_path.rsplit(&['\\', '/'][..]).next().unwrap_or(&m2_archive_path).strip_suffix(".M2"))
        .unwrap_or(m2_archive_path.rsplit(&['\\', '/'][..]).next().unwrap_or(&m2_archive_path));

    let model_name = model.name.as_deref().unwrap_or(m2_stem);
    let mesh_node = Node::builder()
        .name(&format!("{}_Geoset0", model_name))
        .mesh(mesh)
        .build();

    let root_node = Node::builder()
        .name(model_name)
        .child(mesh_node)
        .build();

    let scene = Scene::builder()
        .name(&format!("{}_Scene", model_name))
        .node(root_node)
        .build();

    let document = Document::builder().default_scene(scene).build();

    // 7. Resolve output path
    let output_path = match output {
        Some(out) => {
            let path = PathBuf::from(out);
            if path.extension().is_none() {
                path.with_extension("glb")
            } else {
                path
            }
        }
        None => PathBuf::from(format!("{}.glb", m2_stem)),
    };

    // 8. Write GLB
    let mut file = std::fs::File::create(&output_path)?;
    document.to_writer(&mut file)?;
    println!("Wrote {}", output_path.display());

    Ok(())
}
