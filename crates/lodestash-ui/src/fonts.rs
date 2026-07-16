//! Bundled fonts per DESIGN-GUIDELINES.md §3 / UI-SPEC.md §0.
//! Cinzel = display/titles, Inter = UI/body, JetBrains Mono = numeric/stats.
//! All three are embedded (SIL OFL, licenses alongside the .ttf files) —
//! offline invariant: no runtime font fetch.

use egui::{FontData, FontDefinitions, FontFamily, FontId};

/// The display family name (Cinzel) for titles/wordmark — never body text.
pub const DISPLAY_FAMILY: &str = "display";

/// A `FontId` in the Cinzel display family.
pub fn display(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(DISPLAY_FAMILY.into()))
}

/// Install the bundled fonts: Inter first in Proportional, JetBrains Mono
/// first in Monospace, Cinzel as the named "display" family. egui's default
/// fonts stay as fallbacks for symbol/emoji coverage.
pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    fonts.font_data.insert(
        "inter".to_owned(),
        FontData::from_static(include_bytes!("../assets/fonts/Inter.ttf")),
    );
    fonts.font_data.insert(
        "jetbrains-mono".to_owned(),
        FontData::from_static(include_bytes!("../assets/fonts/JetBrainsMono.ttf")),
    );
    fonts.font_data.insert(
        "cinzel".to_owned(),
        FontData::from_static(include_bytes!("../assets/fonts/Cinzel.ttf")),
    );

    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "inter".to_owned());
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "jetbrains-mono".to_owned());

    // Display family: Cinzel, falling back to the proportional stack.
    let mut display_stack = vec!["cinzel".to_owned()];
    display_stack.extend(
        fonts
            .families
            .get(&FontFamily::Proportional)
            .cloned()
            .unwrap_or_default(),
    );
    fonts
        .families
        .insert(FontFamily::Name(DISPLAY_FAMILY.into()), display_stack);

    ctx.set_fonts(fonts);
}
