//! Appraisal through the real writer (v2 plan Task 2.5): the kicks and the idle lane run the
//! real pass over real files, and every result is read back through a separate read-only
//! connection. Embedders are stand-ins ([`crate::appraise::testkit`]) — no model is loaded, and
//! each engine lives in its own temp dir.

use std::path::Path;
use std::time::{Duration, Instant};

use lodestash_core::catalog::{Catalog, CatalogReader, CrawlRecord, HydratedAsset};

use crate::appraise::testkit::{
    stub_clap, stub_siglip, write_obj, write_pack, write_png, write_wav, CLAP_SOURCE, OBJ,
    OBJ_TRIANGLES,
};
use crate::appraise::Models;
use crate::types::PersistBatch;
use crate::writer::{ImportPack, Op};
use crate::{Engine, EngineConfig, EngineStatus};

const SETTLE: Duration = Duration::from_secs(5);

/// An engine under `root` with the given embedders, and auto-identify as asked. The source
/// watch is off: these tests say when a scan happens.
fn engine(root: &Path, auto_identify: bool, models: Models) -> (Engine, EngineConfig) {
    let cfg = EngineConfig::under(root);
    cfg.open_config()
        .update(|c| {
            c.auto_identify = auto_identify;
            c.watch_sources = false;
        })
        .expect("seed the config");
    let engine = Engine::start_with(cfg.clone(), models);
    assert_eq!(engine.wait_settled(SETTLE), EngineStatus::Ready);
    (engine, cfg)
}

/// Scan `files` — `(name, media type)`, already written under `<root>/lib` — as a crawl would.
fn scan(engine: &Engine, root: &Path, files: &[(&str, &str)]) {
    let records = files
        .iter()
        .map(|(name, media_type)| CrawlRecord {
            rel_path: (*name).into(),
            name: (*name).into(),
            media_type: (*media_type).into(),
            size_bytes: 1,
            mtime: 1,
            workspace_rel: None,
            content_hash: None,
        })
        .collect();
    engine
        .writer()
        .run(Op::PersistScan {
            batches: vec![PersistBatch {
                root: lib(root).to_string_lossy().into_owned(),
                name: "lib".into(),
                excludes_json: None,
                records,
            }],
            packs: Vec::new(),
        })
        .expect("scan");
}

fn lib(root: &Path) -> std::path::PathBuf {
    let lib = root.join("lib");
    std::fs::create_dir_all(&lib).expect("lib");
    lib
}

/// A second connection, read-only by type: what a test reads results through.
fn reader(cfg: &EngineConfig) -> CatalogReader {
    Catalog::open_reader(&cfg.catalog).expect("reader")
}

/// The scanned files of the one source, by name.
fn scanned(cfg: &EngineConfig) -> Vec<HydratedAsset> {
    let r = reader(cfg);
    let source = r.source_infos().expect("sources")[0].id;
    r.entries_for_source(source).expect("entries")
}

fn asset(cfg: &EngineConfig, name: &str) -> HydratedAsset {
    scanned(cfg)
        .into_iter()
        .find(|a| a.rel_path == name)
        .unwrap_or_else(|| panic!("{name} is not in the catalog"))
}

/// The `(source, value)` of every tag on an asset.
fn tags(cfg: &EngineConfig, asset_id: i64) -> Vec<(String, String)> {
    reader(cfg)
        .asset_tag_rows(asset_id)
        .expect("tags")
        .into_iter()
        .map(|t| (t.source, t.value))
        .collect()
}

