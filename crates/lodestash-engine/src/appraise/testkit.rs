//! What the appraisal tests share: embedders that need no model, and small real files of each
//! kind a stage reads. Everything is written under the caller's temp dir.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use lodestash_ai::embedder::Embedder;

use super::SharedEmbedder;

/// Stands in for a joint-space model. Text hashes into the space; a clip lands exactly on the
/// prompt it was told it sounds like, and an image on one fixed point — enough for the real
/// zero-shot scoring to run and pick a label, with no weights anywhere.
pub struct StubEmbedder {
    model: &'static str,
    /// `Some`: this is the audio tower, and every clip embeds as this text.
    sounds_like: Option<String>,
}

/// Wide enough that two different words do not share a bucket.
const DIM: usize = 512;

/// The prompt the audio zero-shot tagger wraps each label in. Dropped before hashing, so two
/// labels are as far apart as their own words.
const AUDIO_PROMPT: &str = "the sound of ";

impl Embedder for StubEmbedder {
    fn model_id(&self) -> &str {
        self.model
    }

    fn dim(&self) -> usize {
        DIM
    }

    fn embed_text(&self, text: &str) -> Vec<f32> {
        let mut v = vec![0.0f32; DIM];
        for token in text.trim_start_matches(AUDIO_PROMPT).split_whitespace() {
            // FNV-1a → bucket and sign: the feature-hashing trick `lodestash-ai`'s own stub uses.
            let mut h: u64 = 0xcbf2_9ce4_8422_2325;
            for b in token.to_lowercase().bytes() {
                h ^= u64::from(b);
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
            v[(h % DIM as u64) as usize] += if h & 1 == 0 { 1.0 } else { -1.0 };
        }
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            v.iter_mut().for_each(|x| *x /= norm);
        }
        v
    }

    fn embed_image(&self, _img: &image::RgbaImage) -> Vec<f32> {
        self.embed_text("a picture")
    }

    fn supports_image(&self) -> bool {
        self.sounds_like.is_none()
    }

    fn embed_audio(&self, _mel: &[f32], _frames: usize) -> Vec<f32> {
        match &self.sounds_like {
            Some(text) => self.embed_text(text),
            None => vec![0.0; DIM],
        }
    }

    fn supports_audio(&self) -> bool {
        self.sounds_like.is_some()
    }
}

/// The pixel tower: images, models and materials get a real, non-zero vector.
pub fn stub_siglip() -> SharedEmbedder {
    Arc::new(StubEmbedder {
        model: "stub-siglip",
        sounds_like: None,
    })
}

/// The audio tower. Every clip sounds like `label` — one of the audio zero-shot labels — so the
/// CLAP tag stage writes exactly that label.
pub fn stub_clap(label: &str) -> SharedEmbedder {
    Arc::new(StubEmbedder {
        model: "stub-clap",
        sounds_like: Some(label.to_string()),
    })
}

/// The association source the CLAP zero-shot tagger writes under.
pub const CLAP_SOURCE: &str = "ai:clap-zeroshot";

/// A second and a quarter of 48 kHz mono PCM — a clip the audio stages really decode.
pub fn write_wav(path: &Path) {
    const RATE: u32 = 48_000;
    let samples: Vec<i16> = (0..RATE * 5 / 4)
        .map(|i| ((i as f32 * 440.0 * std::f32::consts::TAU / RATE as f32).sin() * 12_000.0) as i16)
        .collect();
    let data_len = (samples.len() * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + data_len as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
    bytes.extend_from_slice(&RATE.to_le_bytes());
    bytes.extend_from_slice(&(RATE * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, bytes).expect("wav");
}

/// A model of two triangles.
pub const OBJ: &str = "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf 1 2 3\nf 1 3 4\n";

/// How many triangles [`OBJ`] has — what the metrics stage must record for it.
pub const OBJ_TRIANGLES: i64 = 2;

pub fn write_obj(path: &Path) {
    std::fs::write(path, OBJ).expect("obj");
}

/// A 16×8 picture.
pub fn write_png(path: &Path) {
    image::RgbaImage::from_pixel(16, 8, image::Rgba([120, 110, 100, 255]))
        .save(path)
        .expect("png");
}

/// A real `.unitypackage` at `dir/Pack.unitypackage` holding `entries` as `(guid, pathname,
/// payload)` — built with the tar and gzip crates ingest reads it with.
pub fn write_pack(dir: &Path, entries: &[(&str, &str, &[u8])]) -> PathBuf {
    let path = dir.join("Pack.unitypackage");
    let gz = flate2::write::GzEncoder::new(
        std::fs::File::create(&path).expect("pack"),
        flate2::Compression::default(),
    );
    let mut tar = tar::Builder::new(gz);
    for (guid, pathname, payload) in entries {
        let meta = format!("fileFormatVersion: 2\nguid: {guid}\n");
        for (member, body) in [
            ("asset", *payload),
            ("asset.meta", meta.as_bytes()),
            ("pathname", pathname.as_bytes()),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, format!("{guid}/{member}"), body)
                .expect("member");
        }
    }
    tar.into_inner().expect("tar").finish().expect("gzip");
    path
}
