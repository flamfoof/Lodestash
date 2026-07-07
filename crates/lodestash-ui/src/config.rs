//! Persistent app configuration — survives restarts.
//! Stored at: `{os_config_dir}/Lodestash/config.json`

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// ── Sort preference ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SortBy {
    #[default]
    Name,
    Type,
    Size,
    Modified,
}

impl SortBy {
    pub const OPTIONS: &'static [(SortBy, &'static str)] = &[
        (SortBy::Name, "Name"),
        (SortBy::Type, "Type"),
        (SortBy::Size, "Size"),
        (SortBy::Modified, "Date"),
    ];

    pub fn label(self) -> &'static str {
        Self::OPTIONS
            .iter()
            .find(|&&(s, _)| s == self)
            .map(|&(_, l)| l)
            .unwrap_or("Name")
    }
}

// ── Named library project ─────────────────────────────────────────────────────

/// A named, saved library that aggregates one or more source folders.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryProject {
    pub name: String,
    /// All source directories — may be on different drives or locations.
    pub paths: Vec<PathBuf>,
}

impl LibraryProject {
    pub fn live_paths(&self) -> Vec<&Path> {
        self.paths
            .iter()
            .filter(|p| p.is_dir())
            .map(|p| p.as_path())
            .collect()
    }

    /// Add a source path if not already present. Returns true if it was added.
    pub fn add_path(&mut self, path: PathBuf) -> bool {
        if self.paths.contains(&path) {
            return false;
        }
        self.paths.push(path);
        true
    }
}

// ── View preferences ──────────────────────────────────────────────────────────

/// Browse view for the asset grid (DESIGN-GUIDELINES.md §7 "Browse views").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ViewMode {
    #[default]
    Flat,
    Folders,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewPrefs {
    pub thumb_size: f32,
    pub sort_by: SortBy,
    pub sort_asc: bool,
    #[serde(default)]
    pub mode: ViewMode,
}

impl Default for ViewPrefs {
    fn default() -> Self {
        Self {
            thumb_size: 128.0,
            sort_by: SortBy::Name,
            sort_asc: true,
            mode: ViewMode::Flat,
        }
    }
}

// ── AppConfig ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    /// Name of the project that was open when the app last closed.
    pub last_project_name: Option<String>,
    /// Legacy single-root — kept for migration from older configs.
    #[serde(default)]
    pub last_root: Option<PathBuf>,
    /// Recently opened folders, newest first (capped at 10).
    #[serde(default)]
    pub recent_roots: Vec<PathBuf>,
    /// Named saved libraries the user can reopen from the empty state.
    pub projects: Vec<LibraryProject>,
    /// Persisted view preferences (zoom, sort).
    pub view: ViewPrefs,
}

impl AppConfig {
    const MAX_RECENT: usize = 10;

    pub fn load() -> Self {
        let path = match config_path() {
            Some(p) => p,
            None => return Self::default(),
        };
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(_) => return Self::default(),
        };
        serde_json::from_slice(&bytes).unwrap_or_default()
    }

    pub fn save(&self) {
        let path = match config_path() {
            Some(p) => p,
            None => return,
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(path, json);
        }
    }

    /// Return the project with this name, if any.
    pub fn project_by_name(&self, name: &str) -> Option<&LibraryProject> {
        self.projects.iter().find(|p| p.name == name)
    }

    /// Record that a single folder was opened as a new standalone project.
    /// Returns the project name it was saved under.
    pub fn record_open(&mut self, root: PathBuf) -> String {
        self.touch_recent(root.clone());
        let name = self.upsert_single_path_project(root);
        self.last_project_name = Some(name.clone());
        self.save();
        name
    }

    /// Record that a named multi-source project was opened.
    pub fn record_project_open(&mut self, name: &str) {
        self.last_project_name = Some(name.to_owned());
        if let Some(proj) = self.projects.iter().find(|p| p.name == name) {
            for path in proj.paths.clone() {
                self.touch_recent(path);
            }
        }
        self.save();
    }

    /// Add a source folder to an existing project by name.
    /// Returns false if the project wasn't found.
    pub fn add_source_to_project(&mut self, project_name: &str, path: PathBuf) -> bool {
        self.touch_recent(path.clone());
        if let Some(proj) = self.projects.iter_mut().find(|p| p.name == project_name) {
            proj.add_path(path);
            self.save();
            true
        } else {
            false
        }
    }

    /// Add `path` to saved projects under its folder name if not already in any project.
    /// Returns the project name it was added to (or already existed under).
    fn upsert_single_path_project(&mut self, path: PathBuf) -> String {
        if let Some(proj) = self.projects.iter().find(|p| p.paths.contains(&path)) {
            return proj.name.clone();
        }
        let name = stem_name(&path);
        // Avoid duplicate project names by appending a counter.
        let unique_name = if self.projects.iter().any(|p| p.name == name) {
            format!("{name} ({})", self.projects.len())
        } else {
            name
        };
        self.projects.push(LibraryProject {
            name: unique_name.clone(),
            paths: vec![path],
        });
        unique_name
    }

    fn touch_recent(&mut self, path: PathBuf) {
        self.last_root = Some(path.clone());
        self.recent_roots.retain(|r| r != &path);
        self.recent_roots.insert(0, path);
        self.recent_roots.truncate(Self::MAX_RECENT);
    }
}

