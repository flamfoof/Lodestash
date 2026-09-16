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
#[derive(Clone)]
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
    load_meshes_with(path, tex_files, true)
}

fn load_meshes_with(
    path: &Path,
    tex_files: &BTreeMap<String, PathBuf>,
    external: bool,
) -> Result<Vec<MeshData>, String> {
    let mut opts = load_opts();
    opts.load_external_files = external;
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
        let Some((positions, normals, uvs)) = corners(node, mesh, mesh.faces.iter().copied())
        else {
            continue;
        };
        let indices = (0..positions.len() as u32).collect();
        let first = node.materials.first().map(|m| &**m);
        out.push(MeshData {
            name: node.element.name.to_string(),
            positions,
            normals,
            uvs,
            indices,
            base_color: material_base_color(first),
            albedo: if external {
                material_albedo(first, fbx_dir, tex_files, &mut tex_cache)
            } else {
                None
            },
        });
    }
    if out.is_empty() {
        return Err("no triangle meshes in file".into());
    }
    Ok(out)
}

fn load_opts() -> ufbx::LoadOpts<'static> {
    ufbx::LoadOpts {
        generate_missing_normals: true,
        // Pull in sidecar files (.mtl for OBJ, .fbm texture folders) so material texture
        // references materialize; a missing texture must never fail the mesh load.
        load_external_files: true,
        ignore_missing_external_files: true,
        ..Default::default()
    }
}

/// Positions, and normals/UVs when the mesh has them, of every triangle corner of `faces` in world
/// space — triangle soup. `None` when the faces hold no triangle.
#[allow(clippy::type_complexity)]
fn corners(
    node: &ufbx::Node,
    mesh: &ufbx::Mesh,
    faces: impl Iterator<Item = ufbx::Face>,
) -> Option<(Vec<[f32; 3]>, Option<Vec<[f32; 3]>>, Option<Vec<[f32; 2]>>)> {
    let to_world = node.geometry_to_world;
    let normal_mat = ufbx::matrix_for_normals(&to_world);
    let has_normals = mesh.vertex_normal.exists;
    let has_uvs = mesh.vertex_uv.exists;

    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    let mut tri = vec![0u32; mesh.max_face_triangles * 3];
    for face in faces {
        let n_tris = ufbx::triangulate_face(&mut tri, mesh, face);
        for &ix in &tri[..(n_tris as usize) * 3] {
            let p = ufbx::transform_position(&to_world, mesh.vertex_position[ix as usize]);
            positions.push([p.x as f32, p.y as f32, p.z as f32]);
            if has_normals {
                let nv = ufbx::transform_direction(&normal_mat, mesh.vertex_normal[ix as usize]);
                normals.push([nv.x as f32, nv.y as f32, nv.z as f32]);
            }
            if has_uvs {
                let uv = mesh.vertex_uv[ix as usize];
                uvs.push([uv.x as f32, uv.y as f32]);
            }
        }
    }
    if positions.is_empty() {
        return None;
    }
    Some((
        positions,
        has_normals.then_some(normals),
        has_uvs.then_some(uvs),
    ))
}

/// Load `path` for a **textured preview**: every material slot as its own mesh, tagged with its
/// slot and painted with that slot's own base colour and declared or embedded albedo (v1 plan S07).
///
/// FBX (and OBJ) split each mesh by material part, so a two-material mesh is two meshes rather than
/// one painted with its first material. glTF/GLB carry each primitive's base-colour texture —
/// embedded in a buffer view or data URI, or an external image — with its UVs turned to the
/// bottom-left origin the other loaders emit. [`load_any`] is untouched: the embedding render, the
/// metrics and the caches keep reading geometry with the first material only, so no vector and no
/// count moves.
pub fn load_textured(
    path: &Path,
    tex_files: &BTreeMap<String, PathBuf>,
) -> Result<Vec<(u32, MeshData)>, String> {
    if is_gltf_model(path) {
        return load_gltf_with(path, true);
    }
    let scene = ufbx::load_file(path.to_string_lossy().as_ref(), load_opts())
        .map_err(|e| e.description.to_string())?;
    let fbx_dir = path.parent().unwrap_or(Path::new("."));
    let mut tex_cache: BTreeMap<String, Option<Arc<AlbedoTex>>> = BTreeMap::new();

    let mut out = Vec::new();
    for node in &scene.nodes {
        let Some(mesh) = &node.mesh else { continue };
        if mesh.faces.is_empty() {
            continue;
        }
        // A mesh with no material is still one part: slot 0.
        let parts: Vec<(u32, Vec<ufbx::Face>)> = if mesh.material_parts.is_empty() {
            vec![(0, mesh.faces.iter().copied().collect())]
        } else {
            mesh.material_parts
                .iter()
                .map(|part| {
                    let faces = part
                        .face_indices
                        .iter()
                        .map(|&i| mesh.faces[i as usize])
                        .collect();
                    (part.index, faces)
                })
                .collect()
        };
        for (slot, faces) in parts {
            let Some((positions, normals, uvs)) = corners(node, mesh, faces.into_iter()) else {
                continue;
            };
            let material = node.materials.get(slot as usize).map(|m| &**m);
            let indices = (0..positions.len() as u32).collect();
            out.push((
                slot,
                MeshData {
                    name: node.element.name.to_string(),
                    positions,
                    normals,
                    uvs,
                    indices,
                    base_color: material_base_color(material),
                    albedo: material_albedo(material, fbx_dir, tex_files, &mut tex_cache),
                },
            ));
        }
    }
    if out.is_empty() {
        return Err("no triangle meshes in file".into());
    }
    Ok(out)
}

