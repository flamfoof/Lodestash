//! Stage tests: each drives one stage directly.
use super::*;
use std::sync::atomic::AtomicBool;

use lodestash_core::catalog::{Catalog, VisualEmbedJob};

use super::resolve::{in_pack_pull_ok, IN_PACK_PULL_MAX_BYTES};

fn in_pack_job(pack_size: i64) -> VisualEmbedJob {
    VisualEmbedJob {
        asset_id: 1,
        disk_path: None,
        workspace_rel: None,
        media_type_id: 0,
        source_id: 0,
        media_code: "image".into(),
        pack_path: Some("C:/packs/P.unitypackage".into()),
        pack_offset: Some(0),
        pack_size: Some(pack_size),
        ext: Some("png".into()),
    }
}

/// The defect that made cloud tagging a data-destroying no-op: `OpenAiTagger` reads
/// `TagInput::image`, `compute_tags` never fills it, so the tagger short-circuited to an empty
/// result with no HTTP request and the caller stored that as a finished answer — clearing
/// `tags_dirty` on assets no model had seen. Returning **no rows** is the point: `store_tags`
/// then writes nothing and the assets stay queued for `drain_api_tags`.
#[test]
fn compute_tags_refuses_a_tagger_that_reads_the_image_itself() {
    let api = lodestash_ai::openai::OpenAiTagger::new("http://127.0.0.1:0", "k", "m");
    assert!(
        lodestash_ai::tagger::Tagger::needs_image(&api),
        "the API tagger sends pixels itself"
    );
    let out = compute_tags(
        vec![in_pack_job(1024)],
        &api,
        None,
        None,
        None,
        &Default::default(),
    );
    assert!(
        out.is_empty(),
        "an image-reading tagger must produce no writes on the batch path, \
         or its emptiness is stored as a finished result"
    );
}

/// The local tagger is the one this path was built for and must stay usable on it.
#[test]
fn the_local_tagger_still_runs_on_the_batch_path() {
    assert!(!lodestash_ai::tagger::Tagger::needs_image(
        &lodestash_ai::tagger::FilenameTagger
    ));
}

/// An in-pack payload over the pull cap must be refused *before* `read_payload_at` buffers it —
/// a multi-GB asset must never be pulled wholesale into RAM by the identify drain. The guard
/// runs ahead of any file IO, so the archive itself need not exist for the refusal.
#[test]
fn resolve_job_path_refuses_oversized_in_pack_payloads() {
    assert!(in_pack_pull_ok(IN_PACK_PULL_MAX_BYTES, "P.unitypackage"));
    assert!(!in_pack_pull_ok(
        IN_PACK_PULL_MAX_BYTES + 1,
        "P.unitypackage"
    ));
    assert!(
        resolve_job_path(&in_pack_job(IN_PACK_PULL_MAX_BYTES + 1), None).is_none(),
        "an over-cap payload resolves to None rather than being pulled into memory"
    );
}

fn subject(value: &str, confidence: f64) -> lodestash_core::catalog::TagAssignment {
    lodestash_core::catalog::TagAssignment {
        namespace: "subject".into(),
        value: value.into(),
        confidence: Some(confidence),
    }
}

/// A catalog holding one real PNG under a real source root, queued for tagging.
fn catalog_with_image(dir: &std::path::Path, name: &str) -> (Catalog, i64) {
    image::RgbaImage::from_pixel(16, 16, image::Rgba([120, 110, 100, 255]))
        .save(dir.join(name))
        .expect("png");
    let mut cat = Catalog::open_in_memory().expect("open");
    cat.seed_media_types().expect("seed");
    let src = cat
        .upsert_import_source("Lib", dir.to_str().expect("utf-8 path"), "folder")
        .expect("source");
    cat.record_crawl(
        src,
        None,
        &[lodestash_core::catalog::CrawlRecord {
            workspace_rel: None,
            content_hash: None,
            rel_path: name.into(),
            name: name.into(),
            media_type: "image".into(),
            size_bytes: 16,
            mtime: 1,
        }],
    )
    .expect("crawl");
    let id = cat.assets_needing_tags(16).expect("queue")[0].asset_id;
    (cat, id)
}

