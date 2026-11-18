//! One operation, sent to the app: find it, start it if it is gone, send the request once, and
//! bring back exactly what it answered (v2 spec §7).
//!
//! **Never retried.** A request that reached the app and got no answer may still have been
//! applied, and only a request id (`write_tags`) makes a repeat safe — a decision that belongs to
//! the caller (ADR-0011's rule, kept by ADR-0022). The one time a request is sent after a failure is
//! when the first attempt never connected: then nothing was sent.
//!
//! **Sent only to a live app.** The request carries the session token, so it goes to the
//! handshake's port only while the process that wrote the handshake is running
//! ([`launch::find`], ADR-0024). A file left by a dead app leads to the launch path, not to
//! whoever holds its port now.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use lodestash_protocol::{ApiError, Handshake, WireRequest};
use serde_json::json;

use crate::launch::{self, Found, LaunchError, Launcher};

/// The error code for "no app answered" (ENGINE-API §8.4).
pub const APP_UNAVAILABLE: &str = "app_unavailable";
/// The error code for "the app this server started is still coming up — ask again"
/// (ENGINE-API §8.4). Nothing was sent.
pub const APP_STARTING: &str = "app_starting";
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
    /// How long one call waits for a launched app.
    wait: Duration,
    /// `LODESTASH_APP`, as this server was started with it.
    app_env: Option<OsString>,
    /// This binary, beside which the app is installed.
    own_exe: Option<PathBuf>,
    /// The app this server started, while it runs: one launch at a time.
    launcher: Launcher,
}

impl App {
    /// The handshake from `LODESTASH_ENGINE_JSON` or the platform path, the app binary from
    /// `LODESTASH_APP` or beside this one; a launched app gets [`launch::WAIT`] per call.
    pub fn from_env() -> App {
        App::new(
            launch::handshake_path(),
            launch::WAIT,
            std::env::var_os(launch::APP_ENV),
            std::env::current_exe().ok(),
        )
    }

    fn new(
        handshake: Option<PathBuf>,
        wait: Duration,
        app_env: Option<OsString>,
        own_exe: Option<PathBuf>,
    ) -> App {
        App {
            handshake,
            wait,
            app_env,
            own_exe,
            launcher: Launcher::default(),
        }
    }

    pub fn handshake_path(&self) -> Option<&Path> {
        self.handshake.as_deref()
    }

