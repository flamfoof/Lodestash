//! End-to-end 3D visual search: real `.obj` files on disk → mesh load → CPU multiview render →
//! SigLIP vision tower → mean-pooled vector → `hybrid_search`. Mirrors the app's persist-thread
//! drain (`embed_model_multiview` + `drain_image_embeddings`).
//!
//! Both models get NEUTRAL names, and the query words appear in neither, so a correct ranking
//! can only come from what the models *look like*.
//!
//! `#[ignore]` (needs the towers + ORT):
//! `SIGLIP_VISION=… SIGLIP_TEXT=… SIGLIP_TOKENIZER=… ORT_DYLIB_PATH=… \
//!  cargo test -p lodestash-ai --test siglip_multiview -- --ignored --nocapture`

use std::path::{Path, PathBuf};

use lodestash_ai::embedder::Embedder;
use lodestash_ai::siglip::SiglipEmbedder;
use lodestash_core::catalog::{Catalog, CrawlRecord};
use lodestash_ingest::render3d::{render_views, DEFAULT_VIEWS};

/// Write an axis-aligned box as a Wavefront OBJ (ufbx parses OBJ, so this is a handwritable
/// fixture — no binary FBX needed).
fn write_box_obj(path: &Path, sx: f32, sy: f32, sz: f32) {
    let mut s = String::from("# test box\n");
    for &(dx, dy, dz) in &[
        (-1.0f32, -1.0f32, -1.0f32),
        (1.0, -1.0, -1.0),
        (1.0, 1.0, -1.0),
        (-1.0, 1.0, -1.0),
        (-1.0, -1.0, 1.0),
        (1.0, -1.0, 1.0),
        (1.0, 1.0, 1.0),
        (-1.0, 1.0, 1.0),
    ] {
        s.push_str(&format!("v {} {} {}\n", dx * sx, dy * sy, dz * sz));
    }
    // 12 triangles, 1-indexed.
    for f in [
        [1, 2, 3],
        [1, 3, 4],
        [6, 5, 8],
        [6, 8, 7],
        [5, 1, 4],
        [5, 4, 8],
        [2, 6, 7],
        [2, 7, 3],
        [4, 3, 7],
        [4, 7, 8],
        [5, 6, 2],
        [5, 2, 1],
    ] {
        s.push_str(&format!("f {} {} {}\n", f[0], f[1], f[2]));
    }
    std::fs::write(path, s).expect("write obj");
}

/// The app's `embed_model_multiview`, reproduced (it lives in the UI bin crate).
fn embed_model(path: &Path, emb: &dyn Embedder) -> Vec<f32> {
    let meshes = lodestash_ingest::mesh::load_meshes(path, &std::collections::BTreeMap::new())
        .expect("load mesh");
    let views = render_views(&meshes, 256, DEFAULT_VIEWS);
    assert!(!views.is_empty(), "renderer produced no views for {path:?}");
    let mut acc = vec![0.0f32; emb.dim()];
    for v in &views {
        for (a, x) in acc.iter_mut().zip(emb.embed_image(v)) {
            *a += x;
        }
    }
    let n: f32 = acc.iter().map(|x| x * x).sum::<f32>().sqrt();
    for a in &mut acc {
        *a /= n;
    }
    acc
}

#[test]
#[ignore]
fn text_query_finds_the_right_3d_model_by_shape() {
    let v = std::env::var("SIGLIP_VISION").expect("SIGLIP_VISION");
    let t = std::env::var("SIGLIP_TEXT").expect("SIGLIP_TEXT");
    let tok = std::env::var("SIGLIP_TOKENIZER").expect("SIGLIP_TOKENIZER");
    let emb = SiglipEmbedder::load(Path::new(&v), Path::new(&t), Path::new(&tok), "siglip2-base")
        .expect("load");

    let root: PathBuf =
        std::env::temp_dir().join(format!("lodestash_mv_{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    // Neutral filenames: a compact block vs a long thin rod.
    write_box_obj(&root.join("mesh_a.obj"), 1.0, 1.0, 1.0);
    write_box_obj(&root.join("mesh_b.obj"), 0.07, 0.07, 2.6);

    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let sid = c
        .upsert_import_source("Lib", &root.to_string_lossy(), "folder")
        .expect("src");
    let rec = |rel: &str, name: &str| CrawlRecord {
        rel_path: rel.into(),
        name: name.into(),
        media_type: "model_3d".into(),
        size_bytes: 1,
        mtime: 1,
        workspace_rel: None,
    };
    c.record_crawl(sid, None, &[rec("mesh_a.obj", "mesh_a"), rec("mesh_b.obj", "mesh_b")])
        .expect("crawl");

    // Drain like the app: the queue hands back model_3d jobs, each rendered then embedded.
    let jobs = c.visual_assets_needing_embedding(100).expect("queue");
    assert_eq!(jobs.len(), 2);
    assert!(jobs.iter().all(|j| j.media_code == "model_3d"));
    let mut rod_id = 0;
    for job in jobs {
        let p = job.disk_path.clone().expect("on-disk");
        if p.ends_with("mesh_b.obj") {
            rod_id = job.asset_id;
        }
        let vec = embed_model(Path::new(&p), &emb);
        c.upsert_embedding(
            job.asset_id,
            emb.model_id(),
            "model_3d",
            Some(job.media_type_id),
            Some(job.source_id),
            &vec,
        )
        .expect("store");
    }

    let query = "a long thin stick";
    let qv = emb.embed_text(query);
    let ids = c
        .hybrid_search(query, &[(emb.model_id(), &qv[..])], None, 10)
        .expect("hybrid");
    println!("3D visual-search ids for {query:?}: {ids:?} (rod={rod_id})");
    let _ = std::fs::remove_dir_all(&root);

    assert_eq!(
        ids.first(),
        Some(&rod_id),
        "the elongated mesh should rank first for {query:?} — multiview embedding must encode shape"
    );
}