/// The attribution defect S04 exists for: a cloud pass that fell back to the local model stored
/// the local model's tags under the **cloud** model's id. The endpoint here is a closed local
/// port, so the attempt fails hard at once and the filename tagger substitutes — and its tags
/// land under its own producer and model, with nothing claimed for the endpoint.
#[test]
fn a_fallback_is_credited_to_the_fallback_not_the_failed_endpoint() {
    let dir = tempfile::tempdir().expect("tmp");
    let cache = tempfile::tempdir().expect("cache");
    let (mut cat, id) = catalog_with_image(dir.path(), "rock.png");
    let api = lodestash_ai::openai::OpenAiTagger::new("http://127.0.0.1:1", "k", "cloud-model");
    let progress = crate::jobs::JobProgress::default();
    drain_api_tags(
        &mut cat,
        &crate::config::AppConfig::default(),
        &api,
        Some(&lodestash_ai::tagger::FilenameTagger),
        None,
        0,
        None,
        Some(cache.path()),
        &progress,
        &Scope::All,
    );
    let rows = cat.asset_tag_rows(id).expect("rows");
    assert!(!rows.is_empty(), "the fallback wrote nothing");
    assert!(
        rows.iter()
            .all(|r| r.source == "ai:filename"
                && r.model_id.as_deref() == Some("filename-stub-v1")),
        "the fallback's tags are credited to someone else: {rows:?}"
    );
    assert_eq!(
        progress.failed.load(std::sync::atomic::Ordering::Relaxed),
        1,
        "the endpoint's failure is still counted"
    );
    assert_eq!(
        cat.count_needing_tags().expect("queue"),
        0,
        "the fallback's result finishes the asset"
    );
}

/// The refusal happens *before* the pass exists, not after an upload fails (v1 plan S10,
/// ADR-0020). Without consent the endpoint is never contacted — so nothing is written, the
/// asset stays queued, and **no failure is counted**, because no attempt was made. Granting
/// consent is what turns the same call into an attempt, which is how this test knows the gate
/// is what stopped it: the host is a `.invalid` name (RFC 2606), so the attempt fails at
/// resolution without reaching anyone.
#[test]
fn a_remote_pass_without_consent_makes_no_attempt_at_all() {
    let dir = tempfile::tempdir().expect("tmp");
    let cache = tempfile::tempdir().expect("cache");
    let (mut cat, id) = catalog_with_image(dir.path(), "rock.png");
    let api = lodestash_ai::openai::OpenAiTagger::new("https://tagger.invalid/v1", "k", "m");
    let run = |cat: &mut Catalog, cfg: &crate::config::AppConfig| {
        let progress = crate::jobs::JobProgress::default();
        drain_api_tags(
            cat,
            cfg,
            &api,
            None,
            None,
            0,
            None,
            Some(cache.path()),
            &progress,
            &Scope::All,
        );
        progress.failed.load(std::sync::atomic::Ordering::Relaxed)
    };

    let refused = run(&mut cat, &crate::config::AppConfig::default());
    assert_eq!(refused, 0, "a refusal is not a failed request");
    assert!(
        cat.asset_tag_rows(id).expect("rows").is_empty(),
        "a refused pass wrote tags"
    );
    assert_eq!(
        cat.count_needing_tags().expect("queue"),
        1,
        "a refused pass must leave the asset queued, not mark it done"
    );

    let consented = crate::config::AppConfig {
        remote_image_bytes: true,
        ..crate::config::AppConfig::default()
    };
    assert_eq!(
        run(&mut cat, &consented),
        1,
        "with consent the same call must actually attempt the endpoint"
    );
}

