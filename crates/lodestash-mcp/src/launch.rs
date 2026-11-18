//! Finding the app, and starting it when nothing answers (v2 spec §7, ADR-0022, ADR-0024).
//!
//! The app publishes `engine.json` — port, token, API version, its pid and, from v2, its own
//! binary — and this server reads it again for every call, so an app started or restarted onto a
//! new port after this server is still found.
//!
//! **A handshake is an address only while the process that wrote it is alive** ([`find`]). The
//! file outlives a crashed app, and the port in it is then anyone's: a request sent there would
//! hand the session token to whoever holds the port now. So liveness is checked before anything
//! else — before the version, before a connection.
//!
//! When no live app answers, the app is launched with `--background` and each call gives it
//! [`WAIT`] to publish a handshake that answers. **One launch at a time** ([`Launcher`]): the
//! started process is remembered, and while it runs later calls wait for it instead of starting
//! another.

use std::ffi::OsStr;
use std::net::{SocketAddr, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use lodestash_protocol::Handshake;

/// The handshake file to use instead of the platform one. A launched app is handed the same
/// variable, naming the file this server waits on.
pub const HANDSHAKE_ENV: &str = "LODESTASH_ENGINE_JSON";
/// The app binary to launch, ahead of the sibling binary and the handshake's `exe`.
pub const APP_ENV: &str = "LODESTASH_APP";
/// The API version this server speaks; a running app that serves another is refused.
pub const API_VERSION: &str = "v1";
/// How long one call waits for a launched app to publish a handshake whose port answers.
pub const WAIT: Duration = Duration::from_secs(20);
/// How often the handshake is re-read while waiting.
const POLL: Duration = Duration::from_millis(100);

/// The app binary installed beside this one.
const APP_BINARY: &str = if cfg!(windows) {
    "lodestash-desktop.exe"
} else {
    "lodestash-desktop"
};
/// The only file names a handshake's `exe` may carry.
const APP_BINARY_NAMES: [&str; 2] = ["lodestash-desktop", "lodestash-desktop.exe"];

/// `LODESTASH_ENGINE_JSON` when set, otherwise the platform location.
pub fn handshake_path() -> Option<PathBuf> {
    match std::env::var_os(HANDSHAKE_ENV) {
        Some(path) if !path.is_empty() => Some(PathBuf::from(path)),
        _ => platform_handshake_path(),
    }
}

/// `%LOCALAPPDATA%\Lodestash\engine.json`, `~/Library/Application Support/Lodestash/engine.json`,
/// `~/.local/share/lodestash/engine.json` — the rule the app writes by (ENGINE-API §1,
/// `lodestash-api`'s `handshake_path`), restated because this binary does not link the server. A
/// test holds the two to the same answer.
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

/// What the handshake file says about the app.
#[derive(Debug)]
pub enum Found {
    /// A live process that speaks this server's API version. Its port is still to be tried.
    Live(Handshake),
    /// A live process that serves another API version.
    OtherVersion(Handshake),
    /// No file, an unreadable one, or one whose process is gone. Never an address to send to;
    /// its `exe`, if it recorded one, is still a hint for the launcher.
    Stale(Option<Handshake>),
}

/// Read the handshake at `path` and judge it — liveness first, then the version. A file left
/// by a dead app of any version is stale; only a running one can be "the wrong version".
pub fn find(path: &Path) -> Found {
    match read_handshake(path) {
        Some(h) if !pid_alive(h.pid) => Found::Stale(Some(h)),
        Some(h) if h.api_version != API_VERSION => Found::OtherVersion(h),
        Some(h) => Found::Live(h),
        None => Found::Stale(None),
    }
}

/// Does something accept connections on this loopback port within `within`?
pub fn answers(port: u16, within: Duration) -> bool {
    TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), within).is_ok()
}

