//! Design tokens from DESIGN-GUIDELINES.md §2.
//! Every colour reference in the UI must come from here — no inline hex.
//! Most constants are forward-declared for future panels — the allow is intentional.
#![allow(dead_code)]

use egui::Color32;

// Surfaces & text
pub const BG: Color32 = Color32::from_rgb(0x0E, 0x11, 0x17); // #0E1117
pub const SURFACE: Color32 = Color32::from_rgb(0x16, 0x1B, 0x26); // #161B26
pub const SURFACE_RAISED: Color32 = Color32::from_rgb(0x1E, 0x25, 0x33); // #1E2533
pub const OVERLAY: Color32 = Color32::from_rgb(0x23, 0x2C, 0x3D); // #232C3D
/// Sunken preview recess behind Quick Look 3D/audio previews (prototype #141926).
pub const RECESS: Color32 = Color32::from_rgb(0x14, 0x19, 0x26); // #141926
pub const BORDER: Color32 = Color32::from_rgb(0x2C, 0x35, 0x47); // #2C3547
pub const BORDER_STRONG: Color32 = Color32::from_rgb(0x3A, 0x46, 0x5C); // #3A465C
pub const TEXT: Color32 = Color32::from_rgb(0xE6, 0xE9, 0xF0); // #E6E9F0
pub const TEXT_MUTED: Color32 = Color32::from_rgb(0xA7, 0xB0, 0xC0); // #A7B0C0
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x6E, 0x76, 0x89); // #6E7689

// Brand / action
pub const GOLD: Color32 = Color32::from_rgb(0xE7, 0xB7, 0x65); // #E7B765
pub const GOLD_BRIGHT: Color32 = Color32::from_rgb(0xFF, 0xD9, 0x8A); // #FFD98A
pub const ARCANE: Color32 = Color32::from_rgb(0x49, 0xD6, 0xC9); // #49D6C9
pub const ARCANE_BRIGHT: Color32 = Color32::from_rgb(0x7B, 0xF0, 0xE5);

// Element-type colors (media type categories)
pub const TYPE_MODEL: Color32 = Color32::from_rgb(0xA9, 0x8B, 0xFF); // #A98BFF amethyst
pub const TYPE_SOUND: Color32 = Color32::from_rgb(0x49, 0xC9, 0xE0); // #49C9E0 aqua
pub const TYPE_IMAGE: Color32 = Color32::from_rgb(0x6F, 0xB1, 0xFF); // #6FB1FF sapphire
pub const TYPE_MATERIAL: Color32 = Color32::from_rgb(0xF2, 0xB8, 0x4B); // amber
pub const TYPE_FONT: Color32 = Color32::from_rgb(0xF2, 0x96, 0x6E); // coral
pub const TYPE_SPRITE: Color32 = Color32::from_rgb(0x5B, 0xD8, 0x97); // verdant
pub const TYPE_TEXTURE: Color32 = TYPE_IMAGE; // same visual bucket until textures get a distinct token

// Status / rarity
pub const STATUS_UNIDENTIFIED: Color32 = Color32::from_rgb(0x8A, 0x93, 0xA6);
pub const STATUS_IDENTIFIED: Color32 = Color32::from_rgb(0x5B, 0xD8, 0x97);
pub const STATUS_CONFIRMED: Color32 = Color32::from_rgb(0xE7, 0xB7, 0x65);

// Semantic
pub const SUCCESS: Color32 = Color32::from_rgb(0x5B, 0xD8, 0x97);
pub const WARNING: Color32 = Color32::from_rgb(0xF2, 0xB8, 0x4B);
pub const DANGER: Color32 = Color32::from_rgb(0xF2, 0x66, 0x6E);

use lodestash_core::types::MediaType;

/// Map a media type to its element-type colour.
pub fn media_type_color(mt: MediaType) -> Color32 {
    match mt {
        MediaType::Model3d => TYPE_MODEL,
        MediaType::Texture => TYPE_TEXTURE,
        MediaType::Audio => TYPE_SOUND,
        MediaType::Image => TYPE_IMAGE,
        MediaType::Sprite => TYPE_SPRITE,
        MediaType::Material => TYPE_MATERIAL,
        MediaType::Font => TYPE_FONT,
    }
}

/// Apply the Lodestash dark theme to an egui `Style`.
pub fn apply(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();

    visuals.override_text_color = Some(TEXT);
    visuals.panel_fill = BG;
    visuals.window_fill = SURFACE;
    visuals.extreme_bg_color = BG;
    visuals.faint_bg_color = SURFACE;
    visuals.code_bg_color = SURFACE_RAISED;

    visuals.widgets.noninteractive.bg_fill = SURFACE;
    visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, TEXT_MUTED);
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, BORDER);

    visuals.widgets.inactive.bg_fill = SURFACE_RAISED;
    visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, TEXT);
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, BORDER);

    visuals.widgets.hovered.bg_fill = OVERLAY;
    visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, GOLD_BRIGHT);
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, GOLD);

    visuals.widgets.active.bg_fill = OVERLAY;
    visuals.widgets.active.fg_stroke = egui::Stroke::new(1.5, GOLD_BRIGHT);
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.5, GOLD);

    visuals.selection.bg_fill = GOLD.linear_multiply(0.25);
    visuals.selection.stroke = egui::Stroke::new(1.0, GOLD);

    visuals.window_rounding = egui::Rounding::same(6.0);
    visuals.menu_rounding = egui::Rounding::same(6.0);

    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.window_margin = egui::Margin::same(12.0);
    ctx.set_style(style);
}
