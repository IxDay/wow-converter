use std::io::Write;
use wow_m2::{M2Model, M2Version};
use gltf::{
    document::{Document, Node, Scene},
    mesh::{Mesh, PrimitiveBuilder},
    material::{MaterialBuilder},
    image::{Image, ImageMimeType},
    json::{to_writer},
};

use crate::gltf_writer::GLTFWriter;

/// Convert M2 model to GLTF/GLB format
pub async fn convert_m2_to_gltf(
    m2_data: &[u8],
    output_path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // Parse M2 model
    let mut cursor = std::io::Cursor::new(m2_data);
    let model = M2Model::parse(&mut cursor)?;

    println!("M2 version: {:?}", model.header.version());
    println!("Vertices: {}", model.vertices.len());
    println!("Bones: {}", model.bones.len());
    println!("Materials: {}", model.materials.len());
    println!("Textures: {}", model.textures.len());

    // Create GLTF writer
    let mut gltf_writer = GLTFWriter::new(output_path, "error_cube");

    // Convert vertices to GLTF format
    let vertex_count = model.vertices.len();
    let mut positions = Vec::with_capacity(vertex_count * 3);
    let mut normals = Vec::with_capacity(vertex_count * 3);
    let mut uvs = Vec::with_capacity(vertex_count * 2);
    let mut bone_weights = Vec::with_capacity(vertex_count * 4);
    let mut bone_indices = Vec::with_capacity(vertex_count * 4);

    for (i, vertex) in model.vertices.iter().enumerate() {
        // Position: Convert WoW coordinate system (Y-up) to GLTF (Y-up)
        // M2 uses Y-up, GLTF expects Y-up, so no conversion needed for positions
        positions.push(vertex.position.x);
        positions.push(vertex.position.y);
        positions.push(vertex.position.z);

        // Normal: Convert coordinate system
        normals.push(vertex.normal.x);
        normals.push(vertex.normal.y);
        normals.push(vertex.normal.z);

        // UV: Convert from WoW format (V flipped) to GLTF format
        uvs.push(vertex.tex_coords.x);
        uvs.push(1.0 - vertex.tex_coords.y); // Flip Y coordinate

        // Bone weights and indices
        for j in 0..4 {
            bone_weights.push(vertex.bone_weights[j]);
            bone_indices.push(vertex.bone_indices[j]);
        }
    }

    // Create vertex buffer
    let positions_accessor = gltf_writer.add_positions_array(&positions);
    let normals_accessor = gltf_writer.add_normals_array(&normals);
    let uvs_accessor = gltf_writer.add_uvs_array(&uvs);
    let bone_weights_accessor = gltf_writer.add_bone_weights_array(&bone_weights);
    let bone_indices_accessor = gltf_writer.add_bone_indices_array(&bone_indices);

    // Create primitive
    let mut primitive = PrimitiveBuilder::new();
    primitive
        .positions(positions_accessor)
        .normals(normals_accessor)
        .uvs(0, uvs_accessor)
        .indices(0, None) // We'll create indices later
        .material(0); // Use first material

    // Create mesh with primitive
    gltf_writer.add_mesh("error_cube_mesh", vec![primitive]);

    // Create node and scene
    let node = gltf_writer.add_node("error_cube", Some(0), None, None);
    let scene = gltf_writer.add_scene("error_cube_scene", vec![node]);

    // Handle materials and textures
    if let Some(material) = model.materials.first() {
        let mut mat_builder = MaterialBuilder::new();
        mat_builder.name(format!("error_cube_mat_{}", material.flags));
        let gltf_material = gltf_writer.add_material(mat_builder.material());

        // If material has texture, embed it as PNG
        if material.texture_type > 0 {
            // For now, create a simple colored texture
            let texture_data = create_simple_texture_data(255, 0, 0, 255); // Red texture
            let image = Image::new_embedded(texture_data, ImageMimeType::PNG);
            let texture_index = gltf_writer.add_texture(image);
            
            gltf_writer.set_texture_for_material(gltf_material, Some(texture_index));
        }

    // Write GLTF file
    gltf_writer.write(true, "gltf")?;

    Ok(())
}

/// Create simple texture data for testing
fn create_simple_texture_data(r: u8, g: u8, b: u8, a: u8) -> Vec<u8> {
    let mut data = Vec::with_capacity(4 * 4); // 4x4 texture, RGBA
    for _ in 0..16 {
        data.extend_from_slice(&[r, g, b, a, r, g, b, a]);
    }
    data
}