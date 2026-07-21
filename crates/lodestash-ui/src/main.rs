//! Lodestash desktop app entry point.
//! Spec: ARCHITECTURE.md §6, DESIGN-GUIDELINES.md.

mod app;
mod audio;
mod config;
mod fonts;
mod jobs;
mod models;
mod theme;
mod viewport3d;

fn main() -> eframe::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "lodestash=info".into()),
        )
        .init();

    tracing::info!("Lodestash starting");

    // Point `ort` at a previously-downloaded ONNX Runtime (if the user hasn't set ORT_DYLIB_PATH
    // themselves) *before* any embedder load. On a fresh install neither exists, so ort is left
    // untouched until the model+runtime are downloaded and the first search runs.
    models::apply_ort_dylib_env();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Lodestash")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([640.0, 480.0])
            .with_icon(load_icon()),
        // Glow (OpenGL/WGL) has synchronous buffer swaps on Windows, so there is no
        // swap-chain reconfiguration gap for DWM to fill with a scaled old frame.
        // wgpu causes a visible "revert" jitter on resize because surface.configure()
        // briefly invalidates the current texture before the new frame is ready.
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };

    eframe::run_native(
        "Lodestash",
        options,
        Box::new(|cc| Ok(Box::new(app::LodestashApp::new(cc)))),
    )
}

fn load_icon() -> egui::IconData {
    // Placeholder 1×1 gold pixel until the real icon is wired up (see design/branding/).
    egui::IconData {
        rgba: vec![0xE7, 0xB7, 0x65, 0xFF],
        width: 1,
        height: 1,
    }
}