/// [`load_textured`] with the catalog's per-slot albedo behind it: the model file's own declared or
/// embedded texture wins, and a slot the file does not paint takes the texture the dependency graph
/// resolved for it (`Catalog::model_material_inputs`, v1 plan S07 → S08).
///
/// One resolver for both preview paths — the cold thumbnail renders these meshes, and the warm
/// viewer is served the same slot's texture — so the two cannot disagree about what a model wears.
pub fn load_textured_with(
    path: &Path,
    tex_files: &BTreeMap<String, PathBuf>,
    slot_textures: &BTreeMap<u32, PathBuf>,
) -> Result<Vec<(u32, MeshData)>, String> {
    let mut meshes = load_textured(path, tex_files)?;
    // One decode per slot, however many meshes wear it.
    let mut decoded: BTreeMap<u32, Option<Arc<AlbedoTex>>> = BTreeMap::new();
    for (slot, mesh) in &mut meshes {
        if mesh.albedo.is_some() {
            continue;
        }
        let hit = decoded.entry(*slot).or_insert_with(|| {
            slot_textures
                .get(slot)
                .and_then(|p| albedo_from_file(p))
                .map(Arc::new)
        });
        mesh.albedo = hit.clone();
    }
    Ok(meshes)
}

/// Base color of a material, if it declares one.
fn material_base_color(mat: Option<&ufbx::Material>) -> [u8; 4] {
    const DEFAULT: [u8; 4] = [0xB4, 0xB4, 0xB8, 0xFF];
    let Some(mat) = mat else {
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

/// Resolve + decode a material's albedo texture, if it has one.
fn material_albedo(
    mat: Option<&ufbx::Material>,
    fbx_dir: &Path,
    tex_files: &BTreeMap<String, PathBuf>,
    cache: &mut BTreeMap<String, Option<Arc<AlbedoTex>>>,
) -> Option<Arc<AlbedoTex>> {
    let mat = mat?;
    // Prefer the normalized PBR view; fall back to the raw FBX diffuse (same order as
    // `material_base_color`).
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
    Ok(albedo_from_image(img))
}

/// The tail every albedo decode shares: downscale past [`TEX_MAX_DIM`], then flip to bottom-up rows
/// — GL samples row 0 at `v = 0`, and the loaders emit bottom-left UVs.
fn albedo_from_image(img: image::DynamicImage) -> AlbedoTex {
    let img = if img.width().max(img.height()) > TEX_MAX_DIM {
        img.resize(
            TEX_MAX_DIM,
            TEX_MAX_DIM,
            image::imageops::FilterType::Triangle,
        )
    } else {
        img
    };
    let rgba = img.flipv().to_rgba8();
    let (width, height) = rgba.dimensions();
    AlbedoTex {
        width,
        height,
        rgba: rgba.pixels().map(|p| p.0).collect(),
    }
}

/// Decode one texture **file** as an albedo — the catalog's answer for a slot the model itself does
/// not paint (v1 plan S08).
pub fn albedo_from_file(path: &Path) -> Option<AlbedoTex> {
    crate::raster::decode(path).ok().map(albedo_from_image)
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
    Ok(load_gltf_with(path, false)?
        .into_iter()
        .map(|(_, m)| m)
        .collect())
}

/// [`load_gltf`], each primitive tagged with its material index. `textured` also carries each
/// primitive's base-colour texture and turns its UVs to the bottom-left origin ([`AlbedoTex`] rows
/// are bottom-up); without it the UVs stay as the file wrote them and no texture is read.
fn load_gltf_with(path: &Path, textured: bool) -> Result<Vec<(u32, MeshData)>, String> {
    let mut raw = three_d_asset::io::load(&[path]).map_err(|e| e.to_string())?;
    let model: three_d_asset::Model = raw.deserialize(path).map_err(|e| e.to_string())?;
    flatten_gltf(model, textured)
}

fn flatten_gltf(
    model: three_d_asset::Model,
    textured: bool,
) -> Result<Vec<(u32, MeshData)>, String> {
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
        let material = prim.material_index.and_then(|i| model.materials.get(i));
        let base_color = material
            .map(|mat| {
                let c = mat.albedo;
                [c.r, c.g, c.b, c.a]
            })
            .unwrap_or([180, 180, 185, 255]);
        let albedo = if textured {
            material
                .and_then(|mat| mat.albedo_texture.as_ref())
                .and_then(gltf_albedo)
                .map(Arc::new)
        } else {
            None
        };
        out.push((
            prim.material_index.unwrap_or(0) as u32,
            MeshData {
                name: prim.name.clone(),
                positions,
                // The renderer derives face normals, so skipping them costs nothing here.
                normals: None,
                uvs: tri.uvs.as_ref().map(|us| {
                    us.iter()
                        .map(|u| {
                            if textured {
                                [u.x, 1.0 - u.y]
                            } else {
                                [u.x, u.y]
                            }
                        })
                        .collect()
                }),
                indices,
                base_color,
                albedo,
            },
        ));
    }
    if out.is_empty() {
        return Err("no triangle geometry in model".into());
    }
    Ok(out)
}

/// A decoded glTF texture as an [`AlbedoTex`]: 8-bit channels only (an HDR base colour is not a
/// preview's concern), downscaled like the FBX path, rows flipped bottom-up.
fn gltf_albedo(t: &three_d_asset::Texture2D) -> Option<AlbedoTex> {
    use three_d_asset::TextureData as D;
    let rgba: Vec<[u8; 4]> = match &t.data {
        D::RgbaU8(p) => p.clone(),
        D::RgbU8(p) => p.iter().map(|&[r, g, b]| [r, g, b, 255]).collect(),
        D::RgU8(p) => p.iter().map(|&[l, a]| [l, l, l, a]).collect(),
        D::RU8(p) => p.iter().map(|&l| [l, l, l, 255]).collect(),
        _ => return None,
    };
    let img = image::RgbaImage::from_raw(t.width, t.height, rgba.into_iter().flatten().collect())?;
    Some(albedo_from_image(image::DynamicImage::ImageRgba8(img)))
}

/// Load any supported 3D format into world-space `MeshData`: FBX/OBJ via ufbx, glTF/GLB via
/// three-d-asset. The single entry point for the embed drain and thumbnailer.
pub fn load_any(
    path: &Path,
    tex_files: &BTreeMap<String, PathBuf>,
) -> Result<Vec<MeshData>, String> {
    if is_gltf_model(path) {
        load_gltf(path)
    } else {
        // ufbx covers FBX and OBJ (and is tolerant of a few other odds and ends).
        load_meshes(path, tex_files)
    }
}

/// Agent evidence reads only this payload. External glTF buffers are deliberately unavailable;
/// FBX/OBJ sidecars and texture paths cannot trigger additional disk reads.
/// Keep this separate from the interactive loader, whose user-requested texture resolution is broader.
pub fn load_evidence(path: &Path) -> Result<Vec<MeshData>, String> {
    let meshes = if is_gltf_model(path) {
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .map_err(|e| e.to_string())?
            .take(256 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 256 * 1024 * 1024 {
            return Err("model exceeds 256 MiB".into());
        }
        let document = gltf::Gltf::from_slice(&bytes).map_err(|e| e.to_string())?;
        let uris: Vec<&str> = document
            .buffers()
            .filter_map(|b| match b.source() {
                gltf::buffer::Source::Uri(uri) => Some(uri),
                _ => None,
            })
            .chain(document.images().filter_map(|i| match i.source() {
                gltf::image::Source::Uri { uri, .. } => Some(uri),
                _ => None,
            }))
            .collect();
        if uris.len() > 32 || uris.iter().any(|uri| !uri.starts_with("data:")) {
            return Err(
                "evidence requires a self-contained glTF/GLB with at most 32 embedded inputs"
                    .into(),
            );
        }
        let mut raw = three_d_asset::io::RawAssets::new();
        for uri in uris {
            let url = data_url::DataUrl::process(uri)
                .map_err(|e| format!("invalid embedded data: {e:?}"))?;
            let (data, _) = url
                .decode_to_vec()
                .map_err(|e| format!("invalid embedded data: {e:?}"))?;
            raw.insert(uri, data);
        }
        raw.insert(path, bytes);
        let model = raw.deserialize(path).map_err(|e| {
            format!("selected payload cannot be decoded without external dependencies: {e}")
        })?;
        flatten_gltf(model, false)?
            .into_iter()
            .map(|(_, m)| m)
            .collect()
    } else {
        if std::fs::metadata(path).map_err(|e| e.to_string())?.len() > 256 * 1024 * 1024 {
            return Err("model exceeds 256 MiB".into());
        }
        load_meshes_with(path, &BTreeMap::new(), false)?
    };
    Ok(crate::decimate::decimate_meshes(
        &meshes,
        crate::decimate::PREVIEW_BUDGET,
    ))
}

/// Load `path` through a **normalized-geometry cache** (TECH-STACK §7, parse-once): parse it once
/// (ufbx/three-d-asset), write the flattened world-space geometry to `cache_dir/mesh/<key>.msh`, and
/// on every later call read that back instead of re-parsing. Parsing is the expensive part of a 3D
/// thumbnail/metric/embedding, and metrics (every scan), thumbnails (on browse) and embeddings each
/// parse the **same** file — so the metrics drain warms the cache and browse-time thumbnails hit it,
/// which is what removes the browse spike. **Textures are not cached, and not returned** —
/// thumbnails and embeddings render untextured base-colour, and a cache miss must hand back what a
/// cache hit would, or the first render of a model differs from every later one. The textured
/// preview paths (`load_textured`, `thumb::generate_model_textured`) keep their own uncached route.
///
/// The cache is disposable: any read error or format mismatch silently falls back to a fresh parse,
/// so a stale/corrupt entry can never wedge a load. Keyed by `(path, size, mtime)` so an edited file
/// re-parses.
pub fn load_cached(path: &Path, cache_dir: &Path) -> Result<Vec<MeshData>, String> {
    let cache_path = cache_dir
        .join("mesh")
        .join(format!("{}.msh", cache_key(path)));
    if let Ok(bytes) = std::fs::read(&cache_path) {
        if let Some(meshes) = decode_meshes(&bytes) {
            return Ok(meshes);
        }
    }
    let mut meshes = load_any(path, &BTreeMap::new())?;
    write_cache(&cache_path, &encode_meshes(&meshes));
    // Hand back exactly what the cache would decode next time. A fresh parse resolves the file's
    // own declared textures, the cache does not carry them, and the embedding render samples an
    // albedo when one is attached — so before this the very first encode of a model saw a textured
    // render and every later one saw flat colour, for the same bytes (v1 plan S08).
    for m in &mut meshes {
        m.albedo = None;
    }
    Ok(meshes)
}

/// Best-effort cache write: temp file then rename, so a concurrent reader never sees a half-written
/// file. Every failure is silent — the cache is disposable and the caller already has the data.
fn write_cache(dest: &Path, bytes: &[u8]) {
    let Some(dir) = dest.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    // Append rather than replace the extension: `.msh` and `.lod` share a directory and a stem, so
    // a replaced extension would give both the same temp path and let concurrent writes collide.
    let Some(name) = dest.file_name() else { return };
    let tmp = dest.with_file_name(format!("{}.part", name.to_string_lossy()));
    if std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, dest);
    }
}

