//! Material inputs for previews (v1 plan S07): which texture is a material's albedo, and which
//! material paints each slot of a model — decided by the property a binding fills and by the renderer
//! that declared it, never by which row happens to have the lowest id.
//!
//! Previews only. The embedding render keeps its own lowest-id choice
//! ([`Catalog::material_albedo_paths`]) so that no vector moves until re-embedding is decided.

use super::*;

/// Where a model's material inputs came from, strongest first (DATA-MODEL §10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterialInputSource {
    /// Prefab renderers that draw this mesh declared its slots (ADR-0018).
    Prefab,
    /// Resolved `uses_material` edges with no renderer behind them — slot order is edge order.
    Edge,
    /// A texture matched to the model by name (§10 step 2); no material at all.
    Inferred,
    /// Nothing in the graph paints this model.
    None,
}

/// One material slot of a model, and the texture a preview paints it with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterialSlot {
    pub slot: u32,
    /// `None` when the slot's material is not catalogued, or its link was muted.
    pub material_id: Option<i64>,
    pub albedo_texture_id: Option<i64>,
}

/// What a preview of one model paints with, slot by slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelMaterialInputs {
    pub source: MaterialInputSource,
    pub slots: Vec<MaterialSlot>,
    /// Other appearances the evidence records for this mesh — prefabs that paint it differently.
    /// The one shown is the most common (ties go to the first prefab by path); this counts the rest,
    /// so a preview never presents one of several appearances as the only one.
    pub alternatives: usize,
}

/// Texture properties that are a material's albedo, lower-cased: URP, HDRP, Built-in, and the names
/// custom shaders use most.
const ALBEDO_PROPERTIES: [&str; 12] = [
    "_basemap",
    "_basecolormap",
    "_maintex",
    "_albedo",
    "_albedomap",
    "_diffuse",
    "_diffusemap",
    "_maintexture",
    "_basetexture",
    "_colormap",
    "_texture",
    "_tex",
];

/// Words that make a property anything but an albedo: normal, height, mask and PBR maps.
const NOT_ALBEDO_WORDS: [&str; 17] = [
    "normal",
    "bump",
    "height",
    "parallax",
    "displace",
    "occlusion",
    "metal",
    "gloss",
    "spec",
    "rough",
    "smooth",
    "emiss",
    "mask",
    "detail",
    "thickness",
    "subsurface",
    "coat",
];

/// File-name tokens that name an albedo (`Rock_Albedo`, `Rock_D`)…
const ALBEDO_TOKENS: [&str; 11] = [
    "albedo",
    "diffuse",
    "diff",
    "basecolor",
    "base",
    "color",
    "colour",
    "col",
    "alb",
    "bc",
    "d",
];

/// …and ones that name anything else (`Rock_N`, `Rock_ORM`).
const NOT_ALBEDO_TOKENS: [&str; 26] = [
    "normal",
    "nrm",
    "nor",
    "n",
    "bump",
    "height",
    "disp",
    "displacement",
    "mask",
    "ao",
    "occlusion",
    "metallic",
    "metal",
    "metalness",
    "rough",
    "roughness",
    "spec",
    "specular",
    "gloss",
    "smoothness",
    "emission",
    "emissive",
    "emit",
    "orm",
    "mra",
    "opacity",
];

/// How good a texture binding is as a preview's albedo: lower is better, and `None` means never (a
/// normal, mask, height or emission map).
///
/// The property the material binds the texture to decides. A binding without one — a name-matched
/// edge, or one harvested before M29 — is judged by the texture's file name, where a word that names
/// an albedo outranks one that names anything else (`Arm_Albedo` is an albedo).
pub fn albedo_rank(role: Option<&str>, texture_name: &str) -> Option<u8> {
    if let Some(role) = role {
        let role = role.to_ascii_lowercase();
        return if ALBEDO_PROPERTIES.contains(&role.as_str()) {
            Some(0)
        } else if NOT_ALBEDO_WORDS.iter().any(|w| role.contains(w)) {
            None
        } else {
            Some(1)
        };
    }
    let name = texture_name.to_ascii_lowercase();
    let tokens: Vec<&str> = name
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect();
    let says = |words: &[&str]| tokens.iter().any(|t| words.contains(t));
    if says(&ALBEDO_TOKENS)
        || ["albedo", "diffuse", "basecolor"]
            .iter()
            .any(|w| name.contains(w))
    {
        Some(2)
    } else if says(&NOT_ALBEDO_TOKENS)
        || [
            "normal",
            "rough",
            "metallic",
            "occlusion",
            "emissi",
            "height",
        ]
        .iter()
        .any(|w| name.contains(w))
    {
        None
    } else {
        Some(3)
    }
}

