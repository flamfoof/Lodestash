//! Audio helpers with no renderer in them.
//!
//! Playback (`AudioPlayer`) stays in the UI crate: its rodio sink is `!Send` and belongs to
//! whatever owns the window. Decoding does not — both frontends need a waveform, and the desktop
//! shell has no rodio of its own.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use rodio::{Decoder, Source};

/// Decoded waveform + basic audio facts for the stat line.
pub struct AudioMeta {
    /// Per-bar peak magnitude, normalised to `0.0..=1.0` by the loudest bar.
    pub peaks: Vec<f32>,
    pub duration: f32,
    pub sample_rate: u32,
    pub channels: u16,
}

/// Decode `path` into a `bars`-resolution peak waveform. ponytail: full-decodes into a
/// mono buffer — game audio is short (SFX/loops), and even a multi-minute track is only
/// tens of MB; revisit with streaming peaks if long-form audio shows up.
pub fn load_waveform(path: &Path, bars: usize) -> Result<AudioMeta, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let dec = Decoder::new(BufReader::new(file)).map_err(|e| e.to_string())?;
    let channels = dec.channels().max(1);
    let sample_rate = dec.sample_rate().max(1);

    let samples: Vec<f32> = dec.convert_samples::<f32>().collect();
    let frames = samples.len() / channels as usize;
    if frames == 0 {
        return Err("no audio samples".into());
    }
    let duration = frames as f32 / sample_rate as f32;

    let bars = bars.max(1);
    let mut peaks = vec![0.0f32; bars];
    for (i, frame) in samples.chunks(channels as usize).enumerate() {
        // Downmix to a single magnitude by taking the loudest channel of the frame.
        let mag = frame.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        let b = ((i * bars) / frames).min(bars - 1);
        if mag > peaks[b] {
            peaks[b] = mag;
        }
    }
    let max = peaks.iter().copied().fold(0.0f32, f32::max);
    if max > 0.0 {
        for p in &mut peaks {
            *p /= max;
        }
    }
    Ok(AudioMeta {
        peaks,
        duration,
        sample_rate,
        channels,
    })
}

/// `mm:ss` for the transport readout.
pub fn fmt_time(secs: f32) -> String {
    let secs = secs.max(0.0) as u32;
    format!("{}:{:02}", secs / 60, secs % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_time_is_mmss() {
        assert_eq!(fmt_time(0.0), "0:00");
        assert_eq!(fmt_time(9.7), "0:09");
        assert_eq!(fmt_time(75.0), "1:15");
        assert_eq!(fmt_time(-3.0), "0:00");
    }
}