/// Is this submesh a **collision hull** rather than something to look at?
///
/// Measured before building (1,307 cached meshes, 1,486 submeshes off the reference library):
/// **17 files, 1.3%**, carry one — and every single match was a `_Collision` hull, with **zero**
/// LOD levels anywhere in the library. So this deliberately does *not* try to spot `_LOD1`/`_LOD2`
/// groups: there were none to spot, and a loose match is a live hazard — a substring test for
/// "LOD" hits the asset literally named `lodestone_gem`.
///
/// 1.3% is small, but those cards are *wrong*, not merely coarse: a hull renders as a box drawn
/// over the prop it wraps.
pub(crate) fn is_collision_hull(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("ucx_")
        || n.starts_with("collision")
        || n.ends_with("_collision")
        || n.ends_with("_collider")
}

/// Load `path` as a **low-poly preview proxy** — the geometry every 3D *preview* renders from
/// (grid/list thumbnails, the multiview embedding), as opposed to [`load_cached`]'s full-fidelity
/// mesh. Decimates once (`decimate::decimate_meshes` under `budget`), caches the result as
/// `cache_dir/mesh/<key>.lod` on the same `(path, size, mtime)` key, and serves that on every later
/// call. Wins: smaller cache on disk, cheaper raster, lower VRAM.
///
/// **The `triangles` facet must not come from here** — `metrics::triangle_count` deliberately stays
/// on [`load_cached`] so the count the user sees is the *source* mesh's, not the proxy's.
pub fn load_preview_cached(
    path: &Path,
    cache_dir: &Path,
    budget: crate::decimate::Budget,
) -> Result<Vec<MeshData>, String> {
    let key = cache_key(path);
    // The budget is part of the identity: same file, coarser budget, different proxy.
    let proxy_path = cache_dir
        .join("mesh")
        .join(format!("{key}-{}.lod", budget.tag()));
    if let Ok(bytes) = std::fs::read(&proxy_path) {
        if let Some(meshes) = decode_meshes(&bytes) {
            return Ok(meshes);
        }
    }
    let mut source = load_cached(path, cache_dir)?;
    // Drop collision hulls before anything measures or renders them: they are not the prop,
    // they wrap it, and left in they both draw a box over the card and take a share of the
    // triangle budget away from the geometry the user is looking at. Never drop *everything* —
    // a model that is only a hull still has to draw something.
    if source.iter().any(|m| !is_collision_hull(&m.name)) {
        source.retain(|m| !is_collision_hull(&m.name));
    }
    let proxy = crate::decimate::decimate_meshes(&source, budget);

    // A budget most models never reach is not a proxy, it is a second copy.
    //
    // `decimate` only returns `Some` when it strictly reduced the index count, so equal totals
    // mean *every* mesh came back a clone. Writing that would spend a file and a re-encode to
    // store the staging mesh a second time — and then delete the original, so the next triangle
    // count has to re-parse the source. Measured on the reference library: median model is 300
    // triangles against Low's 1500 ceiling, so this is the common case, not the corner — 91% of
    // models at Low and **all** of them at High.
    let tris = |ms: &[MeshData]| ms.iter().map(|m| m.indices.len()).sum::<usize>();
    if tris(&proxy) == tris(&source) {
        return Ok(source);
    }

    write_cache(&proxy_path, &encode_meshes(&proxy));
    // ponytail: the full mesh is a parse-once staging artifact — once a proxy exists, no preview
    // consumer needs it again, so drop it and let the cache shrink. Ceiling: a re-scan (or a
    // triangle-count refresh) re-parses the source. Keep it instead if that ever shows up as a
    // scan-time cost.
    let _ = std::fs::remove_file(cache_dir.join("mesh").join(format!("{key}.msh")));
    Ok(proxy)
}

