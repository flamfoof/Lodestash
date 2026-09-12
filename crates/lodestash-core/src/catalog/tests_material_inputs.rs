//! Material inputs for previews (v1 plan S07): albedo by property, model slots by renderer.

use super::*;

fn library() -> (Catalog, i64) {
    let c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let src = c
        .upsert_import_source("Lib", "C:/lib", "folder")
        .expect("source");
    (c, src)
}

/// Catalogue one file and give it a Unity GUID, as a dependency scan would.
fn add(c: &mut Catalog, src: i64, rel: &str, media: &str, guid: &str) -> i64 {
    c.record_crawl(
        src,
        None,
        &[CrawlRecord {
            workspace_rel: None,
            content_hash: None,
            rel_path: rel.into(),
            name: rel.into(),
            media_type: media.into(),
            size_bytes: 1,
            mtime: 1,
        }],
    )
    .expect("crawl");
    let id: i64 = c
        .conn
        .query_row(
            "SELECT a.id FROM asset a JOIN asset_file af ON af.id = a.primary_file_id
              WHERE af.rel_path = ?1",
            params![rel],
            |r| r.get(0),
        )
        .expect("id");
    c.record_dependencies(id, Some(guid), "uses_texture", &[])
        .expect("guid");
    id
}

/// A material's texture bindings, each with the property it fills.
fn bind(c: &mut Catalog, material: i64, guid: &str, refs: &[(&str, &str)]) {
    let refs: Vec<(Option<String>, String)> = refs
        .iter()
        .map(|(role, g)| (Some((*role).to_string()), (*g).to_string()))
        .collect();
    c.record_dependencies_with_roles(material, Some(guid), "uses_texture", &refs)
        .expect("bind");
}

/// The defect S07 exists for: a material whose normal map was declared first — so it has the lower
/// id — was previewed as a normal-map ball. The property decides, not the id.
#[test]
fn a_material_previews_with_its_albedo_not_its_lowest_id_texture() {
    let (mut c, src) = library();
    let normal = add(&mut c, src, "t1.png", "image", "g-t1");
    let albedo = add(&mut c, src, "t2.png", "image", "g-t2");
    let mat = add(&mut c, src, "rock.mat", "material", "g-mat");
    bind(
        &mut c,
        mat,
        "g-mat",
        &[("_BumpMap", "g-t1"), ("_MainTex", "g-t2")],
    );
    assert!(normal < albedo);
    assert_eq!(c.material_albedo_texture(mat).unwrap(), Some(albedo));

    // A texture bound to two properties is one edge, and it keeps the role a preview can use.
    let both = add(&mut c, src, "both.mat", "material", "g-both");
    bind(
        &mut c,
        both,
        "g-both",
        &[("_BumpMap", "g-t2"), ("_MainTex", "g-t2")],
    );
    let role: Option<String> = c
        .conn
        .query_row(
            "SELECT role FROM asset_relationship WHERE from_asset_id = ?1",
            params![both],
            |r| r.get(0),
        )
        .expect("one edge");
    assert_eq!(role.as_deref(), Some("_MainTex"));
    assert_eq!(c.material_albedo_texture(both).unwrap(), Some(albedo));

    // Only a normal map: nothing to paint with.
    let bumpy = add(&mut c, src, "bumpy.mat", "material", "g-bumpy");
    bind(&mut c, bumpy, "g-bumpy", &[("_BumpMap", "g-t1")]);
    assert_eq!(
        c.material_albedo_texture(bumpy).unwrap(),
        None,
        "a normal map is never an albedo"
    );

    // No roles (harvested before M29): the file name decides.
    let rock_n = add(&mut c, src, "Rock_Normal.png", "image", "g-rn");
    let rock = add(&mut c, src, "Rock.png", "image", "g-r");
    let legacy = add(&mut c, src, "legacy.mat", "material", "g-legacy");
    c.record_dependencies(
        legacy,
        Some("g-legacy"),
        "uses_texture",
        &["g-rn".into(), "g-r".into()],
    )
    .unwrap();
    assert!(rock_n < rock);
    assert_eq!(c.material_albedo_texture(legacy).unwrap(), Some(rock));

    // Without roles, a name that says albedo outranks a plain one declared before it.
    let plain = add(&mut c, src, "Rock_Plain.png", "image", "g-plain");
    let named = add(&mut c, src, "Rock_Albedo.png", "image", "g-named");
    let unnamed = add(&mut c, src, "unnamed.mat", "material", "g-unnamed");
    c.record_dependencies(
        unnamed,
        Some("g-unnamed"),
        "uses_texture",
        &["g-plain".into(), "g-named".into()],
    )
    .unwrap();
    assert!(plain < named);
    assert_eq!(c.material_albedo_texture(unnamed).unwrap(), Some(named));
}

