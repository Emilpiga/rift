use anyhow::{Context, Result};
use glam::{Vec2, Vec3};
use std::path::Path;

use crate::renderer::mesh::{Mesh, Vertex};

/// A loaded glTF scene containing one or more meshes.
pub struct GltfScene {
    pub meshes: Vec<Mesh>,
    pub names: Vec<String>,
}

/// Load all meshes from a glTF/GLB file.
pub fn load_gltf(path: &Path) -> Result<GltfScene> {
    let (document, buffers, _images) =
        gltf::import(path).with_context(|| format!("Failed to load glTF: {:?}", path))?;

    let mut meshes = Vec::new();
    let mut names = Vec::new();

    for mesh in document.meshes() {
        let name = mesh
            .name()
            .unwrap_or(&format!("mesh_{}", mesh.index()))
            .to_string();

        for primitive in mesh.primitives() {
            let reader = primitive.reader(|buffer| Some(&buffers[buffer.index()]));

            // Positions (required)
            let positions: Vec<Vec3> = reader
                .read_positions()
                .with_context(|| format!("Mesh '{}' has no positions", name))?
                .map(Vec3::from)
                .collect();

            let vertex_count = positions.len();

            // Normals (optional, default to Y-up)
            let normals: Vec<Vec3> = reader
                .read_normals()
                .map(|iter| iter.map(Vec3::from).collect())
                .unwrap_or_else(|| vec![Vec3::Y; vertex_count]);

            // Tex coords (optional, default to 0,0)
            let uvs: Vec<Vec2> = reader
                .read_tex_coords(0)
                .map(|iter| iter.into_f32().map(Vec2::from).collect())
                .unwrap_or_else(|| vec![Vec2::ZERO; vertex_count]);

            // Vertex colors (optional, default to white)
            let colors: Vec<Vec3> = reader
                .read_colors(0)
                .map(|iter| {
                    iter.into_rgba_f32()
                        .map(|c| Vec3::new(c[0], c[1], c[2]))
                        .collect()
                })
                .unwrap_or_else(|| vec![Vec3::ONE; vertex_count]);

            // Build vertices
            let vertices: Vec<Vertex> = (0..vertex_count)
                .map(|i| Vertex {
                    position: positions[i],
                    normal: normals[i],
                    color: colors[i],
                    uv: uvs[i],
                })
                .collect();

            // Indices (optional — generate sequential if missing)
            let indices: Vec<u32> = reader
                .read_indices()
                .map(|iter| iter.into_u32().collect())
                .unwrap_or_else(|| (0..vertex_count as u32).collect());

            meshes.push(Mesh { vertices, indices });
            names.push(name.clone());
        }
    }

    log::info!(
        "Loaded glTF {:?}: {} mesh(es)",
        path.file_name().unwrap_or_default(),
        meshes.len()
    );

    Ok(GltfScene { meshes, names })
}
