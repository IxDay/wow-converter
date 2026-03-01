use std::io::Cursor;
use std::path::Path;

use wow_m2::M2Model;
use wow_m2::SkinFile;

use gltf::document::{Document, Node, Scene};
use gltf::material::{Image, Material, MimeType, Texture, TextureInfo};
use gltf::mesh::{Mesh, Mode, Primitive};

use crate::texture;
use crate::mpq::ArchivePool;

pub fn export_m2(
    pool: &ArchivePool,
    m2_archive_path: &str,
    output_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    // 1. Read and parse the M2
    let m2_data = pool.read_file(m2_archive_path)?;
    let model = M2Model::parse(&mut Cursor::new(&m2_data))?;

    // 2. Construct skin path and read it
    let skin_base = m2_archive_path
        .strip_suffix(".m2")
        .or_else(|| m2_archive_path.strip_suffix(".M2"))
        .unwrap_or(m2_archive_path);
    let skin_archive_path = format!("{}00.skin", skin_base);
    let skin_data = pool.read_file(&skin_archive_path)?;
    let skin = SkinFile::parse(&mut Cursor::new(&skin_data))?;
    let skin_indices = skin.indices();
    let skin_triangles = skin.triangles();
    let submeshes = skin.submeshes();

    // 3. Collect texture paths
    let tex_paths: Vec<String> = model
        .textures
        .iter()
        .filter_map(|tex| {
            let raw = tex.filename.string.to_string_lossy();
            if raw.is_empty() { None } else { Some(raw.to_string()) }
        })
        .collect();

    // 4. Read and convert all textures in parallel using the archive pool
    let png_textures: Vec<(String, Vec<u8>)> = std::thread::scope(|s| {
        let handles: Vec<_> = tex_paths
            .iter()
            .map(|blp_path| {
                s.spawn(move || {
                    let blp_data = pool.read_file(blp_path)
                        .map_err(|e| format!("{}: {}", blp_path, e))?;
                    let png_buf = texture::blp_to_png(&blp_data)
                        .map_err(|e| format!("{}: {}", blp_path, e))?;
                    let basename = blp_path.rsplit(&['\\', '/'][..]).next().unwrap_or(blp_path);
                    let name = basename
                        .strip_suffix(".blp")
                        .or_else(|| basename.strip_suffix(".BLP"))
                        .unwrap_or(basename)
                        .to_string();
                    println!("Converted texture: {} ({} bytes PNG)", basename, png_buf.len());
                    Ok::<_, String>((name, png_buf))
                })
            })
            .collect();
        let mut results = Vec::with_capacity(handles.len());
        for h in handles {
            results.push(h.join().expect("texture thread panicked")?);
        }
        Ok::<_, String>(results)
    })
    .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;

    // 5. Build glTF

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
        Material::builder()
            .name("default")
            .metallic_factor(0.0)
            .build()
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
        .unwrap_or(m2_archive_path)
        .strip_suffix(".m2")
        .or_else(|| {
            m2_archive_path
                .rsplit(&['\\', '/'][..])
                .next()
                .unwrap_or(m2_archive_path)
                .strip_suffix(".M2")
        })
        .unwrap_or(
            m2_archive_path
                .rsplit(&['\\', '/'][..])
                .next()
                .unwrap_or(m2_archive_path),
        );

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

    // 6. Write GLB
    let mut file = std::fs::File::create(output_path)?;
    document.to_writer(&mut file)?;
    println!("Wrote {}", output_path.display());

    Ok(())
}