/// Only a texture that is there paints: a missing file is skipped, and a texture catalogued after
/// its material late-binds into the preview.
#[test]
fn a_missing_texture_is_skipped_and_a_late_one_arrives() {
    let (mut c, src) = library();
    let diffuse = add(&mut c, src, "d.png", "image", "g-d");
    let mat = add(&mut c, src, "m.mat", "material", "g-m");
    bind(&mut c, mat, "g-m", &[("_MainTex", "g-d")]);
    assert_eq!(c.material_albedo_texture(mat).unwrap(), Some(diffuse));
    c.conn
        .execute(
            "UPDATE asset_file SET missing = 1 WHERE asset_id = ?1",
            params![diffuse],
        )
        .unwrap();
    assert_eq!(
        c.material_albedo_texture(mat).unwrap(),
        None,
        "a file that is gone paints nothing"
    );

    let waiting = add(&mut c, src, "w.mat", "material", "g-w");
    bind(&mut c, waiting, "g-w", &[("_MainTex", "g-late")]);
    assert_eq!(c.material_albedo_texture(waiting).unwrap(), None);
    let late = add(&mut c, src, "late.png", "image", "g-late");
    c.resolve_pending_relationships().unwrap();
    assert_eq!(c.material_albedo_texture(waiting).unwrap(), Some(late));
}

/// A mesh that several prefabs paint differently keeps its appearances apart: the most common one
/// is shown, the rest are counted, and a tie goes to the first prefab by path. Slots stay slots, and
/// a muted link paints nothing.
#[test]
fn a_mesh_painted_by_several_prefabs_shows_one_appearance_and_counts_the_rest() {
    let (mut c, src) = library();
    let rock = add(&mut c, src, "rock.fbx", "model_3d", "g-rock");
    let ta = add(&mut c, src, "a.png", "image", "g-ta");
    let tb = add(&mut c, src, "b.png", "image", "g-tb");
    let ma = add(&mut c, src, "a.mat", "material", "g-ma");
    let mb = add(&mut c, src, "b.mat", "material", "g-mb");
    bind(&mut c, ma, "g-ma", &[("_BaseMap", "g-ta")]);
    bind(&mut c, mb, "g-mb", &[("_BaseMap", "g-tb")]);
    let scan = |path: &str, slots: &[&str]| PrefabFileScan {
        path: path.into(),
        size: 1,
        mtime: 1,
        bindings: slots
            .iter()
            .enumerate()
            .map(|(slot, m)| PrefabBindingRow {
                game_object: 1,
                renderer: 2,
                slot: slot as i64,
                mesh_guid: "g-rock".into(),
                material_guid: (*m).into(),
            })
            .collect(),
    };
    c.record_prefab_walk(
        "C:/lib",
        &[
            scan("C:/lib/1.prefab", &["g-mb"]),
            scan("C:/lib/2.prefab", &["g-ma"]),
            scan("C:/lib/3.prefab", &["g-ma"]),
        ],
        &[],
    )
    .unwrap();
    c.reconcile_prefab_edges().unwrap();
    let inputs = c.model_material_inputs(rock).unwrap();
    assert_eq!(inputs.source, MaterialInputSource::Prefab);
    assert_eq!(
        inputs.slots,
        vec![MaterialSlot {
            slot: 0,
            material_id: Some(ma),
            albedo_texture_id: Some(ta),
        }],
        "two prefabs agree on a; one says b"
    );
    assert_eq!(inputs.alternatives, 1);
    assert_eq!(c.model_albedo_texture(rock).unwrap(), Some(ta));

    // A tie goes to the first prefab by path, and 1.prefab says b.
    c.record_prefab_walk("C:/lib", &[], &["C:/lib/3.prefab".into()])
        .unwrap();
    assert_eq!(
        c.model_material_inputs(rock).unwrap().slots[0].material_id,
        Some(mb)
    );

    // Two slots stay two slots.
    c.record_prefab_walk(
        "C:/lib",
        &[scan("C:/lib/1.prefab", &["g-ma", "g-mb"])],
        &["C:/lib/2.prefab".into()],
    )
    .unwrap();
    c.reconcile_prefab_edges().unwrap();
    let inputs = c.model_material_inputs(rock).unwrap();
    assert_eq!(inputs.alternatives, 0);
    assert_eq!(
        inputs
            .slots
            .iter()
            .map(|s| (s.slot, s.material_id))
            .collect::<Vec<_>>(),
        vec![(0, Some(ma)), (1, Some(mb))]
    );

    // A muted link paints nothing; the next slot's albedo is the model's.
    let edge: i64 = c
        .conn
        .query_row(
            "SELECT id FROM asset_relationship WHERE from_asset_id = ?1 AND to_asset_id = ?2",
            params![rock, ma],
            |r| r.get(0),
        )
        .unwrap();
    assert!(c.ignore_relationship(edge).unwrap());
    assert_eq!(
        c.model_material_inputs(rock).unwrap().slots[0].material_id,
        None
    );
    assert_eq!(c.model_albedo_texture(rock).unwrap(), Some(tb));
}

