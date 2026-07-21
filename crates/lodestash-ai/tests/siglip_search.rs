//! End-to-end SigLIP visual search, mirroring the app's persist-thread drain: seed image
//! assets with NEUTRAL names, embed their pixels with the vision tower, then run `hybrid_search`
//! with a text query that appears in NO name — so a correct ranking can only come from the
//! joint-space vision KNN arm, not BM25. Proves "find assets that look like X".
//!
//! `#[ignore]` (needs the towers + ORT):
//! `SIGLIP_VISION=… SIGLIP_TEXT=… SIGLIP_TOKENIZER=… ORT_DYLIB_PATH=… \
//!  cargo test -p lodestash-ai --test siglip_search -- --ignored --nocapture`

use std::path::{Path, PathBuf};

use lodestash_ai::embedder::Embedder;
use lodestash_ai::siglip::SiglipEmbedder;
use lodestash_core::catalog::{Catalog, CrawlRecord};

fn write_solid(path: &Path, r: u8, g: u8, b: u8) {
    image::RgbImage::from_pixel(48, 48, image::Rgb([r, g, b]))
        .save(path)
        .expect("write png");
}

#[test]
#[ignore]
fn visual_search_ranks_by_appearance() {
    let v = std::env::var("SIGLIP_VISION").expect("SIGLIP_VISION");
    let t = std::env::var("SIGLIP_TEXT").expect("SIGLIP_TEXT");
    let tok = std::env::var("SIGLIP_TOKENIZER").expect("SIGLIP_TOKENIZER");
    let emb =
        SiglipEmbedder::load(Path::new(&v), Path::new(&t), Path::new(&tok), "siglip2-base").expect("load");

    // A private temp dir as the "library root".
    let root: PathBuf = std::env::temp_dir().join(format!("lodestash_siglip_{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    write_solid(&root.join("asset_a.png"), 210, 30, 30); // red
    write_solid(&root.join("asset_b.png"), 30, 160, 40); // green

    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let sid = c
        .upsert_import_source("Lib", &root.to_string_lossy(), "folder")
        .expect("src");
    let rec = |rel: &str, name: &str| CrawlRecord {
        rel_path: rel.into(),
        name: name.into(), // neutral names — "red"/"green" appear nowhere
        media_type: "image".into(),
        size_bytes: 1,
        mtime: 1,
        workspace_rel: None,
    };
    c.record_crawl(sid, None, &[rec("asset_a.png", "asset_a"), rec("asset_b.png", "asset_b")])
        .expect("crawl");

    // Drain image assets exactly as the app does (decode → vision embed → store).
    let jobs = c.image_assets_needing_embedding(100).expect("queue");
    assert_eq!(jobs.len(), 2, "both images dirty");
    let disk = |j: &lodestash_core::catalog::ImageEmbedJob| j.disk_path.clone().unwrap();
    let red_id = jobs.iter().find(|j| disk(j).ends_with("asset_a.png")).unwrap().asset_id;
    for job in jobs {
        let img = image::open(disk(&job)).expect("decode").to_rgba8();
        let vec = emb.embed_image(&img);
        c.upsert_embedding(job.asset_id, emb.model_id(), "image", Some(job.media_type_id), Some(job.source_id), &vec)
            .expect("store");
    }

    // Query text that matches NO name → only the vision arm can rank.
    let qv = emb.embed_text("a red square");
    let ids = c
        .hybrid_search("a red square", Some(&qv), emb.model_id(), None, 10)
        .expect("hybrid");
    println!("visual-search ids for 'a red square': {ids:?} (red={red_id})");
    let _ = std::fs::remove_dir_all(&root);

    assert_eq!(ids.first(), Some(&red_id), "the red image must rank first for 'a red square'");
}