/// Identity of a file for the mesh caches: `(path, size, mtime)`. Public because the vector-preview
/// drain keys its stored SVG on the same value, so a preview and its geometry go stale together.
pub fn cache_key(path: &Path) -> String {
    let (size, mtime) = std::fs::metadata(path)
        .map(|m| {
            let mtime = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            (m.len(), mtime)
        })
        .unwrap_or((0, 0));
    let mut h = blake3::Hasher::new();
    h.update(path.to_string_lossy().as_bytes());
    h.update(&size.to_le_bytes());
    h.update(&mtime.to_le_bytes());
    h.finalize().to_hex()[..32].to_string()
}

const MESH_CACHE_MAGIC: &[u8; 4] = b"LSM1";

/// Magic of the **slot trailer** — `"SLT1" u32 n n*u32`, appended after the LSM1 meshes when the
/// wire carries one material slot per mesh (v1 plan S08).
const SLOT_TRAILER_MAGIC: &[u8; 4] = b"SLT1";

/// Which material slot each mesh belongs to, as a trailer after [`encode_meshes`].
///
/// A trailer rather than a field inside each mesh: `decode_meshes` and the frontend parser both stop
/// at the last mesh, so every existing cache file and reader keeps working, and the geometry caches
/// stay byte-identical to what they hold today.
pub fn encode_slots(slots: &[u32]) -> Vec<u8> {
    let mut b = Vec::with_capacity(8 + slots.len() * 4);
    b.extend_from_slice(SLOT_TRAILER_MAGIC);
    b.extend_from_slice(&(slots.len() as u32).to_le_bytes());
    for slot in slots {
        b.extend_from_slice(&slot.to_le_bytes());
    }
    b
}