    /// Perform `req` on the app; the reply, and the port it came from. Any status is a reply —
    /// only "nobody answered" is an `Err`: `app_unavailable`, or `app_starting` while an app this
    /// server started is still coming up.
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
        // Liveness, then version, then the request: a file left by a dead app is neither an
        // address nor a version to refuse.
        let recorded_exe = match launch::find(path) {
            Found::Live(h) => match send(&h, req) {
                Ok(reply) => return Ok((reply, h.port)),
                Err(Failure::NotSent(why)) => {
                    tracing::info!(port = h.port, %why, "the app in the handshake does not answer");
                    h.exe
                }
                Err(failure) => return Err(self.failed(failure)),
            },
            // A running app of another version is the app: starting a second would not help.
            Found::OtherVersion(h) if launch::answers(h.port, CONNECT_TIMEOUT) => {
                return Err(self.other_version(&h.api_version))
            }
            Found::OtherVersion(h) | Found::Stale(Some(h)) => h.exe,
            Found::Stale(None) => None,
        };
        let h = self.start_app(path, recorded_exe)?;
        send(&h, req)
            .map(|reply| (reply, h.port))
            .map_err(|failure| self.failed(failure))
    }

    /// A handshake that answers: the app this server already started, or a new one.
    fn start_app(&self, path: &Path, recorded_exe: Option<String>) -> Result<Handshake, ApiError> {
        let binary = || {
            launch::resolve_app(
                self.app_env.as_deref(),
                self.own_exe.as_deref(),
                recorded_exe.as_deref(),
            )
        };
        self.launcher
            .ready(path, self.wait, binary)
            .map_err(|e| match e {
                LaunchError::NotStarted(why) | LaunchError::NothingPublished(why) => self
                    .unavailable(
                        format!("no Lodestash app is answering, and none could be started: {why}"),
                        false,
                    ),
                LaunchError::OtherVersion(version) => self.other_version(&version),
                LaunchError::Starting(why) => ApiError::new(
                    APP_STARTING,
                    format!(
                        "the Lodestash app is starting and not ready yet ({why}). Nothing was \
                         sent — ask again in a few seconds"
                    ),
                )
                .with_detail(self.detail(false)),
            })
    }

    fn other_version(&self, version: &str) -> ApiError {
        self.unavailable(
            format!(
                "the running app serves API {version} and this server speaks {} — run matching \
                 versions of Lodestash and lodestash-mcp",
                launch::API_VERSION
            ),
            false,
        )
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

    /// `app_unavailable`, with what a host needs to debug its setup.
    fn unavailable(&self, message: String, sent: bool) -> ApiError {
        ApiError::new(APP_UNAVAILABLE, message).with_detail(self.detail(sent))
    }

    /// The handshake file this server reads, whether the request was sent, and this server's
    /// version. A model reads this, so the file is named without the home directory.
    fn detail(&self, sent: bool) -> serde_json::Value {
        json!({
            "handshake": self.handshake.as_deref().map(launch::shown),
            "sent": sent,
            "mcp_version": env!("CARGO_PKG_VERSION"),
        })
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

#[cfg(test)]
mod tests {
    use lodestash_protocol::find_op;

    use super::*;
    use crate::testkit::fake_app::{handshake_json_for, launches, FakeApp};
    use crate::testkit::raw::{Behaviour, RawListener};

    fn search() -> WireRequest {
        find_op("search_assets")
            .unwrap()
            .request(&json!({"query": "rock"}))
            .unwrap()
    }

    /// An app that takes longer to start than one call waits is remembered, not forgotten: the
    /// calls that follow wait for the same process instead of each starting another, they answer
    /// the retryable `app_starting` rather than `app_unavailable`, and the first call after it is
    /// up goes through — to the one app that was ever started.
    ///
    /// Budget: about 4 s — a 2 s start, watched by calls that each wait 0.6 s.
    #[test]
    fn a_slow_start_is_remembered_not_repeated() {
        let dir = tempfile::tempdir().unwrap();
        let handshake = dir.path().join("engine.json");
        let served = RawListener::start(Behaviour::Answer(r#"{"results":[]}"#));
        let exe = FakeApp {
            count_launches: true,
            start_secs: 2,
            handshake: Some(handshake_json_for(
                served.port,
                "t",
                std::process::id(),
                "v1",
            )),
            linger_secs: 3,
            ..FakeApp::default()
        }
        .write(dir.path(), "slow-app");
        let app = App::new(
            Some(handshake),
            Duration::from_millis(600),
            Some(exe.into_os_string()),
            None,
        );

        for call in 1..=2 {
            let err = app.call(&search()).unwrap_err();
            assert_eq!(err.code, APP_STARTING, "call {call}: {err}");
            assert_eq!(err.detail.as_ref().unwrap()["sent"], false, "call {call}");
            assert_eq!(launches(dir.path()), 1, "call {call} started another app");
        }
        assert!(
            served.requests().is_empty(),
            "nothing is sent while it starts"
        );

        // Calls keep arriving; one of them finds the app up.
        let began = std::time::Instant::now();
        let answered = loop {
            match app.call(&search()) {
                Ok((reply, port)) => break (reply.status, port),
                Err(e) if e.code == APP_STARTING => {}
                Err(e) => panic!("{e}"),
            }
            assert!(
                began.elapsed() < Duration::from_secs(4),
                "the app never came up"
            );
        };
        assert_eq!(answered, (200, served.port));
        assert_eq!(launches(dir.path()), 1, "exactly one app was ever started");
        assert_eq!(served.requests().len(), 1, "{:?}", served.requests());
    }
}
