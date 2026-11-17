//! Finding the app, and starting it when nothing answers (v2 spec §7, ADR-0022).
//!
//! The app publishes `engine.json` — port, token, API version and, from v2, its own binary — and
//! this server reads it again for every call, so an app started or restarted onto a new port after
//! this server is still found. When the file is missing or its port refuses, the app is launched
//! with `--background` and given [`WAIT`] to publish a handshake that answers.

use std::ffi::OsString;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use lodestash_protocol::Handshake;

/// The handshake file to use instead of the platform one. A launched app is handed the same
/// variable, naming the file this server waits on.
pub const HANDSHAKE_ENV: &str = "LODESTASH_ENGINE_JSON";
/// The app binary to launch, ahead of the handshake's `exe` and the sibling binary.
pub const APP_ENV: &str = "LODESTASH_APP";
/// The API version this server speaks; a handshake naming another is refused.
pub const API_VERSION: &str = "v1";
/// How long a launched app has to publish a handshake whose port answers.
pub const WAIT: Duration = Duration::from_secs(20);
/// How often the handshake is re-read while waiting.
const POLL: Duration = Duration::from_millis(100);

/// The app binary installed beside this one.
const APP_BINARY: &str = if cfg!(windows) {
    "lodestash-desktop.exe"
} else {
    "lodestash-desktop"
};

/// `LODESTASH_ENGINE_JSON` when set, otherwise the platform location.
pub fn handshake_path() -> Option<PathBuf> {
    match std::env::var_os(HANDSHAKE_ENV) {
        Some(path) if !path.is_empty() => Some(PathBuf::from(path)),
        _ => platform_handshake_path(),
    }
}

/// `%LOCALAPPDATA%\Lodestash\engine.json`, `~/Library/Application Support/Lodestash/engine.json`,
/// `~/.local/share/lodestash/engine.json` — the rule the app writes by (ENGINE-API §1,
/// `lodestash-api`'s `handshake_path`), restated because this binary does not link the server.
fn platform_handshake_path() -> Option<PathBuf> {
    let dir = if cfg!(target_os = "macos") {
        dirs::data_dir()?
    } else {
        dirs::data_local_dir()?
    };
    let app = if cfg!(target_os = "linux") {
        "lodestash"
    } else {
        "Lodestash"
    };
    Some(dir.join(app).join("engine.json"))
}

/// The handshake at `path`, if there is a readable one. A file being written right now reads as
/// none and is read again on the next look.
pub fn read_handshake(path: &Path) -> Option<Handshake> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Does something accept connections on this loopback port within `within`?
pub fn answers(port: u16, within: Duration) -> bool {
    TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), within).is_ok()
}

/// The binary to launch: `LODESTASH_APP`, else the handshake's last `exe`, else
/// `lodestash-desktop` beside this binary.
///
/// `LODESTASH_APP` is a decision, not a hint: when it names nothing, that is the answer — it does
/// not fall through to some other binary the person did not choose. A recorded `exe` that has
/// since moved does fall through, to the sibling.
pub fn resolve_app(
    app_env: Option<OsString>,
    last_exe: Option<&str>,
    own_exe: Option<&Path>,
) -> Result<PathBuf, String> {
    if let Some(app) = app_env.filter(|a| !a.is_empty()) {
        let app = PathBuf::from(app);
        return if app.is_file() {
            Ok(app)
        } else {
            Err(format!(
                "{APP_ENV} names {}, which is not a file",
                app.display()
            ))
        };
    }
    let mut tried = Vec::new();
    if let Some(exe) = last_exe.filter(|e| !e.is_empty()).map(PathBuf::from) {
        if exe.is_file() {
            return Ok(exe);
        }
        tried.push(exe);
    }
    if let Some(dir) = own_exe.and_then(Path::parent) {
        let sibling = dir.join(APP_BINARY);
        if sibling.is_file() {
            return Ok(sibling);
        }
        tried.push(sibling);
    }
    let tried: Vec<String> = tried.iter().map(|p| p.display().to_string()).collect();
    Err(format!(
        "no app binary found (tried {}) — set {APP_ENV} to the Lodestash app, or install \
         lodestash-mcp beside it",
        if tried.is_empty() {
            "nothing".into()
        } else {
            tried.join(", ")
        }
    ))
}