/// One decoded albedo as a PNG, rows top-down again — the form a browser decodes and uploads with
/// `UNPACK_FLIP_Y`, so the warm viewer samples exactly what the cold rasteriser did (v1 plan S08).
pub fn albedo_png(tex: &AlbedoTex) -> Option<Vec<u8>> {
    let raw: Vec<u8> = tex.rgba.iter().flatten().copied().collect();
    let img = image::RgbaImage::from_raw(tex.width, tex.height, raw)?;
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .flipv()
        .write_to(&mut out, image::ImageFormat::Png)
        .ok()?;
    Some(out.into_inner())
}

/// Serialize the flattened geometry (no albedo — see [`load_cached`]). Flat `f32`/`u32` arrays, so
/// read-back is a bounded parse rather than a full mesh re-parse.
///
/// Public because this is also the **wire format** for `ls://mesh`: the desktop viewer parses these
/// exact bytes in TypeScript. Disk cache and wire share one encoder deliberately — two encoders
/// would drift, and the drift would only show up as a garbled mesh in the viewer.
pub fn encode_meshes(meshes: &[MeshData]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(MESH_CACHE_MAGIC);
    b.extend_from_slice(&(meshes.len() as u32).to_le_bytes());
    let put_vec3 = |b: &mut Vec<u8>, v: &[[f32; 3]]| {
        b.extend_from_slice(&(v.len() as u32).to_le_bytes());
        for p in v {
            for f in p {
                b.extend_from_slice(&f.to_le_bytes());
            }
        }
    };
    for m in meshes {
        b.extend_from_slice(&(m.name.len() as u32).to_le_bytes());
        b.extend_from_slice(m.name.as_bytes());
        put_vec3(&mut b, &m.positions);
        match &m.normals {
            Some(n) => {
                b.push(1);
                put_vec3(&mut b, n);
            }
            None => b.push(0),
        }
        match &m.uvs {
            Some(u) => {
                b.push(1);
                b.extend_from_slice(&(u.len() as u32).to_le_bytes());
                for uv in u {
                    for f in uv {
                        b.extend_from_slice(&f.to_le_bytes());
                    }
                }
            }
            None => b.push(0),
        }
        b.extend_from_slice(&(m.indices.len() as u32).to_le_bytes());
        for i in &m.indices {
            b.extend_from_slice(&i.to_le_bytes());
        }
        b.extend_from_slice(&m.base_color);
    }
    b
}

/// Bounds-checked reader: returns `None` on any short/corrupt read, so the caller re-parses.
struct Cur<'a> {
    b: &'a [u8],
    i: usize,
}
impl<'a> Cur<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.i..self.i + n)?;
        self.i += n;
        Some(s)
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn vec3(&mut self) -> Option<Vec<[f32; 3]>> {
        let n = self.u32()? as usize;
        // Guard a corrupt length from a huge speculative allocation.
        let mut v = Vec::with_capacity(n.min(1 << 20));
        for _ in 0..n {
            v.push([self.f32()?, self.f32()?, self.f32()?]);
        }
        Some(v)
    }
}

fn decode_meshes(bytes: &[u8]) -> Option<Vec<MeshData>> {
    let mut c = Cur { b: bytes, i: 0 };
    if c.take(4)? != MESH_CACHE_MAGIC {
        return None;
    }
    let mesh_count = c.u32()? as usize;
    let mut out = Vec::with_capacity(mesh_count.min(1 << 16));
    for _ in 0..mesh_count {
        let name_len = c.u32()? as usize;
        let name = std::str::from_utf8(c.take(name_len)?).ok()?.to_string();
        let positions = c.vec3()?;
        let normals = if c.take(1)?[0] == 1 {
            Some(c.vec3()?)
        } else {
            None
        };
        let uvs = if c.take(1)?[0] == 1 {
            let n = c.u32()? as usize;
            let mut v = Vec::with_capacity(n.min(1 << 20));
            for _ in 0..n {
                v.push([c.f32()?, c.f32()?]);
            }
            Some(v)
        } else {
            None
        };
        let idx_count = c.u32()? as usize;
        let mut indices = Vec::with_capacity(idx_count.min(1 << 22));
        for _ in 0..idx_count {
            indices.push(c.u32()?);
        }
        let base_color: [u8; 4] = c.take(4)?.try_into().ok()?;
        out.push(MeshData {
            name,
            positions,
            normals,
            uvs,
            indices,
            base_color,
            albedo: None,
        });
    }
    Some(out)
}

#[cfg(test)]
mod mesh_cache_tests {
    use super::*;

    fn mesh(name: &str, tris: usize) -> MeshData {
        MeshData {
            name: name.into(),
            positions: vec![[1.0, 2.0, 3.0]; tris],
            normals: Some(vec![[0.0, 1.0, 0.0]; tris]),
            uvs: Some(vec![[0.25, 0.75]; tris]),
            indices: (0..tris as u32 * 3).collect(),
            base_color: [10, 20, 30, 40],
            albedo: None,
        }
    }

