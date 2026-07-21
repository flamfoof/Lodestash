//! FBX mesh import for the 3D viewport — `ufbx` (single-file C library, builds with
//! `cc` alone). Spec: INGESTION.md §5 (3D models), TECH-STACK §7.
//!
//! Output is renderer-agnostic plain data (`MeshData`) so this crate stays free of GPU
//! deps; the UI converts it to its renderer's types. glTF/GLB/OBJ don't come through
//! here — three-d-asset loads those directly in the UI.
//!
//! ponytail: static world-space pose only (no skinning/animation), triangle soup with
//! sequential indices (per-corner normals/uvs make dedup pointless for viewing), and
//! materials reduced to a base color + optional albedo texture.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Decoded albedo texture: RGBA8, rows **bottom-up** (flipped at decode so the raw FBX
/// UVs — bottom-left origin, like GL — sample it correctly without touching the UVs).
pub struct AlbedoTex {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<[u8; 4]>,
}

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
    /// Albedo texture, shared between meshes using the same material.
    pub albedo: Option<Arc<AlbedoTex>>,
}

/// Extensions routed through ufbx (three-d-asset handles glTF/GLB/OBJ natively).
pub fn is_fbx_model(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("fbx"))
}

/// Load and flatten every triangle mesh in `path` (FBX; ufbx also parses OBJ, which the
/// tests use as a handwritable fixture).
///
/// `tex_files` maps **lowercase texture basenames** to on-disk files — the caller's escape
/// hatch for extracted packages, where payloads land in content-addressed workspace dirs
/// and the FBX's relative texture paths no longer resolve. Loose files on real folders
/// resolve through the FBX's own absolute/relative paths first.
pub fn load_meshes(
    path: &Path,
    tex_files: &BTreeMap<String, PathBuf>,
) -> Result<Vec<MeshData>, String> {
    let opts = ufbx::LoadOpts {
        generate_missing_normals: true,
        // Pull in sidecar files (.mtl for OBJ, .fbm texture folders) so material texture
        // references materialize; a missing texture must never fail the mesh load.
        load_external_files: true,
        ignore_missing_external_files: true,
        ..Default::default()
    };
    let scene = ufbx::load_file(path.to_string_lossy().as_ref(), opts)
        .map_err(|e| e.description.to_string())?;

    let fbx_dir = path.parent().unwrap_or(Path::new("."));
    // Same texture file referenced by many materials decodes once per load.
    let mut tex_cache: BTreeMap<String, Option<Arc<AlbedoTex>>> = BTreeMap::new();

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
            albedo: node_albedo(node, fbx_dir, tex_files, &mut tex_cache),
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

/// Viewport textures larger than this are downscaled at decode (8k atlases are common in
/// Unity packs; full res buys nothing in a preview and quadruples upload time).
const TEX_MAX_DIM: u32 = 4096;

/// Resolve + decode the albedo texture of the node's first material, if any.
fn node_albedo(
    node: &ufbx::Node,
    fbx_dir: &Path,
    tex_files: &BTreeMap<String, PathBuf>,
    cache: &mut BTreeMap<String, Option<Arc<AlbedoTex>>>,
) -> Option<Arc<AlbedoTex>> {
    let mat = node.materials.first()?;
    // Prefer the normalized PBR view; fall back to the raw FBX diffuse (same order as
    // `node_base_color`).
    let tex = mat
        .pbr
        .base_color
        .texture
        .as_ref()
        .or(mat.fbx.diffuse_color.texture.as_ref())?;

    let key = if tex.filename.is_empty() {
        format!("embedded:{}", tex.element.name)
    } else {
        tex.filename.to_string()
    };
    if let Some(hit) = cache.get(&key) {
        return hit.clone();
    }
    let decoded = decode_texture(tex, fbx_dir, tex_files)
        .map_err(|e| tracing::debug!(texture = %key, %e, "albedo texture skipped"))
        .ok()
        .map(Arc::new);
    cache.insert(key, decoded.clone());
    decoded
}

/// Decode a ufbx texture reference: embedded blob first, then the file paths the FBX
/// declares, then the caller's basename lookup (extracted-package workspace).
fn decode_texture(
    tex: &ufbx::Texture,
    fbx_dir: &Path,
    tex_files: &BTreeMap<String, PathBuf>,
) -> Result<AlbedoTex, String> {
    let img = if tex.content.size > 0 {
        image::load_from_memory(&tex.content).map_err(|e| e.to_string())?
    } else {
        let path = resolve_texture_path(tex, fbx_dir, tex_files)
            .ok_or_else(|| "texture file not found".to_string())?;
        crate::raster::decode(&path).map_err(|e| e.to_string())?
    };
    let img = if img.width().max(img.height()) > TEX_MAX_DIM {
        img.resize(
            TEX_MAX_DIM,
            TEX_MAX_DIM,
            image::imageops::FilterType::Triangle,
        )
    } else {
        img
    };
    // Flip to bottom-up rows: GL samples row 0 at v=0, and FBX UVs are bottom-left origin.
    let rgba = img.flipv().to_rgba8();
    let (width, height) = rgba.dimensions();
    Ok(AlbedoTex {
        width,
        height,
        rgba: rgba.pixels().map(|p| p.0).collect(),
    })
}

/// First existing candidate: the FBX's absolute path, its relative path against the FBX's
/// folder, then the basename in the caller-supplied lookup.
fn resolve_texture_path(
    tex: &ufbx::Texture,
    fbx_dir: &Path,
    tex_files: &BTreeMap<String, PathBuf>,
) -> Option<PathBuf> {
    let candidates = [
        tex.absolute_filename.as_ref(),
        tex.filename.as_ref(),
        tex.relative_filename.as_ref(),
    ];
    for cand in candidates {
        if cand.is_empty() {
            continue;
        }
        let norm = cand.replace('\\', "/");
        let p = PathBuf::from(&norm);
        if p.is_file() {
            return Some(p);
        }
        let rel = fbx_dir.join(&norm);
        if rel.is_file() {
            return Some(rel);
        }
    }
    // Basename fallback — content-addressed extraction breaks every declared path.
    candidates.iter().find_map(|cand| {
        let base = cand.replace('\\', "/");
        let base = base.rsplit('/').next()?.to_ascii_lowercase();
        tex_files.get(&base).cloned()
    })
}

/// glTF/GLB, loaded through `three-d-asset` (data-only; the same loader the viewer uses) and
/// flattened to the identical world-space `MeshData` the ufbx path produces — so the multiview
/// renderer and thumbnailer don't care which format an asset came in.
pub fn is_gltf_model(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("gltf") || e.eq_ignore_ascii_case("glb"))
}