/// Is `pid` a running process this user can see?
///
/// The app runs as the user who started this server and writes the handshake into that user's
/// own profile, so the app's process can always be queried. A pid that cannot be — it belongs to
/// another user, or to nothing — is not the app, and reads as not alive.
///
/// - **Windows:** `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` and `GetExitCodeProcess`; alive
///   is `STILL_ACTIVE`. (A process that exited with code 259 reads as alive — the port must then
///   answer too before anything is sent.)
/// - **Linux, macOS:** `kill(pid, 0)`, which delivers nothing; alive is success. `EPERM` (another
///   user's process) and `ESRCH` (no process) are not.
///
/// The OS reuses pids: a dead app's pid can come to name an unrelated live process, and this
/// then answers `true`. The window is narrow — that process must appear while the stale file's
/// port is also held by a third party — and the app removes its handshake on a clean exit.
pub fn pid_alive(pid: u32) -> bool {
    // 0 is "no process" everywhere; on Unix it and anything past i32 would also address a
    // process group instead of a process.
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    #[cfg(windows)]
    {
        use std::ffi::c_void;

        #[link(name = "kernel32")]
        extern "system" {
            fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
            fn GetExitCodeProcess(process: *mut c_void, code: *mut u32) -> i32;
            fn CloseHandle(handle: *mut c_void) -> i32;
        }
        const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
        const STILL_ACTIVE: u32 = 259;

        // SAFETY: plain kernel32 calls. The handle is checked for null, used for one query that
        // writes one u32 we own, and closed on every path.
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() {
                return false;
            }
            let mut code = 0u32;
            let known = GetExitCodeProcess(process, &mut code) != 0;
            CloseHandle(process);
            known && code == STILL_ACTIVE
        }
    }
    #[cfg(unix)]
    {
        extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        // SAFETY: signal 0 performs the permission and existence checks and sends nothing.
        unsafe { kill(pid as i32, 0) == 0 }
    }
    #[cfg(not(any(windows, unix)))]
    {
        false
    }
}

/// `path` as a model may read it: the home directory — and the OS user name in it — shown as `~`.
/// (A path spelled some other way than the OS reports the home directory, a Windows 8.3 short
/// name for one, is shown as it is.)
pub fn shown(path: &Path) -> String {
    shown_from(path, dirs::home_dir().as_deref())
}

fn shown_from(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => Path::new("~").join(rest).display().to_string(),
        None => path.display().to_string(),
    }
}

/// The binary to launch: `LODESTASH_APP`, else `lodestash-desktop` beside this binary, else the
/// `exe` the handshake recorded.
///
/// `LODESTASH_APP` is a decision, not a hint: when it names nothing, that is the answer — it does
/// not fall through to some other binary the person did not choose.
///
/// The handshake's `exe` comes last and is the least trusted: it is a string in a file, and
/// running it is running whatever that file says. The co-installed binary outranks it, and it is
/// used only when it passes [`trusted_exe`].
pub fn resolve_app(
    app_env: Option<&OsStr>,
    own_exe: Option<&Path>,
    recorded_exe: Option<&str>,
) -> Result<PathBuf, String> {
    if let Some(app) = app_env.filter(|a| !a.is_empty()) {
        let app = PathBuf::from(app);
        return if app.is_file() {
            Ok(app)
        } else {
            Err(format!(
                "{APP_ENV} names {}, which is not a file",
                shown(&app)
            ))
        };
    }
    let mut tried = Vec::new();
    if let Some(dir) = own_exe.and_then(Path::parent) {
        let sibling = dir.join(APP_BINARY);
        if sibling.is_file() {
            return Ok(sibling);
        }
        tried.push(shown(&sibling));
    }
    if let Some(recorded) = recorded_exe.filter(|e| !e.is_empty()) {
        match trusted_exe(recorded) {
            Ok(exe) if exe.is_file() => return Ok(exe),
            Ok(exe) => tried.push(shown(&exe)),
            Err(why) => tried.push(format!(
                "{} (the handshake's exe, not used: {why})",
                shown(Path::new(recorded))
            )),
        }
    }
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

/// A handshake's `exe` as a path this server is willing to run: named `lodestash-desktop` (or
/// `lodestash-desktop.exe`), absolute, and on a local disk.
///
/// On Windows "local" is a drive-letter path (`C:\…`, `\\?\C:\…`) whose drive is not a network
/// drive (`GetDriveTypeW`); UNC (`\\server\share\…`, `\\?\UNC\…`) and device paths are refused.
/// On Linux and macOS it is any absolute path: a network mount looks like any other directory
/// there and is not told apart.
pub fn trusted_exe(recorded: &str) -> Result<PathBuf, &'static str> {
    let path = PathBuf::from(recorded);
    let named = path
        .file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| {
            APP_BINARY_NAMES
                .iter()
                .any(|app| name.eq_ignore_ascii_case(app))
        });
    if !named {
        return Err("it is not named lodestash-desktop");
    }
    if !path.is_absolute() {
        return Err("it is not an absolute path");
    }
    if !on_local_disk(&path) {
        return Err("it is not on a local disk");
    }
    Ok(path)
}

