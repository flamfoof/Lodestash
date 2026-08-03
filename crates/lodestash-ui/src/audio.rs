//! Quick Look audio player — rodio playback + a peak waveform (DESIGN-GUIDELINES §7,
//! INGESTION.md viewers; TECH-STACK §2 "Audio"). rodio bundles symphonia, so one crate
//! both decodes for the waveform and streams playback.
//!
//! Lifecycle mirrors `ImageViewer`/`Viewport3d`: the waveform decodes on a background
//! thread; playback (the rodio `OutputStream`/`Sink`, which are `!Send`) lives on the UI
//! thread. Buffers free when Quick Look closes (`clear`).

use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};

// Decoding moved to `lodestash-app` (both frontends need a waveform); playback stays here.
pub use lodestash_app::audio::{load_waveform, AudioMeta};

/// Bars in the rendered waveform (fixed resolution regardless of clip length).
const WAVEFORM_BARS: usize = 256;

/// The Quick Look audio player: async waveform load + rodio transport.
pub struct AudioPlayer {
    // Kept alive for the stream's lifetime; dropping it silences output.
    _stream: OutputStream,
    handle: OutputStreamHandle,
    sink: Option<Sink>,
    pub entry_idx: Option<usize>,
    path: Option<PathBuf>,
    pub meta: Option<AudioMeta>,
    pub loading: bool,
    pub error: Option<String>,
    rx: Option<Receiver<Result<AudioMeta, String>>>,
    /// Global playback volume `0.0..=1.0`, applied to every sink (mirrors `config.volume`).
    volume: f32,
}

impl AudioPlayer {
    /// `None` if no audio output device is available (headless / no sound card).
    pub fn new(volume: f32) -> Option<Self> {
        let (stream, handle) = OutputStream::try_default()
            .map_err(|e| tracing::warn!("audio unavailable: {e}"))
            .ok()?;
        Some(Self {
            _stream: stream,
            handle,
            sink: None,
            entry_idx: None,
            path: None,
            meta: None,
            loading: false,
            error: None,
            rx: None,
            volume: volume.clamp(0.0, 1.0),
        })
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }

    /// Set the global volume and apply it to any currently playing sink.
    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        if let Some(sink) = &self.sink {
            sink.set_volume(self.volume);
        }
    }

    /// Load `path`'s waveform on a worker thread, stopping any current playback.
    pub fn load(&mut self, entry_idx: usize, path: PathBuf, egui_ctx: &egui::Context) {
        self.stop();
        self.entry_idx = Some(entry_idx);
        self.path = Some(path.clone());
        self.meta = None;
        self.error = None;
        self.loading = true;

        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        let egui_ctx = egui_ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(load_waveform(&path, WAVEFORM_BARS));
            egui_ctx.request_repaint();
        });
    }

    /// Drain a finished waveform decode.
    pub fn poll(&mut self) {
        let Some(rx) = &self.rx else { return };
        let Ok(result) = rx.try_recv() else { return };
        self.rx = None;
        self.loading = false;
        match result {
            Ok(meta) => self.meta = Some(meta),
            Err(e) => self.error = Some(e),
        }
    }

    /// Stop playback and drop everything (Quick Look closed / navigated away).
    pub fn clear(&mut self) {
        self.stop();
        self.entry_idx = None;
        self.path = None;
        self.meta = None;
        self.error = None;
        self.loading = false;
        self.rx = None;
    }

    fn stop(&mut self) {
        if let Some(sink) = self.sink.take() {
            sink.stop();
        }
    }

    /// Play/pause. Restarts from the beginning if the clip has finished (or never started).
    pub fn toggle(&mut self) {
        match &self.sink {
            Some(sink) if !sink.empty() => {
                if sink.is_paused() {
                    sink.play();
                } else {
                    sink.pause();
                }
            }
            _ => self.start(),
        }
    }

    fn start(&mut self) {
        let Some(path) = &self.path else { return };
        let decode = File::open(path)
            .map_err(|e| e.to_string())
            .and_then(|f| Decoder::new(BufReader::new(f)).map_err(|e| e.to_string()));
        match decode.and_then(|dec| {
            let sink = Sink::try_new(&self.handle).map_err(|e| e.to_string())?;
            sink.set_volume(self.volume);
            sink.append(dec);
            Ok(sink)
        }) {
            Ok(sink) => self.sink = Some(sink),
            Err(e) => self.error = Some(e),
        }
    }

    /// Seek to `secs`; if the clip already finished, restart then seek.
    pub fn seek(&mut self, secs: f32) {
        let secs = secs.max(0.0);
        if self.sink.as_ref().map_or(true, |s| s.empty()) {
            self.start();
            // A fresh sink plays; pause so a seek-scrub doesn't blast audio unexpectedly.
            if let Some(sink) = &self.sink {
                sink.pause();
            }
        }
        if let Some(sink) = &self.sink {
            let _ = sink.try_seek(Duration::from_secs_f32(secs));
        }
    }

    pub fn is_playing(&self) -> bool {
        self.sink
            .as_ref()
            .is_some_and(|s| !s.is_paused() && !s.empty())
    }

    /// Current playhead in seconds, clamped to the clip duration.
    pub fn position(&self) -> f32 {
        let dur = self.meta.as_ref().map_or(0.0, |m| m.duration);
        self.sink
            .as_ref()
            .map(|s| s.get_pos().as_secs_f32().min(dur))
            .unwrap_or(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_waveform_of_a_synthetic_wav() {
        // 0.1s of 440Hz sine, mono 44.1k, written as a minimal PCM16 WAV.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tone.wav");
        write_sine_wav(&path, 44_100, 4410);
        let meta = load_waveform(&path, 64).expect("decodes");
        assert_eq!(meta.sample_rate, 44_100);
        assert_eq!(meta.channels, 1);
        assert!((meta.duration - 0.1).abs() < 0.01, "≈0.1s");
        assert_eq!(meta.peaks.len(), 64);
        assert!(meta.peaks.iter().any(|&p| p > 0.5), "a loud bar exists");
        assert!(meta.peaks.iter().all(|&p| (0.0..=1.0).contains(&p)));
    }

    fn write_sine_wav(path: &std::path::Path, sample_rate: u32, frames: u32) {
        use std::io::Write;
        let bytes_per_sample = 2u32;
        let data_len = frames * bytes_per_sample;
        let mut f = File::create(path).unwrap();
        // RIFF/WAVE header (PCM16 mono).
        f.write_all(b"RIFF").unwrap();
        f.write_all(&(36 + data_len).to_le_bytes()).unwrap();
        f.write_all(b"WAVE").unwrap();
        f.write_all(b"fmt ").unwrap();
        f.write_all(&16u32.to_le_bytes()).unwrap();
        f.write_all(&1u16.to_le_bytes()).unwrap(); // PCM
        f.write_all(&1u16.to_le_bytes()).unwrap(); // mono
        f.write_all(&sample_rate.to_le_bytes()).unwrap();
        f.write_all(&(sample_rate * bytes_per_sample).to_le_bytes())
            .unwrap();
        f.write_all(&(bytes_per_sample as u16).to_le_bytes())
            .unwrap();
        f.write_all(&16u16.to_le_bytes()).unwrap();
        f.write_all(b"data").unwrap();
        f.write_all(&data_len.to_le_bytes()).unwrap();
        for i in 0..frames {
            let t = i as f32 / sample_rate as f32;
            let s = (t * 440.0 * std::f32::consts::TAU).sin();
            let v = (s * 30000.0) as i16;
            f.write_all(&v.to_le_bytes()).unwrap();
        }
    }
}
