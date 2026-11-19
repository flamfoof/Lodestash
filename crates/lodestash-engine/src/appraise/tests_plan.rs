//! The plan itself (v2 plan Task 2.5): the one stage order, and what a scope and a budget mean
//! to every stage. Each test runs the real [`run`] over real files in a catalog of its own and
//! reads the results back through a separate read-only connection.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use lodestash_core::catalog::{Catalog, CatalogReader, CrawlRecord, HydratedAsset};

use super::testkit::{
    stub_clap, stub_siglip, write_obj, write_png, write_wav, CLAP_SOURCE, OBJ_TRIANGLES,
};
use super::*;
use crate::changes::Origin;
use crate::config::AppConfig;
use crate::jobs::JobProgress;

/// A library on disk with its catalog: one source, `<dir>/lib`.
struct Library {
    dir: tempfile::TempDir,
    cat: Catalog,
    config: AppConfig,
    progress: JobProgress,
    graph_due: AtomicBool,
}

impl Library {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tmp");
        std::fs::create_dir_all(dir.path().join("lib")).expect("lib");
        let cat = Catalog::open(&dir.path().join("catalog.db")).expect("open");
        cat.seed_media_types().expect("seed");
        Self {
            dir,
            cat,
            // No tagger model and no endpoint: the generic tag stage is the filename tagger.
            config: AppConfig::default(),
            progress: JobProgress::default(),
            graph_due: AtomicBool::new(false),
        }
    }

    fn root(&self) -> PathBuf {
        self.dir.path().join("lib")
    }

    /// Write one picture, one model and one clip into `<lib>/<folder>` and index them, the way a
    /// scan does. The picture and the model share a name, so the naming heuristic pairs them.
    fn add_folder(&mut self, folder: &str) {
        let at = self.root().join(folder);
        std::fs::create_dir_all(&at).expect("folder");
        write_png(&at.join("rock.png"));
        write_obj(&at.join("rock.obj"));
        write_wav(&at.join("boom.wav"));
        let source = self
            .cat
            .upsert_import_source("lib", &self.root().to_string_lossy(), "folder")
            .expect("source");
        let records: Vec<CrawlRecord> = [
            ("rock.png", "image"),
            ("rock.obj", "model_3d"),
            ("boom.wav", "audio"),
        ]
        .into_iter()
        .map(|(name, media_type)| CrawlRecord {
            rel_path: format!("{folder}/{name}"),
            name: name.into(),
            media_type: media_type.into(),
            size_bytes: 1,
            mtime: 1,
            workspace_rel: None,
            content_hash: None,
        })
        .collect();
        self.cat
            .record_crawl(source, None, &records)
            .expect("crawl");
    }

    /// Run `plan` with stand-in embedders for both spaces.
    fn run(&mut self, plan: &AppraisePlan) -> Appraised {
        let cache = self.dir.path().join("cache");
        let env = Env {
            workspace_dir: None,
            cache_dir: Some(&cache),
            config: &self.config,
            embedder: Some(stub_siglip()),
            audio_embedder: Some(stub_clap("explosion")),
            progress: &self.progress,
            graph_due: &self.graph_due,
        };
        run(&mut self.cat, plan, &env, None)
    }

    fn reader(&self) -> CatalogReader {
        Catalog::open_reader(&self.dir.path().join("catalog.db")).expect("reader")
    }

    /// What a second connection sees of `rel_path`.
    fn seen(&self, rel_path: &str) -> Seen {
        let r = self.reader();
        let source = r.source_infos().expect("sources")[0].id;
        let asset = r
            .entries_for_source(source)
            .expect("entries")
            .into_iter()
            .find(|a| a.rel_path == rel_path)
            .unwrap_or_else(|| panic!("{rel_path} is not in the catalog"));
        let id = asset.asset_id;
        Seen {
            tags: r
                .asset_tag_rows(id)
                .expect("tags")
                .into_iter()
                .map(|t| (t.source, t.value))
                .collect(),
            embedded: r.embedding_of(id).expect("embedding").is_some(),
            preview: r.assets_with_preview().expect("previews").contains(&id),
            linked: r
                .asset_edges()
                .expect("edges")
                .iter()
                .any(|e| e.from == id || e.to == id),
            asset,
        }
    }

    fn ids(&self, rel_paths: &[&str]) -> Vec<i64> {
        rel_paths
            .iter()
            .map(|p| self.seen(p).asset.asset_id)
            .collect()
    }
}

