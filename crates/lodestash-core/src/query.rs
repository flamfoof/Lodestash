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
//! surface — media type, size, rating, status, and (since the M9 technical facets landed)
//! triangle count and audio duration. `color`/`verts`/`bones`/facets are in the §6 grammar but
//! nothing stores them yet (P3 colour, P4 tags), so parsing them would build filters no query
//! could apply. They join when their data does.

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
    /// Triangle-count range, models only (§6 numeric ranges; DATA-MODEL §14).
    pub max_tris: Option<i64>,
    pub min_tris: Option<i64>,
    /// Audio clip length in milliseconds (§6 `duration_ms`).
    pub max_duration_ms: Option<i64>,
    pub min_duration_ms: Option<i64>,
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
            || self.max_tris.is_some()
            || self.min_tris.is_some()
            || self.max_duration_ms.is_some()
            || self.min_duration_ms.is_some()
            || self.status.is_some()
    }

    /// Clamp every field into the known grammar (§6). Idempotent, and the single choke point an
    /// LLM-produced `ParsedQuery` must also pass through — a hallucinated 9-star rating or a
    /// negative size becomes harmless here rather than a wrong-but-confident filter.
    pub fn validate(mut self) -> Self {
        self.min_rating = self.min_rating.map(|r| r.clamp(1, 5));
        self.min_size_bytes = self.min_size_bytes.map(|b| b.max(0));
        self.max_size_bytes = self.max_size_bytes.map(|b| b.max(0));
        self.min_tris = self.min_tris.map(|t| t.max(0));
        self.max_tris = self.max_tris.map(|t| t.max(0));
        self.min_duration_ms = self.min_duration_ms.map(|d| d.max(0));
        self.max_duration_ms = self.max_duration_ms.map(|d| d.max(0));
        // An inverted range matches nothing and is always a parse artefact — drop the weaker end.
        for (lo, hi) in [
            (&mut self.min_size_bytes, self.max_size_bytes),
            (&mut self.min_tris, self.max_tris),
            (&mut self.min_duration_ms, self.max_duration_ms),
        ] {
            if matches!((*lo, hi), (Some(l), Some(h)) if l > h) {
                *lo = None;
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

/// The three numeric ranges the catalog can actually filter on (§6 numeric ranges).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Quantity {
    Bytes(i64),
    Tris(i64),
    Millis(i64),
}

fn tri_word(w: &str) -> bool {
    matches!(
        w,
        "tri" | "tris" | "triangle" | "triangles" | "poly" | "polys" | "polygon" | "polygons"
    )
}

/// Seconds-per-unit for a duration word, as a float so `ms` works.
fn time_scale(w: &str) -> Option<f64> {
    Some(match w {
        "ms" | "millisecond" | "milliseconds" => 0.001,
        "s" | "sec" | "secs" | "second" | "seconds" => 1.0,
        "min" | "mins" | "minute" | "minutes" => 60.0,
        _ => return None,
    })
}

/// A bare magnitude prefix — only meaningful in front of a counted unit ("5k tris"), never in
/// front of bytes, where "k"/"m" already mean kb/mb.
fn magnitude(w: &str) -> Option<f64> {
    Some(match w {
        "k" => 1_000.0,
        "m" => 1_000_000.0,
        _ => return None,
    })
}

/// Read a quantity starting at `at`: `"5mb"`, `"5 mb"`, `"5k tris"`, `"5000 triangles"`,
/// `"3 seconds"`, `"500ms"`. Returns the value and the index of its last token, so the caller can
/// mark the whole phrase consumed. The unit word decides which range it is — that is what keeps
/// "5k tris" out of the size filter, where a bare "k" would otherwise read as kilobytes.
fn read_quantity(tokens: &[String], at: usize) -> Option<(Quantity, usize)> {
    let (n, suffix) = number_with_suffix(tokens.get(at)?)?;
    // Unit glued to the number, else the following token.
    let (unit, last) = if suffix.is_empty() || magnitude(&suffix).is_some() {
        match tokens.get(at + 1) {
            Some(next) if tri_word(next) || time_scale(next).is_some() => (next.as_str(), at + 1),
            _ => (suffix.as_str(), at),
        }
    } else {
        (suffix.as_str(), at)
    };
    let scaled = n * magnitude(&suffix).unwrap_or(1.0);
    if tri_word(unit) {
        return Some((Quantity::Tris(scaled as i64), last));
    }
    if let Some(secs) = time_scale(unit) {
        return Some((Quantity::Millis((scaled * secs * 1000.0) as i64), last));
    }
    // Bytes: the unit may still be one token further out ("5 mb" parsed as number then unit).
    if let Some(scale) = unit_scale(unit) {
        return Some((Quantity::Bytes((n * scale as f64) as i64), last));
    }
    let unit = tokens.get(at + 1)?;
    if tri_word(unit) {
        Some((Quantity::Tris(scaled as i64), at + 1))
    } else if let Some(secs) = time_scale(unit) {
        Some((Quantity::Millis((scaled * secs * 1000.0) as i64), at + 1))
    } else {
        unit_scale(unit).map(|s| (Quantity::Bytes((n * s as f64) as i64), at + 1))
    }
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
            // The unit decides which range this is: bytes, triangles or duration.
            if let Some((q, last)) = read_quantity(&lower, j) {
                let (slot, value) = match q {
                    Quantity::Bytes(v) if dir_max => (&mut out.max_size_bytes, v),
                    Quantity::Bytes(v) => (&mut out.min_size_bytes, v),
                    Quantity::Tris(v) if dir_max => (&mut out.max_tris, v),
                    Quantity::Tris(v) => (&mut out.min_tris, v),
                    Quantity::Millis(v) if dir_max => (&mut out.max_duration_ms, v),
                    Quantity::Millis(v) => (&mut out.min_duration_ms, v),
                };
                *slot = Some(value);
                for u in used.iter_mut().take(last + 1).skip(i) {
                    *u = true;
                }
                i = last + 1;
                continue;
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
    fn triangle_budgets_are_a_range_not_a_size() {
        // "5k tris" must not read as 5 kilobytes — the unit word decides the range.
        for q in [
            "stylized rock under 5k tris",
            "stylized rock under 5000 triangles",
            "stylized rock <5k polys",
        ] {
            let p = parse_query(q);
            assert_eq!(p.max_tris, Some(5000), "{q}");
            assert_eq!(p.max_size_bytes, None, "{q}");
            assert_eq!(p.semantic_text, "stylized rock", "{q}");
        }
        let p = parse_query("hero mesh over 20k tris");
        assert_eq!(p.min_tris, Some(20_000));
        assert_eq!(p.max_tris, None);
    }

    #[test]
    fn audio_durations_parse_in_seconds_and_milliseconds() {
        assert_eq!(
            parse_query("footstep under 2 seconds").max_duration_ms,
            Some(2000)
        );
        assert_eq!(
            parse_query("footstep under 500ms").max_duration_ms,
            Some(500)
        );
        assert_eq!(
            parse_query("loop over 3 min").min_duration_ms,
            Some(180_000)
        );
        let p = parse_query("sword swing under 2 seconds");
        assert_eq!(p.semantic_text, "sword swing");
        assert!(p.has_filters());
    }

    #[test]
    fn inverted_ranges_are_clamped_away() {
        let p = ParsedQuery {
            min_tris: Some(9000),
            max_tris: Some(100),
            min_duration_ms: Some(-5),
            ..Default::default()
        }
        .validate();
        assert_eq!(p.min_tris, None, "inverted tri range drops its weaker end");
        assert_eq!(p.max_tris, Some(100));
        assert_eq!(p.min_duration_ms, Some(0), "negative duration clamps to 0");
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
