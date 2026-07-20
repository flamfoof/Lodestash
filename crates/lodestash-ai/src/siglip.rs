//! SigLIP 2 joint image↔text embedder (AI-PIPELINE §3, IDENTIFICATION-AND-SEARCH §5.B, Tier 1):
//! the real "Search with AI" vector space. Two ONNX towers share a 768-d joint space — a text
//! query and an asset image land in the same space, so `cosine(query_text, asset_image)` ranks
//! assets by what they *look* like, not just their names.
//!
//! I/O (from the `onnx-community/siglip2-base-patch16-256-ONNX` towers, probed directly):
//!   vision `pixel_values` f32 `[b,3,256,256]` → `pooler_output` `[b,768]`
//!   text   `input_ids`    i64 `[b,seq]`       → `pooler_output` `[b,768]`  (no attention mask)
//!
//! Preprocessing is SigLIP-exact (`preprocessor_config.json`): resize to 256×256 (bilinear,
//! squash — no crop), rescale 1/255, normalise mean/std 0.5 → **[-1,1]**. Text pads/truncates to
//! a fixed 64 (SigLIP pools the last position of a max-length-padded sequence), pad id 0.

use std::path::Path;
use std::sync::Mutex;

use image::RgbaImage;
use ndarray::{Array2, Array4};
use ort::session::Session;
use ort::value::Tensor;
use tokenizers::Tokenizer;

use crate::embedder::Embedder;

const IMAGE_SIZE: u32 = 256;
const TEXT_LEN: usize = 64; // SigLIP max_length padding; the pooler reads the last position.
const PAD_ID: i64 = 0; // <pad> per tokenizer_config.json.
const DIM: usize = 768; // SigLIP 2 base joint-space width.

pub struct SiglipEmbedder {
    // `Session::run` needs `&mut`; the trait is `&self`. One Mutex per tower — the query side
    // (text) and the embed-worker (vision) run on different threads and never block each other.
    vision: Mutex<Session>,
    text: Mutex<Session>,
    tokenizer: Tokenizer,
    model_id: String,
}

impl SiglipEmbedder {
    /// Load the vision + text towers and the tokenizer. `Err` (never panic) if the ONNX Runtime
    /// is missing/incompatible or a file doesn't parse — the caller falls back to no-AI.
    pub fn load(
        vision_path: &Path,
        text_path: &Path,
        tokenizer_path: &Path,
        model_id: impl Into<String>,
    ) -> Result<Self, String> {
        // ort *panics* on a missing/incompatible runtime (rc.10 wants ORT 1.22.x); catch it so a
        // bad runtime is a clean Err, not a crashed thread.
        let build = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let vision = Session::builder()?.commit_from_file(vision_path)?;
            let text = Session::builder()?.commit_from_file(text_path)?;
            Ok::<_, ort::Error>((vision, text))
        }))
        .map_err(|_| "ONNX Runtime unavailable or incompatible (ort rc.10 needs ORT 1.22.x)".to_string())?;
        let (vision, text) = build.map_err(|e| format!("load SigLIP towers: {e}"))?;
        let tokenizer = Tokenizer::from_file(tokenizer_path).map_err(|e| format!("tokenizer: {e}"))?;
        Ok(Self {
            vision: Mutex::new(vision),
            text: Mutex::new(text),
            tokenizer,
            model_id: model_id.into(),
        })
    }

    fn try_text(&self, text: &str) -> Result<Vec<f32>, String> {
        let enc = self.tokenizer.encode(text, true).map_err(|e| format!("encode: {e}"))?;
        // Pad/truncate to exactly TEXT_LEN (SigLIP was trained with max_length=64 padding and
        // pools the final position, so the length must be fixed).
        let mut ids: Vec<i64> = enc.get_ids().iter().take(TEXT_LEN).map(|&x| x as i64).collect();
        ids.resize(TEXT_LEN, PAD_ID);
        let arr = Array2::from_shape_vec((1, TEXT_LEN), ids).map_err(|e| e.to_string())?;
        let input = Tensor::from_array(arr).map_err(|e| e.to_string())?;
        let mut sess = self.text.lock().map_err(|e| e.to_string())?;
        let out = sess.run(vec![("input_ids", input)]).map_err(|e| format!("text run: {e}"))?;
        pooled(&out)
    }

    fn try_image(&self, img: &RgbaImage) -> Result<Vec<f32>, String> {
        // Resize to a square 256×256 (bilinear ≈ Triangle; SigLIP squashes, no crop).
        let dyn_img = image::DynamicImage::ImageRgba8(img.clone());
        let resized = dyn_img
            .resize_exact(IMAGE_SIZE, IMAGE_SIZE, image::imageops::FilterType::Triangle)
            .to_rgb8();
        // CHW, normalise [0,255] → [-1,1]  ((x/255 - 0.5)/0.5 == x/127.5 - 1).
        let n = (IMAGE_SIZE * IMAGE_SIZE) as usize;
        let mut chw = vec![0.0f32; 3 * n];
        for (i, px) in resized.pixels().enumerate() {
            chw[i] = px[0] as f32 / 127.5 - 1.0; // R plane
            chw[n + i] = px[1] as f32 / 127.5 - 1.0; // G plane
            chw[2 * n + i] = px[2] as f32 / 127.5 - 1.0; // B plane
        }
        let arr = Array4::from_shape_vec((1, 3, IMAGE_SIZE as usize, IMAGE_SIZE as usize), chw)
            .map_err(|e| e.to_string())?;
        let input = Tensor::from_array(arr).map_err(|e| e.to_string())?;
        let mut sess = self.vision.lock().map_err(|e| e.to_string())?;
        let out = sess.run(vec![("pixel_values", input)]).map_err(|e| format!("vision run: {e}"))?;
        pooled(&out)
    }
}