/// A slice's VLM tags and zero-shot proposals are two producers, stored as two sources — not the
/// zero-shot merged under the VLM's model id, which is what M26 had to mark unrecoverable.
#[test]
fn a_slice_stores_each_producer_under_its_own_source() {
    let dir = tempfile::tempdir().expect("tmp");
    let (mut cat, id) = catalog_with_image(dir.path(), "wall.png");
    let producers = TagProducers {
        tagger: ("ollama@localhost:11434".into(), "qwen3:8b".into()),
        zero_shot: Some(("siglip-zeroshot".into(), "siglip2-base".into())),
    };
    let stored = store_tags(
        &mut cat,
        &producers,
        &[ComputedTags {
            asset_id: id,
            caption: Some("A mossy wall.".into()),
            tags: vec![subject("wall", 0.9)],
            zero_shot: vec![subject("brick", 0.6)],
        }],
    );
    assert_eq!(stored, Some(2));
    let rows: Vec<_> = cat
        .asset_tag_rows(id)
        .expect("rows")
        .into_iter()
        .map(|r| (r.source, r.value, r.model_id))
        .collect();
    assert_eq!(
        rows,
        vec![
            (
                "ai:ollama@localhost:11434".to_string(),
                "wall".to_string(),
                Some("qwen3:8b".to_string())
            ),
            (
                "ai:siglip-zeroshot".to_string(),
                "brick".to_string(),
                Some("siglip2-base".to_string())
            ),
        ]
    );
    assert_eq!(cat.count_needing_tags().expect("queue"), 0);
}

/// A producer id names the endpoint's host so two endpoints stay apart — and never carries the
/// key a URL can embed.
#[test]
fn producer_ids_name_the_host_and_never_the_credentials() {
    assert_eq!(
        lodestash_ai::tagger::endpoint_host(
            "https://user:sk-secret@API.example.com:8443/v1/chat?k=1"
        ),
        "api.example.com:8443"
    );
    let api =
        lodestash_ai::openai::OpenAiTagger::new("https://key:secret@api.example.com/v1", "k", "m");
    let id = lodestash_ai::tagger::Tagger::producer_id(&api);
    assert_eq!(id, "openai-compatible@api.example.com");
    assert!(!id.contains("secret"));
    assert!(
        lodestash_core::catalog::source_problem(&lodestash_core::catalog::ai_source(&id)).is_none()
    );
}

