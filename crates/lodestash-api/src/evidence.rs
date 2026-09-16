use super::*;
use serde_json::{json, Value};
use std::io::{Cursor, Read};

pub type ModelEvidenceFn =
    Box<dyn Fn(&std::path::Path, u32, usize) -> Result<Vec<Vec<u8>>, AgentError> + Send + Sync>;

pub struct EvidenceBundle {
    pub manifest: Value,
    pub images: Vec<Vec<u8>>,
}

impl ServerState {
    pub fn set_model_evidence(&self, renderer: ModelEvidenceFn) {
        *self.model_evidence.lock().unwrap() = Some(Arc::new(renderer));
    }

    pub fn get_asset_evidence(&self, id: i64, req: &EvidenceRequest) -> Result<Value, AgentError> {
        self.asset_evidence_bundle(id, req)
            .map(|bundle| bundle.manifest)
    }

    /// Build image blocks and their manifest together, so the advertised pixels are those delivered.
    pub fn asset_evidence_bundle(
        &self,
        id: i64,
        req: &EvidenceRequest,
    ) -> Result<EvidenceBundle, AgentError> {
        let row = self
            .asset(id)?
            .ok_or_else(|| AgentError::asset_not_found(id))?;
        let kind = req.kind.as_deref().unwrap_or("auto");
        let dimension = req.max_dimension.unwrap_or(512);
        let views = req.views.unwrap_or(1);
        if !matches!(kind, "auto" | "thumbnail" | "model_views" | "text")
            || !(1..=1024).contains(&dimension)
            || !(1..=4).contains(&views)
        {
            return Err(AgentError::invalid(
                "invalid evidence kind, dimension (1–1024), or view count (1–4)",
            ));
        }
        if row.media_type != "model_3d" && (kind == "model_views" || views > 1) {
            return Err(AgentError::invalid("model views require a model_3d asset"));
        }
        let revision = self.catalog.lock().unwrap().analysis_revision(id)?;
        let mut manifest = json!({"asset_id":id,"analysis_revision":revision,"status":"denied","representations":[],"file_url":format!("/v1/assets/{id}/file"),"file_resource_uri":format!("asset://{id}/file"),"dependencies_used":[],"limitations":[],"metadata":{"media_type":row.media_type,"size_bytes":row.size_bytes,"triangles":row.metrics.tri_count}});
        if !self.byte_release() {
            manifest["limitations"] = json!([{"code":"bytes_not_released","message":"preview and file bytes require byte release in Settings"}]);
            return Ok(EvidenceBundle {
                manifest,
                images: vec![],
            });
        }
        let mut limitations = Vec::new();
        let mut images = Vec::new();
        let used: Vec<i64> = Vec::new();
        let renderer = self.model_evidence.lock().unwrap().clone();
        if kind == "text" {
            let path = self.resolve_file(id)?;
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if matches!(
                ext.as_str(),
                "txt" | "json" | "yaml" | "yml" | "mat" | "prefab" | "shader" | "obj" | "gltf"
            ) {
                let size = std::fs::metadata(&path)
                    .map_err(|e| AgentError::not_local(e.to_string()))?
                    .len();
                let mut bytes = Vec::new();
                std::fs::File::open(path)
                    .map_err(|e| AgentError::not_local(e.to_string()))?
                    .take(65540)
                    .read_to_end(&mut bytes)
                    .map_err(|e| AgentError::not_local(e.to_string()))?;
                let text = std::str::from_utf8(&bytes)
                    .or_else(|error| {
                        if error.error_len().is_none() {
                            std::str::from_utf8(&bytes[..error.valid_up_to()])
                        } else {
                            Err(error)
                        }
                    })
                    .map_err(|_| {
                        AgentError::invalid("the selected representation is not UTF-8 text")
                    })?;
                let mut end = text.len().min(65536);
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                manifest["text"] = json!({"content":&text[..end],"truncated":size > end as u64,"original_size_bytes":size});
            } else {
                limitations.push(json!({"code":"unsupported_evidence","message":"no safe text representation for this file format"}));
            }
        } else if row.media_type == "model_3d" && kind != "thumbnail" && renderer.is_some() {
            let path = self.resolve_file(id)?;
            let inputs = self.catalog.lock().unwrap().model_material_inputs(id)?;
            let mut dependencies = Vec::new();
            for slot in inputs.slots.iter().take(32) {
                let availability = match slot.albedo_texture_id {
                    Some(texture_id) => self
                        .asset(texture_id)?
                        .map(|asset| self.summary(&asset)["file_availability"].clone()),
                    None => None,
                };
                dependencies.push(json!({"slot":slot.slot,"material_id":slot.material_id,"albedo_texture_id":slot.albedo_texture_id,"availability":availability,"used_for_render":false}));
            }
            manifest["dependencies"] = json!(dependencies);
            limitations.push(json!({"code":"rendering_limits","message":"Geometry-only CPU proxy views; textures, shader effects and animation are not rendered. Material dependencies are reported separately","alternative_appearances":inputs.alternatives}));
            if inputs.slots.len() > 32 {
                limitations.push(json!({"code":"dependency_limit","message":"only the first 32 material slots are resolved"}));
            }
            let renderer = renderer.unwrap();
            images = self.heavy(move || renderer(&path, dimension as u32, views))?;
        } else if let Some(path) = self.preview_path(id) {
            let mut bytes = Vec::new();
            std::fs::File::open(path)
                .map_err(|e| AgentError::not_local(e.to_string()))?
                .take(discovery::MAX_EVIDENCE_IMAGE_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| AgentError::not_local(e.to_string()))?;
            if bytes.len() as u64 > discovery::MAX_EVIDENCE_IMAGE_BYTES {
                limitations.push(json!({"code":"evidence_too_large","message":"cached preview exceeds the image byte limit"}));
            } else {
                images.push(resize_png(&bytes, dimension as u32)?);
            }
            if views > 1 {
                limitations.push(json!({"code":"additional_views_unavailable","message":"no multi-view renderer is installed"}));
            }
        }
        if images.len() > 4
            || images.iter().map(Vec::len).sum::<usize>()
                > discovery::MAX_EVIDENCE_IMAGE_BYTES as usize
        {
            return Err(AgentError::resource_too_large(
                "evidence images exceed the aggregate 4 MiB limit",
            ));
        }
        let evidence_kind = if row.media_type == "model_3d" && kind != "thumbnail" {
            "model_views"
        } else {
            "thumbnail"
        };
        let mut representations = Vec::new();
        for (view, bytes) in images.iter().enumerate() {
            let (width, height) = image::ImageReader::new(Cursor::new(bytes))
                .with_guessed_format()
                .map_err(|e| AgentError::invalid(e.to_string()))?
                .into_dimensions()
                .map_err(|e| AgentError::invalid(e.to_string()))?;
            if width > dimension as u32 || height > dimension as u32 {
                return Err(AgentError::resource_too_large(
                    "renderer exceeded requested dimensions",
                ));
            }
            representations.push(json!({"kind":evidence_kind,"mime_type":"image/png","width":width,"height":height,"url":format!("/v1/assets/{id}/preview?kind={evidence_kind}&view={view}&max_dimension={dimension}&views={views}")}));
        }
        let available = !images.is_empty() || manifest.get("text").is_some();
        manifest["status"] = json!(if !available {
            "unavailable"
        } else if images.len() < views && kind != "text" {
            "partial"
        } else {
            "ready"
        });
        if !available && limitations.is_empty() {
            limitations.push(json!({"code":"evidence_unavailable","message":"no viewable evidence is available"}));
        }
        manifest["representations"] = json!(representations);
        manifest["dependencies_used"] = json!(used);
        manifest["limitations"] = json!(limitations);
        if !self.byte_release() {
            return Err(AgentError::bytes_not_released());
        }
        Ok(EvidenceBundle { manifest, images })
    }
}

