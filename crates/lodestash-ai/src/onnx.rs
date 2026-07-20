//! Local ONNX text embedder (AI-PIPELINE §3, IDENTIFICATION-AND-SEARCH §5.B) — the real
//! `Embedder` behind the query side of Search-with-AI, via `ort` (ONNX Runtime).
//!
//! `ort` is built with `load-dynamic`, so the ONNX Runtime shared library is discovered at
//! runtime (not linked at build). If it's absent, `load` fails cleanly and the caller falls
//! back to the stub embedder — nothing panics.
//!
//! Model contract: a sentence-transformers-style encoder exporting `last_hidden_state`
//! `[batch, seq, hidden]` for `input_ids` / `attention_mask` / `token_type_ids`; we
//! mask-mean-pool the token states and L2-normalise (the standard sentence embedding).

use std::path::Path;
use std::sync::Mutex;

use ndarray::Array2;
use ort::session::Session;
use ort::value::Tensor;
use tokenizers::Tokenizer;

use crate::embedder::Embedder;

/// Longest token sequence fed to the model (queries are short; this bounds cost).
const MAX_TOKENS: usize = 128;

pub struct OnnxTextEmbedder {
    // `Session::run` needs `&mut`; the trait is `&self`, and the embed worker is one thread,
    // so a Mutex is the cheapest way to satisfy both.
    session: Mutex<Session>,
    tokenizer: Tokenizer,
    model_id: String,
    dim: usize,
}

impl OnnxTextEmbedder {
    /// Load an ONNX model + its `tokenizer.json`. `Err` if the ONNX Runtime library is
    /// missing, the files don't parse, or the model can't be built.
    pub fn load(
        model_path: &Path,
        tokenizer_path: &Path,
        model_id: impl Into<String>,
        dim: usize,
    ) -> Result<Self, String> {
        // ort *panics* if the ONNX Runtime library is missing or a version it doesn't
        // support (rc.10 wants ORT 1.22.x). Catch it so a bad runtime is a clean Err the
        // caller can fall back from, never a crashed embed thread.
        let build = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Session::builder()?.commit_from_file(model_path)
        }))
        .map_err(|_| {
            "ONNX Runtime unavailable or incompatible (ort rc.10 needs ORT 1.22.x)".to_string()
        })?;
        let session = build.map_err(|e| format!("load {}: {e}", model_path.display()))?;
        let tokenizer =
            Tokenizer::from_file(tokenizer_path).map_err(|e| format!("tokenizer: {e}"))?;
        Ok(Self {
            session: Mutex::new(session),
            tokenizer,
            model_id: model_id.into(),
            dim,
        })
    }

    fn try_embed(&self, text: &str) -> Result<Vec<f32>, String> {
        let enc = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| format!("encode: {e}"))?;
        let ids: Vec<i64> = enc
            .get_ids()
            .iter()
            .take(MAX_TOKENS)
            .map(|&x| x as i64)
            .collect();
        let mask: Vec<i64> = enc
            .get_attention_mask()
            .iter()
            .take(MAX_TOKENS)
            .map(|&x| x as i64)
            .collect();
        let n = ids.len();
        if n == 0 {
            return Ok(vec![0.0; self.dim]);
        }
        let type_ids = vec![0i64; n];

        let id_arr = Array2::from_shape_vec((1, n), ids).map_err(|e| e.to_string())?;
        let mask_arr = Array2::from_shape_vec((1, n), mask.clone()).map_err(|e| e.to_string())?;
        let type_arr = Array2::from_shape_vec((1, n), type_ids).map_err(|e| e.to_string())?;

        let inputs = vec![
            (
                "input_ids",
                Tensor::from_array(id_arr).map_err(|e| e.to_string())?,
            ),
            (
                "attention_mask",
                Tensor::from_array(mask_arr).map_err(|e| e.to_string())?,
            ),
            (
                "token_type_ids",
                Tensor::from_array(type_arr).map_err(|e| e.to_string())?,
            ),
        ];

        let mut session = self.session.lock().map_err(|e| e.to_string())?;
        let outputs = session.run(inputs).map_err(|e| format!("run: {e}"))?;
        let hidden_state = outputs
            .get("last_hidden_state")
            .ok_or("model has no `last_hidden_state` output")?;
        let (_shape, data) = hidden_state
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("extract: {e}"))?;

        // data = [1, n, hidden] row-major; hidden inferred so we don't index Shape.
        let hidden = data.len() / n;
        let mut pooled = vec![0.0f32; hidden];
        let mut count = 0.0f32;
        for (t, &m) in mask.iter().enumerate().take(n) {
            if m == 0 {
                continue;
            }
            count += 1.0;
            let row = &data[t * hidden..(t + 1) * hidden];
            for (p, &v) in pooled.iter_mut().zip(row) {
                *p += v;
            }
        }
        if count > 0.0 {
            for p in &mut pooled {
                *p /= count;
            }
        }
        l2_normalise(&mut pooled);
        Ok(pooled)
    }
}

