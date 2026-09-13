//! A headless MCP server's line to the running app (ENGINE-API §1, ADR-0002, v1 plan S11).
//!
//! `lodestash-desktop --mcp` serves the agent plane over stdio with its own **read-only** catalog
//! connection. Reads are safe that way — SQLite WAL admits any number of readers — but nothing
//! else is: ARCHITECTURE §5 funnels every write through one writer thread, jobs live in the app's
//! registry where the app can show and cancel them, and approvals wait on the app's window.
//!
//! So a headless server does none of those itself. It **forwards** each app-owned tool to the
//! running app's loopback REST plane — the same operation, reached the long way round — and relays
//! the app's answer. The port and token come from the `engine.json` the app publishes, read again
//! on every call, so an app that started, or restarted onto a new port, after this server did is
//! still found.
//!
//! With no app running the call fails to connect and the tool says so; reads keep working.

use std::time::Duration;

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

/// The proxy a headless server installs: the real handshake file, read per call, forwarding only
/// to an app with `catalog_id` — the catalog this server reads — open.
pub fn app_proxy(catalog_id: String) -> lodestash_api::AppProxyFn {
    proxy_with(handshake, catalog_id)
}

/// A proxy over any handshake source — the seam that lets a test restart "the app" between calls.
fn proxy_with(
    read: impl Fn() -> Option<lodestash_api::Handshake> + Send + Sync + 'static,
    catalog_id: String,
) -> lodestash_api::AppProxyFn {
    Box::new(move |method, path, body| {
        let h = read().ok_or_else(|| {
            "no running Lodestash app to hand this to — open the app; searching and reading work \
             without it"
                .to_string()
        })?;
        if h.api_version != "v1" {
            return Err(format!(
                "the running app serves API {} and this server speaks v1 — run one Lodestash \
                 version for both",
                h.api_version
            ));
        }
        // What the agent read came from this catalog; a write must land in the same one. An app
        // with another library open, or an older build that does not say, is not that app.
        if h.catalog_id.as_deref() != Some(catalog_id.as_str()) {
            return Err(
                "the running Lodestash app has a different catalog open than this server reads — \
                 nothing was sent"
                    .to_string(),
            );
        }
        call(&h, method, path, body)
    })
}

/// How long a forwarded request may take. Generous on purpose: a write waits its turn at the
/// app's writer, and the handler holding it runs off the async thread, so a long wait costs this
/// request and nothing else.
const CALL_TIMEOUT: Duration = Duration::from_secs(120);

