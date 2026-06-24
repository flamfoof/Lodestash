//! Lodestash desktop app entry point.
//! Spec: ARCHITECTURE.md §6, DESIGN-GUIDELINES.md.

mod app;
mod config;
mod theme;

fn main() -> eframe::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "lodestash=info".into()),
        )
        .init();

    tracing::info!("Lodestash starting");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Lodestash")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([640.0, 480.0])
            .with_icon(load_icon()),
        renderer: eframe::Renderer::Wgpu,
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