/// The best albedo among `(texture id, role, texture name)` candidates in declaration order: the
/// lowest rank, then the earliest declared.
fn best_albedo(candidates: Vec<(i64, Option<String>, String)>) -> Option<i64> {
    candidates
        .into_iter()
        .enumerate()
        .filter_map(|(order, (id, role, name))| {
            albedo_rank(role.as_deref(), &name).map(|rank| (rank, order, id))
        })
        .min()
        .map(|(_, _, id)| id)
}

/// One prefab renderer's appearance of a mesh: its `(slot, material GUID)` list.
type Appearance = Vec<(i64, String)>;

impl Catalog {
    /// The live textures `from` resolves over `uses_texture` whose file is there, in declaration
    /// order, as `(texture id, role, texture name)`.
    fn texture_candidates(&self, from: i64) -> Result<Vec<(i64, Option<String>, String)>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT t.id, x.role, t.name
                   FROM asset_relationship x
                   JOIN asset t       ON t.id = x.to_asset_id AND t.deleted_at IS NULL
                   JOIN asset_file af ON af.id = t.primary_file_id AND af.missing = 0
                   JOIN media_type mt ON mt.id = t.media_type_id
                  WHERE x.from_asset_id = ?1
                    AND x.kind = 'uses_texture' AND x.state = 'resolved'
                    AND mt.code IN ('image', 'texture', 'sprite')
                  ORDER BY x.id",
            )
            .map_err(|e| CoreError::Catalog(format!("texture candidates prep: {e}")))?;
        let rows = stmt
            .query_map(params![from], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(|e| CoreError::Catalog(format!("texture candidates query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("texture candidates row: {e}")))
    }

    /// The texture a **material** paints with in a preview: the one bound to its albedo property
    /// (`_BaseMap`, `_MainTex`…), never a normal, mask or emission map (v1 plan S07). Only a resolved
    /// edge to a live texture whose file is there counts; a texture catalogued late arrives with the
    /// late-binding pass.
    pub fn material_albedo_texture(&self, material_id: i64) -> Result<Option<i64>> {
        Ok(best_albedo(self.texture_candidates(material_id)?))
    }

    /// The catalogued material `model_id` resolves for `material_guid` — only over a resolved
    /// `uses_material` edge, so a muted link paints nothing.
    fn model_material(&self, model_id: i64, material_guid: &str) -> Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT r.to_asset_id FROM asset_relationship r
                   JOIN asset t ON t.id = r.to_asset_id AND t.deleted_at IS NULL
                  WHERE r.from_asset_id = ?1 AND r.kind = 'uses_material'
                    AND r.state = 'resolved' AND r.declared_path = ?2
                  ORDER BY r.id LIMIT 1",
                params![model_id, material_guid],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| CoreError::Catalog(format!("model material: {e}")))
    }

    fn slot(&self, slot: u32, material_id: Option<i64>) -> Result<MaterialSlot> {
        let albedo_texture_id = match material_id {
            Some(m) => self.material_albedo_texture(m)?,
            None => None,
        };
        Ok(MaterialSlot {
            slot,
            material_id,
            albedo_texture_id,
        })
    }

    /// What a preview of a **model** paints each material slot with (DATA-MODEL §10, v1 plan S07).
    ///
    /// The prefab renderers that draw this mesh come first: each is one appearance, its slots in
    /// order. When prefabs paint the mesh differently, the most common appearance is taken — ties go
    /// to the first prefab by path — and the others are counted, never merged. Without prefab
    /// evidence, the model's resolved `uses_material` edges give the slots in edge order; without
    /// those, a name-matched texture (§10 step 2) paints slot 0. The model file's own embedded or
    /// declared textures are read by the loader and outrank all of this where they exist.
    pub fn model_material_inputs(&self, model_id: i64) -> Result<ModelMaterialInputs> {
        let rows: Vec<(String, i64, i64, String)> = {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT f.path, b.renderer, b.slot, b.material_guid
                       FROM asset m
                       JOIN prefab_binding b ON b.mesh_guid = m.unity_guid
                       JOIN prefab_file f    ON f.id = b.prefab_id
                      WHERE m.id = ?1 AND f.scope <> ?2
                      ORDER BY f.path, b.renderer, b.slot",
                )
                .map_err(|e| CoreError::Catalog(format!("appearances prep: {e}")))?;
            let rows = stmt
                .query_map(params![model_id, LEGACY_PREFAB_SCOPE], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })
                .map_err(|e| CoreError::Catalog(format!("appearances query: {e}")))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| CoreError::Catalog(format!("appearances row: {e}")))?
        };
        // One appearance per (prefab, renderer): its (slot, material GUID) list.
        let mut appearances: Vec<((String, i64), Appearance)> = Vec::new();
        for (path, renderer, slot, material) in rows {
            match appearances.last_mut() {
                Some((owner, slots)) if owner.0 == path && owner.1 == renderer => {
                    slots.push((slot, material))
                }
                _ => appearances.push(((path, renderer), vec![(slot, material)])),
            }
        }
        if !appearances.is_empty() {
            let mut distinct: Vec<(&Appearance, usize)> = Vec::new();
            for (_, slots) in &appearances {
                match distinct.iter_mut().find(|(seen, _)| *seen == slots) {
                    Some((_, n)) => *n += 1,
                    None => distinct.push((slots, 1)),
                }
            }
            // `max_by_key` keeps the last of equals; walking backwards makes that the first prefab.
            let (chosen, _) = *distinct
                .iter()
                .rev()
                .max_by_key(|(_, n)| *n)
                .expect("at least one appearance");
            let slots = chosen
                .iter()
                .map(|(slot, guid)| self.slot(*slot as u32, self.model_material(model_id, guid)?))
                .collect::<Result<Vec<_>>>()?;
            return Ok(ModelMaterialInputs {
                source: MaterialInputSource::Prefab,
                slots,
                alternatives: distinct.len() - 1,
            });
        }

        let materials: Vec<i64> = {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT r.to_asset_id FROM asset_relationship r
                       JOIN asset t ON t.id = r.to_asset_id AND t.deleted_at IS NULL
                      WHERE r.from_asset_id = ?1 AND r.kind = 'uses_material'
                        AND r.state = 'resolved'
                      ORDER BY r.id",
                )
                .map_err(|e| CoreError::Catalog(format!("model materials prep: {e}")))?;
            let rows = stmt
                .query_map(params![model_id], |r| r.get(0))
                .map_err(|e| CoreError::Catalog(format!("model materials query: {e}")))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| CoreError::Catalog(format!("model materials row: {e}")))?
        };
        if !materials.is_empty() {
            let slots = materials
                .into_iter()
                .enumerate()
                .map(|(slot, m)| self.slot(slot as u32, Some(m)))
                .collect::<Result<Vec<_>>>()?;
            return Ok(ModelMaterialInputs {
                source: MaterialInputSource::Edge,
                slots,
                alternatives: 0,
            });
        }

        let (source, slots) = match best_albedo(self.texture_candidates(model_id)?) {
            Some(texture) => (
                MaterialInputSource::Inferred,
                vec![MaterialSlot {
                    slot: 0,
                    material_id: None,
                    albedo_texture_id: Some(texture),
                }],
            ),
            None => (MaterialInputSource::None, Vec::new()),
        };
        Ok(ModelMaterialInputs {
            source,
            slots,
            alternatives: 0,
        })
    }

    /// The one albedo a single-texture preview of a model paints with: the first slot of
    /// [`Self::model_material_inputs`] that has one.
    pub fn model_albedo_texture(&self, asset_id: i64) -> Result<Option<i64>> {
        Ok(self
            .model_material_inputs(asset_id)?
            .slots
            .iter()
            .find_map(|s| s.albedo_texture_id))
    }
}