/// An export with no material at all: its name-matched texture edges (§10 step 2) paint slot 0, and
/// the file name keeps the normal map out.
#[test]
fn a_name_matched_texture_paints_an_edgeless_model() {
    let (mut c, src) = library();
    let human = add(&mut c, src, "Human3.fbx", "model_3d", "g-h");
    let normal = add(&mut c, src, "Human3_Normal.png", "image", "g-hn");
    let albedo = add(&mut c, src, "Human3_Albedo.png", "image", "g-ha");
    for t in [normal, albedo] {
        c.conn
            .execute(
                "INSERT INTO asset_relationship
                     (from_asset_id, to_asset_id, kind, source, confidence, state)
                 VALUES (?1, ?2, 'uses_texture', 'inferred', 0.7, 'resolved')",
                params![human, t],
            )
            .unwrap();
    }
    let inputs = c.model_material_inputs(human).unwrap();
    assert_eq!(inputs.source, MaterialInputSource::Inferred);
    assert_eq!(c.model_albedo_texture(human).unwrap(), Some(albedo));
    assert_eq!(
        c.model_material_inputs(normal).unwrap().source,
        MaterialInputSource::None
    );
}

/// The preview paths ask for files, not ids (v1 plan S08): a slot whose texture is on disk resolves
/// to its path, and one whose texture is only a catalog row — still in a pack, or gone from disk —
/// is left out rather than handed a path that opens nothing.
#[test]
fn slot_textures_resolve_to_files_that_are_there() {
    let dir = tempfile::tempdir().expect("tmp");
    let root = dir.path().to_string_lossy().into_owned();
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let src = c.upsert_import_source("Lib", &root, "folder").expect("src");
    std::fs::write(dir.path().join("paint.png"), b"x").expect("texture on disk");
    for (rel, media, guid) in [
        ("rock.fbx", "model_3d", "g-rock"),
        ("paint.png", "image", "g-paint"),
        ("gone.png", "image", "g-gone"),
        ("rock.mat", "material", "g-mat"),
        ("bare.mat", "material", "g-bare"),
    ] {
        c.record_crawl(
            src,
            None,
            &[CrawlRecord {
                workspace_rel: None,
                content_hash: None,
                rel_path: rel.into(),
                name: rel.into(),
                media_type: media.into(),
                size_bytes: 1,
                mtime: 1,
            }],
        )
        .expect("crawl");
        let id: i64 = c
            .conn
            .query_row(
                "SELECT a.id FROM asset a JOIN asset_file af ON af.id = a.primary_file_id
                  WHERE af.rel_path = ?1",
                params![rel],
                |r| r.get(0),
            )
            .expect("id");
        c.record_dependencies(id, Some(guid), "uses_texture", &[])
            .expect("guid");
    }
    let id_of = |c: &Catalog, rel: &str| -> i64 {
        c.conn
            .query_row(
                "SELECT a.id FROM asset a JOIN asset_file af ON af.id = a.primary_file_id
                  WHERE af.rel_path = ?1",
                params![rel],
                |r| r.get(0),
            )
            .expect("id")
    };
    let rock = id_of(&c, "rock.fbx");
    let (mat, bare) = (id_of(&c, "rock.mat"), id_of(&c, "bare.mat"));
    bind(&mut c, mat, "g-mat", &[("_MainTex", "g-paint")]);
    bind(&mut c, bare, "g-bare", &[("_MainTex", "g-gone")]);
    c.record_prefab_walk(
        &root,
        &[PrefabFileScan {
            path: format!("{root}/rock.prefab"),
            size: 1,
            mtime: 1,
            bindings: vec![
                PrefabBindingRow {
                    game_object: 1,
                    renderer: 2,
                    slot: 0,
                    mesh_guid: "g-rock".into(),
                    material_guid: "g-mat".into(),
                },
                PrefabBindingRow {
                    game_object: 1,
                    renderer: 2,
                    slot: 1,
                    mesh_guid: "g-rock".into(),
                    material_guid: "g-bare".into(),
                },
            ],
        }],
        &[],
    )
    .expect("walk");
    c.reconcile_prefab_edges().expect("reconcile");

    let paths = c.model_slot_texture_paths(rock, None).expect("slots");
    assert_eq!(
        paths.keys().copied().collect::<Vec<_>>(),
        vec![0],
        "slot 1's texture has no file, so it offers the renderer nothing"
    );
    assert_eq!(paths[&0], dir.path().join("paint.png"));
}