/// One asset, as a reader sees it after a pass.
struct Seen {
    asset: HydratedAsset,
    tags: Vec<(String, String)>,
    embedded: bool,
    preview: bool,
    linked: bool,
}

impl Seen {
    /// Has no stage written anything about this asset?
    fn untouched(&self) -> bool {
        self.asset.metrics == Default::default()
            && self.asset.status == "discovered"
            && self.tags.is_empty()
            && !self.embedded
            && !self.preview
            && !self.linked
    }
}

fn plan(scope: Scope, budget: Budget) -> AppraisePlan {
    AppraisePlan {
        scope,
        stages: Stages::ALL,
        budget,
        origin: Origin::User,
    }
}

const FILES: [&str; 3] = ["rock.png", "rock.obj", "boom.wav"];

fn in_folder(folder: &str) -> Vec<String> {
    FILES.iter().map(|f| format!("{folder}/{f}")).collect()
}

/// Every stage did its work on this folder's three files.
fn assert_appraised(lib: &Library, folder: &str) {
    let filename_tag = ("ai:filename".to_string(), "rock".to_string());
    let picture = lib.seen(&format!("{folder}/rock.png"));
    assert_eq!(
        (
            picture.asset.metrics.width_px,
            picture.asset.metrics.height_px
        ),
        (Some(16), Some(8)),
        "{folder}: the picture was not measured"
    );
    assert!(picture.embedded && picture.tags.contains(&filename_tag));
    assert_eq!(picture.asset.status, "identified");

    let model = lib.seen(&format!("{folder}/rock.obj"));
    assert_eq!(model.asset.metrics.tri_count, Some(OBJ_TRIANGLES));
    assert!(model.preview, "{folder}: the model has no vector preview");
    assert!(model.embedded && model.tags.contains(&filename_tag));
    assert_eq!(model.asset.status, "identified");

    let clip = lib.seen(&format!("{folder}/boom.wav"));
    assert!(
        clip.asset.metrics.duration_ms.is_some(),
        "{folder}: the clip was not measured"
    );
    assert_eq!(
        clip.tags,
        [(CLAP_SOURCE.to_string(), "explosion".to_string())],
        "{folder}: a clip's tags are the audio stage's alone"
    );
    assert!(clip.embedded);
    assert_eq!(clip.asset.status, "identified");
}

/// The order is a property of the plan, stated once (v2 spec §4.6) — and no caller can change
/// it: a set of stages has no order of its own.
#[test]
fn the_stage_order_is_fixed_on_the_plan() {
    use Stage::*;
    assert_eq!(
        Stage::ORDER,
        [
            Metrics,
            Dependencies,
            Previews,
            AudioEmbed,
            AudioTags,
            VisualEmbed,
            Tags,
            Promote
        ],
        "metrics → dependencies → previews → audio embed → audio tags → image/3D embed → tags \
         → promote"
    );
    assert_eq!(
        Stages::of(&[Promote, Tags, AudioTags, Metrics])
            .in_order()
            .collect::<Vec<_>>(),
        [Metrics, AudioTags, Tags, Promote],
        "naming stages in another order does not run them in it"
    );

    // The callers' plans, as sets: a kick runs everything; the idle lane always runs the
    // backfills and the lifecycle (review B4), and identifies when that is automatic.
    assert_eq!(
        AppraisePlan::kick(Scope::All, Origin::User).stages,
        Stages::ALL
    );
    assert_eq!(
        AppraisePlan::idle(false)
            .stages
            .in_order()
            .collect::<Vec<_>>(),
        [Metrics, Dependencies, Previews, Promote]
    );
    assert_eq!(AppraisePlan::idle(true).stages, Stages::ALL);

    // And the pass runs what the plan names, in that order.
    let mut lib = Library::new();
    lib.add_folder("a");
    let done = lib.run(&plan(Scope::All, Budget::UNBOUNDED));
    assert_eq!(done.ran, Stage::ORDER);
}