pub fn load_gltf(path: &Path) -> Result<Vec<MeshData>, String> {
    let mut raw = three_d_asset::io::load(&[path]).map_err(|e| e.to_string())?;
    let model: three_d_asset::Model = raw.deserialize(path).map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for prim in &model.geometries {
        let three_d_asset::geometry::Geometry::Triangles(tri) = &prim.geometry else {
            continue; // point clouds carry no silhouette
        };
        // Bake the node transform in, so every mesh is world-space like the ufbx path.
        let m = prim.transformation;
        let positions: Vec<[f32; 3]> = tri
            .positions
            .to_f32()
            .iter()
            .map(|p| {
                let v = m * three_d_asset::prelude::Vec4::new(p.x, p.y, p.z, 1.0);
                [v.x, v.y, v.z]
            })
            .collect();
        // No indices ⇒ every three consecutive positions form a triangle.
        let indices = tri
            .indices
            .to_u32()
            .unwrap_or_else(|| (0..positions.len() as u32).collect());
        let base_color = prim
            .material_index
            .and_then(|i| model.materials.get(i))
            .map(|mat| {
                let c = mat.albedo;
                [c.r, c.g, c.b, c.a]
            })
            .unwrap_or([180, 180, 185, 255]);
        out.push(MeshData {
            name: prim.name.clone(),
            positions,
            // The renderer derives face normals, so skipping them costs nothing here.
            normals: None,
            uvs: tri.uvs.as_ref().map(|us| us.iter().map(|u| [u.x, u.y]).collect()),
            indices,
            base_color,
            albedo: None,
        });
    }
    if out.is_empty() {
        return Err("no triangle geometry in model".into());
    }
    Ok(out)
}

/// Load any supported 3D format into world-space `MeshData`: FBX/OBJ via ufbx, glTF/GLB via
/// three-d-asset. The single entry point for the embed drain and thumbnailer.
pub fn load_any(path: &Path, tex_files: &BTreeMap<String, PathBuf>) -> Result<Vec<MeshData>, String> {
    if is_gltf_model(path) {
        load_gltf(path)
    } else {
        // ufbx covers FBX and OBJ (and is tolerant of a few other odds and ends).
        load_meshes(path, tex_files)
    }
}

#[cfg(test)]
mod gltf_tests {
    use super::*;

    /// Minimal base64 (no padding shortcuts) so the fixture can embed its buffer as a data URI
    /// and stay a single self-contained file — no external .bin to resolve.
    fn b64(data: &[u8]) -> String {
        const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for c in data.chunks(3) {
            let (b0, b1, b2) = (c[0] as u32, *c.get(1).unwrap_or(&0) as u32, *c.get(2).unwrap_or(&0) as u32);
            let n = (b0 << 16) | (b1 << 8) | b2;
            out.push(T[(n >> 18) as usize & 63] as char);
            out.push(T[(n >> 12) as usize & 63] as char);
            out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
            out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
        }
        out
    }