/// Wait for `done`, at most four seconds — the off-writer lanes answer in milliseconds here.
fn eventually(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(4);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Review B5. One pass used to run the generic tag stage first, over every queued asset: the
/// filename tagger answered for the clip and finished it, so the audio stage found nothing
/// queued and CLAP never tagged it. Audio belongs to the audio stage alone.
#[test]
fn an_audio_asset_keeps_its_audio_tags_after_a_full_pass_with_the_filename_tagger() {
    let dir = tempfile::tempdir().expect("tmp");
    // No tagger model is configured, so the generic tag stage is the filename tagger — and
    // `wall` is one of the words it knows.
    let (engine, cfg) = engine(
        dir.path(),
        false,
        Models::fixed(None, Some(stub_clap("explosion"))),
    );
    write_wav(&lib(dir.path()).join("wall_hit.wav"));
    write_png(&lib(dir.path()).join("wall.png"));
    scan(
        &engine,
        dir.path(),
        &[("wall_hit.wav", "audio"), ("wall.png", "image")],
    );

    engine.writer().run(Op::IdentifyAll).expect("identify all");

    let clip = tags(&cfg, asset(&cfg, "wall_hit.wav").asset_id);
    assert!(
        clip.contains(&(CLAP_SOURCE.to_string(), "explosion".to_string())),
        "the clip has no CLAP tag after a full pass: {clip:?}"
    );
    assert!(
        clip.iter().all(|(source, _)| source == CLAP_SOURCE),
        "the generic tag stage wrote on an audio asset: {clip:?}"
    );
    // The picture is still the generic stage's.
    let picture = tags(&cfg, asset(&cfg, "wall.png").asset_id);
    assert!(
        picture.contains(&("ai:filename".to_string(), "wall".to_string())),
        "the filename tagger did not tag the image: {picture:?}"
    );

    // Asked again — the per-card kick re-flags the clip and runs a pass — its tags are still
    // the audio stage's, and still nothing else writes on it.
    let asset_id = asset(&cfg, "wall_hit.wav").asset_id;
    engine
        .writer()
        .run(Op::IdentifyOne { asset_id })
        .expect("second pass");
    let clip = tags(&cfg, asset_id);
    assert_eq!(
        clip,
        [(CLAP_SOURCE.to_string(), "explosion".to_string())],
        "after a re-run"
    );
}

/// Review B4. With auto-identify on, the idle lane told its pass "no assets" to keep tagging
/// off the writer — and the same value switched off metrics, previews and dependency scans. A
/// freshly scanned model was never measured until someone pressed Identify.
#[test]
fn an_idle_pass_with_auto_identify_on_fills_metrics_for_a_freshly_scanned_model() {
    let dir = tempfile::tempdir().expect("tmp");
    let (engine, cfg) = engine(
        dir.path(),
        true,
        Models::fixed(Some(stub_siglip()), Some(stub_clap("explosion"))),
    );
    write_obj(&lib(dir.path()).join("crate.obj"));
    write_wav(&lib(dir.path()).join("boom.wav"));
    scan(
        &engine,
        dir.path(),
        &[("crate.obj", "model_3d"), ("boom.wav", "audio")],
    );
    assert_eq!(asset(&cfg, "crate.obj").metrics.tri_count, None);

    engine.writer().run(Op::IdleTurn).expect("one idle turn");

    let model = asset(&cfg, "crate.obj");
    assert_eq!(
        model.metrics.tri_count,
        Some(OBJ_TRIANGLES),
        "the idle lane did not measure the model"
    );
    assert!(
        reader(&cfg)
            .assets_with_preview()
            .expect("previews")
            .contains(&model.asset_id),
        "nor draw its preview"
    );
    // The audio stages run in the idle lane too — off the writer, so their results land a
    // moment later.
    let clip = asset(&cfg, "boom.wav").asset_id;
    eventually("the clip's CLAP embedding and tag", || {
        reader(&cfg)
            .embedding_of(clip)
            .expect("embedding")
            .is_some()
            && tags(&cfg, clip).contains(&(CLAP_SOURCE.to_string(), "explosion".to_string()))
    });
}

/// The backfills are not identification: they run in the idle lane with auto-identify off too
/// (AI-PIPELINE §2), and nothing is embedded or tagged.
#[test]
fn the_idle_lane_measures_with_auto_identify_off_and_identifies_nothing() {
    let dir = tempfile::tempdir().expect("tmp");
    let (engine, cfg) = engine(
        dir.path(),
        false,
        Models::fixed(Some(stub_siglip()), Some(stub_clap("explosion"))),
    );
    write_obj(&lib(dir.path()).join("crate.obj"));
    scan(&engine, dir.path(), &[("crate.obj", "model_3d")]);

    engine.writer().run(Op::IdleTurn).expect("one idle turn");
    engine.writer().run(Op::IdleTurn).expect("and another");

    let model = asset(&cfg, "crate.obj");
    assert_eq!(model.metrics.tri_count, Some(OBJ_TRIANGLES));
    assert_eq!(model.status, "indexed", "measured, so indexed");
    assert!(tags(&cfg, model.asset_id).is_empty(), "tagged unasked");
    assert!(
        reader(&cfg)
            .embedding_of(model.asset_id)
            .expect("embedding")
            .is_none(),
        "embedded unasked"
    );
}

/// Review B3. A model still inside its `.unitypackage` cannot have a vector preview (its bytes
/// have no stable path to key one on), so it stayed in the preview queue for ever — and every
/// idle turn pulled its payload out of the archive again, only to skip it.
#[test]
fn an_in_pack_model_is_not_pulled_again_on_every_idle_turn() {
    let dir = tempfile::tempdir().expect("tmp");
    // Backfills only: with nothing embedding or tagging, every pull is the backfills' own.
    let (engine, cfg) = engine(dir.path(), false, Models::fixed(None, None));
    let packs = dir.path().join("packs");
    std::fs::create_dir_all(&packs).expect("packs");
    let pack = write_pack(
        &packs,
        &[(
            "0123456789abcdef0123456789abcdef",
            "Assets/Props/crate.obj",
            OBJ.as_bytes(),
        )],
    );
    let entries = lodestash_ingest::unitypackage::read_manifest(&pack)
        .expect("manifest")
        .iter()
        .map(|e| lodestash_core::catalog::PackageEntryRecord {
            guid: e.guid.clone(),
            pathname: e.pathname.clone(),
            tar_offset: e.asset_offset.map(|o| o as i64),
            size_bytes: e.asset_size.map(|s| s as i64),
        })
        .collect();
    engine
        .writer()
        .run(Op::ImportPacks {
            root: packs.to_string_lossy().into_owned(),
            packs: vec![ImportPack {
                abs_path: pack.to_string_lossy().into_owned(),
                name: "Pack".into(),
                publisher: None,
                category: None,
                entries,
            }],
        })
        .expect("import the pack");
    let in_pack = {
        let r = reader(&cfg);
        let source = r.source_infos().expect("sources")[0].id;
        r.in_pack_entries_for_source(source).expect("in-pack rows")
    };
    // Importing into a catalog no folder was ever scanned into: this used to answer "unknown
    // media_type code", because only a scan seeded the media types.
    assert_eq!(in_pack.len(), 1, "the model is catalogued in its pack");
    let pulls = || crate::appraise::pulls::count(&pack);

    engine.writer().run(Op::IdleTurn).expect("first idle turn");
    let after_first = pulls();
    assert!(
        after_first <= 1,
        "one pull measures the model; the turn made {after_first}"
    );
    engine.writer().run(Op::IdleTurn).expect("second idle turn");
    engine.writer().run(Op::IdleTurn).expect("third idle turn");
    assert_eq!(
        pulls(),
        after_first,
        "an idle turn pulled the in-pack model again"
    );
    assert!(
        reader(&cfg)
            .assets_with_preview()
            .expect("previews")
            .is_empty(),
        "an in-pack model has no vector preview until its pack is extracted"
    );
}
