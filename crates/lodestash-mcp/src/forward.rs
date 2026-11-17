//! One operation, sent to the app: find it, start it if it is gone, send the request once, and
//! bring back exactly what it answered (v2 spec §7).
//!
//! **Never retried.** A request that reached the app and got no answer may still have been
//! applied, and only a request id (`write_tags`) makes a repeat safe — a decision that belongs to
//! the caller (ADR-0011's rule, kept by ADR-0022). The one time a request is sent after a failure is
//! when the first attempt never connected: then nothing was sent.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use lodestash_protocol::{ApiError, Handshake, WireRequest};
use serde_json::json;

use crate::launch;

/// The error code for "no app answered" (ENGINE-API §8.4).
pub const APP_UNAVAILABLE: &str = "app_unavailable";
/// Loopback connects in well under a millisecond when something listens. A closed port on Windows
/// retries its SYN for about two seconds before refusing, so the bound is what keeps "is the app
/// there?" cheap when it is not.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
/// Generous on purpose: a write waits its turn at the app's writer, and a preview may render. The
/// call runs on the blocking pool, so a long wait costs this call and nothing else.
const CALL_TIMEOUT: Duration = Duration::from_secs(120);
/// The largest answer relayed. The biggest real ones are a batch of 100 asset details and a
/// rendered preview, both far below this.
const MAX_BODY: u64 = 64 << 20;

/// The app's answer, as it came: status, content type, body.
#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
}

/// The app this server speaks to, found through its handshake file.
pub struct App {
    handshake: Option<PathBuf>,
    wait: Duration,
    /// One launch at a time: a call that arrives while the app is starting waits for that start.
    launching: Mutex<()>,
}

impl App {
    /// The handshake from `LODESTASH_ENGINE_JSON` or the platform path; a launched app gets
    /// [`launch::WAIT`].
    pub fn from_env() -> App {
        App {
            handshake: launch::handshake_path(),
            wait: launch::WAIT,
            launching: Mutex::new(()),
        }
    }

    pub fn handshake_path(&self) -> Option<&Path> {
        self.handshake.as_deref()
    }

    /// Perform `req` on the app; the reply, and the port it came from. Any status is a reply —
    /// only "nobody answered" is an `Err`, and it is always `app_unavailable`.
    pub fn call(&self, req: &WireRequest) -> Result<(Reply, u16), ApiError> {
        let path = self.handshake.as_deref().ok_or_else(|| {
            self.unavailable(
                format!(
                    "this system has no data directory for engine.json; set {}",
                    launch::HANDSHAKE_ENV
                ),
                false,
            )
        })?;
        let found = launch::read_handshake(path);
        if let Some(h) = &found {
            self.check_version(h)?;
            match send(h, req) {
                Ok(reply) => return Ok((reply, h.port)),
                Err(Failure::NotSent(why)) => {
                    tracing::info!(port = h.port, %why, "the app in the handshake does not answer");
                }
                Err(failure) => return Err(self.failed(failure)),
            }
        }
        let h = self.start_app(path, found)?;
        send(&h, req)
            .map(|reply| (reply, h.port))
            .map_err(|failure| self.failed(failure))
    }

    /// A handshake that answers, starting the app if there is none.
    fn start_app(&self, path: &Path, found: Option<Handshake>) -> Result<Handshake, ApiError> {
        let _one = self
            .launching
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Another call may have started the app while this one waited for the lock.
        let now = launch::read_handshake(path);
        if let Some(h) = now.as_ref().filter(|h| Some(*h) != found.as_ref()) {
            self.check_version(h)?;
            if launch::answers(h.port, CONNECT_TIMEOUT) {
                return Ok(h.clone());
            }
        }
        let last_exe = now.or(found).and_then(|h| h.exe);
        let not_started = |why: String| {
            self.unavailable(
                format!("no Lodestash app is answering, and none could be started: {why}"),
                false,
            )
        };
        let exe = launch::resolve_app(
            std::env::var_os(launch::APP_ENV),
            last_exe.as_deref(),
            std::env::current_exe().ok().as_deref(),
        )
        .map_err(not_started)?;
        launch::launch(&exe, path, self.wait).map_err(not_started)
    }

    fn check_version(&self, h: &Handshake) -> Result<(), ApiError> {
        if h.api_version == launch::API_VERSION {
            return Ok(());
        }
        Err(self.unavailable(
            format!(
                "the running app serves API {} and this server speaks {} — run matching versions \
                 of Lodestash and lodestash-mcp",
                h.api_version,
                launch::API_VERSION
            ),
            false,
        ))
    }

    fn failed(&self, failure: Failure) -> ApiError {
        match failure {
            Failure::NotSent(why) => self.unavailable(
                format!("the app's handshake names a port that refuses the connection ({why})"),
                false,
            ),
            Failure::NoAnswer(why) => self.unavailable(
                format!(
                    "the app did not answer ({why}). If this was a change it may still have been \
                     applied — check before asking again; nothing was retried"
                ),
                true,
            ),
            Failure::TooLarge => ApiError::new(
                "too_large",
                format!("the app's answer is larger than {MAX_BODY} bytes"),
            ),
        }
    }

    /// `app_unavailable`, with what a host needs to debug its setup: the handshake file this server
    /// reads, whether the request was sent, and this server's version.
    fn unavailable(&self, message: String, sent: bool) -> ApiError {
        ApiError::new(APP_UNAVAILABLE, message).with_detail(json!({
            "handshake": self.handshake.as_ref().map(|p| p.display().to_string()),
            "sent": sent,
            "mcp_version": env!("CARGO_PKG_VERSION"),
        }))
    }
}

enum Failure {
    /// The connection was refused or timed out: nothing reached the app.
    NotSent(String),
    /// The request went out and no complete answer came back.
    NoAnswer(String),
    TooLarge,
}

/// One HTTP request to the app, with its bearer token. Any status is a reply.
fn send(h: &Handshake, req: &WireRequest) -> Result<Reply, Failure> {
    // A fresh agent per call: there is no pooled connection for ureq to quietly retry on.
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(CONNECT_TIMEOUT)
        .timeout(CALL_TIMEOUT)
        // The request carries the session token; a redirect would hand it to whoever is named.
        .redirects(0)
        .build();
    let request = agent
        .request(
            req.method.as_str(),
            &format!("http://127.0.0.1:{}{}", h.port, req.target),
        )
        .set("Authorization", &format!("Bearer {}", h.token));
    let sent = match &req.body {
        Some(body) => request
            .set("Content-Type", "application/json")
            .send_string(&body.to_string()),
        None => request.call(),
    };
    let response = match sent {
        Ok(r) | Err(ureq::Error::Status(_, r)) => r,
        Err(ureq::Error::Transport(t)) => {
            return Err(if matches!(t.kind(), ureq::ErrorKind::ConnectionFailed) {
                Failure::NotSent(t.to_string())
            } else {
                Failure::NoAnswer(t.to_string())
            })
        }
    };
    let status = response.status();
    let content_type = response.content_type().to_string();
    let mut body = Vec::new();
    response
        .into_reader()
        .take(MAX_BODY + 1)
        .read_to_end(&mut body)
        .map_err(|e| Failure::NoAnswer(format!("reading the answer failed: {e}")))?;
    if body.len() as u64 > MAX_BODY {
        return Err(Failure::TooLarge);
    }
    Ok(Reply {
        status,
        content_type,
        body,
    })
}
