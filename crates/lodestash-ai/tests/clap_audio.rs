//! CLAP joint audio↔text validation: synthesise acoustically unambiguous sounds, embed them
//! through the real mel front end + audio tower, and check the **full similarity matrix** —
//! every text description must match its own sound better than the others.
//!
//! A matrix is the point: a single ordering can pass with a subtly wrong front end, whereas a
//! diagonal that wins across several distinct timbres is hard to get right by accident.
//!
//! `#[ignore]` (needs the towers + ORT):
//! `CLAP_AUDIO=… CLAP_TEXT=… CLAP_TOKENIZER=… ORT_DYLIB_PATH=… \
//!  cargo test -p lodestash-ai --test clap_audio -- --ignored --nocapture`

use std::path::Path;

use lodestash_ai::clap::ClapEmbedder;
use lodestash_ai::embedder::Embedder;
use lodestash_ingest::melspec::{log_mel_spectrogram, SAMPLE_RATE};

/// 3 s of a pure sine at `hz`.
fn tone(hz: f64) -> Vec<f32> {
    let n = SAMPLE_RATE as usize * 3;
    (0..n)
        .map(|i| {
            (2.0 * std::f64::consts::PI * hz * i as f64 / SAMPLE_RATE as f64).sin() as f32 * 0.8
        })
        .collect()
}

/// 3 s of deterministic white noise (xorshift, so the test never flakes).
fn noise() -> Vec<f32> {
    let n = SAMPLE_RATE as usize * 3;
    let mut h: u64 = 0x2545_F491_4F6C_DD1D;
    (0..n)
        .map(|_| {
            h ^= h << 13;
            h ^= h >> 7;
            h ^= h << 17;
            ((h as u32 as f64 / u32::MAX as f64) as f32 - 0.5) * 1.6
        })
        .collect()
}

#[test]
#[ignore]
fn text_descriptions_match_their_own_sounds() {
    let a = std::env::var("CLAP_AUDIO").expect("CLAP_AUDIO");
    let t = std::env::var("CLAP_TEXT").expect("CLAP_TEXT");
    let tok = std::env::var("CLAP_TOKENIZER").expect("CLAP_TOKENIZER");
    let emb = ClapEmbedder::load(
        Path::new(&a),
        Path::new(&t),
        Path::new(&tok),
        "clap-htsat-unfused",
    )
    .expect("load");
    assert!(emb.supports_audio());
    assert_eq!(emb.dim(), 512);

    // Acoustically very different: a high whistle, a deep hum, and broadband hiss.
    let sounds: Vec<(&str, Vec<f32>)> = vec![
        ("high beep", tone(3000.0)),
        ("low hum", tone(60.0)),
        ("white noise", noise()),
    ];
    let queries = [
        "a high pitched beep tone",
        "a deep low frequency hum",
        "loud static white noise hiss",
    ];

    let audio_vecs: Vec<Vec<f32>> = sounds
        .iter()
        .map(|(name, samples)| {
            let (frames, mel) = log_mel_spectrogram(samples);
            let v = emb.embed_audio(&mel, frames);
            assert_eq!(v.len(), 512, "{name}: wrong embedding width");
            assert!(v.iter().any(|x| *x != 0.0), "{name}: all-zero embedding");
            v
        })
        .collect();

    let cos = |x: &[f32], y: &[f32]| x.iter().zip(y).map(|(a, b)| a * b).sum::<f32>();

    println!(
        "\n            {:>12} {:>12} {:>12}",
        sounds[0].0, sounds[1].0, sounds[2].0
    );
    let mut correct = 0;
    for (qi, q) in queries.iter().enumerate() {
        let qv = emb.embed_text(q);
        let sims: Vec<f32> = audio_vecs.iter().map(|av| cos(&qv, av)).collect();
        let best = sims
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap();
        println!(
            "{:<28} {:>12.4} {:>12.4} {:>12.4}   → {}",
            q, sims[0], sims[1], sims[2], sounds[best].0
        );
        if best == qi {
            correct += 1;
        }
    }
    println!();

    assert_eq!(
        correct,
        queries.len(),
        "every description should match its own sound; only {correct}/{} did",
        queries.len()
    );
}
