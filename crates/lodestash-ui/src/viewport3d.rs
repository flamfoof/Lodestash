//! 3D orbit viewport for Quick Look — three-d on eframe's shared Glow context.
//!
//! Spike resolution (TECH-STACK §7): three-d 0.18 and eframe 0.29 both pin `glow 0.14`,
//! so the viewport renders straight into egui's frame through a `PaintCallback` on the
//! one shared GL context — no offscreen handoff, no readback. The two crates must be
//! upgraded together (their `glow` versions must stay unified).
//!
//! Threading mirrors the image viewer: file → `CpuModel` parses on a background thread
//! (pure CPU, `Send`); the GPU upload and every render happen on the UI thread. The
//! scene lives in an `Arc<Mutex<…>>` because egui paint callbacks must be `Send + Sync`.
//!
//! ponytail: direct-load formats only (glTF/GLB/OBJ) — FBX arrives with the
//! normalize→glTF cache (KANBAN P1 card), which feeds this same viewport.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};

use eframe::egui_glow::CallbackFn;
use egui::{PointerButton, Response, Sense, Ui, Vec2};
use three_d::{
    degrees, vec3, AmbientLight, AxisAlignedBoundingBox, Camera, ClearState, Context,
    DirectionalLight, Geometry, InnerSpace, Model, ModelPart, PhysicalMaterial, RenderTarget,
    ScissorBox, Srgba, Vec3, Viewport,
};

/// File extensions the viewport can load directly (no normalization pass needed).
pub fn is_direct_model(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        e.eq_ignore_ascii_case("glb")
            || e.eq_ignore_ascii_case("gltf")
            || e.eq_ignore_ascii_case("obj")
    })
}

/// The Quick Look 3D viewer: async model loading + an orbit camera rendered in-frame.
pub struct Viewport3d {
    scene: Arc<Mutex<Scene>>,
    rx: Option<Receiver<Result<three_d_asset::Model, String>>>,
    /// Grid entry currently loaded/loading — the staleness guard, like `ImageViewer`.
    pub entry_idx: Option<usize>,
    pub loading: bool,
    pub error: Option<String>,
}

impl Viewport3d {
    /// Wrap eframe's shared GL context. `None` if three-d rejects the context.
    pub fn new(gl: Arc<eframe::glow::Context>) -> Option<Self> {
        let ctx = Context::from_gl_context(gl)
            .map_err(|e| tracing::warn!("3D viewport unavailable: {e}"))
            .ok()?;
        let ambient = AmbientLight::new(&ctx, 0.4, Srgba::WHITE);
        let sun = DirectionalLight::new(&ctx, 1.6, Srgba::WHITE, vec3(-0.5, -1.0, -0.6));
        let fill = DirectionalLight::new(&ctx, 0.4, Srgba::WHITE, vec3(0.6, 0.2, 0.8));
        Some(Self {
            scene: Arc::new(Mutex::new(Scene {
                ctx,
                models: Vec::new(),
                ambient,
                sun,
                fill,
                yaw: 0.9,
                pitch: 0.45,
                dist: 3.0,
                target: vec3(0.0, 0.0, 0.0),
                radius: 1.0,
            })),
            rx: None,
            entry_idx: None,
            loading: false,
            error: None,
        })
    }

    /// Kick off an async load for `path`, replacing whatever is shown.
    pub fn load(&mut self, entry_idx: usize, path: PathBuf, egui_ctx: &egui::Context) {
        self.entry_idx = Some(entry_idx);
        self.loading = true;
        self.error = None;
        self.scene.lock().unwrap().models.clear();

        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        let egui_ctx = egui_ctx.clone();
        std::thread::spawn(move || {
            let result = load_cpu_model(&path);
            let _ = tx.send(result);
            egui_ctx.request_repaint();
        });
    }

