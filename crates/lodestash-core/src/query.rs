//! Query understanding (IDENTIFICATION-AND-SEARCH §5.B step 1, §6 grammar): split a plain-language
//! query into `{ semantic_text, filters }` so the embedding does the semantic lift while cheap
//! structural constraints stay cheap.
//!
//! *"low-poly stylized rock under 5 MB"* → `semantic_text: "low-poly stylized rock"` +
//! `max_size_bytes: 5 MB`. Pulling the size words out matters twice: they become an exact filter,
//! **and** they stop polluting the vector — embedding "under 5 mb" as if it described appearance
//! makes the query vector worse.
//!
//! §6 specifies the parser emits JSON from a local model or Haiku, and that Lodestash then
//! **validates and clamps it against the known grammar** "so a hallucinated facet can never
//! produce a wrong-but-confident filter". That validation is the part that must exist regardless
//! of who writes the JSON, so [`ParsedQuery`] + [`ParsedQuery::validate`] are the seam: an LLM
//! parser slots in later and goes through exactly the same clamping.
//!
//! ponytail: the deterministic parser below covers only the unambiguous, *already-filterable*
//! surface — media type, size, rating, status. `tris`/`color`/`duration`/facets are in the §6
//! grammar but nothing stores them yet (P1 metadata, P3 colour, P4 tags), so parsing them would
//! build filters no query could apply. They join when their data does.

use crate::types::MediaType;

/// A plain-language query split into its semantic and structural halves.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedQuery {
    /// What's left for the embedder / BM25 after structural terms are removed.
    pub semantic_text: String,
    /// Restrict to one media type (§6 categorical).
    pub media_type: Option<MediaType>,
    /// Minimum star rating, 1–5 (DATA-MODEL §12).
    pub min_rating: Option<u8>,
    pub max_size_bytes: Option<i64>,
    pub min_size_bytes: Option<i64>,
    /// User-facing status term, normalised to a catalog code.
    pub status: Option<&'static str>,
}

impl ParsedQuery {
    /// Any structural constraint at all? (Drives whether the UI shows a "filters applied" hint.)
    pub fn has_filters(&self) -> bool {
        self.media_type.is_some()
            || self.min_rating.is_some()
            || self.max_size_bytes.is_some()
            || self.min_size_bytes.is_some()
            || self.status.is_some()
    }

    /// Clamp every field into the known grammar (§6). Idempotent, and the single choke point an
    /// LLM-produced `ParsedQuery` must also pass through — a hallucinated 9-star rating or a
    /// negative size becomes harmless here rather than a wrong-but-confident filter.
    pub fn validate(mut self) -> Self {
        self.min_rating = self.min_rating.map(|r| r.clamp(1, 5));
        self.min_size_bytes = self.min_size_bytes.map(|b| b.max(0));
        self.max_size_bytes = self.max_size_bytes.map(|b| b.max(0));
        // An inverted range matches nothing and is always a parse artefact — drop the weaker end.
        if let (Some(lo), Some(hi)) = (self.min_size_bytes, self.max_size_bytes) {
            if lo > hi {
                self.min_size_bytes = None;
            }
        }
        self.semantic_text = self
            .semantic_text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        self
    }
}

/// Words that name a media type. Several map to the same code (a user says "sound", the schema
/// says `audio`); the 7-value enum itself is the invariant and is never extended here.
fn media_type_for(word: &str) -> Option<MediaType> {
    Some(match word {
        "audio" | "sound" | "sounds" | "sfx" | "music" | "song" => MediaType::Audio,
        "model" | "models" | "mesh" | "meshes" | "3d" => MediaType::Model3d,
        "texture" | "textures" => MediaType::Texture,
        "sprite" | "sprites" => MediaType::Sprite,
        "image" | "images" | "picture" | "pictures" | "photo" => MediaType::Image,
        "material" | "materials" => MediaType::Material,
        "font" | "fonts" | "typeface" => MediaType::Font,
        _ => return None,
    })
}

fn status_for(word: &str) -> Option<&'static str> {
    Some(match word {
        "unidentified" | "untagged" => "discovered",
        "identified" => "identified",
        "confirmed" => "confirmed",
        _ => return None,
    })
}

fn unit_scale(u: &str) -> Option<i64> {
    Some(match u {
        "b" | "byte" | "bytes" => 1,
        "k" | "kb" => 1024,
        "m" | "mb" => 1024 * 1024,
        "g" | "gb" => 1024 * 1024 * 1024,
        _ => return None,
    })
}

