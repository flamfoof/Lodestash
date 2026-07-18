//! FBX mesh import for the 3D viewport — `ufbx` (single-file C library, builds with
//! `cc` alone). Spec: INGESTION.md §5 (3D models), TECH-STACK §7.
//!
//! Output is renderer-agnostic plain data (`MeshData`) so this crate stays free of GPU
//! deps; the UI converts it to its renderer's types. glTF/GLB/OBJ don't come through
//! here — three-d-asset loads those directly in the UI.
//!
//! ponytail: static world-space pose only (no skinning/animation), triangle soup with
//! sequential indices (per-corner normals/uvs make dedup pointless for viewing), and
//! materials reduced to a base color — textures ride in with the normalize→glTF cache.

use std::path::Path;

/// One flattened, triangulated mesh in world space.
pub struct MeshData {
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    /// Same length as `positions` when present.
    pub normals: Option<Vec<[f32; 3]>>,
    pub uvs: Option<Vec<[f32; 2]>>,
    pub indices: Vec<u32>,
    /// Material base color as RGBA (defaults to a neutral grey).
    pub base_color: [u8; 4],
}

/// Extensions routed through ufbx (three-d-asset handles glTF/GLB/OBJ natively).
pub fn is_fbx_model(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("fbx"))
}

/// Load and flatten every triangle mesh in `path` (FBX; ufbx also parses OBJ, which the
/// tests use as a handwritable fixture).
pub fn load_meshes(path: &Path) -> Result<Vec<MeshData>, String> {
    let opts = ufbx::LoadOpts {
        generate_missing_normals: true,
        ..Default::default()
    };
    let scene = ufbx::load_file(path.to_string_lossy().as_ref(), opts)
        .map_err(|e| e.description.to_string())?;

    let mut out = Vec::new();
    for node in &scene.nodes {
        let Some(mesh) = &node.mesh else { continue };
        if mesh.faces.is_empty() {
            continue;
        }
        let to_world = node.geometry_to_world;
        let normal_mat = ufbx::matrix_for_normals(&to_world);
        let has_normals = mesh.vertex_normal.exists;
        let has_uvs = mesh.vertex_uv.exists;

        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut uvs = Vec::new();
        let mut tri = vec![0u32; mesh.max_face_triangles * 3];
        for &face in &mesh.faces {
            let n_tris = ufbx::triangulate_face(&mut tri, mesh, face);
            for &ix in &tri[..(n_tris as usize) * 3] {
                let p = ufbx::transform_position(&to_world, mesh.vertex_position[ix as usize]);
                positions.push([p.x as f32, p.y as f32, p.z as f32]);
                if has_normals {
                    let nv =
                        ufbx::transform_direction(&normal_mat, mesh.vertex_normal[ix as usize]);
                    normals.push([nv.x as f32, nv.y as f32, nv.z as f32]);
                }
                if has_uvs {
                    let uv = mesh.vertex_uv[ix as usize];
                    uvs.push([uv.x as f32, uv.y as f32]);
                }
            }
        }
        if positions.is_empty() {
            continue;
        }
        let indices = (0..positions.len() as u32).collect();
        out.push(MeshData {
            name: node.element.name.to_string(),
            positions,
            normals: has_normals.then_some(normals),
            uvs: has_uvs.then_some(uvs),
            indices,
            base_color: node_base_color(node),
        });
    }
    if out.is_empty() {
        return Err("no triangle meshes in file".into());
    }
    Ok(out)
}

/// Base color of the node's first material, if declared.
fn node_base_color(node: &ufbx::Node) -> [u8; 4] {
    const DEFAULT: [u8; 4] = [0xB4, 0xB4, 0xB8, 0xFF];
    let Some(mat) = node.materials.first() else {
        return DEFAULT;
    };
    // Prefer the normalized PBR view; fall back to the raw FBX diffuse.
    let map = if mat.pbr.base_color.has_value {
        &mat.pbr.base_color
    } else if mat.fbx.diffuse_color.has_value {
        &mat.fbx.diffuse_color
    } else {
        return DEFAULT;
    };
    let v = map.value_vec4;
    let c = |x: f64| (x.clamp(0.0, 1.0) * 255.0) as u8;
    [c(v.x), c(v.y), c(v.z), 0xFF]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fbx_extension_routing() {
        assert!(is_fbx_model(Path::new("Model.FBX")));
        assert!(is_fbx_model(Path::new("a/b/prop.fbx")));
        assert!(!is_fbx_model(Path::new("mesh.glb"))); // three-d-asset's job
        assert!(!is_fbx_model(Path::new("mesh.obj")));
        assert!(!is_fbx_model(Path::new("noext")));
    }

    #[test]
    fn load_meshes_flattens_a_minimal_obj_via_ufbx() {
        // ufbx parses OBJ too — a handwritable stand-in for a binary FBX fixture,
        // exercising the same triangulate/transform path.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tri.obj");
        std::fs::write(&path, "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n").unwrap();
        let meshes = load_meshes(&path).expect("ufbx parses the OBJ");
        assert_eq!(meshes.len(), 1);
        assert_eq!(meshes[0].positions.len(), 3, "triangle soup: 3 corners");
        assert_eq!(meshes[0].indices, vec![0, 1, 2]);
        // generate_missing_normals gives us per-corner normals even without them in-file.
        assert!(meshes[0].normals.is_some());
    }
}

#[cfg(test)]
mod probe {
    use super::*;

    /// Diagnostic probe: `LODESTASH_PROBE_FBX=<path> cargo test -p lodestash-ingest probe -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn probe_real_fbx() {
        let path = std::env::var("LODESTASH_PROBE_FBX").expect("set LODESTASH_PROBE_FBX");
        match load_meshes(Path::new(&path)) {
            Ok(meshes) => {
                for m in &meshes {
                    println!(
                        "mesh '{}': {} verts, {} indices, normals={} uvs={} color={:?}",
                        m.name,
                        m.positions.len(),
                        m.indices.len(),
                        m.normals.is_some(),
                        m.uvs.is_some(),
                        m.base_color
                    );
                    if let Some(p) = m.positions.first() {
                        println!("  first pos: {p:?}");
                    }
                }
            }
            Err(e) => println!("LOAD FAILED: {e}"),
        }
    }
}
