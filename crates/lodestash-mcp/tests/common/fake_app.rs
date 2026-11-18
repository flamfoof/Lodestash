//! Stand-ins for the app binary, and for a process that has gone. Shared by the integration tests
//! and the crate's unit tests (std only).
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// A script that plays `lodestash-desktop --background`: it refuses to run without the flag
/// (exit 3), does what it was built to do, in the order the fields are listed, and exits.
#[derive(Default)]
pub struct FakeApp {
    /// Append a line to `launches.txt` beside the script — one per start.
    pub count_launches: bool,
    /// Write the directory it was started in to `cwd.txt` beside the script.
    pub record_cwd: bool,
    /// Seconds to spend starting before it publishes anything.
    pub start_secs: u32,
    /// The handshake to write to the file `LODESTASH_ENGINE_JSON` names — the one the launcher
    /// waits on.
    pub handshake: Option<String>,
    /// Seconds to stay alive afterwards, as a background app would.
    pub linger_secs: u32,
    pub exit: i32,
}

impl FakeApp {
    /// Write the script into `dir` as `name` (`name.cmd` on Windows) and return its path.
    pub fn write(&self, dir: &Path, name: &str) -> PathBuf {
        #[cfg(windows)]
        {
            let path = dir.join(format!("{name}.cmd"));
            let mut s = String::from("@echo off\r\nif not \"%~1\"==\"--background\" exit /b 3\r\n");
            if self.count_launches {
                s.push_str(">> \"%~dp0launches.txt\" echo launched\r\n");
            }
            if self.record_cwd {
                s.push_str("> \"%~dp0cwd.txt\" cd\r\n");
            }
            // Out of its own directory, so a script still running when its test ends does not
            // pin the test's temp directory.
            s.push_str("cd /d \"%SystemRoot%\"\r\n");
            if self.start_secs > 0 {
                s.push_str(&format!(
                    "ping -n {} 127.0.0.1 >nul\r\n",
                    self.start_secs + 1
                ));
            }
            if let Some(json) = &self.handshake {
                s.push_str(&format!("> \"%LODESTASH_ENGINE_JSON%\" echo {json}\r\n"));
            }
            if self.linger_secs > 0 {
                s.push_str(&format!(
                    "ping -n {} 127.0.0.1 >nul\r\n",
                    self.linger_secs + 1
                ));
            }
            s.push_str(&format!("exit /b {}\r\n", self.exit));
            std::fs::write(&path, s).unwrap();
            path
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.join(name);
            let mut s = String::from(
                "#!/bin/sh\n[ \"$1\" = \"--background\" ] || exit 3\nhere=$(dirname \"$0\")\n",
            );
            if self.count_launches {
                s.push_str("echo launched >> \"$here/launches.txt\"\n");
            }
            if self.record_cwd {
                s.push_str("pwd > \"$here/cwd.txt\"\n");
            }
            s.push_str("cd /\n");
            if self.start_secs > 0 {
                s.push_str(&format!("sleep {}\n", self.start_secs));
            }
            if let Some(json) = &self.handshake {
                s.push_str(&format!(
                    "printf '%s' '{json}' > \"$LODESTASH_ENGINE_JSON\"\n"
                ));
            }
            if self.linger_secs > 0 {
                s.push_str(&format!("sleep {}\n", self.linger_secs));
            }
            s.push_str(&format!("exit {}\n", self.exit));
            std::fs::write(&path, s).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        }
    }
}

/// How many times a [`FakeApp`] with `count_launches` in `dir` has been started.
pub fn launches(dir: &Path) -> usize {
    std::fs::read_to_string(dir.join("launches.txt"))
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

/// A v1-shaped handshake file's JSON.
pub fn handshake_json_for(port: u16, token: &str, pid: u32, api_version: &str) -> String {
    format!(
        r#"{{"version":1,"port":{port},"token":"{token}","pid":{pid},"api_version":"{api_version}"}}"#
    )
}

/// A process that has exited. The handle is kept, so on Windows its pid cannot be handed to
/// another process while a test is using it as "a pid nothing runs under".
pub struct Gone {
    child: Child,
}

impl Gone {
    pub fn pid(&self) -> u32 {
        self.child.id()
    }
}

/// Start a process that exits at once, and wait for it.
pub fn gone_process() -> Gone {
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/c", "exit", "0"]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", "exit 0"]);
        c
    };
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn a short-lived process");
    child.wait().expect("wait for it");
    Gone { child }
}