/// Start `exe --background`, detached, and wait up to `wait` for a `v1` handshake at `handshake`
/// whose port answers.
///
/// An app that exits with a failure before publishing is reported at once. One that exits
/// successfully may have handed over to an instance already running, so the wait goes on.
pub fn launch(exe: &Path, handshake: &Path, wait: Duration) -> Result<Handshake, String> {
    let mut child = spawn_detached(exe, handshake)
        .map_err(|e| format!("could not start {}: {e}", exe.display()))?;
    tracing::info!(exe = %exe.display(), handshake = %handshake.display(), "started the app");
    let until = Instant::now() + wait;
    let mut other_version = None;
    loop {
        if let Some(h) = read_handshake(handshake) {
            if h.api_version != API_VERSION {
                other_version = Some(h.api_version);
            } else if answers(h.port, POLL) {
                return Ok(h);
            }
        }
        if let Ok(Some(status)) = child.try_wait() {
            if !status.success() {
                return Err(format!(
                    "{} exited ({status}) before it published a handshake",
                    exe.display()
                ));
            }
        }
        if Instant::now() >= until {
            return Err(match other_version {
                Some(v) => format!(
                    "{} published API {v}; this server speaks {API_VERSION}",
                    exe.display()
                ),
                None => format!(
                    "{} did not publish a handshake that answers at {} within {} s",
                    exe.display(),
                    handshake.display(),
                    wait.as_secs()
                ),
            });
        }
        std::thread::sleep(POLL);
    }
}

/// `exe --background` with no stdio and no tie to this process: it outlives the host that started
/// this server, which is the point of a background app.
fn spawn_detached(exe: &Path, handshake: &Path) -> std::io::Result<Child> {
    let mut cmd = Command::new(exe);
    cmd.arg("--background")
        .env(HANDSHAKE_ENV, handshake)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window for a console build, its own Ctrl+C group, and out of the host's job
        // when the job allows it — a host that kills its job on exit would otherwise take the app.
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const ERROR_ACCESS_DENIED: i32 = 5;
        cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB);
        match cmd.spawn() {
            // A job that does not allow breakaway refuses the flag: start inside it instead.
            Err(e) if e.raw_os_error() == Some(ERROR_ACCESS_DENIED) => {
                cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
            }
            other => return other,
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn()
}