pub(crate) fn stem_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn config_path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("Lodestash").join("config.json"))
}

// ── Tests ─────────────────────────────────────────────────────────────────────
// Pure project/recent logic only. The public record_*/add_source_* wrappers call
// save() which writes to the real OS config dir, so tests exercise the private
// helpers (upsert_single_path_project, touch_recent) and add_path directly — never
// save() — to avoid clobbering a developer's actual config.
// Spec: TECH-STACK.md §5.1 (multi-source projects, this session's feature work).

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn upsert_names_project_from_folder_stem() {
        let mut cfg = AppConfig::default();
        let name = cfg.upsert_single_path_project(p("/assets/Synty"));
        assert_eq!(name, "Synty");
        assert_eq!(cfg.projects.len(), 1);
        assert_eq!(cfg.projects[0].paths, vec![p("/assets/Synty")]);
    }

    #[test]
    fn upsert_is_idempotent_for_same_path() {
        let mut cfg = AppConfig::default();
        let a = cfg.upsert_single_path_project(p("/assets/Synty"));
        let b = cfg.upsert_single_path_project(p("/assets/Synty"));
        assert_eq!(a, b);
        assert_eq!(
            cfg.projects.len(),
            1,
            "same path must not create a second project"
        );
    }

    #[test]
    fn upsert_disambiguates_same_stem_different_path() {
        let mut cfg = AppConfig::default();
        let a = cfg.upsert_single_path_project(p("/a/Synty"));
        let b = cfg.upsert_single_path_project(p("/b/Synty"));
        assert_eq!(a, "Synty");
        assert_ne!(
            a, b,
            "same folder name from a different path gets a unique label"
        );
        assert_eq!(cfg.projects.len(), 2);
    }

    #[test]
    fn add_path_dedups() {
        let mut proj = LibraryProject {
            name: "Synty".into(),
            paths: vec![p("/a")],
        };
        assert!(proj.add_path(p("/b")), "new path is added");
        assert!(!proj.add_path(p("/a")), "existing path is not re-added");
        assert_eq!(proj.paths, vec![p("/a"), p("/b")]);
    }

    #[test]
    fn touch_recent_is_newest_first_deduped_and_capped() {
        let mut cfg = AppConfig::default();
        for i in 0..15 {
            cfg.touch_recent(p(&format!("/r{i}")));
        }
        assert_eq!(cfg.recent_roots.len(), AppConfig::MAX_RECENT, "capped");
        assert_eq!(cfg.recent_roots[0], p("/r14"), "most recent is first");

        // Re-touching an existing path moves it to front without duplicating.
        cfg.touch_recent(p("/r10"));
        assert_eq!(cfg.recent_roots[0], p("/r10"));
        assert_eq!(
            cfg.recent_roots.iter().filter(|r| **r == p("/r10")).count(),
            1,
            "no duplicate entry"
        );
    }

    #[test]
    fn sortby_label_round_trips_options() {
        for &(s, label) in SortBy::OPTIONS {
            assert_eq!(s.label(), label);
        }
    }
}