/// One REST call to the app. A `4xx`/`5xx` is an answer — the app's error body is relayed as it is —
/// and only a request that got no answer is an `Err`.
///
/// **Never retried.** A request that timed out after it was sent may still be applied, and only a
/// request id (`write_tags`) makes a repeat safe; resending anything else could apply it twice.
fn call(
    h: &lodestash_api::Handshake,
    method: &str,
    path: &str,
    body: Option<&serde_json::Value>,
) -> Result<(u16, serde_json::Value), String> {
    let agent = ureq::AgentBuilder::new()
        // Loopback: a listening app accepts in well under a millisecond. A closed port on Windows
        // retries its SYN for seconds before refusing, so the bound is what keeps "is the app
        // there?" — asked by get_server_info too — from costing two seconds when it is not.
        .timeout_connect(Duration::from_millis(500))
        .timeout(CALL_TIMEOUT)
        // The request carries the session token; a redirect would hand it to whoever is named.
        .redirects(0)
        .build();
    let req = agent
        .request(method, &format!("http://127.0.0.1:{}{path}", h.port))
        .set("authorization", &format!("Bearer {}", h.token));
    let sent = match body {
        Some(b) => req.send_json(b.clone()),
        None => req.call(),
    };
    match sent {
        Ok(r) => Ok((r.status(), r.into_json().unwrap_or(serde_json::Value::Null))),
        Err(ureq::Error::Status(code, r)) => {
            Ok((code, r.into_json().unwrap_or(serde_json::Value::Null)))
        }
        Err(ureq::Error::Transport(t)) => Err(match t.kind() {
            ureq::ErrorKind::ConnectionFailed | ureq::ErrorKind::Dns => format!(
                "no running Lodestash app answered ({t}) — open the app; searching and reading \
                 work without it"
            ),
            _ => format!(
                "the app did not answer ({t}). If this was a change, it may still have been \
                 applied — check before asking again"
            ),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// A one-request HTTP server that answers `status` with `body` and reports the request line.
    fn one_shot(status: u16, body: &'static str) -> (u16, std::thread::JoinHandle<String>) {
        let l = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = l.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut sock, _) = l.accept().expect("accept");
            sock.set_read_timeout(Some(Duration::from_millis(300))).ok();
            let mut seen = Vec::new();
            let mut buf = [0u8; 4096];
            while let Ok(n) = sock.read(&mut buf) {
                if n == 0 {
                    break;
                }
                seen.extend_from_slice(&buf[..n]);
            }
            let _ = sock.write_all(
                format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            );
            String::from_utf8_lossy(&seen)
                .lines()
                .next()
                .unwrap_or_default()
                .to_string()
        });
        (port, server)
    }

    /// The app can restart onto a new port between two requests, and the second still reaches it:
    /// the handshake is read for every call, not once at startup (v1 plan S11).
    #[test]
    fn a_restarted_app_is_found_on_the_next_call() {
        let (first, a) = one_shot(202, r#"{"id":"identify_1","state":"running"}"#);
        let (second, b) = one_shot(200, r#"{"id":"identify_1","state":"done"}"#);
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let proxy = proxy_with(
            move || {
                let n = calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Some(
                    lodestash_api::Handshake::new(if n == 0 { first } else { second }, "t")
                        .with_catalog("cat"),
                )
            },
            "cat".into(),
        );

        let (status, body) = proxy(
            "POST",
            "/v1/identify",
            Some(&serde_json::json!({"asset_ids": [1]})),
        )
        .expect("the first app answers");
        assert_eq!((status, body["state"].as_str()), (202, Some("running")));
        assert_eq!(a.join().unwrap(), "POST /v1/identify HTTP/1.1");

        let (status, body) =
            proxy("GET", "/v1/jobs/identify_1", None).expect("the restarted app answers");
        assert_eq!((status, body["state"].as_str()), (200, Some("done")));
        assert_eq!(b.join().unwrap(), "GET /v1/jobs/identify_1 HTTP/1.1");
    }

    /// A refusal is the app's answer and is relayed with its status, not turned into "no app".
    #[test]
    fn the_apps_refusal_is_relayed_as_an_answer() {
        let (port, server) = one_shot(409, r#"{"error":{"code":"project_not_registered"}}"#);
        let proxy = proxy_with(
            move || Some(lodestash_api::Handshake::new(port, "t").with_catalog("cat")),
            "cat".into(),
        );
        let (status, body) = proxy("POST", "/v1/stage/import", None).expect("an answer");
        assert_eq!(status, 409);
        assert_eq!(body["error"]["code"], "project_not_registered");
        server.join().unwrap();
    }

    /// No handshake, or nothing listening: a plain error that says what to do — never a panic or a
    /// hang, and the server stays usable for reads.
    #[test]
    fn no_running_app_is_an_error_not_a_panic() {
        let none = proxy_with(|| None, "cat".into());
        let err = none("GET", "/v1/jobs/x", None).expect_err("no handshake");
        assert!(err.contains("no running Lodestash"), "unhelpful: {err}");

        // Port 1 on loopback: nothing binds it, and connect fails immediately.
        let closed = proxy_with(
            || Some(lodestash_api::Handshake::new(1, "t").with_catalog("cat")),
            "cat".into(),
        );
        let err = closed("GET", "/v1/jobs/x", None).expect_err("nothing listening");
        assert!(err.contains("no running Lodestash"), "unhelpful: {err}");
    }

    /// An app with another catalog open is not this server's app: nothing is sent to it, however
    /// well it would have answered (v1 plan S11). An app that does not name its catalog is treated
    /// the same way.
    #[test]
    fn an_app_with_another_catalog_is_never_sent_anything() {
        let l = TcpListener::bind("127.0.0.1:0").expect("bind");
        l.set_nonblocking(true).expect("nonblocking");
        let port = l.local_addr().unwrap().port();
        for named in [Some("someone-elses"), None] {
            let proxy = proxy_with(
                move || {
                    let h = lodestash_api::Handshake::new(port, "t");
                    Some(match named {
                        Some(id) => h.with_catalog(id),
                        None => h,
                    })
                },
                "cat".into(),
            );
            let err = proxy(
                "POST",
                "/v1/stage",
                Some(&serde_json::json!({"asset_id": 1})),
            )
            .expect_err("a different catalog");
            assert!(err.contains("different catalog"), "unhelpful: {err}");
        }
        std::thread::sleep(Duration::from_millis(100));
        assert!(l.accept().is_err(), "the other app was contacted");
    }
}
