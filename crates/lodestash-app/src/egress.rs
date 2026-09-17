//! What may leave this machine, and on whose say-so (AI-PIPELINE §5.1, v1 plan S10).
//!
//! One gate for every way bytes can go out — the cloud tagger, the REST plane, the MCP plane — so a
//! new entry point cannot quietly acquire its own policy. Three rules:
//!
//! - **Locality is parsed, not guessed** ([`lodestash_ai::endpoint`]). A loopback endpoint is this
//!   machine; everything else is somebody's network, free tier or not.
//! - **Consent is per modality and defaults to off.** A setting that predates these toggles is not
//!   read as permission, and metadata access never implies permission to upload the file.
//! - **Audio bytes do not leave.** Neither do pixels derived from audio: a spectrogram is evidence
//!   about the clip, and routing it to a remote model would be the prohibition with an extra step.

use lodestash_ai::endpoint::{locality, Locality};

use crate::config::AppConfig;

/// What kind of asset the bytes are of. Derived pixels answer for their source: a material's ball
/// and a model's render are images; a spectrogram is audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modality {
    Image,
    Audio,
}

impl Modality {
    /// The modality of a `media_type.code`. `None` for a type with no bytes worth sending.
    pub fn of(media_code: &str) -> Option<Self> {
        match media_code {
            "image" | "texture" | "sprite" | "model_3d" | "material" => Some(Self::Image),
            "audio" => Some(Self::Audio),
            _ => None,
        }
    }
}

/// Why bytes stayed here. Carries what the user would have to change, so a caller can say it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Denied {
    /// The endpoint is remote and this modality has no consent.
    NeedsConsent { host: String, modality: Modality },
    /// Audio bytes — including pixels derived from audio — never go to a remote endpoint.
    AudioNeverLeaves { host: String },
    /// An agent asked for bytes and byte release to agents is off.
    AgentBytesOff,
}

impl std::fmt::Display for Denied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Denied::NeedsConsent { host, modality } => write!(
                f,
                "sending {} to {host} needs consent in Settings — no request was made",
                match modality {
                    Modality::Image => "images",
                    Modality::Audio => "audio",
                }
            ),
            Denied::AudioNeverLeaves { host } => {
                write!(f, "audio is never uploaded, so nothing was sent to {host}")
            }
            Denied::AgentBytesOff => write!(
                f,
                "this agent may read metadata and links; sending it asset bytes needs \
                 the agent byte switch, destination and modality consent in Settings"
            ),
        }
    }
}

/// May the bytes of a `modality` asset go to `endpoint`?
///
/// A loopback endpoint is this machine and needs no consent. Anything else does, per modality, and
/// audio never does.
pub fn may_send(cfg: &AppConfig, endpoint: &str, modality: Modality) -> Result<(), Denied> {
    if locality(endpoint) == Locality::Loopback {
        return Ok(());
    }
    let host = lodestash_ai::endpoint::host_of(endpoint).unwrap_or_else(|| endpoint.to_string());
    match modality {
        Modality::Audio => Err(Denied::AudioNeverLeaves { host }),
        Modality::Image if cfg.remote_image_bytes => Ok(()),
        Modality::Image => Err(Denied::NeedsConsent {
            host,
            modality: Modality::Image,
        }),
    }
}

/// May an agent — REST or MCP, whatever it claims to be — receive asset bytes?
///
/// A stdio connection proves the *client process* is local; it says nothing about the model behind
/// it, which is why this is a configured choice rather than an inference from the transport.
pub fn agent_may_read_bytes(cfg: &AppConfig, media_type: &str) -> Result<(), Denied> {
    if cfg.mcp_bytes_enabled && cfg.mcp_byte_profile.allows(Some(media_type)) {
        Ok(())
    } else {
        Err(Denied::AgentBytesOff)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(images: bool, audio_setting_that_should_not_matter: bool) -> AppConfig {
        AppConfig {
            remote_image_bytes: images,
            mcp_bytes_enabled: audio_setting_that_should_not_matter,
            ..AppConfig::default()
        }
    }

    /// The gate answers on locality first, consent second — and never yes for audio.
    #[test]
    fn only_consented_images_leave_and_audio_never_does() {
        let off = cfg(false, false);
        let on = cfg(true, true);

        // This machine: no consent needed, for either modality.
        for endpoint in ["http://localhost:1234/v1", "http://127.0.0.1:11434"] {
            assert_eq!(may_send(&off, endpoint, Modality::Image), Ok(()));
            assert_eq!(may_send(&off, endpoint, Modality::Audio), Ok(()));
        }

        // A remote endpoint, however free, needs consent for images.
        let remote = "https://api.example.com/v1";
        assert!(matches!(
            may_send(&off, remote, Modality::Image),
            Err(Denied::NeedsConsent { .. })
        ));
        assert_eq!(may_send(&on, remote, Modality::Image), Ok(()));

        // Audio is refused with consent granted and byte release on: it is not a consent question.
        assert!(matches!(
            may_send(&on, remote, Modality::Audio),
            Err(Denied::AudioNeverLeaves { .. })
        ));

        // A host named to look local is remote (v1 plan S10).
        assert!(matches!(
            may_send(&off, "https://localhost.example.com/v1", Modality::Image),
            Err(Denied::NeedsConsent { .. })
        ));
    }

    /// Derived pixels answer for the asset they came from: a model's render is an image, a
    /// spectrogram is audio and stays here.
    #[test]
    fn derived_pixels_inherit_their_source_modality() {
        assert_eq!(Modality::of("model_3d"), Some(Modality::Image));
        assert_eq!(Modality::of("material"), Some(Modality::Image));
        assert_eq!(Modality::of("sprite"), Some(Modality::Image));
        assert_eq!(Modality::of("audio"), Some(Modality::Audio));
        assert_eq!(Modality::of("font"), None);
    }

    /// Metadata is not bytes: reading one never implies the other.
    #[test]
    fn an_agent_reads_bytes_only_when_that_is_configured() {
        assert!(matches!(
            agent_may_read_bytes(&cfg(true, false), "model_3d"),
            Err(Denied::AgentBytesOff)
        ));
        let mut configured = cfg(false, true);
        assert!(agent_may_read_bytes(&configured, "model_3d").is_err());
        configured.mcp_byte_profile = lodestash_api::AgentByteProfile {
            host: "fixture".into(),
            destination: "remote".into(),
            visual_bytes: true,
            audio_bytes: true,
        };
        assert_eq!(agent_may_read_bytes(&configured, "model_3d"), Ok(()));
        assert!(agent_may_read_bytes(&configured, "audio").is_err());
    }
}
