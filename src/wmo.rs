use std::io::Cursor;
use std::path::Path;

use wow_wmo::{ParsedWmo, parse_wmo};
use wow_wmo::group_parser::WmoGroup;

use gltf::document::{Document, Node, Scene};
use gltf::material::{Image, Material, MimeType, Texture, TextureInfo};
use gltf::mesh::{Mesh, Mode, Primitive};

use crate::{mpq, texture};
use crate::mpq::ArchivePool;

pub fn export_wmo(
    pool: &ArchivePool,
    wmo_archive_path: &str,
    output_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    // 1. Parse root WMO
    let mut archives = pool.acquire();
    let data = mpq::read_file(&mut archives, wmo_archive_path)?;
    pool.release(archives);

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

    // 2. Collect all work items
    let wmo_base = wmo_archive_path
        .strip_suffix(".wmo")
        .or_else(|| wmo_archive_path.strip_suffix(".WMO"))
        .unwrap_or(wmo_archive_path);

    // Unique texture BLP paths referenced by materials
    let mut tex_paths: Vec<String> = Vec::new();
    let mut tex_index_map: Vec<Option<usize>> = Vec::new(); // material idx -> png_textures idx
    for momt in &root.materials {
        if let Some(&tex_idx) = root.texture_offset_index_map.get(&momt.texture_1) {
            let tex_idx = tex_idx as usize;
            if tex_idx < root.textures.len() {
                let blp_path = &root.textures[tex_idx];
                // Deduplicate: find existing or add new
                let pos = tex_paths.iter().position(|p| p == blp_path);
                let idx = match pos {
                    Some(i) => i,
                    None => {
                        tex_paths.push(blp_path.clone());
                        tex_paths.len() - 1
                    }
                };
                tex_index_map.push(Some(idx));
            } else {
                tex_index_map.push(None);
            }
        } else {
            tex_index_map.push(None);
        }
    }

    let group_paths: Vec<String> = (0..root.n_groups)
        .map(|i| format!("{}_{:03}.wmo", wmo_base, i))
        .collect();

    // 3. Read and process all textures + groups in parallel using the archive pool
    let (png_textures, parsed_groups) = std::thread::scope(|s| {
        // Spawn texture threads
        let tex_handles: Vec<_> = tex_paths
            .iter()
            .map(|blp_path| {
                s.spawn(move || {
                    let mut archives = pool.acquire();
                    let blp_data = mpq::read_file(&mut archives, blp_path)
                        .map_err(|e| format!("{}: {}", blp_path, e));
                    pool.release(archives);
                    let blp_data = blp_data?;
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

        // Spawn group threads
        let group_handles: Vec<_> = group_paths
            .iter()
            .enumerate()
            .map(|(idx, group_path)| {
                s.spawn(move || {
                    let mut archives = pool.acquire();
                    let group_data = mpq::read_file(&mut archives, group_path)
                        .map_err(|e| format!("{}: {}", group_path, e));
                    pool.release(archives);
                    let group_data = group_data?;
                    let group = match parse_wmo(&mut Cursor::new(&group_data)) {
                        Ok(ParsedWmo::Group(g)) => g,
                        Ok(ParsedWmo::Root(_)) => {
                            return Err(format!("{}: parsed as root, expected group", group_path));
                        }
                        Err(e) => {
                            return Err(format!("{}: {}", group_path, e));
                        }
                    };
                    Ok::<_, String>((idx, group))
                })
            })
            .collect();

        // Collect texture results
        let mut textures = Vec::with_capacity(tex_handles.len());
        for h in tex_handles {
            textures.push(h.join().expect("texture thread panicked"));
        }

        // Collect group results
        let mut groups = Vec::with_capacity(group_handles.len());
        for h in group_handles {
            groups.push(h.join().expect("group thread panicked"));
        }

        (textures, groups)
    });

    // 4. Build glTF materials from converted textures
    let png_results: Vec<Option<(String, Vec<u8>)>> = png_textures
        .into_iter()
        .map(|r| match r {
            Ok(v) => Some(v),
            Err(e) => {
                eprintln!("Warning: {}", e);
                None
            }
        })
        .collect();

    let mut gltf_materials: Vec<Material> = Vec::new();
    for (i, tex_idx) in tex_index_map.iter().enumerate() {
        let material = match tex_idx.and_then(|idx| png_results[idx].as_ref()) {
            Some((name, png_data)) => {
                let image = Image::new(png_data.clone(), MimeType::Png);
                let tex = Texture::new(image);
                Material::builder()
                    .name(name)
                    .metallic_factor(0.0)
                    .base_color_texture(TextureInfo::new(tex))
                    .build()
            }
            None => Material::builder()
                .name(&format!("material_{}", i))
                .metallic_factor(0.0)
                .build(),
        };
        gltf_materials.push(material);
    }

    // 5. Build mesh nodes from parsed groups
    let mut group_nodes: Vec<Node> = Vec::new();

    for result in parsed_groups {
        let (group_idx, group) = match result {
            Ok(v) => v,
            Err(e) => {
                eprintln!("Warning: {}", e);
                continue;
            }
        };

        let primitives = build_group_primitives(&group, &gltf_materials);
        if primitives.is_empty() {
            continue;
        }

        let mut mesh_builder = Mesh::builder();
        for prim in primitives {
            mesh_builder = mesh_builder.primitive(prim);
        }

        let node = Node::builder()
            .name(&format!("group_{:03}", group_idx))
            .mesh(mesh_builder.build())
            .build();

        group_nodes.push(node);
    }

    if group_nodes.is_empty() {
        return Err("No renderable groups found in WMO".into());
    }

    // 6. Assemble document
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

    let scene = Scene::builder()
        .name(&format!("{}_Scene", wmo_stem))
        .node(root_builder.build())
        .build();

    let document = Document::builder().default_scene(scene).build();

    let mut file = std::fs::File::create(output_path)?;
    document.to_writer(&mut file)?;
    println!("Wrote {}", output_path.display());

    Ok(())
}

fn build_group_primitives(group: &WmoGroup, materials: &[Material]) -> Vec<Primitive> {
    if group.render_batches.is_empty() {
        return Vec::new();
    }

    let mut primitives = Vec::new();

    for batch in &group.render_batches {
        let min = batch.min_index as usize;
        let max = batch.max_index as usize;
        let vert_count = max - min + 1;

        let mut positions: Vec<f32> = Vec::with_capacity(vert_count * 3);
        let mut normals: Vec<f32> = Vec::with_capacity(vert_count * 3);
        let mut uvs: Vec<f32> = Vec::with_capacity(vert_count * 2);

        for v_idx in min..=max {
            let pos = &group.vertex_positions[v_idx];
            positions.push(pos.x);
            positions.push(pos.z);
            positions.push(-pos.y);

            let nor = &group.vertex_normals[v_idx];
            normals.push(nor.x);
            normals.push(nor.z);
            normals.push(-nor.y);

            let tc = &group.texture_coords[v_idx];
            uvs.push(tc.u);
            uvs.push(1.0 - tc.v);
        }

        let idx_start = batch.start_index as usize;
        let idx_count = batch.count as usize;
        let mut indices: Vec<u16> = Vec::with_capacity(idx_count);
        for i in idx_start..(idx_start + idx_count) {
            let original = group.vertex_indices[i];
            indices.push(original - batch.min_index);
        }

        let material = if (batch.material_id as usize) < materials.len() {
            materials[batch.material_id as usize].clone()
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

    primitives
}
