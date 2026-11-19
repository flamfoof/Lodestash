//! The local embedders — SigLIP for pixels, CLAP for audio — and the query side that embeds a
//! search text into every space the library was written in.
use std::sync::Arc;

use crate::models;
use crate::paths::*;

/// Load the SigLIP joint image↔text embedder if its artifacts (+ a matching ONNX Runtime,
/// discovered via `load-dynamic`) are present. Any failure is `None` — AI search falls back to
/// its stub, never crashes. On a stock machine with no model this returns `None` immediately.
pub fn resolve_embedder() -> Option<Arc<dyn lodestash_ai::embedder::Embedder + Send + Sync>> {
    let (vision, text, tok) = siglip_model_paths()?;
    match lodestash_ai::siglip::SiglipEmbedder::load(&vision, &text, &tok, SIGLIP_MODEL_ID) {
        Ok(e) => {
            tracing::info!("siglip embedder loaded — Search-with-AI (visual) is live");
            Some(Arc::new(e))
        }
        Err(e) => {
            tracing::info!(%e, "no local siglip model; Search-with-AI stays stubbed");
            None
        }
    }
}

/// Model id for the audio space. Kept distinct from the visual one so the two never mix in
/// `asset_embedding` — `embeddings_for_knn` shards on `model_id`.
pub const CLAP_MODEL_ID: &str = "clap-htsat-unfused";

/// Load the CLAP audio embedder if its artifacts are present, else `None` (audio simply gets no
/// vectors — visual search is unaffected). Mirrors `resolve_embedder`.
pub fn resolve_audio_embedder() -> Option<Arc<dyn lodestash_ai::embedder::Embedder + Send + Sync>> {
    let dir = models::clap_dir()?;
    let (a, t, k) = (
        dir.join("audio.onnx"),
        dir.join("text.onnx"),
        dir.join("tokenizer.json"),
    );
    if !(a.exists() && t.exists() && k.exists()) {
        return None;
    }
    match lodestash_ai::clap::ClapEmbedder::load(&a, &t, &k, CLAP_MODEL_ID) {
        Ok(e) => {
            tracing::info!("clap embedder loaded — audio is searchable");
            Some(Arc::new(e))
        }
        Err(e) => {
            tracing::info!(%e, "no local clap model; audio stays unsearchable");
            None
        }
    }
}

/// A text query embedded into every local space the library was written in: SigLIP's for pixels,
/// CLAP's for audio (IDENTIFICATION-AND-SEARCH §B "joint SigLIP/CLAP space", v1 plan S11).
///
/// Both search surfaces used to embed the query in SigLIP's space only, so audio — whose vectors
/// are CLAP's — could be found by name and never by description, while the MCP tool promised
/// "metallic clang". One function, so the grid and the agent planes cannot drift apart again.
/// Empty when neither model is installed; a space whose model is missing is simply absent.
///
/// Each embedder is resolved once and kept. A miss is retried after [`EMBEDDER_RETRY`], so models
/// installed while the app runs become searchable without a restart, and a missing model does not
/// cost a failed load on every query.
pub fn query_spaces(query: &str) -> Vec<(String, Vec<f32>)> {
    static VISUAL: EmbedderSlot = EmbedderSlot::new();
    static AUDIO: EmbedderSlot = EmbedderSlot::new();
    [
        VISUAL.get(resolve_embedder),
        AUDIO.get(resolve_audio_embedder),
    ]
    .into_iter()
    .flatten()
    .filter_map(|e| {
        let v = e.embed_text(query);
        // A stub embedder answers empty: it has no text space worth searching.
        (!v.is_empty()).then(|| (e.model_id().to_string(), v))
    })
    .collect()
}

/// How long a failed embedder resolve is remembered before [`query_spaces`] tries again.
pub const EMBEDDER_RETRY: std::time::Duration = std::time::Duration::from_secs(60);

/// An embedder a pass and its workers share.
pub type SharedEmbedder = Arc<dyn lodestash_ai::embedder::Embedder + Send + Sync>;

type Resolve = Arc<dyn Fn() -> Option<SharedEmbedder> + Send + Sync>;

/// Where the engine gets its embedders: the models installed on this machine
/// ([`Models::installed`]), or whatever a test hands it. Resolving loads the model, so callers
/// resolve once per pass (a kick) or once per run (the idle lane) and keep the result.
#[derive(Clone)]
pub struct Models {
    visual: Resolve,
    audio: Resolve,
}

impl Models {
    /// SigLIP and CLAP from the model directory, when their files and a runtime are there.
    pub fn installed() -> Self {
        Self {
            visual: Arc::new(resolve_embedder),
            audio: Arc::new(resolve_audio_embedder),
        }
    }

    /// These embedders, whatever is installed — `None` for a space with no model.
    pub fn fixed(visual: Option<SharedEmbedder>, audio: Option<SharedEmbedder>) -> Self {
        Self {
            visual: Arc::new(move || visual.clone()),
            audio: Arc::new(move || audio.clone()),
        }
    }

    /// The pixel space (images, models, materials).
    pub fn visual(&self) -> Option<SharedEmbedder> {
        (self.visual)()
    }

    /// The audio space.
    pub fn audio(&self) -> Option<SharedEmbedder> {
        (self.audio)()
    }
}

struct EmbedderSlot(std::sync::Mutex<(Option<SharedEmbedder>, Option<std::time::Instant>)>);

impl EmbedderSlot {
    const fn new() -> Self {
        Self(std::sync::Mutex::new((None, None)))
    }

    /// The loaded embedder, resolving it on first use or once a remembered miss has aged out. The
    /// lock is held across the load so concurrent first queries wait for one load, not start two.
    fn get(&self, resolve: fn() -> Option<SharedEmbedder>) -> Option<SharedEmbedder> {
        let mut slot = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if slot.0.is_none() && slot.1.is_none_or(|t| t.elapsed() >= EMBEDDER_RETRY) {
            slot.0 = resolve();
            slot.1 = Some(std::time::Instant::now());
        }
        slot.0.clone()
    }
}
