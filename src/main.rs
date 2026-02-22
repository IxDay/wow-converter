use std::io::Cursor;
use std::path::PathBuf;

use clap::Parser;
use wow_m2::M2Model;
use wow_m2::SkinFile;

use gltf::document::{Document, Node, Scene};
use gltf::mesh::{Mesh, Mode, Primitive};
use gltf::material::{Image, Material, MimeType, Texture, TextureInfo};

#[derive(Parser)]
#[command(about = "Convert M2 models to glTF")]
struct Cli {
    /// Path to directory containing M2, skin, and texture files
    directory: PathBuf,

    /// Output path (default: <directory_name>.glb inside the directory)
    #[arg(short, long)]
    output: Option<String>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let dir = &cli.directory;

    if !dir.is_dir() {
        return Err(format!("'{}' is not a directory", dir.display()).into());
    }

    // 1. Find the M2 file
    let mut m2_files: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("m2")) == Some(true) {
            m2_files.push(path);
        }
    }
    let m2_path = match m2_files.len() {
        0 => return Err(format!("no .m2 file found in '{}'", dir.display()).into()),
        1 => m2_files.remove(0),
        n => return Err(format!("found {} .m2 files in '{}', expected exactly 1", n, dir.display()).into()),
    };
    let m2_stem = m2_path.file_stem().unwrap().to_string_lossy().to_string();

    // 2. Parse the M2
    let m2_data = std::fs::read(&m2_path)?;
    let model = M2Model::parse(&mut Cursor::new(&m2_data))?;

    // 3. Find and load the first skin file
    let skin_path = dir.join(format!("{}00.skin", m2_stem));
    if !skin_path.exists() {
        return Err(format!("skin file not found: '{}'", skin_path.display()).into());
    }
    let skin = SkinFile::load(&skin_path)?;
    let skin_indices = skin.indices();
    let skin_triangles = skin.triangles();
    let submeshes = skin.submeshes();

    // 4. Find texture files
    let mut texture_paths: Vec<PathBuf> = Vec::new();
    for tex in &model.textures {
        let raw_path = tex.filename.string.to_string_lossy();
        // Extract basename from paths like "SPELLS\FRANKCUBE.BLP"
        let basename = raw_path.rsplit(&['\\', '/'][..]).next().unwrap_or(&raw_path);
        // Replace .blp/.BLP extension with .png
        let png_name = if let Some(stripped) = basename.strip_suffix(".blp").or_else(|| basename.strip_suffix(".BLP")) {
            format!("{}.png", stripped)
        } else {
            basename.to_string()
        };
        let tex_path = dir.join(&png_name);
        if !tex_path.exists() {
            // Try case-insensitive search
            let lower = png_name.to_lowercase();
            let mut found = None;
            for entry in std::fs::read_dir(dir)? {
                let path = entry?.path();
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.to_lowercase() == lower {
                        found = Some(path);
                        break;
                    }
                }
            }
            match found {
                Some(p) => texture_paths.push(p),
                None => return Err(format!("texture file not found: '{}'", tex_path.display()).into()),
            }
        } else {
            texture_paths.push(tex_path);
        }
    }

    // 5. Resolve output path
    let output_path = resolve_output_path(dir, &cli.output);

    // Extract vertex data
    let vertex_count = model.vertices.len();
    let mut positions: Vec<f32> = Vec::with_capacity(vertex_count * 3);
    let mut normals: Vec<f32> = Vec::with_capacity(vertex_count * 3);
    let mut uvs: Vec<f32> = Vec::with_capacity(vertex_count * 2);
    let mut uvs2: Vec<f32> = Vec::with_capacity(vertex_count * 2);
    let mut bone_indices: Vec<u8> = Vec::with_capacity(vertex_count * 4);
    let mut bone_weights: Vec<u8> = Vec::with_capacity(vertex_count * 4);

    for vertex in &model.vertices {
        // Convert M2 coordinate system (Z-up) to glTF (Y-up): (X, Y, Z) → (X, Z, -Y)
        positions.push(vertex.position.x);
        positions.push(vertex.position.z);
        positions.push(-vertex.position.y);

        normals.push(vertex.normal.x);
        normals.push(vertex.normal.z);
        normals.push(-vertex.normal.y);

        // UVs: pass raw M2 values through
        uvs.push(vertex.tex_coords.x);
        uvs.push(vertex.tex_coords.y);

        // Secondary UVs
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

    // Load first texture
    let texture_data = std::fs::read(&texture_paths[0])?;
    let image = Image::new(texture_data, MimeType::Png);
    let texture = Texture::new(image);
    let tex_basename = texture_paths[0].file_stem().unwrap().to_string_lossy();
    let material = Material::builder()
        .name(&*tex_basename)
        .metallic_factor(0.0)
        .base_color_texture(TextureInfo::new(texture))
        .build();

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

    // Build mesh
    let mesh = Mesh::builder()
        .primitive(primitive)
        .build();

    // Build scene graph
    let model_name = model.name.as_deref().unwrap_or(&m2_stem);
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

    let document = Document::builder()
        .default_scene(scene)
        .build();

    // Write GLB
    let mut file = std::fs::File::create(&output_path)?;
    document.to_writer(&mut file)?;
    println!("Wrote {}", output_path.display());

    Ok(())
}

fn resolve_output_path(dir: &PathBuf, output: &Option<String>) -> PathBuf {
    match output {
        None => {
            let dir_name = dir.file_name().unwrap().to_string_lossy();
            dir.join(format!("{}.glb", dir_name))
        }
        Some(out) => {
            let path = PathBuf::from(out);
            let path = if path.extension().is_none() {
                path.with_extension("glb")
            } else {
                path
            };
            // If it's just a filename (no directory components), place inside the input directory
            if path.parent() == Some(std::path::Path::new("")) || path.parent().is_none() {
                dir.join(path)
            } else {
                // Has directory components or is absolute — use as-is
                path
            }
        }
    }
}
