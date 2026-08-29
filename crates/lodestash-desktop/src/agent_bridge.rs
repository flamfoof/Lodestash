//! Headless MCP writes, forwarded to the running app (ENGINE-API §1, ADR-0002).
//!
//! `lodestash-desktop --mcp` serves the agent plane over stdio with its own **read-only** catalog
//! connection. Reads are safe that way — SQLite WAL admits any number of readers — but writes are
//! not: ARCHITECTURE §5 funnels every write through one dedicated writer thread, and a second
//! process opening its own writer is exactly the `database is locked` the rule exists to prevent.
//!
//! So the headless server does not write. It **forwards** each mutating op to the running app over
//! the loopback REST plane the app already serves, using the port and token the app already
//! publishes in `engine.json`. One writer, in one process, whoever asked.
//!
//! With no app running the forward fails to connect and the tool answers honestly — the same
//! read-only behaviour as before, with a message that says what to do about it.

use lodestash_api::AgentWrite;

/// Where the running app publishes its port + token, if it is running at all.
///
/// No liveness check on the recorded pid: a stale file simply fails to connect, and the bearer
/// token means a *different* process that happened to take the port answers `401` rather than
/// being handed someone else's write. Cheaper than a per-platform process probe, and it fails in
/// the same direction.
fn handshake() -> Option<lodestash_api::Handshake> {
    let path = lodestash_api::handshake_path()?;
    let raw = std::fs::read(path).ok()?;
    serde_json::from_slice(&raw).ok()
}

/// A write bridge that POSTs to the running app, or `None` when no handshake file exists.
pub fn forwarding_write_fn() -> Option<lodestash_api::AgentWriteFn> {
    let h = handshake()?;
    tracing::info!(
        port = h.port,
        pid = h.pid,
        "agent writes forward to the running app"
    );
    Some(Box::new(move |w: AgentWrite| forward(&h, w)))
}

/// One mutating op → one REST call. The mapping is deliberately 1:1 with the route table in
/// ENGINE-API §2: this is the same operation the GUI's in-process bridge performs, taking the
/// long way round to reach the same writer.
fn forward(h: &lodestash_api::Handshake, w: AgentWrite) -> Result<usize, String> {
    let (path, body) = match &w {
        AgentWrite::AddTag {
            asset_id,
            namespace,
            value,
        } => (
            format!("/v1/assets/{asset_id}/tags"),
            serde_json::json!({ "namespace": namespace, "value": value }),
        ),
        AgentWrite::ApplyMapping { taxonomy_id } => (
            "/v1/mapping/apply".to_string(),
            serde_json::json!({ "taxonomy_id": taxonomy_id }),
        ),
        AgentWrite::Identify { asset_ids } => (
            "/v1/identify".to_string(),
            serde_json::json!({ "asset_ids": asset_ids }),
        ),
        AgentWrite::UnpackPackage { package_id } => (
            "/v1/packages/unpack".to_string(),
            serde_json::json!({ "package_id": package_id }),
        ),
        AgentWrite::StageAsset { asset_id } => (
            "/v1/stage".to_string(),
            serde_json::json!({ "asset_id": asset_id }),
        ),
        // Neither has a REST route today, so neither can be forwarded — and the MCP surface
        // exposes no tool for either (`add_tag` has no `remove` counterpart, `stage_asset` no
        // unstage). Refused loudly rather than silently reporting zero rows changed.
        AgentWrite::RemoveTag { .. } | AgentWrite::UnstageAsset { .. } => {
            return Err(
                "this operation has no REST route, so a headless server cannot forward it — \
                 do it in the app"
                    .into(),
            )
        }
    };
    post(h, &path, &body)
}

fn post(
    h: &lodestash_api::Handshake,
    path: &str,
    body: &serde_json::Value,
) -> Result<usize, String> {
    let url = format!("http://127.0.0.1:{}{path}", h.port);
    let resp = ureq::post(&url)
        .set("authorization", &format!("Bearer {}", h.token))
        .send_json(body.clone());
    match resp {
        Ok(r) => changed_from(r),
        // A 4xx/5xx carries the engine's own error body, which says more than the status.
        Err(ureq::Error::Status(code, r)) => {
            let text = r.into_string().unwrap_or_default();
            Err(format!("the app refused this write ({code}): {text}"))
        }
        Err(e) => Err(format!(
            "no running Lodestash to write through ({e}) — open the app, or use it there"
        )),
    }
}

/// `{"changed": n}` for the synchronous writes; a job body for the long-running ones
/// (`identify`, `unpack`), which report **1 job accepted** rather than a row count the caller
/// cannot have yet. Reporting 0 there would read as "nothing happened".
fn changed_from(r: ureq::Response) -> Result<usize, String> {
    let v: serde_json::Value = r
        .into_json()
        .map_err(|e| format!("the app's reply was not JSON: {e}"))?;
    if let Some(n) = v.get("changed").and_then(serde_json::Value::as_u64) {
        return Ok(n as usize);
    }
    if v.get("job_id").is_some() {
        return Ok(1);
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two ops with no route must fail loudly. Reporting `Ok(0)` would tell an agent the tag
    /// was removed and zero rows happened to match, which is a different and wrong story.
    #[test]
    fn ops_without_a_route_are_refused_not_silently_zero() {
        let h = lodestash_api::Handshake::new(1, "t");
        for w in [
            AgentWrite::RemoveTag {
                asset_id: 1,
                namespace: "subject".into(),
                value: "rock".into(),
            },
            AgentWrite::UnstageAsset { asset_id: 1 },
        ] {
            let err = forward(&h, w).expect_err("must refuse");
            assert!(err.contains("no REST route"), "unhelpful refusal: {err}");
        }
    }

    /// A forward with nothing listening is a plain error, not a panic or a hang — the headless
    /// server stays usable read-only when the app is closed.
    #[test]
    fn no_running_app_is_an_error_not_a_panic() {
        // Port 1 on loopback: nothing binds it, and connect fails immediately.
        let h = lodestash_api::Handshake::new(1, "t");
        let err = forward(
            &h,
            AgentWrite::AddTag {
                asset_id: 1,
                namespace: "subject".into(),
                value: "rock".into(),
            },
        )
        .expect_err("must fail");
        assert!(
            err.contains("no running Lodestash"),
            "the error should say what to do: {err}"
        );
    }
}