#[cfg(windows)]
const DRIVE_UNKNOWN: u32 = 0;
#[cfg(windows)]
const DRIVE_NO_ROOT_DIR: u32 = 1;
#[cfg(windows)]
const DRIVE_REMOTE: u32 = 4;

/// `GetDriveTypeW` for the drive `letter:` — fixed, removable, network, or nothing there.
#[cfg(windows)]
fn drive_type(letter: u8) -> u32 {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetDriveTypeW(root: *const u16) -> u32;
    }
    let root: Vec<u16> = OsStr::new(&format!("{}:\\", letter as char))
        .encode_wide()
        .chain(Some(0))
        .collect();
    // SAFETY: `root` is a NUL-terminated UTF-16 string that outlives the call.
    unsafe { GetDriveTypeW(root.as_ptr()) }
}

#[cfg(windows)]
fn on_local_disk(path: &Path) -> bool {
    use std::path::Prefix;

    let Some(Component::Prefix(prefix)) = path.components().next() else {
        return false;
    };
    match prefix.kind() {
        Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => !matches!(
            drive_type(letter),
            DRIVE_UNKNOWN | DRIVE_NO_ROOT_DIR | DRIVE_REMOTE
        ),
        // UNC shares, `\\.\` devices and other verbatim paths.
        _ => false,
    }
}

#[cfg(not(windows))]
fn on_local_disk(path: &Path) -> bool {
    matches!(path.components().next(), Some(Component::RootDir))
}

/// Why no app could be reached through a launch.
#[derive(Debug, PartialEq, Eq)]
pub enum LaunchError {
    /// No binary to start, one that would not start, or one that exited with a failure before it
    /// published anything.
    NotStarted(String),
    /// A running app serves this API version instead of [`API_VERSION`].
    OtherVersion(String),
    /// The app this server started is still running and has not published a handshake that
    /// answers within this call's wait. Nothing is wrong yet: ask again.
    Starting(String),
    /// The started process left without a failure and no handshake answers.
    NothingPublished(String),
}

/// The app this server started, if it is still running — the memory that keeps a slow start from
/// becoming several apps.
#[derive(Default)]
pub struct Launcher {
    state: Mutex<Launches>,
}

#[derive(Default)]
struct Launches {
    /// The started process, until it is seen to have exited.
    running: Option<(Child, PathBuf)>,
    /// Counts starts, so a call can tell the launch it is waiting on from a later one.
    generation: u64,
    /// The start that exited with a failure before publishing, and what to say about it.
    failed: Option<(u64, String)>,
}

impl Launches {
    /// Notice a started process that has exited.
    fn reap(&mut self) {
        let Some((child, exe)) = self.running.as_mut() else {
            return;
        };
        match child.try_wait() {
            Ok(None) => return,
            Ok(Some(status)) if !status.success() => {
                self.failed = Some((
                    self.generation,
                    format!(
                        "{} exited ({status}) before it published a handshake",
                        shown(exe)
                    ),
                ));
            }
            // Exited cleanly — it may have handed over to an instance already running — or its
            // state cannot be read. Either way it is no longer ours to wait on.
            Ok(Some(_)) | Err(_) => {}
        }
        self.running = None;
    }
}