fn resize_png(bytes: &[u8], dimension: u32) -> Result<Vec<u8>, AgentError> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| AgentError::invalid(e.to_string()))?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|e| AgentError::invalid(e.to_string()))?;
    if image.width() <= dimension
        && image.height() <= dimension
        && bytes.starts_with(b"\x89PNG\r\n\x1a\n")
    {
        return Ok(bytes.to_vec());
    }
    let image = if image.width() > dimension || image.height() > dimension {
        image.thumbnail(dimension, dimension)
    } else {
        image
    };
    let mut output = Cursor::new(Vec::new());
    image
        .write_to(&mut output, image::ImageFormat::Png)
        .map_err(|e| AgentError::invalid(e.to_string()))?;
    Ok(output.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn downscaling_preserves_aspect_ratio_and_actual_pixels() {
        let source = image::RgbaImage::from_pixel(120, 60, image::Rgba([20, 50, 90, 255]));
        let mut bytes = Cursor::new(Vec::new());
        source
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        let resized = resize_png(bytes.get_ref(), 30).unwrap();
        let image = image::load_from_memory(&resized).unwrap().to_rgba8();
        assert_eq!(image.dimensions(), (30, 15));
        assert_eq!(image.get_pixel(0, 0).0, [20, 50, 90, 255]);
    }
    #[tokio::test]
    async fn model_views_match_authenticated_urls_and_text_is_bounded() {
        use lodestash_core::catalog::{Catalog, CrawlRecord};
        use tower::ServiceExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.obj");
        let original = "x".repeat(70000);
        std::fs::write(&path, &original).unwrap();
        let db = dir.path().join("catalog.db");
        let mut c = Catalog::open(&db).unwrap();
        c.seed_media_types().unwrap();
        let source = c
            .upsert_import_source("Fixture", dir.path().to_str().unwrap(), "folder")
            .unwrap();
        c.record_crawl(
            source,
            None,
            &[CrawlRecord {
                workspace_rel: None,
                content_hash: None,
                rel_path: "model.obj".into(),
                name: "model.obj".into(),
                media_type: "model_3d".into(),
                size_bytes: 70000,
                mtime: 1,
            }],
        )
        .unwrap();
        let id = c.assets_needing_tags_within(1, None).unwrap()[0].asset_id;
        drop(c);
        let state =
            Arc::new(ServerState::new(&db, dir.path().to_path_buf(), Box::new(|_| None)).unwrap());
        state.set_model_evidence(Box::new(move |_, dimension, views| {
            (0..views)
                .map(|index| {
                    let img = image::RgbaImage::from_pixel(
                        dimension,
                        dimension,
                        image::Rgba([index as u8 * 30, 50, 80, 255]),
                    );
                    let mut bytes = Cursor::new(Vec::new());
                    img.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
                    Ok(bytes.into_inner())
                })
                .collect()
        }));
        let request = EvidenceRequest {
            kind: Some("model_views".into()),
            max_dimension: Some(32),
            views: Some(4),
        };
        assert_eq!(
            state.asset_evidence_bundle(id, &request).unwrap().manifest["status"],
            "denied"
        );
        state.set_byte_release(true);
        let bundle = state.asset_evidence_bundle(id, &request).unwrap();
        assert_eq!(bundle.images.len(), 4);
        for (index, representation) in bundle.manifest["representations"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
        {
            let response = crate::routes::router(state.clone())
                .oneshot(
                    axum::http::Request::get(representation["url"].as_str().unwrap())
                        .header("host", "127.0.0.1:51731")
                        .header("authorization", format!("Bearer {}", state.token()))
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), axum::http::StatusCode::OK);
            let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
                .await
                .unwrap();
            assert_eq!(&bytes[..], bundle.images[index].as_slice());
        }
        let text = state
            .get_asset_evidence(
                id,
                &EvidenceRequest {
                    kind: Some("text".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(text["text"]["content"].as_str().unwrap().len(), 65536);
        assert_eq!(text["text"]["truncated"], true);
        assert_eq!(std::fs::read_to_string(path).unwrap(), original);
    }
}
