//! The `engine.json` handshake file (ENGINE-API §1): how a plugin finds the port and token.

use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Handshake {
    pub version: u32,
    pub port: u16,
    pub token: String,
    pub pid: u32,
    pub api_version: String,
    /// The catalog the app serving this port has open (M31). A headless server forwards only to an
    /// app with *its* catalog — otherwise a write lands in a different library than the one the
    /// agent just read. Absent from a file written before v1 plan S11.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_id: Option<String>,
}

impl Handshake {
    pub fn new(port: u16, token: impl Into<String>) -> Self {
        Self {
            version: 1,
            port,
            token: token.into(),
            pid: std::process::id(),
            api_version: "v1".into(),
            catalog_id: None,
        }
    }

    /// The same handshake, naming the catalog the serving app has open.
    pub fn with_catalog(mut self, id: impl Into<String>) -> Self {
        self.catalog_id = Some(id.into());
        self
    }
}

/// `%LOCALAPPDATA%\Lodestash\engine.json` / `~/Library/Application Support/Lodestash/engine.json`
/// / `~/.local/share/lodestash/engine.json` — `dirs` already resolves all three roots.
pub fn handshake_path() -> Option<PathBuf> {
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

/// Write the handshake file, replacing any previous session's. Returns the path written.
pub fn write_handshake(h: &Handshake) -> io::Result<PathBuf> {
    let path = handshake_path()
        .ok_or_else(|| io::Error::other("no platform data directory for the handshake file"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_handshake_to(h, &path)?;
    Ok(path)
}

pub(crate) fn write_handshake_to(h: &Handshake, path: &std::path::Path) -> io::Result<()> {
    let json = serde_json::to_vec_pretty(h).map_err(io::Error::other)?;
    std::fs::write(path, json)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Remove this process's handshake file on clean shutdown (ENGINE-API §1), so no client is left
/// with a dead app's port and token. The app calls it as it exits.
///
/// Only a file that still names **this** process is removed: one that names another pid was
/// written by an app started since, and is that app's. A missing file is success. A crash runs
/// nothing, which is why a client also checks that `pid` is alive before it trusts the file.
pub fn remove_handshake() {
    if let Some(path) = handshake_path() {
        remove_handshake_at(&path);
    }
}

pub(crate) fn remove_handshake_at(path: &std::path::Path) {
    let ours = std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Handshake>(&bytes).ok())
        .is_some_and(|h| h.pid == std::process::id());
    if ours {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_round_trips_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("engine.json");
        let h = Handshake::new(51731, "b3f1");
        write_handshake_to(&h, &path).unwrap();

        let read: Handshake = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(read, h);
        assert_eq!(read.version, 1);
        assert_eq!(read.api_version, "v1");
        assert_eq!(read.pid, std::process::id());
    }

    /// A clean exit takes the handshake with it, so no client is left reading a dead app's port
    /// and token — but only this process's own file. One that names another pid belongs to an
    /// app started since, and is that app's to remove.
    #[test]
    fn a_clean_exit_removes_only_this_processs_own_handshake() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("engine.json");

        write_handshake_to(&Handshake::new(51731, "ours"), &path).unwrap();
        remove_handshake_at(&path);
        assert!(!path.exists(), "our own handshake outlived a clean exit");

        let mut theirs = Handshake::new(51732, "theirs");
        theirs.pid = std::process::id() + 1;
        write_handshake_to(&theirs, &path).unwrap();
        remove_handshake_at(&path);
        let left: Handshake = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(left, theirs, "another app's handshake was removed");

        // Nothing to remove, and a file that is not a handshake: both left as they are.
        std::fs::remove_file(&path).unwrap();
        remove_handshake_at(&path);
        std::fs::write(&path, b"not json").unwrap();
        remove_handshake_at(&path);
        assert_eq!(std::fs::read(&path).unwrap(), b"not json");
    }
}