    #[test]
    fn encode_decode_round_trips_geometry() {
        let m = mesh("cube", 2);
        let bytes = encode_meshes(std::slice::from_ref(&m));
        let back = decode_meshes(&bytes).expect("decode");
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].name, "cube");
        assert_eq!(back[0].positions, m.positions);
        assert_eq!(back[0].normals, m.normals);
        assert_eq!(back[0].uvs, m.uvs);
        assert_eq!(back[0].indices, m.indices);
        assert_eq!(back[0].base_color, m.base_color);
        assert!(back[0].albedo.is_none());
        // A truncated / bad buffer decodes to None (caller re-parses) rather than panicking.
        assert!(decode_meshes(&bytes[..bytes.len() - 3]).is_none());
        assert!(decode_meshes(b"nope").is_none());
    }

    #[test]
    fn load_cached_serves_the_cache_over_reparsing() {
        let dir = tempfile::tempdir().unwrap();
        let obj = dir.path().join("tri.obj");
        std::fs::write(&obj, "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n").unwrap();
        let cache = dir.path().join("cache");

        let first = load_cached(&obj, &cache).expect("parse + cache");
        assert!(!first.is_empty(), "OBJ parsed to geometry");
        let key = cache_key(&obj);
        let cache_file = cache.join("mesh").join(format!("{key}.msh"));
        assert!(cache_file.exists(), "cache written on first load");

        // Overwrite the cache with a distinct 2-triangle mesh (source untouched, so the key is
        // stable). A second load must return *that* — proving it read the cache, not re-parsed.
        std::fs::write(&cache_file, encode_meshes(&[mesh("fake", 2)])).unwrap();
        let second = load_cached(&obj, &cache).expect("cache hit");
        assert_eq!(
            second.iter().map(|m| m.indices.len()).sum::<usize>(),
            6,
            "served the cached mesh, not a fresh parse of the 1-triangle OBJ"
        );
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
            let (b0, b1, b2) = (
                c[0] as u32,
                *c.get(1).unwrap_or(&0) as u32,
                *c.get(2).unwrap_or(&0) as u32,
            );
            let n = (b0 << 16) | (b1 << 8) | b2;
            out.push(T[(n >> 18) as usize & 63] as char);
            out.push(T[(n >> 12) as usize & 63] as char);
            out.push(if c.len() > 1 {
                T[(n >> 6) as usize & 63] as char
            } else {
                '='
            });
            out.push(if c.len() > 2 {
                T[n as usize & 63] as char
            } else {
                '='
            });
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
        let evidence = load_evidence(&path).expect("self-contained evidence");
        assert_eq!(evidence[0].positions.len(), 3);
        // A declared buffer outside the selected payload must not be fetched, even if present.
        std::fs::write(dir.join("outside.bin"), &buf).unwrap();
        let mut external: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        external["buffers"][0]["uri"] = serde_json::json!("outside.bin");
        std::fs::write(&path, external.to_string()).unwrap();
        assert!(
            load_evidence(&path).is_err(),
            "evidence followed an undeclared disk dependency"
        );
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(meshes.len(), 1, "one primitive");
        assert_eq!(meshes[0].positions.len(), 3);
        assert_eq!(
            meshes[0].indices,
            vec![0, 1, 2],
            "sequential indices generated"
        );
        // And it renders (the whole point of loading it).
        let views =
            crate::render3d::render_views(&meshes, 64, &crate::render3d::DEFAULT_VIEWS[..1]);
        assert_eq!(views.len(), 1);
    }

    /// A 1x2 PNG: top row red, bottom row blue.
    fn red_over_blue() -> Vec<u8> {
        let mut img = image::RgbaImage::new(1, 2);
        img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        img.put_pixel(0, 1, image::Rgba([0, 0, 255, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    /// One textured triangle: 36 bytes of positions, then 24 of UVs, then `extra` (an embedded
    /// image); `buffer` and `images` are the glTF entries that locate them.
    fn textured_gltf(buffer: &str, images: &str, extra_view: &str) -> String {
        format!(
            r#"{{"asset":{{"version":"2.0"}},"scene":0,"scenes":[{{"nodes":[0]}}],"nodes":[{{"mesh":0}}],
"meshes":[{{"primitives":[{{"attributes":{{"POSITION":0,"TEXCOORD_0":1}},"material":0}}]}}],
"materials":[{{"pbrMetallicRoughness":{{"baseColorTexture":{{"index":0}}}}}}],
"textures":[{{"source":0}}],"images":[{images}],"buffers":[{buffer}],
"bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":36}},{{"buffer":0,"byteOffset":36,"byteLength":24}}{extra_view}],
"accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0.0,0.0,0.0],"max":[1.0,1.0,0.0]}},
{{"bufferView":1,"componentType":5126,"count":3,"type":"VEC2"}}]}}"#
        )
    }

    fn triangle_bin() -> Vec<u8> {
        let mut bin = Vec::new();
        for f in [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
            bin.extend_from_slice(&f.to_le_bytes());
        }
        for f in [0.0f32, 0.0, 1.0, 0.0, 0.0, 1.0] {
            bin.extend_from_slice(&f.to_le_bytes());
        }
        bin
    }

    /// The texture landed, bottom row first, and the UVs turned with it: a glTF `v = 0` is the top
    /// of the image. The default loader still reads no texture.
    fn assert_textured(path: &Path) {
        let parts = load_textured(path, &BTreeMap::new()).expect("textured load");
        let (slot, mesh) = &parts[0];
        assert_eq!(*slot, 0);
        let albedo = mesh.albedo.as_ref().expect("base-colour texture read");
        assert_eq!((albedo.width, albedo.height), (1, 2));
        assert_eq!(albedo.rgba[0], [0, 0, 255, 255], "rows are bottom-up");
        assert_eq!(albedo.rgba[1], [255, 0, 0, 255]);
        assert_eq!(
            mesh.uvs.as_ref().unwrap()[0],
            [0.0, 1.0],
            "v turned to bottom-left"
        );
        let plain = load_any(path, &BTreeMap::new()).expect("default load");
        assert!(
            plain[0].albedo.is_none(),
            "the default loader reads no texture"
        );
        assert_eq!(plain[0].uvs.as_ref().unwrap()[0], [0.0, 0.0]);
    }

    #[test]
    fn load_textured_reads_a_data_uri_image() {
        let bin = triangle_bin();
        let json = textured_gltf(
            &format!(
                r#"{{"byteLength":{},"uri":"data:application/octet-stream;base64,{}"}}"#,
                bin.len(),
                b64(&bin)
            ),
            &format!(
                r#"{{"uri":"data:image/png;base64,{}"}}"#,
                b64(&red_over_blue())
            ),
            "",
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tri.gltf");
        std::fs::write(&path, json).unwrap();
        assert_textured(&path);
    }

    #[test]
    fn load_textured_reads_an_external_image() {
        let bin = triangle_bin();
        let json = textured_gltf(
            &format!(
                r#"{{"byteLength":{},"uri":"data:application/octet-stream;base64,{}"}}"#,
                bin.len(),
                b64(&bin)
            ),
            r#"{"uri":"tex.png"}"#,
            "",
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("tex.png"), red_over_blue()).unwrap();
        let path = dir.path().join("tri.gltf");
        std::fs::write(&path, json).unwrap();
        assert_textured(&path);
    }

    /// A GLB: the image lives in the binary chunk, behind a buffer view.
    #[test]
    fn load_textured_reads_a_glb_embedded_png() {
        let png = red_over_blue();
        let mut bin = triangle_bin();
        bin.extend_from_slice(&png);
        while bin.len() % 4 != 0 {
            bin.push(0);
        }
        let mut json = textured_gltf(
            &format!(r#"{{"byteLength":{}}}"#, bin.len()),
            r#"{"bufferView":2,"mimeType":"image/png"}"#,
            &format!(
                r#",{{"buffer":0,"byteOffset":60,"byteLength":{}}}"#,
                png.len()
            ),
        )
        .into_bytes();
        while json.len() % 4 != 0 {
            json.push(b' ');
        }
        let mut glb = Vec::new();
        glb.extend_from_slice(b"glTF");
        glb.extend_from_slice(&2u32.to_le_bytes());
        glb.extend_from_slice(&((12 + 8 + json.len() + 8 + bin.len()) as u32).to_le_bytes());
        glb.extend_from_slice(&(json.len() as u32).to_le_bytes());
        glb.extend_from_slice(b"JSON");
        glb.extend_from_slice(&json);
        glb.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        glb.extend_from_slice(b"BIN\0");
        glb.extend_from_slice(&bin);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tri.glb");
        std::fs::write(&path, glb).unwrap();
        assert_textured(&path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // TEMPORARY probe — never committed.
    #[test]
    #[ignore = "probe"]
    fn probe_load_textured() {
        let path = std::env::var("LODESTASH_MODEL").expect("LODESTASH_MODEL");
        let path = Path::new(&path);
        match load_textured(path, &BTreeMap::new()) {
            Ok(v) => eprintln!(
                "PROBE textured meshes={} slots={:?} painted={}",
                v.len(),
                v.iter().map(|(s, _)| *s).take(12).collect::<Vec<_>>(),
                v.iter().filter(|(_, m)| m.albedo.is_some()).count()
            ),
            Err(e) => eprintln!("PROBE textured ERR {e}"),
        }
        match load_any(path, &BTreeMap::new()) {
            Ok(v) => eprintln!("PROBE plain meshes={}", v.len()),
            Err(e) => eprintln!("PROBE plain ERR {e}"),
        }
    }

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

    /// A mesh painted with two materials is two textured meshes, each with its own slot and albedo —
    /// while the default loader, which the embedding render reads, still gives one mesh.
    #[test]
    fn load_textured_splits_a_two_material_mesh_by_slot() {
        let dir = tempfile::tempdir().unwrap();
        for (name, px) in [
            ("red.png", [255, 0, 0, 255]),
            ("green.png", [0, 255, 0, 255]),
        ] {
            let mut img = image::RgbaImage::new(1, 1);
            img.put_pixel(0, 0, image::Rgba(px));
            img.save(dir.path().join(name)).unwrap();
        }
        std::fs::write(
            dir.path().join("two.mtl"),
            "newmtl a\nmap_Kd red.png\nnewmtl b\nmap_Kd green.png\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("two.obj"),
            "mtllib two.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nv 1 1 0\nvt 0 0\nvt 1 0\nvt 0 1\nvt 1 1\n\
             usemtl a\nf 1/1 2/2 3/3\nusemtl b\nf 2/2 4/4 3/3\n",
        )
        .unwrap();
        let path = dir.path().join("two.obj");

        let parts = load_textured(&path, &BTreeMap::new()).expect("parts");
        let mut got: Vec<(u32, usize, [u8; 4])> = parts
            .iter()
            .map(|(slot, m)| {
                let albedo = m.albedo.as_ref().expect("each slot has its own albedo");
                (*slot, m.positions.len(), albedo.rgba[0])
            })
            .collect();
        got.sort();
        assert_eq!(
            got,
            vec![(0, 3, [255, 0, 0, 255]), (1, 3, [0, 255, 0, 255])]
        );

        let plain = load_meshes(&path, &BTreeMap::new()).unwrap();
        assert_eq!(plain.len(), 1, "the default loader is unchanged");
        assert_eq!(plain[0].positions.len(), 6);
    }

    /// The catalog paints the slots the file leaves bare, and never the ones it does not: a model
    /// that declares its own texture keeps it (v1 plan S08).
    #[test]
    fn the_catalog_paints_only_the_slots_the_file_leaves_bare() {
        let dir = tempfile::tempdir().unwrap();
        let px = |name: &str, px: [u8; 4]| {
            let mut img = image::RgbaImage::new(1, 1);
            img.put_pixel(0, 0, image::Rgba(px));
            img.save(dir.path().join(name)).unwrap();
            dir.path().join(name)
        };
        let own = px("own.png", [255, 0, 0, 255]);
        let from_catalog = px("cat.png", [0, 0, 255, 255]);
        std::fs::write(
            dir.path().join("two.mtl"),
            "newmtl a\nmap_Kd own.png\nnewmtl b\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("two.obj"),
            "mtllib two.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nv 1 1 0\nvt 0 0\nvt 1 0\nvt 0 1\nvt 1 1\n\
             usemtl a\nf 1/1 2/2 3/3\nusemtl b\nf 2/2 4/4 3/3\n",
        )
        .unwrap();
        let path = dir.path().join("two.obj");

        // Slot 0 declares its own; slot 1 declares none and takes the catalog's.
        let slots: BTreeMap<u32, PathBuf> = [(0, from_catalog.clone()), (1, from_catalog)]
            .into_iter()
            .collect();
        let painted = load_textured_with(&path, &BTreeMap::new(), &slots).expect("load");
        let mut got: Vec<(u32, [u8; 4])> = painted
            .iter()
            .map(|(slot, m)| (*slot, m.albedo.as_ref().expect("painted").rgba[0]))
            .collect();
        got.sort();
        assert_eq!(
            got,
            vec![(0, [255, 0, 0, 255]), (1, [0, 0, 255, 255])],
            "the file's own texture wins its slot; the catalog fills the bare one"
        );
        assert!(
            std::fs::metadata(&own).is_ok(),
            "the loader reads textures, it does not move them"
        );

        // Nothing offered for a slot leaves it unpainted rather than borrowing another's.
        let none = load_textured_with(&path, &BTreeMap::new(), &BTreeMap::new()).expect("load");
        let bare = none.iter().find(|(slot, _)| *slot == 1).expect("slot 1");
        assert!(bare.1.albedo.is_none());
    }

    /// The embedding renders the geometry proxy, and S07/S08 must not have coloured it: the proxy
    /// loader still returns one mesh per node, with the first material's colour and no texture
    /// (AI-PIPELINE §3 — a re-coloured input would silently re-embed the library).
    #[test]
    fn the_proxy_the_embedding_renders_stays_geometry_only() {
        let dir = tempfile::tempdir().unwrap();
        let mut img = image::RgbaImage::new(1, 1);
        img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        img.save(dir.path().join("red.png")).unwrap();
        std::fs::write(
            dir.path().join("two.mtl"),
            "newmtl a\nmap_Kd red.png\nnewmtl b\nmap_Kd red.png\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("two.obj"),
            "mtllib two.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nv 1 1 0\nvt 0 0\nvt 1 0\nvt 0 1\nvt 1 1\n\
             usemtl a\nf 1/1 2/2 3/3\nusemtl b\nf 2/2 4/4 3/3\n",
        )
        .unwrap();
        let path = dir.path().join("two.obj");
        let cache = dir.path().join("cache");

        let proxy = load_preview_cached(&path, &cache, crate::decimate::PREVIEW_BUDGET).unwrap();
        assert_eq!(
            proxy.len(),
            1,
            "one mesh per node, not one per material slot"
        );
        assert_eq!(proxy[0].indices.len() / 3, 2, "both faces, undecimated");
        assert!(
            proxy[0].albedo.is_none(),
            "the embedding's input carries no texture"
        );
        // And again from the cache, which is the path a warmed library takes.
        let again = load_preview_cached(&path, &cache, crate::decimate::PREVIEW_BUDGET).unwrap();
        assert!(again[0].albedo.is_none());
        assert_eq!(again[0].base_color, proxy[0].base_color);
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

    /// A model already under budget must not get a `.lod`, and must keep its `.msh`.
    ///
    /// The reference library's median model is 300 triangles against Low's 1500 ceiling, so
    /// `decimate` returns `None` for 91% of it and the old code stored a byte-for-byte re-encode of
    /// the staging mesh — then deleted the staging mesh, so the next triangle count re-parsed the
    /// source. Both halves of that are checked here.
    #[test]
    fn a_model_already_under_budget_gets_no_proxy_and_keeps_its_staging_mesh() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tri.obj");
        std::fs::write(
            &path,
            "v 0 0 0
v 1 0 0
v 0 1 0
f 1 2 3
",
        )
        .unwrap();
        let cache = dir.path().join("cache");
        let budget = crate::decimate::Budget::new(0.02, 300, 1500);

        let meshes = load_preview_cached(&path, &cache, budget).expect("loads");
        assert_eq!(meshes[0].indices, vec![0, 1, 2], "one triangle, undamaged");

        let key = cache_key(&path);
        let mesh_dir = cache.join("mesh");
        assert!(
            !mesh_dir
                .join(format!("{key}-{}.lod", budget.tag()))
                .exists(),
            "a 1-triangle model has nothing to collapse — storing a proxy just doubles the disk"
        );
        assert!(
            mesh_dir.join(format!("{key}.msh")).exists(),
            "and the staging mesh must survive, since it is now the only cached geometry"
        );

        // Second call is served from the staging mesh, same answer.
        let again = load_preview_cached(&path, &cache, budget).expect("loads again");
        assert_eq!(again[0].indices, meshes[0].indices);
    }

    /// Hulls out, props in, and never everything — plus the false positive that shaped the match.
    #[test]
    fn collision_hulls_are_recognised_without_eating_the_asset_named_lodestone() {
        for hull in [
            "UCX_Crate_01",
            "SM_Bld_Base_Stairs_02_Collision",
            "barrel_collider",
            "Collision",
        ] {
            assert!(
                is_collision_hull(hull),
                "{hull} wraps the prop, it is not the prop"
            );
        }
        for keep in [
            "SM_Bld_Base_Stairs_02",
            // The library really holds this one. A substring test for "lod" would delete it, which
            // is why the match is anchored and why LOD levels are not matched at all.
            "lodestone_gem",
            "Collider_Housing_Mesh",
            "rock_LOD0",
        ] {
            assert!(
                !is_collision_hull(keep),
                "{keep} is geometry the user wants to see"
            );
        }
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
