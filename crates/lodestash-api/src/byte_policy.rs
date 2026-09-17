//! App-owned destination consent, shared by REST and MCP (ENGINE-API §8.2).
use crate::{AgentError, ServerState};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentByteProfile {
    /// User-visible host/model destination; not a URL or a claim supplied by a tool caller.
    pub host: String,
    /// `unknown`, `local`, or `remote`. Unrecognized values fail closed.
    pub destination: String,
    /// Models, textures, images, sprites, materials and fonts, including derived previews.
    pub visual_bytes: bool,
    /// Only honored for an explicitly local destination, including audio-derived pixels.
    pub audio_bytes: bool,
}

impl Default for AgentByteProfile {
    fn default() -> Self {
        Self {
            host: String::new(),
            destination: "unknown".into(),
            visual_bytes: false,
            audio_bytes: false,
        }
    }
}

impl AgentByteProfile {
    pub fn allows(&self, media_type: Option<&str>) -> bool {
        if self.host.trim().is_empty() || !matches!(self.destination.as_str(), "local" | "remote") {
            return false;
        }
        match media_type {
            None => self.visual_bytes || (self.audio_bytes && self.destination == "local"),
            Some("model_3d" | "texture" | "image" | "sprite" | "material" | "font") => {
                self.visual_bytes
            }
            Some("audio") => self.audio_bytes && self.destination == "local",
            Some(_) => false,
        }
    }
}

/// Read current app configuration at release time, including in a long-lived headless child.
pub type BytePolicyFn = Box<dyn Fn() -> (bool, AgentByteProfile) + Send + Sync>;

impl ServerState {
    pub fn set_byte_profile(&self, profile: AgentByteProfile) {
        *self.byte_profile.lock().unwrap() = profile;
    }

    pub fn set_byte_policy_reader(&self, read: BytePolicyFn) {
        *self.byte_policy_reader.lock().unwrap() = Some(std::sync::Arc::new(read));
    }

    pub fn may_release_media(&self, media_type: Option<&str>) -> bool {
        let reader = self.byte_policy_reader.lock().unwrap().clone();
        let (enabled, profile) = match reader {
            Some(read) => read(),
            None => (
                self.byte_release.load(std::sync::atomic::Ordering::Relaxed),
                self.byte_profile.lock().unwrap().clone(),
            ),
        };
        enabled && profile.allows(media_type)
    }

    pub fn require_asset_bytes(&self, id: i64) -> Result<(), AgentError> {
        if !self.byte_release() {
            return Err(AgentError::bytes_not_released());
        }
        let row = self
            .asset(id)?
            .ok_or_else(|| AgentError::asset_not_found(id))?;
        if !self.may_release_media(Some(&row.media_type)) {
            return Err(AgentError::bytes_not_released());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destination_and_modality_are_both_required() {
        let mut p = AgentByteProfile {
            visual_bytes: true,
            audio_bytes: true,
            ..Default::default()
        };
        assert!(!p.allows(None));
        p.host = "Codex / test model".into();
        assert!(!p.allows(Some("model_3d")));
        p.destination = "remote".into();
        assert!(p.allows(Some("model_3d")));
        assert!(!p.allows(Some("audio")));
        assert!(!p.allows(Some("unknown")));
        p.visual_bytes = false;
        assert!(!p.allows(None));
        p.destination = "local".into();
        assert!(p.allows(Some("audio")));
        assert!(!p.allows(Some("texture")));
    }
}
