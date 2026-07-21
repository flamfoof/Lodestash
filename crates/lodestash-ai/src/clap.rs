//! CLAP joint audio↔text embedder (AI-PIPELINE §3, IDENTIFICATION-AND-SEARCH §3 Tier 1 /
//! §5.B "joint SigLIP/CLAP space") — the audio half of Search-with-AI, so "a heavy metal door
//! slam" finds the sound that *is* one rather than the file that happens to say so.
//!
//! CLAP is a **separate 512-d space** from SigLIP's 768-d visual space; the two never mix. A text
//! query is embedded into *both* and each space is searched independently, then fused (RRF).
//!
//! I/O (from the `Xenova/clap-htsat-unfused` towers, probed directly):
//!   audio `input_features` f32 `[b,1,frames,mels]` → `audio_embeds` `[b,512]`
//!   text  `input_ids`      i64 `[b,seq]`          → `text_embeds`  `[b,512]`
//!
//! Neither tower takes an attention mask. That's fine here because we embed **one** text at a
//! time, so the sequence needs no padding — `seq` is simply the token count.
//!
//! The log-mel input must match `ClapFeatureExtractor` exactly; that lives in
//! `lodestash_ingest::melspec` (which documents the reference it was built from).

use std::path::Path;
use std::sync::Mutex;

use ndarray::{Array2, Array4};
use ort::session::Session;
use ort::value::Tensor;
use tokenizers::Tokenizer;

use crate::embedder::Embedder;

/// CLAP's joint projection width (`projection_dim`).
const DIM: usize = 512;

pub struct ClapEmbedder {
    audio: Mutex<Session>,
    text: Mutex<Session>,
    tokenizer: Tokenizer,
    model_id: String,
}

impl ClapEmbedder {
    /// Load the audio + text towers and the tokenizer. `Err` (never panic) if the ONNX Runtime is
    /// missing/incompatible or a file doesn't parse — the caller falls back to no audio search.
    pub fn load(
        audio_path: &Path,
        text_path: &Path,
        tokenizer_path: &Path,
        model_id: impl Into<String>,
    ) -> Result<Self, String> {
        // ort panics on a missing/incompatible runtime (rc.10 wants ORT 1.22.x) — catch it so a
        // bad runtime is a clean Err, not a crashed thread.
        let build = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let audio = Session::builder()?.commit_from_file(audio_path)?;
            let text = Session::builder()?.commit_from_file(text_path)?;
            Ok::<_, ort::Error>((audio, text))
        }))
        .map_err(|_| {
            "ONNX Runtime unavailable or incompatible (ort rc.10 needs ORT 1.22.x)".to_string()
        })?;
        let (audio, text) = build.map_err(|e| format!("load CLAP towers: {e}"))?;
        let tokenizer =
            Tokenizer::from_file(tokenizer_path).map_err(|e| format!("tokenizer: {e}"))?;
        Ok(Self {
            audio: Mutex::new(audio),
            text: Mutex::new(text),
            tokenizer,
            model_id: model_id.into(),
        })
    }

    fn try_text(&self, text: &str) -> Result<Vec<f32>, String> {
        let enc = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| format!("encode: {e}"))?;
        let ids: Vec<i64> = enc.get_ids().iter().map(|&x| x as i64).collect();
        if ids.is_empty() {
            return Ok(vec![0.0; DIM]);
        }
        let arr = Array2::from_shape_vec((1, ids.len()), ids).map_err(|e| e.to_string())?;
        let input = Tensor::from_array(arr).map_err(|e| e.to_string())?;
        let mut sess = self.text.lock().map_err(|e| e.to_string())?;
        let out = sess
            .run(vec![("input_ids", input)])
            .map_err(|e| format!("clap text run: {e}"))?;
        extract(&out, "text_embeds")
    }

    fn try_audio(&self, mel: &[f32], frames: usize) -> Result<Vec<f32>, String> {
        if frames == 0 || mel.is_empty() || mel.len() % frames != 0 {
            return Err("malformed mel spectrogram".into());
        }
        let mels = mel.len() / frames;
        // [batch, channels, frames, mels]
        let arr = Array4::from_shape_vec((1, 1, frames, mels), mel.to_vec())
            .map_err(|e| e.to_string())?;
        let input = Tensor::from_array(arr).map_err(|e| e.to_string())?;
        let mut sess = self.audio.lock().map_err(|e| e.to_string())?;
        let out = sess
            .run(vec![("input_features", input)])
            .map_err(|e| format!("clap audio run: {e}"))?;
        extract(&out, "audio_embeds")
    }
}

/// Pull a named `[1, 512]` output and L2-normalise it, so cosine == dot in the joint space.
fn extract(out: &ort::session::SessionOutputs, name: &str) -> Result<Vec<f32>, String> {
    let t = out.get(name).ok_or_else(|| format!("no `{name}` output"))?;
    let (_shape, data) = t
        .try_extract_tensor::<f32>()
        .map_err(|e| format!("extract {name}: {e}"))?;
    let mut v = data.to_vec();
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in &mut v {
            *x /= norm;
        }
    }
    Ok(v)
}

impl Embedder for ClapEmbedder {
    fn model_id(&self) -> &str {
        &self.model_id
    }
    fn dim(&self) -> usize {
        DIM
    }
    fn embed_text(&self, text: &str) -> Vec<f32> {
        self.try_text(text).unwrap_or_else(|e| {
            tracing::warn!(%e, "clap text embed failed");
            vec![0.0; DIM]
        })
    }
    fn embed_audio(&self, mel: &[f32], frames: usize) -> Vec<f32> {
        self.try_audio(mel, frames).unwrap_or_else(|e| {
            tracing::warn!(%e, "clap audio embed failed");
            vec![0.0; DIM]
        })
    }
    fn supports_audio(&self) -> bool {
        true
    }
}