/// Extract `pooler_output` `[1,768]` and L2-normalise it (so cosine == dot in the joint space).
fn pooled(out: &ort::session::SessionOutputs) -> Result<Vec<f32>, String> {
    let t = out.get("pooler_output").ok_or("no pooler_output")?;
    let (_shape, data) = t.try_extract_tensor::<f32>().map_err(|e| format!("extract: {e}"))?;
    let mut v = data.to_vec();
    l2_normalise(&mut v);
    Ok(v)
}

impl Embedder for SiglipEmbedder {
    fn model_id(&self) -> &str {
        &self.model_id
    }
    fn dim(&self) -> usize {
        DIM
    }
    fn embed_text(&self, text: &str) -> Vec<f32> {
        self.try_text(text).unwrap_or_else(|e| {
            tracing::warn!(%e, "siglip text embed failed");
            vec![0.0; DIM]
        })
    }
    fn embed_image(&self, img: &RgbaImage) -> Vec<f32> {
        self.try_image(img).unwrap_or_else(|e| {
            tracing::warn!(%e, "siglip image embed failed");
            vec![0.0; DIM]
        })
    }
    fn supports_image(&self) -> bool {
        true
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

    fn solid(r: u8, g: u8, b: u8) -> RgbaImage {
        RgbaImage::from_pixel(64, 64, image::Rgba([r, g, b, 255]))
    }

    /// Real-model joint-space check: a text query lands nearer the image it describes than an
    /// unrelated one. Needs the towers + ORT, so `#[ignore]`:
    /// `SIGLIP_VISION=… SIGLIP_TEXT=… SIGLIP_TOKENIZER=… cargo test -p lodestash-ai siglip -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn text_matches_image_in_joint_space() {
        let v = std::env::var("SIGLIP_VISION").expect("SIGLIP_VISION");
        let t = std::env::var("SIGLIP_TEXT").expect("SIGLIP_TEXT");
        let tok = std::env::var("SIGLIP_TOKENIZER").expect("SIGLIP_TOKENIZER");
        let emb = SiglipEmbedder::load(Path::new(&v), Path::new(&t), Path::new(&tok), "siglip2-base")
            .expect("load");

        let cos = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
        let red = emb.embed_image(&solid(220, 30, 30));
        let green = emb.embed_image(&solid(30, 200, 60));
        let q_red = emb.embed_text("a solid red image");

        let s_red = cos(&q_red, &red);
        let s_green = cos(&q_red, &green);
        println!("dim={} sim(red_q,red)={s_red:.3} sim(red_q,green)={s_green:.3}", red.len());
        assert_eq!(red.len(), DIM);
        assert!(s_red > s_green, "'red' query should match the red image over the green one");
    }
}