    /// A non-indexed single-triangle glTF — exercises the real `three-d-asset` path including
    /// the "no indices ⇒ sequential" branch.
    #[test]
    fn load_gltf_reads_a_minimal_model() {
        let verts: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let mut buf = Vec::new();
        for v in verts {
            for f in v {
                buf.extend_from_slice(&f.to_le_bytes());
            }
        }
        let json = format!(
            r#"{{"asset":{{"version":"2.0"}},"scene":0,"scenes":[{{"nodes":[0]}}],
"nodes":[{{"mesh":0}}],"meshes":[{{"primitives":[{{"attributes":{{"POSITION":0}}}}]}}],
"buffers":[{{"byteLength":{len},"uri":"data:application/octet-stream;base64,{b}"}}],
"bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":{len}}}],
"accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3",
"min":[0.0,0.0,0.0],"max":[1.0,1.0,0.0]}}]}}"#,
            len = buf.len(),
            b = b64(&buf)
        );
        let dir = std::env::temp_dir().join(format!("lodestash_gltf_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tri.gltf");
        std::fs::write(&path, json).unwrap();

        assert!(is_gltf_model(&path));
        let meshes = load_any(&path, &BTreeMap::new()).expect("load gltf");
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(meshes.len(), 1, "one primitive");
        assert_eq!(meshes[0].positions.len(), 3);
        assert_eq!(meshes[0].indices, vec![0, 1, 2], "sequential indices generated");
        // And it renders (the whole point of loading it).
        let views = crate::render3d::render_views(&meshes, 64, &crate::render3d::DEFAULT_VIEWS[..1]);
        assert_eq!(views.len(), 1);
    }
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
        let meshes = load_meshes(&path, &BTreeMap::new()).expect("ufbx parses the OBJ");
        assert_eq!(meshes.len(), 1);
        assert_eq!(meshes[0].positions.len(), 3, "triangle soup: 3 corners");
        assert_eq!(meshes[0].indices, vec![0, 1, 2]);
        // generate_missing_normals gives us per-corner normals even without them in-file.
        assert!(meshes[0].normals.is_some());
        assert!(meshes[0].albedo.is_none(), "no material, no texture");
    }

    /// OBJ+MTL with a `map_Kd` texture exercises the full resolve→decode→flip path
    /// (same ufbx material machinery the FBX route uses).
    #[test]
    fn load_meshes_resolves_and_flips_an_albedo_texture() {
        let dir = tempfile::tempdir().unwrap();
        // 1x2 PNG: top row red, bottom row blue.
        let mut img = image::RgbaImage::new(1, 2);
        img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        img.put_pixel(0, 1, image::Rgba([0, 0, 255, 255]));
        img.save(dir.path().join("tex.png")).unwrap();
        std::fs::write(dir.path().join("tri.mtl"), "newmtl m\nmap_Kd tex.png\n").unwrap();
        std::fs::write(
            dir.path().join("tri.obj"),
            "mtllib tri.mtl\nusemtl m\nv 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0 0\nvt 1 0\nvt 0 1\nf 1/1 2/2 3/3\n",
        )
        .unwrap();

        let meshes =
            load_meshes(&dir.path().join("tri.obj"), &BTreeMap::new()).expect("OBJ parses");
        let albedo = meshes[0].albedo.as_ref().expect("map_Kd resolves");
        assert_eq!((albedo.width, albedo.height), (1, 2));
        // Rows are flipped bottom-up: row 0 must be the image's bottom (blue) row.
        assert_eq!(albedo.rgba[0], [0, 0, 255, 255]);
        assert_eq!(albedo.rgba[1], [255, 0, 0, 255]);
    }

    /// The basename lookup rescues textures whose declared paths don't exist —
    /// the extracted-package (content-addressed workspace) case.
    #[test]
    fn texture_basename_lookup_rescues_broken_declared_paths() {
        let dir = tempfile::tempdir().unwrap();
        let stash = dir.path().join("stash");
        std::fs::create_dir(&stash).unwrap();
        let mut img = image::RgbaImage::new(2, 2);
        img.put_pixel(0, 0, image::Rgba([1, 2, 3, 255]));
        img.save(stash.join("Rock_Albedo.png")).unwrap();
        std::fs::write(
            dir.path().join("tri.mtl"),
            // Declared path doesn't exist relative to the OBJ — only the lookup can win.
            "newmtl m\nmap_Kd Textures/Rock_Albedo.png\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("tri.obj"),
            "mtllib tri.mtl\nusemtl m\nv 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0 0\nvt 1 0\nvt 0 1\nf 1/1 2/2 3/3\n",
        )
        .unwrap();

        let lookup: BTreeMap<String, PathBuf> =
            [("rock_albedo.png".to_string(), stash.join("Rock_Albedo.png"))].into();
        let meshes = load_meshes(&dir.path().join("tri.obj"), &lookup).expect("OBJ parses");
        assert!(meshes[0].albedo.is_some(), "basename fallback resolves");
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
        match load_meshes(Path::new(&path), &BTreeMap::new()) {
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