/// A scope confines **every** stage. The old `only` scoped four queues and switched the other
/// three stages off; the backfills now run for the assets named, and for nothing else.
#[test]
fn a_scoped_plan_touches_only_its_assets_and_a_library_wide_one_runs_the_backfills() {
    let mut lib = Library::new();
    lib.add_folder("a");
    lib.add_folder("b");
    let (a, b) = (in_folder("a"), in_folder("b"));
    let a_ids = lib.ids(&a.iter().map(String::as_str).collect::<Vec<_>>());

    let done = lib.run(&plan(Scope::Assets(a_ids.clone()), Budget::UNBOUNDED));

    assert_appraised(&lib, "a");
    for path in &b {
        assert!(
            lib.seen(path).untouched(),
            "{path} is outside the scope and a stage wrote on it"
        );
    }
    let mut promoted = done.promoted.clone();
    promoted.sort();
    assert_eq!(promoted, a_ids, "the pass reports what it promoted, only");
    // The graph is the library's: a scoped pass harvests and leaves the reconcile due.
    assert!(
        !lib.seen("a/rock.obj").linked,
        "a scoped pass reconciled the library's graph"
    );
    assert!(lib.graph_due.load(Ordering::Relaxed));

    lib.run(&plan(Scope::All, Budget::UNBOUNDED));

    assert_appraised(&lib, "b");
    for folder in ["a", "b"] {
        assert!(
            lib.seen(&format!("{folder}/rock.obj")).linked,
            "{folder}: the library-wide pass did not pair the model with its texture"
        );
    }
    assert!(
        !lib.graph_due.load(Ordering::Relaxed),
        "the reconcile ran and is no longer due"
    );
}

/// An explicit empty set is no assets. It used to be how the idle lane said "do not tag inline",
/// which is why it has to mean exactly nothing: not one row read, not one written.
#[test]
fn an_empty_scope_does_nothing() {
    let mut lib = Library::new();
    lib.add_folder("a");
    let before = lib.cat.total_changes();

    let done = lib.run(&plan(Scope::Assets(Vec::new()), Budget::UNBOUNDED));

    assert!(done.ran.is_empty() && done.promoted.is_empty());
    assert_eq!(lib.cat.total_changes(), before, "an empty scope wrote");
    for path in in_folder("a") {
        assert!(lib.seen(&path).untouched(), "{path}");
    }
}

/// A budget is tested before every unit of work. A pass whose time is already spent takes
/// nothing off a timed queue — and the same plan with time does.
#[test]
fn a_pass_out_of_time_leaves_its_queues_for_the_next_one() {
    let mut lib = Library::new();
    lib.add_folder("a");
    let spent = Budget {
        time: TimeBudget::Pass(Duration::ZERO),
        ..Budget::IDLE
    };
    let backfill = |budget| AppraisePlan {
        scope: Scope::All,
        stages: Stages::BACKFILL.with(Stage::Promote),
        budget,
        origin: Origin::Background,
    };

    lib.run(&backfill(spent));
    let model = lib.seen("a/rock.obj");
    assert_eq!(model.asset.metrics.tri_count, None, "measured after time");
    assert!(!model.preview, "rendered after time");

    lib.run(&backfill(Budget::IDLE));
    let model = lib.seen("a/rock.obj");
    assert_eq!(model.asset.metrics.tri_count, Some(OBJ_TRIANGLES));
    assert!(model.preview);
    assert_eq!(model.asset.status, "indexed", "measured, not identified");
}