/// Keep this process's stdio out of every child it starts (Windows).
///
/// A Windows child inherits every inheritable handle of its parent, and the pipes a host hands its
/// MCP server are inheritable. A background app launched from here would hold the host's stdout
/// open long after this server exited, and a host waiting for that pipe to close would wait for
/// the app. Clearing the flag on our own three handles leaves the child only what it is given.
#[cfg(windows)]
pub fn keep_stdio_out_of_children() {
    use std::os::windows::io::AsRawHandle;

    #[link(name = "kernel32")]
    extern "system" {
        fn SetHandleInformation(handle: *mut std::ffi::c_void, mask: u32, flags: u32) -> i32;
    }
    const HANDLE_FLAG_INHERIT: u32 = 0x1;
    for handle in [
        std::io::stdin().as_raw_handle(),
        std::io::stdout().as_raw_handle(),
        std::io::stderr().as_raw_handle(),
    ] {
        if !handle.is_null() {
            // SAFETY: a handle this process holds for its whole life; only its inherit flag changes,
            // and a failure leaves it as it was.
            unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn file(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, b"").unwrap();
        p
    }

    /// `LODESTASH_APP`, then the handshake's `exe`, then the sibling — and an explicit
    /// `LODESTASH_APP` that names nothing is the answer, never a fall-through.
    #[test]
    fn the_app_binary_is_resolved_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let env_app = file(dir.path(), "chosen-app");
        let recorded = file(dir.path(), "recorded-app");
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let sibling = file(&bin, APP_BINARY);
        let own = bin.join("lodestash-mcp");
        let rec = Some(recorded.to_str().unwrap());

        assert_eq!(
            resolve_app(Some(env_app.clone().into()), rec, Some(&own)),
            Ok(env_app)
        );
        assert_eq!(resolve_app(None, rec, Some(&own)), Ok(recorded.clone()));
        assert_eq!(
            resolve_app(Some(OsString::new()), rec, Some(&own)),
            Ok(recorded.clone()),
            "an empty LODESTASH_APP is unset"
        );
        let moved = dir.path().join("moved-app");
        assert_eq!(
            resolve_app(None, Some(moved.to_str().unwrap()), Some(&own)),
            Ok(sibling.clone()),
            "a recorded exe that is gone falls through to the sibling"
        );
        assert_eq!(resolve_app(None, None, Some(&own)), Ok(sibling));

        let missing = dir.path().join("no-such-app");
        let err = resolve_app(Some(missing.into()), rec, Some(&own)).unwrap_err();
        assert!(err.contains(APP_ENV), "{err}");

        let lonely = dir.path().join("elsewhere").join("lodestash-mcp");
        let err = resolve_app(None, None, Some(&lonely)).unwrap_err();
        assert!(err.contains("no app binary"), "{err}");
    }

    /// The wait the server gives a launched app is the spec's 20 s.
    #[test]
    fn the_launch_budget_is_twenty_seconds() {
        assert_eq!(WAIT, Duration::from_secs(20));
    }

    /// A fake app that publishes the handshake it is told to, at the path the launcher names in
    /// `LODESTASH_ENGINE_JSON` — and only when started with `--background`.
    fn fake_app(dir: &Path, json: Option<&str>, exit: i32) -> PathBuf {
        #[cfg(windows)]
        {
            let path = dir.join("fake-app.cmd");
            let mut s = String::from("@echo off\r\nif not \"%~1\"==\"--background\" exit /b 3\r\n");
            if let Some(json) = json {
                s.push_str(&format!("> \"%LODESTASH_ENGINE_JSON%\" echo {json}\r\n"));
            }
            s.push_str(&format!("exit /b {exit}\r\n"));
            std::fs::write(&path, s).unwrap();
            path
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.join("fake-app");
            let mut s = String::from("#!/bin/sh\n[ \"$1\" = \"--background\" ] || exit 3\n");
            if let Some(json) = json {
                s.push_str(&format!(
                    "printf '%s' '{json}' > \"$LODESTASH_ENGINE_JSON\"\n"
                ));
            }
            s.push_str(&format!("exit {exit}\n"));
            std::fs::write(&path, s).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        }
    }

    fn handshake_for(port: u16) -> String {
        format!(r#"{{"version":1,"port":{port},"token":"t","pid":1,"api_version":"v1"}}"#)
    }

    /// The launched app gets `--background` and the handshake path, and the wait ends as soon as
    /// the handshake it publishes answers.
    #[test]
    fn a_launched_app_is_ready_when_its_handshake_answers() {
        let dir = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let exe = fake_app(dir.path(), Some(&handshake_for(port)), 0);
        let handshake = dir.path().join("engine.json");

        let start = Instant::now();
        let h = launch(&exe, &handshake, Duration::from_secs(4)).unwrap();
        assert_eq!(h.port, port);
        assert!(start.elapsed() < Duration::from_secs(4));
    }

    /// Nothing published: the wait ends at its budget with an error that names the file — not
    /// sooner, and not much later. (The production budget is `WAIT`; a shorter one keeps this fast.)
    #[test]
    fn a_launched_app_that_never_publishes_times_out_at_the_budget() {
        let dir = tempfile::tempdir().unwrap();
        let exe = fake_app(dir.path(), None, 0);
        let handshake = dir.path().join("engine.json");

        let start = Instant::now();
        let err = launch(&exe, &handshake, Duration::from_millis(1500)).unwrap_err();
        let took = start.elapsed();
        assert!(err.contains("engine.json"), "{err}");
        assert!(
            took >= Duration::from_millis(1500),
            "gave up after {took:?}"
        );
        assert!(took < Duration::from_secs(3), "overran: {took:?}");
    }

    /// A handshake whose port refuses (left behind by an app that died) is not readiness.
    #[test]
    fn a_handshake_whose_port_refuses_is_not_ready() {
        let dir = tempfile::tempdir().unwrap();
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let exe = fake_app(dir.path(), Some(&handshake_for(port)), 0);
        let err = launch(
            &exe,
            &dir.path().join("engine.json"),
            Duration::from_secs(1),
        )
        .unwrap_err();
        assert!(
            err.contains("did not publish a handshake that answers"),
            "{err}"
        );
    }

    /// An app that fails before it publishes is reported with its status, without waiting out the
    /// budget.
    #[test]
    fn a_launched_app_that_fails_is_reported_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let exe = fake_app(dir.path(), None, 7);
        let start = Instant::now();
        let err = launch(
            &exe,
            &dir.path().join("engine.json"),
            Duration::from_secs(5),
        )
        .unwrap_err();
        assert!(err.contains("exited"), "{err}");
        assert!(start.elapsed() < Duration::from_secs(3));
    }

    /// `engine.json` under `Lodestash` (`lodestash` on Linux) in the platform data directory.
    #[test]
    fn the_platform_handshake_path_is_the_one_the_app_writes() {
        // `handshake_path` reads the process environment, which tests share; the platform rule is
        // checked directly instead.
        let p = platform_handshake_path().expect("a data directory");
        assert!(p.ends_with("engine.json"), "{}", p.display());
        assert!(
            p.parent()
                .unwrap()
                .file_name()
                .unwrap()
                .eq_ignore_ascii_case("lodestash"),
            "{}",
            p.display()
        );
    }
}
