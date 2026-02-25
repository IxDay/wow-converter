use std::io::Cursor;
use std::path::Path;

use wow_mpq::Archive;
use wow_wmo::{ParsedWmo, parse_wmo};

use gltf::document::{Document, Node, Scene};
use gltf::material::{Image, Material, MimeType, Texture, TextureInfo};
use gltf::mesh::{Mesh, Mode, Primitive};

use crate::{mpq, texture};

pub fn export_wmo(
    archives: &mut [Archive],
    wmo_archive_path: &str,
    output_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    // 1. Parse root WMO
    let data = mpq::read_file(archives, wmo_archive_path)?;
    let root = match parse_wmo(&mut Cursor::new(&data))? {
        ParsedWmo::Root(r) => r,
        ParsedWmo::Group(_) => {
            return Err("Expected WMO root file, got group file".into());
        }
    };

    println!(
        "WMO root: {} groups, {} materials, {} textures",
        root.n_groups,
        root.materials.len(),
        root.textures.len()
    );

    // 2. Build glTF materials (one per root material)
    let mut gltf_materials: Vec<Material> = Vec::new();
    for (i, momt) in root.materials.iter().enumerate() {
        let material = if let Some(&tex_idx) = root.texture_offset_index_map.get(&momt.texture_1) {
            let tex_idx = tex_idx as usize;
            if tex_idx < root.textures.len() {
                let blp_path = &root.textures[tex_idx];
                match texture::load_texture(archives, blp_path) {
                    Ok((name, png_data)) => {
                        let image = Image::new(png_data, MimeType::Png);
                        let tex = Texture::new(image);
                        Material::builder()
                            .name(&name)
                            .metallic_factor(0.0)
                            .base_color_texture(TextureInfo::new(tex))
                            .build()
                    }
                    Err(e) => {
                        eprintln!("Warning: failed to load texture '{}': {}", blp_path, e);
                        Material::builder()
                            .name(&format!("material_{}", i))
                            .metallic_factor(0.0)
                            .build()
                    }
                }
            } else {
                Material::builder()
                    .name(&format!("material_{}", i))
                    .metallic_factor(0.0)
                    .build()
            }
        } else {
            Material::builder()
                .name(&format!("material_{}", i))
                .metallic_factor(0.0)
                .build()
        };
        gltf_materials.push(material);
    }

    // 3. Construct base path for group files
    // e.g. "World\wmo\building.wmo" -> "World\wmo\building"
    let wmo_base = wmo_archive_path
        .strip_suffix(".wmo")
        .or_else(|| wmo_archive_path.strip_suffix(".WMO"))
        .unwrap_or(wmo_archive_path);

    // 4. Load each group and build meshes
    let mut group_nodes: Vec<Node> = Vec::new();

    for group_idx in 0..root.n_groups {
        let group_path = format!("{}_{:03}.wmo", wmo_base, group_idx);
        let group_data = match mpq::read_file(archives, &group_path) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("Warning: could not read group {}: {}", group_path, e);
                continue;
            }
        };

        let group = match parse_wmo(&mut Cursor::new(&group_data))? {
            ParsedWmo::Group(g) => g,
            ParsedWmo::Root(_) => {
                eprintln!("Warning: {} parsed as root, expected group", group_path);
                continue;
            }
        };

        if group.render_batches.is_empty() {
            continue;
        }

        let mut primitives: Vec<Primitive> = Vec::new();

        for batch in &group.render_batches {
            let min = batch.min_index as usize;
            let max = batch.max_index as usize;
            let vert_count = max - min + 1;

            // Extract vertex slice for this batch
            let mut positions: Vec<f32> = Vec::with_capacity(vert_count * 3);
            let mut normals: Vec<f32> = Vec::with_capacity(vert_count * 3);
            let mut uvs: Vec<f32> = Vec::with_capacity(vert_count * 2);

            for v_idx in min..=max {
                let pos = &group.vertex_positions[v_idx];
                // Coordinate conversion: (X, Y, Z) -> (X, Z, -Y)
                positions.push(pos.x);
                positions.push(pos.z);
                positions.push(-pos.y);

                let nor = &group.vertex_normals[v_idx];
                normals.push(nor.x);
                normals.push(nor.z);
                normals.push(-nor.y);

                let tc = &group.texture_coords[v_idx];
                uvs.push(tc.u);
                uvs.push(1.0 - tc.v); // WMO needs V flip
            }

            // Extract and rebase indices
            let idx_start = batch.start_index as usize;
            let idx_count = batch.count as usize;
            let mut indices: Vec<u16> = Vec::with_capacity(idx_count);
            for i in idx_start..(idx_start + idx_count) {
                let original = group.vertex_indices[i];
                indices.push(original - batch.min_index);
            }

            // Pick material
            let material = if (batch.material_id as usize) < gltf_materials.len() {
                gltf_materials[batch.material_id as usize].clone()
            } else {
                Material::builder()
                    .name("default")
                    .metallic_factor(0.0)
                    .build()
            };

            let prim = Primitive::builder()
                .mode(Mode::Triangles)
                .position(positions)
                .normal(normals)
                .tex_coord(uvs)
                .indices(indices)
                .material(material)
                .build();

            primitives.push(prim);
        }

        if primitives.is_empty() {
            continue;
        }

        let mut mesh_builder = Mesh::builder();
        for prim in primitives {
            mesh_builder = mesh_builder.primitive(prim);
        }
        let mesh = mesh_builder.build();

        let node = Node::builder()
            .name(&format!("group_{:03}", group_idx))
            .mesh(mesh)
            .build();

        group_nodes.push(node);
    }

    if group_nodes.is_empty() {
        return Err("No renderable groups found in WMO".into());
    }

    // 5. Assemble document
    let wmo_stem = wmo_archive_path
        .rsplit(&['\\', '/'][..])
        .next()
        .unwrap_or(wmo_archive_path)
        .strip_suffix(".wmo")
        .or_else(|| {
            wmo_archive_path
                .rsplit(&['\\', '/'][..])
                .next()
                .unwrap_or(wmo_archive_path)
                .strip_suffix(".WMO")
        })
        .unwrap_or(
            wmo_archive_path
                .rsplit(&['\\', '/'][..])
                .next()
                .unwrap_or(wmo_archive_path),
        );

    let mut root_builder = Node::builder().name(wmo_stem);
    for node in group_nodes {
        root_builder = root_builder.child(node);
    }
    let root_node = root_builder.build();

    let scene = Scene::builder()
        .name(&format!("{}_Scene", wmo_stem))
        .node(root_node)
        .build();

    let document = Document::builder().default_scene(scene).build();

    let mut file = std::fs::File::create(output_path)?;
    document.to_writer(&mut file)?;
    println!("Wrote {}", output_path.display());

    Ok(())
}
