//! Live check against a running Ollama daemon. `#[ignore]`d and env-gated like the model-backed
//! embedder tests, so a normal `cargo test` never needs a model installed.
//!
//! ```bash
//! OLLAMA_MODEL=qwen3:8b            cargo test -p lodestash-ai --test ollama_tagger -- --ignored --nocapture
//! OLLAMA_MODEL=moondream VISION=1  cargo test -p lodestash-ai --test ollama_tagger -- --ignored --nocapture
//! ```

use lodestash_ai::ollama::{OllamaTagger, DEFAULT_HOST};
use lodestash_ai::tagger::{validate, TagInput, Tagger};

fn tagger() -> OllamaTagger {
    let model = std::env::var("OLLAMA_MODEL").expect("set OLLAMA_MODEL");
    let vision = std::env::var("VISION").is_ok();
    OllamaTagger::new(model, vision)
}

#[test]
#[ignore = "needs a running Ollama daemon"]
fn lists_installed_models() {
    let models = OllamaTagger::installed_models(DEFAULT_HOST).expect("daemon reachable");
    println!("installed: {models:?}");
    assert!(!models.is_empty());
}

/// A discrimination matrix, not one lucky answer: four asset names with distinct meanings, each
/// expected to pull its own term **and none of the others'**. A tagger that answers generically
/// ("game", "asset") or echoes the prompt fails the off-diagonal even when it passes its own row.
#[test]
#[ignore = "needs a running Ollama daemon"]
fn names_produce_distinguishing_facets() {
    let t = tagger();
    // (asset name, media_code, the term that identifies this row)
    let cases: &[(&str, &str, &str)] = &[
        ("SM_Chr_Ghost_01", "model_3d", "ghost"),
        ("SM_Wep_Sword_Rusty_02", "model_3d", "sword"),
        ("Ambient_Forest_Loop", "audio", "forest"),
        ("UI_Button_Hover", "image", "button"),
    ];

    let mut rows = Vec::new();
    for (name, media, _) in cases {
        let tags = validate(t.tag(&TagInput {
            media_code: media,
            name,
            image: None,
        }));
        let flat: Vec<String> = tags
            .iter()
            .map(|x| format!("{}={}", x.namespace, x.value))
            .collect();
        println!("{name:26} -> {}", flat.join(", "));
        rows.push(flat.join(" ").to_lowercase());
    }

    for (i, (name, _, expected)) in cases.iter().enumerate() {
        assert!(
            rows[i].contains(expected),
            "{name}: expected {expected:?} somewhere in {:?}",
            rows[i]
        );
        // Off-diagonal: this row must not carry another row's identifying term.
        for (j, (_, _, other)) in cases.iter().enumerate() {
            if i != j {
                assert!(
                    !rows[i].contains(other),
                    "{name} picked up {other:?} — the rows are not distinguishable"
                );
            }
        }
    }
}