/// S06 end to end over real files: a prefab's bindings become edges by GameObject, follow an
/// edit or a deletion of the prefab alone, survive a source going away, and reach a prefab that
/// lives only in the package workspace.
#[test]
fn prefab_evidence_follows_the_prefab_and_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let lib = dir.path().join("lib");
    std::fs::create_dir(&lib).unwrap();
    let guid = |c: char| c.to_string().repeat(32);
    let files = [
        ("rock.fbx", 'a', "model_3d"),
        ("rock.mat", 'c', "material"),
        ("tree.fbx", 'b', "model_3d"),
        ("tree.mat", 'd', "material"),
    ];
    for (name, g, _) in files {
        std::fs::write(lib.join(name), "Material:\n").unwrap();
        std::fs::write(
            lib.join(format!("{name}.meta")),
            format!("fileFormatVersion: 2\nguid: {}\n", guid(g)),
        )
        .unwrap();
    }
    // Two GameObjects, each a MeshFilter + MeshRenderer; `rock_mat` paints the first.
    let prefab = |rock_mat: char| {
        format!(
            "--- !u!33 &101\nMeshFilter:\n  m_GameObject: {{fileID: 100}}\n  \
             m_Mesh: {{fileID: 4300000, guid: {}, type: 3}}\n\
             --- !u!23 &102\nMeshRenderer:\n  m_GameObject: {{fileID: 100}}\n  \
             m_Materials:\n  - {{fileID: 2100000, guid: {}, type: 2}}\n\
             --- !u!33 &201\nMeshFilter:\n  m_GameObject: {{fileID: 200}}\n  \
             m_Mesh: {{fileID: 4300000, guid: {}, type: 3}}\n\
             --- !u!23 &202\nMeshRenderer:\n  m_GameObject: {{fileID: 200}}\n  \
             m_Materials:\n  - {{fileID: 2100000, guid: {}, type: 2}}\n",
            guid('a'),
            guid(rock_mat),
            guid('b'),
            guid('d')
        )
    };
    std::fs::write(lib.join("Scene.prefab"), prefab('c')).unwrap();

    let mut cat = Catalog::open(&dir.path().join("catalog.db")).unwrap();
    cat.seed_media_types().unwrap();
    let src = cat
        .upsert_import_source("Lib", &lib.to_string_lossy(), "folder")
        .unwrap();
    cat.record_crawl(
        src,
        None,
        &files.map(|(name, _, media)| lodestash_core::catalog::CrawlRecord {
            workspace_rel: None,
            content_hash: None,
            rel_path: name.into(),
            name: name.into(),
            media_type: media.into(),
            size_bytes: 1,
            mtime: 1,
        }),
    )
    .unwrap();
    let ids: std::collections::HashMap<String, i64> = cat
        .assets_needing_dep_scan(10)
        .unwrap()
        .into_iter()
        .map(|j| {
            let path = j.disk_path.expect("on disk");
            let name = path.rsplit(['/', '\\']).next().unwrap().to_string();
            (name, j.asset_id)
        })
        .collect();
    let (rock, rock_mat, tree, tree_mat) = (
        ids["rock.fbx"],
        ids["rock.mat"],
        ids["tree.fbx"],
        ids["tree.mat"],
    );
    let closure = |cat: &Catalog, id: i64| cat.dependency_closure(&[id]).unwrap();
    let sorted = |mut v: Vec<i64>| {
        v.sort();
        v
    };

    // The dependency drain harvests the GUIDs and reads the prefab: two pairings, not four.
    drain_dependencies(&mut cat, &Scope::All, None, None, &AtomicBool::new(false));
    assert_eq!(closure(&cat, rock), sorted(vec![rock, rock_mat]));
    assert_eq!(closure(&cat, tree), sorted(vec![tree, tree_mat]));
    assert!(
        !cat.prefab_reharvest_pending().unwrap(),
        "a complete walk of the source retired M28's legacy evidence"
    );

    // Only the prefab changes, as a watch rescan finds it: rock now wears tree's material.
    let scope = [PrefabScope {
        root: lib.clone(),
        absent_is_empty: false,
    }];
    std::fs::write(lib.join("Scene.prefab"), prefab('d') + "\n").unwrap();
    assert!(refresh_prefab_evidence(&mut cat, &scope, false) > 0);
    assert_eq!(closure(&cat, rock), sorted(vec![rock, tree_mat]));

    // Deleting the prefab alone takes its edges with it.
    std::fs::remove_file(lib.join("Scene.prefab")).unwrap();
    refresh_prefab_evidence(&mut cat, &scope, false);
    assert_eq!(closure(&cat, rock), vec![rock]);

    // A source that is gone proves nothing: what it said stays.
    std::fs::write(lib.join("Scene.prefab"), prefab('c')).unwrap();
    refresh_prefab_evidence(&mut cat, &scope, false);
    std::fs::rename(&lib, dir.path().join("unplugged")).unwrap();
    refresh_prefab_evidence(&mut cat, &scope, false);
    assert_eq!(
        closure(&cat, rock),
        sorted(vec![rock, rock_mat]),
        "unavailable is not deleted"
    );

    // A prefab that lives only in the package workspace is evidence too, and a workspace
    // nothing was extracted into yet is simply empty.
    let ws = dir.path().join("ws");
    let ws_scope = [PrefabScope {
        root: ws.clone(),
        absent_is_empty: true,
    }];
    refresh_prefab_evidence(&mut cat, &ws_scope, false);
    std::fs::create_dir_all(ws.join("ab")).unwrap();
    std::fs::write(
        ws.join("ab/Pack.prefab"),
        format!(
            "--- !u!137 &1\nSkinnedMeshRenderer:\n  m_GameObject: {{fileID: 2}}\n  \
             m_Materials:\n  - {{fileID: 2100000, guid: {}, type: 2}}\n  \
             m_Mesh: {{fileID: 4300000, guid: {}, type: 3}}\n",
            guid('c'),
            guid('b')
        ),
    )
    .unwrap();
    refresh_prefab_evidence(&mut cat, &ws_scope, false);
    assert_eq!(
        closure(&cat, tree),
        sorted(vec![tree, rock_mat, tree_mat]),
        "the workspace prefab adds its pairing; the unplugged source's stands"
    );

    // Evidence from a scope the library no longer has — a removed source — is forgotten only
    // by a walk of every scope that completed, never while one of them is unplugged.
    cat.record_prefab_walk(
        "C:/removed-source",
        &[lodestash_core::catalog::PrefabFileScan {
            path: "C:/removed-source/Old.prefab".into(),
            size: 1,
            mtime: 1,
            bindings: vec![lodestash_core::catalog::PrefabBindingRow {
                game_object: 1,
                renderer: 2,
                slot: 0,
                mesh_guid: guid('a'),
                material_guid: guid('d'),
            }],
        }],
        &[],
    )
    .unwrap();
    let everything = [
        PrefabScope {
            root: lib.clone(),
            absent_is_empty: false,
        },
        PrefabScope {
            root: ws.clone(),
            absent_is_empty: true,
        },
    ];
    refresh_prefab_evidence(&mut cat, &everything, true);
    assert_eq!(
        closure(&cat, rock),
        sorted(vec![rock, rock_mat, tree_mat]),
        "one scope unplugged: nothing is forgotten"
    );
    std::fs::rename(dir.path().join("unplugged"), &lib).unwrap();
    refresh_prefab_evidence(&mut cat, &everything, true);
    assert_eq!(
        closure(&cat, rock),
        sorted(vec![rock, rock_mat]),
        "every scope walked: the removed source's evidence goes"
    );
}