impl Launcher {
    fn state(&self) -> std::sync::MutexGuard<'_, Launches> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// A handshake at `handshake` that answers, starting the app if this server has none
    /// running, and waiting up to `wait`.
    ///
    /// `binary` names what to start; it is asked only when something has to be started. While a
    /// process this server started is still running, no other is started: the call waits for
    /// that one, and answers [`LaunchError::Starting`] if it is still coming up when `wait` ends.
    ///
    /// A start that exits with a failure before publishing is reported at once. One that exits
    /// cleanly may have handed over to an instance already running, so the wait goes on.
    pub fn ready(
        &self,
        handshake: &Path,
        wait: Duration,
        binary: impl FnOnce() -> Result<PathBuf, String>,
    ) -> Result<Handshake, LaunchError> {
        let until = Instant::now() + wait;
        let generation = {
            let mut state = self.state();
            // Another call may have brought the app up while this one waited its turn.
            match find(handshake) {
                Found::Live(h) if answers(h.port, POLL) => return Ok(h),
                Found::OtherVersion(h) if answers(h.port, POLL) => {
                    return Err(LaunchError::OtherVersion(h.api_version))
                }
                _ => {}
            }
            state.reap();
            if state.running.is_none() {
                let exe = binary().map_err(LaunchError::NotStarted)?;
                let child = spawn_detached(&exe, handshake).map_err(|e| {
                    LaunchError::NotStarted(format!("could not start {}: {e}", shown(&exe)))
                })?;
                tracing::info!(exe = %exe.display(), handshake = %handshake.display(), "started the app");
                state.generation += 1;
                state.running = Some((child, exe));
            }
            state.generation
        };
        let mut other_version = None;
        loop {
            match find(handshake) {
                Found::Live(h) if answers(h.port, POLL) => return Ok(h),
                // A running app of another version will not turn into this one by waiting.
                Found::OtherVersion(h) if answers(h.port, POLL) => {
                    return Err(LaunchError::OtherVersion(h.api_version))
                }
                Found::OtherVersion(h) => other_version = Some(h.api_version),
                Found::Live(_) | Found::Stale(_) => {}
            }
            let starting = {
                let mut state = self.state();
                state.reap();
                if let Some((failed, why)) = &state.failed {
                    if *failed == generation {
                        return Err(LaunchError::NotStarted(why.clone()));
                    }
                }
                state.running.as_ref().map(|(_, exe)| shown(exe))
            };
            if Instant::now() >= until {
                return Err(match (other_version, starting) {
                    (Some(version), _) => LaunchError::OtherVersion(version),
                    (None, Some(exe)) => LaunchError::Starting(format!(
                        "{exe} is still starting: no handshake answers at {} after {} s",
                        shown(handshake),
                        wait.as_secs()
                    )),
                    (None, None) => LaunchError::NothingPublished(format!(
                        "the app was started and left without publishing a handshake that \
                         answers at {} within {} s",
                        shown(handshake),
                        wait.as_secs()
                    )),
                });
            }
            std::thread::sleep(POLL);
        }
    }
}