#[cfg(test)]
mod tests {
    use super::albedo_rank;

    #[test]
    fn the_property_decides_and_the_name_only_when_there_is_none() {
        assert_eq!(albedo_rank(Some("_MainTex"), "x"), Some(0));
        assert_eq!(
            albedo_rank(Some("_BaseMap"), "Rock_Normal"),
            Some(0),
            "the property outranks the name"
        );
        assert_eq!(albedo_rank(Some("_BumpMap"), "Rock_Albedo"), None);
        for p in [
            "_NormalMap",
            "_MetallicGlossMap",
            "_OcclusionMap",
            "_EmissionMap",
            "_MaskMap",
            "_DetailAlbedoMap",
            "_ParallaxMap",
        ] {
            assert_eq!(albedo_rank(Some(p), "x"), None, "{p}");
        }
        assert_eq!(
            albedo_rank(Some("_Paint"), "x"),
            Some(1),
            "an unknown property is still paint"
        );
        assert_eq!(
            albedo_rank(None, "Arm_Albedo.png"),
            Some(2),
            "a word for albedo wins"
        );
        assert_eq!(albedo_rank(None, "Rock_N.png"), None);
        assert_eq!(albedo_rank(None, "RockNormal.png"), None);
        assert_eq!(albedo_rank(None, "Rock_ORM.png"), None);
        assert_eq!(albedo_rank(None, "PolygonDungeon_Texture_01.png"), Some(3));
    }
}