/// A material's texture bindings keep the property they fill (M29). Its normal map is declared
/// first and has the lower id, and the names say nothing either way — only the property can pick
/// the albedo.
#[test]
fn a_material_harvest_keeps_each_texture_role() {
    let dir = tempfile::tempdir().unwrap();
    let lib = dir.path().join("lib");
    std::fs::create_dir(&lib).unwrap();
    let guid = |c: char| c.to_string().repeat(32);
    let files = [
        ("t_a.png", 'b', "image"),
        ("t_b.png", 'c', "image"),
        ("rock.mat", 'a', "material"),
    ];
    for (name, g, _) in files {
        std::fs::write(lib.join(name), "x").unwrap();
        std::fs::write(
            lib.join(format!("{name}.meta")),
            format!("fileFormatVersion: 2\nguid: {}\n", guid(g)),
        )
        .unwrap();
    }
    std::fs::write(
        lib.join("rock.mat"),
        format!(
            "Material:\n  m_SavedProperties:\n    m_TexEnvs:\n    - _BumpMap:\n        \
             m_Texture: {{fileID: 2800000, guid: {}, type: 3}}\n    - _MainTex:\n        \
             m_Texture: {{fileID: 2800000, guid: {}, type: 3}}\n",
            guid('b'),
            guid('c')
        ),
    )
    .unwrap();
    let mut cat = Catalog::open(&dir.path().join("catalog.db")).unwrap();
    cat.seed_media_types().unwrap();
    let src = cat
        .upsert_import_source("Lib", &lib.to_string_lossy(), "folder")
        .unwrap();
    cat.record_crawl(
        src,
        None,
        &files.map(|(name, _, media)| lodestash_core::catalog::CrawlRecord {
            workspace_rel: None,
            content_hash: None,
            rel_path: name.into(),
            name: name.into(),
            media_type: media.into(),
            size_bytes: 1,
            mtime: 1,
        }),
    )
    .unwrap();
    let ids: std::collections::HashMap<String, i64> = cat
        .assets_needing_dep_scan(10)
        .unwrap()
        .into_iter()
        .map(|j| {
            let path = j.disk_path.expect("on disk");
            let name = path.rsplit(['/', '\\']).next().unwrap().to_string();
            (name, j.asset_id)
        })
        .collect();
    assert!(ids["t_a.png"] < ids["t_b.png"]);
    drain_dependencies(&mut cat, &Scope::All, None, None, &AtomicBool::new(false));
    assert_eq!(
        cat.material_albedo_texture(ids["rock.mat"]).unwrap(),
        Some(ids["t_b.png"])
    );
}