/// `exe --background` with no stdio and no tie to this process: it outlives the host that started
/// this server, which is the point of a background app. It starts in its own directory — not the
/// host's, which it would otherwise hold open and resolve relative paths against.
fn spawn_detached(exe: &Path, handshake: &Path) -> std::io::Result<Child> {
    let mut cmd = Command::new(exe);
    cmd.arg("--background")
        .env(HANDSHAKE_ENV, handshake)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(dir) = exe.parent().filter(|d| d.is_dir()) {
        cmd.current_dir(dir);
    }
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
    use std::net::TcpListener;

    use super::*;
    use crate::testkit::fake_app::{gone_process, handshake_json_for, FakeApp};
    use crate::testkit::raw::{Behaviour, RawListener};

    fn file(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, b"").unwrap();
        p
    }

    /// A live handshake for `port`: this test process stands in for the app's.
    fn live(port: u16) -> String {
        handshake_json_for(port, "t", std::process::id(), "v1")
    }

    fn publishes(dir: &Path, handshake: Option<String>, exit: i32) -> PathBuf {
        FakeApp {
            handshake,
            exit,
            ..FakeApp::default()
        }
        .write(dir, "fake-app")
    }

    fn start(exe: &Path, handshake: &Path, wait: Duration) -> Result<Handshake, LaunchError> {
        Launcher::default().ready(handshake, wait, || Ok(exe.to_path_buf()))
    }

    /// `LODESTASH_APP`, then the binary installed beside this one, then the handshake's `exe` —
    /// and an explicit `LODESTASH_APP` that names nothing is the answer, never a fall-through.
    #[test]
    fn the_app_binary_is_resolved_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let env_app = file(dir.path(), "chosen-app");
        let elsewhere = dir.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        let recorded = file(&elsewhere, APP_BINARY);
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let sibling = file(&bin, APP_BINARY);
        let own = bin.join("lodestash-mcp");
        let rec = Some(recorded.to_str().unwrap());

        assert_eq!(
            resolve_app(Some(env_app.as_os_str()), Some(&own), rec),
            Ok(env_app)
        );
        assert_eq!(
            resolve_app(None, Some(&own), rec),
            Ok(sibling.clone()),
            "the co-installed app outranks whatever the handshake names"
        );
        assert_eq!(
            resolve_app(Some(OsStr::new("")), Some(&own), rec),
            Ok(sibling.clone()),
            "an empty LODESTASH_APP is unset"
        );
        assert_eq!(resolve_app(None, Some(&own), None), Ok(sibling));

        let lonely = dir.path().join("lonely").join("lodestash-mcp");
        assert_eq!(
            resolve_app(None, Some(&lonely), rec),
            Ok(recorded.clone()),
            "with no app beside this binary, the recorded one is used"
        );
        let moved = elsewhere.join("moved").join(APP_BINARY);
        let err = resolve_app(None, Some(&lonely), moved.to_str()).unwrap_err();
        assert!(err.contains("no app binary"), "{err}");

        let missing = dir.path().join("no-such-app");
        let err = resolve_app(Some(missing.as_os_str()), Some(&own), rec).unwrap_err();
        assert!(err.contains(APP_ENV), "{err}");
    }

    /// The handshake's `exe` is a string in a file: it is run only when it is an absolute path on
    /// a local disk to a file called `lodestash-desktop` — never some other program, a relative
    /// path, or a network share.
    #[test]
    fn a_handshakes_exe_is_run_only_as_a_local_lodestash_desktop() {
        let dir = tempfile::tempdir().unwrap();
        let lonely = dir.path().join("lonely").join("lodestash-mcp");
        let from_handshake = |exe: &str| resolve_app(None, Some(&lonely), Some(exe));

        // Another program, though the file exists and the path is absolute and local.
        let other = file(dir.path(), "calc.exe");
        assert_eq!(
            trusted_exe(other.to_str().unwrap()),
            Err("it is not named lodestash-desktop")
        );
        let err = from_handshake(other.to_str().unwrap()).unwrap_err();
        assert!(err.contains("not used"), "{err}");

        // The right name, both spellings, any case: accepted where it is.
        for name in ["lodestash-desktop", "Lodestash-Desktop.EXE"] {
            let exe = file(dir.path(), name);
            assert_eq!(
                trusted_exe(exe.to_str().unwrap()),
                Ok(exe.clone()),
                "{name}"
            );
            assert_eq!(from_handshake(exe.to_str().unwrap()), Ok(exe), "{name}");
        }

        assert_eq!(
            trusted_exe("lodestash-desktop.exe"),
            Err("it is not an absolute path")
        );
        assert_eq!(
            trusted_exe("bin/lodestash-desktop"),
            Err("it is not an absolute path")
        );
        #[cfg(windows)]
        {
            for network in [
                r"\\fileserver\share\lodestash-desktop.exe",
                r"\\?\UNC\fileserver\share\lodestash-desktop.exe",
                r"\\.\pipe\lodestash-desktop.exe",
            ] {
                assert_eq!(
                    trusted_exe(network),
                    Err("it is not on a local disk"),
                    "{network}"
                );
            }
            // A drive letter with no drive behind it is not a local disk either — the same check
            // that refuses a mapped network drive.
            if let Some(letter) = (b'D'..=b'Z').find(|l| drive_type(*l) == DRIVE_NO_ROOT_DIR) {
                let unmounted = format!("{}:\\apps\\lodestash-desktop.exe", letter as char);
                assert_eq!(
                    trusted_exe(&unmounted),
                    Err("it is not on a local disk"),
                    "{unmounted}"
                );
            }
        }
    }

    /// This process is alive; one that has exited is not; and the pids that address no single
    /// process are never alive.
    #[test]
    fn a_pid_is_alive_only_while_its_process_runs() {
        assert!(pid_alive(std::process::id()));
        let gone = gone_process();
        assert!(!pid_alive(gone.pid()), "pid {} has exited", gone.pid());
        assert!(!pid_alive(0));
        assert!(!pid_alive(u32::MAX));
    }

    /// The handshake is judged liveness first: a file from a dead process is stale whatever
    /// version it names, and only a live one can be the wrong version.
    #[test]
    fn a_handshake_is_judged_by_liveness_before_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("engine.json");
        assert!(matches!(find(&path), Found::Stale(None)));

        let gone = gone_process();
        let me = std::process::id();
        for (pid, version, expect) in [
            (me, "v1", "live"),
            (me, "v0", "other"),
            (gone.pid(), "v1", "stale"),
            (gone.pid(), "v0", "stale"),
        ] {
            std::fs::write(&path, handshake_json_for(4321, "t", pid, version)).unwrap();
            let found = match find(&path) {
                Found::Live(_) => "live",
                Found::OtherVersion(_) => "other",
                Found::Stale(Some(_)) => "stale",
                Found::Stale(None) => "unreadable",
            };
            assert_eq!(found, expect, "pid {pid} {version}");
        }
    }

    /// The wait the server gives a launched app is the spec's 20 s.
    #[test]
    fn the_launch_budget_is_twenty_seconds() {
        assert_eq!(WAIT, Duration::from_secs(20));
    }

    /// The launched app gets `--background` and the handshake path, and the wait ends as soon as
    /// the handshake it publishes answers.
    #[test]
    fn a_launched_app_is_ready_when_its_handshake_answers() {
        let dir = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let exe = publishes(dir.path(), Some(live(port)), 0);
        let handshake = dir.path().join("engine.json");

        let began = Instant::now();
        let h = start(&exe, &handshake, Duration::from_secs(4)).unwrap();
        assert_eq!(h.port, port);
        assert!(began.elapsed() < Duration::from_secs(4));
    }

    /// Nothing published: the wait ends at its budget with an error that names the file — not
    /// sooner, and not much later. (The production budget is `WAIT`; a shorter one keeps this fast.)
    #[test]
    fn a_launched_app_that_never_publishes_times_out_at_the_budget() {
        let dir = tempfile::tempdir().unwrap();
        let exe = publishes(dir.path(), None, 0);
        let handshake = dir.path().join("engine.json");

        let began = Instant::now();
        let err = start(&exe, &handshake, Duration::from_millis(1500)).unwrap_err();
        let took = began.elapsed();
        let LaunchError::NothingPublished(why) = err else {
            panic!("{err:?}")
        };
        assert!(why.contains("engine.json"), "{why}");
        assert!(
            took >= Duration::from_millis(1500),
            "gave up after {took:?}"
        );
        assert!(took < Duration::from_secs(3), "overran: {took:?}");
    }

    /// A handshake whose port refuses is not readiness.
    #[test]
    fn a_handshake_whose_port_refuses_is_not_ready() {
        let dir = tempfile::tempdir().unwrap();
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let exe = publishes(dir.path(), Some(live(port)), 0);
        let err = start(
            &exe,
            &dir.path().join("engine.json"),
            Duration::from_secs(1),
        )
        .unwrap_err();
        assert!(matches!(err, LaunchError::NothingPublished(_)), "{err:?}");
    }

    /// The file a dead app left is not the launched app's answer, even when something else now
    /// answers on its port: the wait goes on for a handshake a live process wrote, and the port's
    /// new holder is never handed to the caller as "the app".
    #[test]
    fn a_dead_apps_handshake_is_not_the_launched_apps() {
        let dir = tempfile::tempdir().unwrap();
        let handshake = dir.path().join("engine.json");
        let squatter = RawListener::start(Behaviour::Answer("{}"));
        let gone = gone_process();
        std::fs::write(
            &handshake,
            handshake_json_for(squatter.port, "t", gone.pid(), "v1"),
        )
        .unwrap();
        let exe = publishes(dir.path(), None, 0);

        let err = start(&exe, &handshake, Duration::from_millis(800)).unwrap_err();
        assert!(matches!(err, LaunchError::NothingPublished(_)), "{err:?}");
        assert!(squatter.connections().is_empty());
    }

    /// A launched app that publishes another API version on a port that never answers: the wait
    /// runs its budget, and then says which versions disagree rather than "nothing published".
    #[test]
    fn a_launched_app_of_another_version_is_named_when_the_wait_ends() {
        let dir = tempfile::tempdir().unwrap();
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let exe = publishes(
            dir.path(),
            Some(handshake_json_for(port, "t", std::process::id(), "v2")),
            0,
        );
        let err = start(
            &exe,
            &dir.path().join("engine.json"),
            Duration::from_millis(800),
        )
        .unwrap_err();
        assert_eq!(err, LaunchError::OtherVersion("v2".into()));
    }

    /// An app that fails before it publishes is reported with its status, without waiting out the
    /// budget.
    #[test]
    fn a_launched_app_that_fails_is_reported_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let exe = publishes(dir.path(), None, 7);
        let began = Instant::now();
        let err = start(
            &exe,
            &dir.path().join("engine.json"),
            Duration::from_secs(5),
        )
        .unwrap_err();
        let LaunchError::NotStarted(why) = err else {
            panic!("{err:?}")
        };
        assert!(why.contains("exited"), "{why}");
        assert!(began.elapsed() < Duration::from_secs(3));
    }

    /// `engine.json` under `Lodestash` (`lodestash` on Linux) in the platform data directory —
    /// the very path the app writes (`lodestash-api`), which this binary restates instead of
    /// linking.
    #[test]
    fn the_platform_handshake_path_is_the_one_the_app_writes() {
        // `handshake_path` reads the process environment, which tests share; the platform rule is
        // checked directly instead.
        let p = platform_handshake_path().expect("a data directory");
        assert_eq!(Some(&p), lodestash_api::handshake_path().as_ref());
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

    /// A path a model will read does not spell out the home directory.
    #[test]
    fn a_shown_path_abbreviates_the_home_directory() {
        let home = Path::new(if cfg!(windows) {
            r"C:\Users\grace"
        } else {
            "/home/grace"
        });
        let inside = home.join("AppData").join("engine.json");
        let shown = shown_from(&inside, Some(home));
        assert!(!shown.contains("grace"), "{shown}");
        assert_eq!(
            shown,
            Path::new("~")
                .join("AppData")
                .join("engine.json")
                .display()
                .to_string()
        );
        assert_eq!(shown_from(home, Some(home)), "~");

        let outside = Path::new(if cfg!(windows) {
            r"D:\lodestash\engine.json"
        } else {
            "/srv/lodestash/engine.json"
        });
        assert_eq!(
            shown_from(outside, Some(home)),
            outside.display().to_string()
        );
        assert_eq!(shown_from(&inside, None), inside.display().to_string());
    }
}
