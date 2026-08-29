//! Which Unity render pipeline a pack targets — `package.pipeline` (DATA-MODEL §12,
//! INGESTION §2), the input to the `pipeline_mismatch` import advisory (ENGINE-API §5.4).
//!
//! Detection reads **names, not payloads**: the pack's own name, the `pathname`s its manifest
//! already recorded (`package_entry`, no archive re-inflate), and the dependency list of any
//! embedded UPM manifest the metadata pass already reads. A pack that ships a variant per
//! pipeline says so in its folder and sub-package names — that is the shape store packs actually
//! take, and it costs nothing to read.
//!
//! **Shader GUIDs are deliberately not used**, though INGESTION §2 first proposed them. A store
//! pack's `.mat` points at *its own* shader asset, so the GUID identifies the pack, not a
//! pipeline: across 149 real `.mat` files in the reference library, eight distinct custom GUIDs
//! appear and the only recurring well-known one is Unity's default-resources GUID
//! (`0000000000000000f000000000000000`), which every pipeline uses for UI and legacy materials —
//! so reading it as "Built-in" would mislabel URP packs and fire a false "materials may appear
//! pink" warning. Resolving a shader GUID to a `.shadergraph` and reading its target would
//! discriminate, but that needs a payload pull per material; names already answer it.

use std::collections::BTreeSet;

pub const BUILT_IN: &str = "built_in";
pub const URP: &str = "urp";
pub const HDRP: &str = "hdrp";
pub const MIXED: &str = "mixed";

/// Split a path or name the way a human reads it: `Upgrade/URP_10+.unitypackage` is three tokens
/// with `urp` among them, and `Burp.png` is one token that is not `urp`.
fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric() && c != '+')
        .filter(|t| !t.is_empty())
        .map(|t| t.to_ascii_lowercase())
        .collect()
}

/// Pipelines named by a path or a pack name. Empty when it names none.
///
/// Whole-token matching only: a substring rule would read `urp` out of "Burp" and `hd` out of
/// almost anything. Multi-word forms ("Universal RP", "High Definition") are matched as pairs.
pub fn from_path(path: &str) -> BTreeSet<&'static str> {
    let t = tokens(path);
    let mut out = BTreeSet::new();
    for (i, tok) in t.iter().enumerate() {
        let next = t.get(i + 1).map(String::as_str);
        match tok.as_str() {
            "urp" => {
                out.insert(URP);
            }
            "hdrp" => {
                out.insert(HDRP);
            }
            "builtin" => {
                out.insert(BUILT_IN);
            }
            "universal" if matches!(next, Some("rp" | "render")) => {
                out.insert(URP);
            }
            "hd" | "high" if matches!(next, Some("rp" | "render" | "definition")) => {
                out.insert(HDRP);
            }
            "built" if next == Some("in") => {
                out.insert(BUILT_IN);
            }
            _ => {}
        }
    }
    out
}

/// Pipelines named by an embedded UPM manifest's `com.unity.render-pipelines.*` dependencies.
///
/// `…render-pipelines.core` is deliberately ignored: URP and HDRP both depend on it, so it says
/// only "some SRP", which is not a value `package.pipeline` can hold.
pub fn from_manifest(json: &str) -> BTreeSet<&'static str> {
    let mut out = BTreeSet::new();
    if json.contains("com.unity.render-pipelines.universal") {
        out.insert(URP);
    }
    if json.contains("com.unity.render-pipelines.high-definition") {
        out.insert(HDRP);
    }
    out
}

/// Collapse everything a pack said about itself into the one `package.pipeline` value.
///
/// Nothing seen ⇒ `None` (the column's "unknown"), never a guess: a pack we cannot classify must
/// stay silent rather than warn about a mismatch it may not have. Several ⇒ `mixed`, which is
/// what a pack shipping a variant per pipeline is.
pub fn resolve(seen: &BTreeSet<&'static str>) -> Option<&'static str> {
    match seen.len() {
        0 => None,
        1 => seen.iter().next().copied(),
        _ => Some(MIXED),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&'static str]) -> BTreeSet<&'static str> {
        items.iter().copied().collect()
    }

    /// Every case here is a real path from the reference library's `package_entry` rows.
    #[test]
    fn real_pack_paths_classify() {
        assert_eq!(
            from_path("Assets/ARTnGAME/Toon Effects Maker/TEM_URP_FX/SpecialFX/Assets/Lightning"),
            set(&[URP])
        );
        assert_eq!(
            from_path("Assets/Artificer/Other Bits/Artificer Double Sided HDRP.unitypackage"),
            set(&[HDRP])
        );
        assert_eq!(
            from_path("Assets/Artificer/Other Bits/Artificer Builtin Double Sided.unitypackage"),
            set(&[BUILT_IN])
        );
        assert_eq!(
            from_path("Assets/Layer Lab/3D Characters Pro-Fantasy/Upgrade/Built_in.unitypackage"),
            set(&[BUILT_IN])
        );
        assert_eq!(
            from_path("Assets/Layer Lab/3D Characters Pro-Fantasy/Upgrade/URP_10+.unitypackage"),
            set(&[URP])
        );
        assert_eq!(
            from_path("Altos - Volumetric Clouds Skybox and Weather for Unity URP"),
            set(&[URP])
        );
    }

    #[test]
    fn multi_word_forms() {
        assert_eq!(from_path("Materials/Universal RP/Rock.mat"), set(&[URP]));
        assert_eq!(from_path("Demo/High Definition/Scene.unity"), set(&[HDRP]));
        assert_eq!(from_path("Upgrade/HD Render Pipeline"), set(&[HDRP]));
    }

    /// The reason matching is token-wise: these must all stay silent.
    #[test]
    fn substrings_do_not_match() {
        for p in [
            "Assets/Sounds/Burp.wav",
            "Assets/Textures/Slurp_01.png",
            "Assets/HDR/Skybox.exr",
            "Assets/Characters/Highlander.fbx",
            "Assets/Standard Assets/Water/Water.mat",
        ] {
            assert!(from_path(p).is_empty(), "{p} should name no pipeline");
        }
    }

    #[test]
    fn manifest_dependencies() {
        let urp = r#"{"dependencies":{"com.unity.render-pipelines.universal":"14.0.8"}}"#;
        let hdrp = r#"{"dependencies":{"com.unity.render-pipelines.high-definition":"14.0.8"}}"#;
        let core = r#"{"dependencies":{"com.unity.render-pipelines.core":"14.0.8"}}"#;
        assert_eq!(from_manifest(urp), set(&[URP]));
        assert_eq!(from_manifest(hdrp), set(&[HDRP]));
        assert!(
            from_manifest(core).is_empty(),
            "core alone names no pipeline"
        );
    }

    #[test]
    fn resolve_collapses() {
        assert_eq!(resolve(&set(&[])), None);
        assert_eq!(resolve(&set(&[URP])), Some(URP));
        assert_eq!(resolve(&set(&[URP, HDRP])), Some(MIXED));
        assert_eq!(resolve(&set(&[URP, HDRP, BUILT_IN])), Some(MIXED));
    }
}