impl Embedder for OnnxTextEmbedder {
    fn model_id(&self) -> &str {
        &self.model_id
    }
    fn dim(&self) -> usize {
        self.dim
    }
    fn embed_text(&self, text: &str) -> Vec<f32> {
        // Never panics the caller — a failed inference degrades to the zero vector (which
        // cosine-scores 0 everywhere) and a warning.
        self.try_embed(text).unwrap_or_else(|e| {
            tracing::warn!(%e, "onnx text embedding failed");
            vec![0.0; self.dim]
        })
    }
}

fn l2_normalise(v: &mut [f32]) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l2_normalise_makes_unit_length() {
        let mut v = vec![3.0, 4.0];
        l2_normalise(&mut v);
        assert!((v[0] - 0.6).abs() < 1e-6 && (v[1] - 0.8).abs() < 1e-6);
        let mut z = vec![0.0, 0.0];
        l2_normalise(&mut z); // zero vector stays zero, never NaN
        assert_eq!(z, vec![0.0, 0.0]);
    }

    /// A missing model (or an unavailable/incompatible ONNX Runtime) must be a clean `Err`,
    /// never a panic. Requires ort init, so it's `#[ignore]` — the version guard above
    /// converts the runtime panic to an Err; run it where a matching ORT is installed.
    #[test]
    #[ignore]
    fn missing_model_fails_cleanly() {
        let err = OnnxTextEmbedder::load(
            Path::new("Z:/nope/model.onnx"),
            Path::new("Z:/nope/tokenizer.json"),
            "x",
            384,
        );
        assert!(err.is_err(), "a missing model must Err, not panic");
    }

    /// Real-model probe: point at an exported sentence encoder + tokenizer and run it.
    /// `LODESTASH_ONNX_MODEL=model.onnx LODESTASH_ONNX_TOKENIZER=tokenizer.json \
    ///  cargo test -p lodestash-ai probe -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn probe_real_model() {
        let model = std::env::var("LODESTASH_ONNX_MODEL").expect("set LODESTASH_ONNX_MODEL");
        let tok = std::env::var("LODESTASH_ONNX_TOKENIZER").expect("set LODESTASH_ONNX_TOKENIZER");
        let emb = OnnxTextEmbedder::load(Path::new(&model), Path::new(&tok), "probe", 384)
            .expect("load");
        let a = emb.embed_text("a low-poly stylized rock");
        let b = emb.embed_text("a stylized boulder, low poly");
        let c = emb.embed_text("orchestral battle music");
        let cos = |x: &[f32], y: &[f32]| x.iter().zip(y).map(|(p, q)| p * q).sum::<f32>();
        println!("dim={} sim(rock,boulder)={:.3} sim(rock,music)={:.3}", a.len(), cos(&a, &b), cos(&a, &c));
        assert!(cos(&a, &b) > cos(&a, &c), "related text is closer than unrelated");
    }
}