    /// Drain a finished load: GPU-upload the meshes and frame the camera on them.
    pub fn poll(&mut self) {
        let Some(rx) = &self.rx else { return };
        let Ok(result) = rx.try_recv() else { return };
        self.rx = None;
        self.loading = false;
        match result {
            Ok(cpu) => {
                let mut scene = self.scene.lock().unwrap();
                match Model::<PhysicalMaterial>::new(&scene.ctx, &cpu) {
                    Ok(mut model) => {
                        // Model derefs to Vec<Gm<..>>; take the parts out of the wrapper.
                        scene.models = std::mem::take(&mut *model);
                        scene.frame_content();
                    }
                    Err(e) => self.error = Some(e.to_string()),
                }
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// Drop the loaded meshes (GPU buffers free on the UI thread, context alive).
    pub fn clear(&mut self) {
        self.rx = None;
        self.entry_idx = None;
        self.loading = false;
        self.error = None;
        self.scene.lock().unwrap().models.clear();
    }

    pub fn has_content(&self) -> bool {
        !self.scene.lock().unwrap().models.is_empty()
    }

    /// The interactive viewport: left-drag orbits, right/middle-drag pans, scroll
    /// dollies, double-click reframes. Renders via a paint callback into `size`.
    pub fn ui(&mut self, ui: &mut Ui, size: Vec2) -> Response {
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
        {
            let mut s = self.scene.lock().unwrap();
            if resp.dragged_by(PointerButton::Primary) {
                let d = resp.drag_delta();
                s.yaw += d.x * 0.01;
                s.pitch = (s.pitch + d.y * 0.01).clamp(-1.54, 1.54);
            }
            if resp.dragged_by(PointerButton::Secondary) || resp.dragged_by(PointerButton::Middle) {
                let d = resp.drag_delta();
                // Pan in the camera plane, scaled so a full-height drag ≈ one view height.
                let (right, up) = s.camera_axes();
                let k = s.dist * 0.0016;
                let pan = right * (-d.x * k) + up * (d.y * k);
                s.target += pan;
            }
            if resp.hovered() {
                let scroll = ui.input(|i| i.raw_scroll_delta.y);
                if scroll != 0.0 {
                    s.dist =
                        (s.dist * (-scroll * 0.002).exp()).clamp(s.radius * 0.05, s.radius * 40.0);
                }
            }
            if resp.double_clicked() {
                s.frame_content();
            }
        }

        let scene = self.scene.clone();
        ui.painter().add(egui::PaintCallback {
            rect,
            callback: Arc::new(CallbackFn::new(move |info, _painter| {
                // egui_glow re-arms its own GL state after every callback, so the
                // depth/cull/scissor state three-d sets here doesn't leak out.
                scene.lock().unwrap().render(&info);
            })),
        });
        resp
    }
}

struct Scene {
    ctx: Context,
    models: Vec<ModelPart<PhysicalMaterial>>,
    ambient: AmbientLight,
    sun: DirectionalLight,
    fill: DirectionalLight,
    yaw: f32,
    pitch: f32,
    dist: f32,
    target: Vec3,
    radius: f32,
}

impl Scene {
    /// Re-center and re-distance the orbit around the loaded meshes' bounds.
    fn frame_content(&mut self) {
        let mut aabb = AxisAlignedBoundingBox::EMPTY;
        for m in &self.models {
            aabb.expand_with_aabb(m.aabb());
        }
        if aabb.is_empty() {
            return;
        }
        let size = aabb.size();
        self.radius = (0.5 * size.magnitude()).max(1e-4);
        self.target = aabb.center();
        self.dist = self.radius * 2.4;
        self.yaw = 0.9;
        self.pitch = 0.45;
    }

    fn camera_position(&self) -> Vec3 {
        let dir = vec3(
            self.yaw.cos() * self.pitch.cos(),
            self.pitch.sin(),
            self.yaw.sin() * self.pitch.cos(),
        );
        self.target + dir * self.dist
    }

    /// Camera-plane right/up vectors for panning.
    fn camera_axes(&self) -> (Vec3, Vec3) {
        let forward = (self.target - self.camera_position()).normalize();
        let right = forward.cross(vec3(0.0, 1.0, 0.0)).normalize();
        let up = right.cross(forward).normalize();
        (right, up)
    }

    fn render(&mut self, info: &egui::PaintCallbackInfo) {
        if self.models.is_empty() {
            return;
        }
        let vp = info.viewport_in_pixels();
        let viewport = Viewport {
            x: vp.left_px,
            y: vp.from_bottom_px, // GL origin is bottom-left
            width: vp.width_px as u32,
            height: vp.height_px as u32,
        };
        if viewport.width == 0 || viewport.height == 0 {
            return;
        }
        let scissor = ScissorBox {
            x: viewport.x,
            y: viewport.y,
            width: viewport.width,
            height: viewport.height,
        };
        let camera = Camera::new_perspective(
            viewport,
            self.camera_position(),
            self.target,
            vec3(0.0, 1.0, 0.0),
            degrees(45.0),
            (self.radius * 0.01).max(1e-4),
            self.radius * 100.0,
        );
        // #141926 — the Quick Look preview recess (UI-SPEC §14).
        RenderTarget::screen(&self.ctx, info.screen_size_px[0], info.screen_size_px[1])
            .clear_partially(
                scissor,
                ClearState::color_and_depth(0.078, 0.098, 0.149, 1.0, 1.0),
            )
            .render_partially(
                scissor,
                &camera,
                &self.models,
                &[&self.ambient, &self.sun, &self.fill],
            );
    }
}

/// Parse a model file into CPU-side meshes + materials. Runs on a worker thread.
fn load_cpu_model(path: &Path) -> Result<three_d_asset::Model, String> {
    let mut raw = three_d_asset::io::load(&[path]).map_err(|e| e.to_string())?;
    raw.deserialize::<three_d_asset::Model>(path)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_model_detection_is_by_extension_case_insensitive() {
        assert!(is_direct_model(Path::new("a/b/mesh.glb")));
        assert!(is_direct_model(Path::new("Mesh.GLTF")));
        assert!(is_direct_model(Path::new("thing.Obj")));
        assert!(!is_direct_model(Path::new("thing.fbx"))); // needs the glTF cache first
        assert!(!is_direct_model(Path::new("noext")));
    }

    #[test]
    fn load_cpu_model_parses_a_minimal_obj() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tri.obj");
        std::fs::write(&path, "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n").unwrap();
        let model = load_cpu_model(&path).expect("minimal OBJ parses");
        assert_eq!(model.geometries.len(), 1, "one triangle geometry");
    }

    #[test]
    fn load_cpu_model_reports_missing_file_as_error() {
        let err = load_cpu_model(Path::new("Z:/definitely/not/here.obj")).unwrap_err();
        assert!(!err.is_empty());
    }
}
