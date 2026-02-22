use std::io::Cursor;

use wow_m2::M2Model;
use wow_m2::SkinFile;

use gltf::document::{Document, Node, Scene};
use gltf::mesh::{Mesh, Mode, Primitive};
use gltf::material::{Image, Material, MimeType, Texture, TextureInfo};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let base_dir = "../exports/error-cube";
    let m2_path = format!("{}/error_cube.m2", base_dir);
    let skin_path = format!("{}/error_cube00.skin", base_dir);
    let texture_path = format!("{}/frankcube.png", base_dir);
    let output_path = format!("{}/output.glb", base_dir);

    // Parse M2 model
    let m2_data = std::fs::read(&m2_path)?;
    let model = M2Model::parse(&mut Cursor::new(&m2_data))?;

    println!("M2 version: {:?}", model.header.version);
    println!("Vertices: {}", model.vertices.len());
    println!("Bones: {}", model.bones.len());
    println!("Materials: {}", model.materials.len());
    println!("Textures: {}", model.textures.len());

    // Parse skin file
    let skin = SkinFile::load(&skin_path)?;
    let skin_indices = skin.indices();
    let skin_triangles = skin.triangles();
    let submeshes = skin.submeshes();

    println!("Skin indices: {}", skin_indices.len());
    println!("Skin triangles: {}", skin_triangles.len());
    println!("Skin submeshes: {}", submeshes.len());

    // Debug: print first few raw vertex positions
    for (i, v) in model.vertices.iter().take(4).enumerate() {
        println!("Raw V{}: pos=({:.6}, {:.6}, {:.6}) normal=({:.4}, {:.4}, {:.4}) uv=({:.4}, {:.4})",
            i, v.position.x, v.position.y, v.position.z,
            v.normal.x, v.normal.y, v.normal.z,
            v.tex_coords.x, v.tex_coords.y);
    }
    if let Some(bone) = model.bones.first() {
        println!("Bone 0 pivot: ({:.6}, {:.6}, {:.6})", bone.pivot.x, bone.pivot.y, bone.pivot.z);
    }

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

        // UVs: pass raw M2 values through (wow.export double-flips which cancels out)
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
    // wow.export uses two-level indirection: skin.indices[skin.triangles[start + i]]
    // We combine all submeshes into a single index list for the error cube
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

    println!("Total triangle indices: {}", all_indices.len());

    // Load texture
    let texture_data = std::fs::read(&texture_path)?;
    let image = Image::new(texture_data, MimeType::Png);
    let texture = Texture::new(image);
    let material = Material::builder()
        .name("frankcube")
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
    let mesh_node = Node::builder()
        .name("errorcube_Geoset0")
        .mesh(mesh)
        .build();

    let root_node = Node::builder()
        .name("errorcube")
        .child(mesh_node)
        .build();

    let scene = Scene::builder()
        .name("errorcube_Scene")
        .node(root_node)
        .build();

    let document = Document::builder()
        .default_scene(scene)
        .build();

    // Write GLB
    let mut file = std::fs::File::create(&output_path)?;
    document.to_writer(&mut file)?;

    println!("Written GLB to {}", output_path);

    Ok(())
}
