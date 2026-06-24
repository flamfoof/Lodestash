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
        (SortBy::Name,     "Name"),
        (SortBy::Type,     "Type"),
        (SortBy::Size,     "Size"),
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
    pub name:  String,
    /// All source directories — may be on different drives or locations.
    pub paths: Vec<PathBuf>,
}

impl LibraryProject {
    pub fn live_paths(&self) -> Vec<&Path> {
        self.paths.iter().filter(|p| p.is_dir()).map(|p| p.as_path()).collect()
    }
}

// ── View preferences ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewPrefs {
    pub thumb_size: f32,
    pub sort_by:    SortBy,
    pub sort_asc:   bool,
}

impl Default for ViewPrefs {
    fn default() -> Self {
        Self { thumb_size: 128.0, sort_by: SortBy::Name, sort_asc: true }
    }
}

// ── AppConfig ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    /// Name of the project that was open when the app last closed.
    pub last_project_name: Option<String>,
    /// Legacy single-root — kept for migration from older configs.
    #[serde(default)]
    pub last_root:    Option<PathBuf>,
    /// Recently opened folders, newest first (capped at 10).
    #[serde(default)]
    pub recent_roots: Vec<PathBuf>,
    /// Named saved libraries the user can reopen from the empty state.
    pub projects:     Vec<LibraryProject>,
    /// Persisted view preferences (zoom, sort).
    pub view:         ViewPrefs,
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
            if !proj.paths.contains(&path) {
                proj.paths.push(path);
            }
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
        self.projects.push(LibraryProject { name: unique_name.clone(), paths: vec![path] });
        unique_name
    }

    fn touch_recent(&mut self, path: PathBuf) {
        self.last_root = Some(path.clone());
        self.recent_roots.retain(|r| r != &path);
        self.recent_roots.insert(0, path);
        self.recent_roots.truncate(Self::MAX_RECENT);
    }
}

fn stem_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn config_path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("Lodestash").join("config.json"))
}