/// Split a token into its leading number and trailing alphabetic suffix (`"5mb"` → `(5.0, "mb")`).
fn number_with_suffix(tok: &str) -> Option<(f64, String)> {
    let split = tok.find(|c: char| c.is_alphabetic()).unwrap_or(tok.len());
    let (num, rest) = tok.split_at(split);
    // The number may be glued to its suffix by punctuation ("4-star"), so drop any trailing
    // non-numeric characters before parsing.
    let num = num.trim_end_matches(|c: char| !c.is_ascii_digit() && c != '.');
    num.parse::<f64>().ok().map(|n| (n, rest.to_string()))
}

/// Parse a plain-language query. Everything not recognised as a structural term stays in
/// `semantic_text` (§6: "unrecognized terms fall back into semantic_text").
pub fn parse_query(query: &str) -> ParsedQuery {
    let mut raw: Vec<String> = Vec::new();
    for w in query.split_whitespace() {
        let mut w = w.trim_matches(|c: char| {
            !c.is_alphanumeric() && c != '<' && c != '>' && c != '+' && c != '.'
        });
        // A comparison operator glued to its number ("<5mb") becomes its own token.
        if let Some(rest) = w.strip_prefix('<').or_else(|| w.strip_prefix('>')) {
            if !rest.is_empty() {
                raw.push(w[..1].to_string());
                w = rest;
            }
        }
        // Keep words/numbers and the bare operators; drop pure punctuation ("...").
        if w.chars().any(|c| c.is_alphanumeric()) || w == "<" || w == ">" {
            raw.push(w.to_string());
        }
    }
    let lower: Vec<String> = raw.iter().map(|w| w.to_lowercase()).collect();

    let mut out = ParsedQuery::default();
    let mut used = vec![false; raw.len()];

    let mut i = 0;
    while i < lower.len() {
        let w = lower[i].as_str();

        // ── size: "under 5mb" / "under 5 mb" / "<5mb" / "over 100 kb" / "bigger than 2gb"
        let dir_max = matches!(w, "under" | "below" | "less" | "smaller" | "<");
        let dir_min = matches!(w, "over" | "above" | "more" | "bigger" | "larger" | ">");
        if dir_max || dir_min {
            // Skip a filler word ("less *than* 5mb").
            let mut j = i + 1;
            if lower.get(j).map(String::as_str) == Some("than") {
                j += 1;
            }
            // The number may carry its unit ("5mb") or have it in the next token ("5 mb").
            if let Some((n, suffix)) = lower.get(j).and_then(|t| number_with_suffix(t)) {
                let (scale, last) = match unit_scale(&suffix) {
                    Some(s) => (Some(s), j),
                    None => match lower.get(j + 1).and_then(|u| unit_scale(u)) {
                        Some(s) => (Some(s), j + 1),
                        None => (None, j),
                    },
                };
                if let Some(scale) = scale {
                    let bytes = (n * scale as f64) as i64;
                    if dir_max {
                        out.max_size_bytes = Some(bytes);
                    } else {
                        out.min_size_bytes = Some(bytes);
                    }
                    for u in used.iter_mut().take(last + 1).skip(i) {
                        *u = true;
                    }
                    i = last + 1;
                    continue;
                }
            }
        }

        // ── rating: "4 stars" / "4-star" / "4+ stars" / "rated 4"
        if let Some((n, suffix)) = number_with_suffix(w) {
            let n_ok = (1.0..=5.0).contains(&n);
            let next_is_star = lower.get(i + 1).is_some_and(|t| t.starts_with("star"));
            if n_ok && (suffix.starts_with("star") || next_is_star) {
                out.min_rating = Some(n as u8);
                used[i] = true;
                if next_is_star {
                    used[i + 1] = true;
                    i += 2;
                } else {
                    i += 1;
                }
                // "rated 4 stars" — swallow the preceding verb too.
                if i >= 2 && lower[i - 2] == "rated" {
                    used[i - 2] = true;
                }
                continue;
            }
        }
        if w == "rated" {
            if let Some(n) = lower
                .get(i + 1)
                .and_then(|t| number_with_suffix(t))
                .map(|(n, _)| n)
            {
                if (1.0..=5.0).contains(&n) {
                    out.min_rating = Some(n as u8);
                    used[i] = true;
                    used[i + 1] = true;
                    i += 2;
                    continue;
                }
            }
        }

        // ── media type / status: single words, and only the first wins.
        if out.media_type.is_none() {
            if let Some(mt) = media_type_for(w) {
                out.media_type = Some(mt);
                used[i] = true;
                i += 1;
                continue;
            }
        }
        if out.status.is_none() {
            if let Some(st) = status_for(w) {
                out.status = Some(st);
                used[i] = true;
                i += 1;
                continue;
            }
        }
        i += 1;
    }

    // Drop dangling connectives left behind by a consumed phrase ("rocks that are under 5mb").
    out.semantic_text = raw
        .iter()
        .zip(&used)
        .filter(|(_, u)| !**u)
        .map(|(w, _)| w.as_str())
        .filter(|w| {
            !matches!(
                w.to_lowercase().as_str(),
                "that" | "are" | "is" | "with" | "and"
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    out.validate()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_query_is_all_semantic() {
        let p = parse_query("a low poly stylized rock");
        assert_eq!(p.semantic_text, "a low poly stylized rock");
        assert!(!p.has_filters());
    }

    #[test]
    fn size_constraints_are_extracted_and_removed() {
        for q in [
            "stylized rock under 5mb",
            "stylized rock under 5 mb",
            "stylized rock smaller than 5MB",
            "stylized rock <5mb",
        ] {
            let p = parse_query(q);
            assert_eq!(p.max_size_bytes, Some(5 * 1024 * 1024), "{q}");
            assert_eq!(
                p.semantic_text, "stylized rock",
                "{q}: size words must not pollute the vector"
            );
        }
        let p = parse_query("music over 2 gb");
        assert_eq!(p.min_size_bytes, Some(2 * 1024 * 1024 * 1024));
        assert_eq!(p.media_type, Some(MediaType::Audio));
    }

    #[test]
    fn rating_forms() {
        for q in ["rock 4 stars", "rock 4-star", "rock rated 4"] {
            let p = parse_query(q);
            assert_eq!(p.min_rating, Some(4), "{q}");
            assert_eq!(p.semantic_text, "rock", "{q}");
        }
    }

    #[test]
    fn media_type_and_status_words() {
        let p = parse_query("footstep sound unidentified");
        assert_eq!(p.media_type, Some(MediaType::Audio));
        assert_eq!(p.status, Some("discovered"));
        assert_eq!(p.semantic_text, "footstep");

        // A number that isn't a rating stays semantic.
        let p = parse_query("sci fi 12 barrel");
        assert_eq!(p.min_rating, None);
        assert_eq!(p.semantic_text, "sci fi 12 barrel");
    }

    #[test]
    fn combined_query_splits_cleanly() {
        let p = parse_query("low-poly stylized rock models under 5mb rated 4");
        assert_eq!(p.media_type, Some(MediaType::Model3d));
        assert_eq!(p.max_size_bytes, Some(5 * 1024 * 1024));
        assert_eq!(p.min_rating, Some(4));
        assert_eq!(p.semantic_text, "low-poly stylized rock");
        assert!(p.has_filters());
    }

    /// The §6 guarantee: whatever produced the struct (parser today, LLM later), out-of-range
    /// values are clamped rather than becoming wrong-but-confident filters.
    #[test]
    fn validate_clamps_out_of_range_values() {
        let p = ParsedQuery {
            semantic_text: "  rock   ".into(),
            min_rating: Some(9),
            min_size_bytes: Some(-5),
            max_size_bytes: Some(-1),
            ..Default::default()
        }
        .validate();
        assert_eq!(p.min_rating, Some(5), "rating clamped into 1..=5");
        assert_eq!(
            p.min_size_bytes,
            Some(0),
            "negatives clamp to 0, not dropped"
        );
        assert_eq!(p.max_size_bytes, Some(0));
        assert_eq!(p.semantic_text, "rock", "whitespace normalised");

        // A genuinely inverted range (min > max) matches nothing, so the weaker end is dropped.
        let inverted = ParsedQuery {
            min_size_bytes: Some(10_000_000),
            max_size_bytes: Some(1_000_000),
            ..Default::default()
        }
        .validate();
        assert_eq!(inverted.min_size_bytes, None);
        assert_eq!(inverted.max_size_bytes, Some(1_000_000));

        assert_eq!(
            ParsedQuery {
                min_rating: Some(0),
                ..Default::default()
            }
            .validate()
            .min_rating,
            Some(1)
        );
    }

    #[test]
    fn empty_and_punctuation_only_are_safe() {
        assert_eq!(parse_query("").semantic_text, "");
        assert_eq!(parse_query("   ...  ").semantic_text, "");
        assert!(!parse_query("").has_filters());
    }
}
