//! CLAP log-mel front end (AI-PIPELINE §3 Tier 1: CLAP audio embeddings).
//!
//! CLAP's audio tower takes `input_features` — a log-mel spectrogram — not raw samples, and the
//! feature extraction must match HuggingFace's `ClapFeatureExtractor` *exactly* or the embeddings
//! are silently wrong rather than obviously broken. Every constant and formula below is taken
//! from the reference implementation (`transformers/models/clap/feature_extraction_clap.py` +
//! `transformers/audio_utils.py`), not inferred:
//!
//! - 48 kHz mono, `n_fft`/frame 1024, hop 480, 64 mel bins, 50–14000 Hz, 10 s (480 000 samples)
//! - `truncation="rand_trunc"` (this model's config) ⇒ the **Slaney** mel scale with **Slaney**
//!   normalisation — *not* HTK. (HTK is only used for `truncation="fusion"`.)
//! - periodic Hann window; `center=True` with **reflect** padding by `n_fft/2`
//! - `power=2.0` (power spectrum), then `log_mel="dB"` ⇒ `10*log10(max(x, 1e-10))`, and
//!   `top_db: null` in this model's preprocessor config ⇒ no dynamic-range clipping
//! - short clips use `repeatpad`: tile `floor(max_len/len)` times, then zero-pad to `max_len`
//!
//! ponytail: a hand-rolled radix-2 FFT rather than a new dependency — the size is a fixed 1024,
//! and it's checked against a naive DFT in the tests.

pub const SAMPLE_RATE: u32 = 48_000;
/// Mel bins per frame (`feature_size`) — the encoder's expected width.
pub const N_MELS: usize = 64;
/// 10 s at 48 kHz (`nb_max_samples`).
pub const MAX_SAMPLES: usize = 480_000;

const N_FFT: usize = 1024;
const HOP: usize = 480;
const N_FREQ_BINS: usize = N_FFT / 2 + 1; // 513
const F_MIN: f64 = 50.0;
const F_MAX: f64 = 14_000.0;
/// `power_to_db`'s `min_value` — clamps silence instead of producing -inf.
const MIN_POWER: f64 = 1e-10;

// ── Slaney mel scale (audio_utils.hertz_to_mel / mel_to_hertz, mel_scale="slaney") ───────────

const MIN_LOG_HZ: f64 = 1000.0;
const MIN_LOG_MEL: f64 = 15.0;

fn hz_to_mel(f: f64) -> f64 {
    if f >= MIN_LOG_HZ {
        MIN_LOG_MEL + (f / MIN_LOG_HZ).ln() * (27.0 / 6.4f64.ln())
    } else {
        3.0 * f / 200.0
    }
}

fn mel_to_hz(m: f64) -> f64 {
    if m >= MIN_LOG_MEL {
        MIN_LOG_HZ * ((6.4f64.ln() / 27.0) * (m - MIN_LOG_MEL)).exp()
    } else {
        200.0 * m / 3.0
    }
}

/// Triangular mel filterbank, `[N_MELS][N_FREQ_BINS]`, Slaney-normalised
/// (`enorm = 2/(f[j+2]-f[j])`, i.e. approximately constant energy per channel).
fn mel_filterbank() -> Vec<Vec<f64>> {
    // FFT bin centre frequencies: linspace(0, sr/2, n_freq_bins).
    let fft_freqs: Vec<f64> = (0..N_FREQ_BINS)
        .map(|i| (SAMPLE_RATE as f64 / 2.0) * i as f64 / (N_FREQ_BINS - 1) as f64)
        .collect();
    // N_MELS+2 band edges, evenly spaced on the mel scale.
    let (mel_min, mel_max) = (hz_to_mel(F_MIN), hz_to_mel(F_MAX));
    let filter_freqs: Vec<f64> = (0..N_MELS + 2)
        .map(|i| mel_to_hz(mel_min + (mel_max - mel_min) * i as f64 / (N_MELS + 1) as f64))
        .collect();

    let mut fb = vec![vec![0.0f64; N_FREQ_BINS]; N_MELS];
    for (j, row) in fb.iter_mut().enumerate() {
        let (f0, f1, f2) = (filter_freqs[j], filter_freqs[j + 1], filter_freqs[j + 2]);
        let enorm = 2.0 / (f2 - f0); // Slaney normalisation
        for (i, &f) in fft_freqs.iter().enumerate() {
            let down = (f - f0) / (f1 - f0);
            let up = (f2 - f) / (f2 - f1);
            row[i] = down.min(up).max(0.0) * enorm;
        }
    }
    fb
}

/// Periodic Hann (`window_function(..., periodic=True)`): divides by N, not N-1.
fn hann(n: usize) -> Vec<f64> {
    (0..n)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos())
        .collect()
}

/// In-place iterative radix-2 Cooley–Tukey FFT. `re`/`im` must be a power-of-two length.
fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    debug_assert!(n.is_power_of_two());
    // Bit-reversal permutation.
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    // Butterflies.
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * std::f64::consts::PI / len as f64;
        let (wr, wi) = (ang.cos(), ang.sin());
        let mut i = 0;
        while i < n {
            let (mut cr, mut ci) = (1.0f64, 0.0f64);
            for k in 0..len / 2 {
                let (ur, ui) = (re[i + k], im[i + k]);
                let (xr, xi) = (re[i + k + len / 2], im[i + k + len / 2]);
                let (vr, vi) = (xr * cr - xi * ci, xr * ci + xi * cr);
                re[i + k] = ur + vr;
                im[i + k] = ui + vi;
                re[i + k + len / 2] = ur - vr;
                im[i + k + len / 2] = ui - vi;
                let ncr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = ncr;
            }
            i += len;
        }
        len <<= 1;
    }
}

/// `repeatpad` then reflect-pad for centring, per the reference.
fn prepare(samples: &[f32]) -> Vec<f64> {
    let mut w: Vec<f64> = samples.iter().map(|&x| x as f64).collect();
    if w.is_empty() {
        w = vec![0.0; MAX_SAMPLES];
    } else if w.len() < MAX_SAMPLES {
        // repeatpad: tile floor(max/len) times, then zero-pad the remainder.
        let n_repeat = MAX_SAMPLES / w.len();
        let base = w.clone();
        for _ in 1..n_repeat {
            w.extend_from_slice(&base);
        }
        w.resize(MAX_SAMPLES, 0.0);
    } else {
        w.truncate(MAX_SAMPLES);
    }
    // center=True, pad_mode="reflect", by frame_length/2 each side.
    let pad = N_FFT / 2;
    let mut out = Vec::with_capacity(w.len() + 2 * pad);
    for i in (1..=pad).rev() {
        out.push(w[i.min(w.len() - 1)]);
    }
    out.extend_from_slice(&w);
    for i in 1..=pad {
        out.push(w[w.len().saturating_sub(1 + i)]);
    }
    out
}

/// Log-mel spectrogram of `samples` (mono, 48 kHz) as row-major `[frames][N_MELS]`.
/// Returns `(frames, data)`.
pub fn log_mel_spectrogram(samples: &[f32]) -> (usize, Vec<f32>) {
    let w = prepare(samples);
    let window = hann(N_FFT);
    let fb = mel_filterbank();
    let frames = 1 + (w.len() - N_FFT) / HOP;

    let mut out = vec![0.0f32; frames * N_MELS];
    let mut re = vec![0.0f64; N_FFT];
    let mut im = vec![0.0f64; N_FFT];
    let mut power = vec![0.0f64; N_FREQ_BINS];

    for t in 0..frames {
        let start = t * HOP;
        for i in 0..N_FFT {
            re[i] = w[start + i] * window[i];
            im[i] = 0.0;
        }
        fft(&mut re, &mut im);
        // power = 2.0 ⇒ |X|²; one-sided (bins 0..=n/2).
        for (k, p) in power.iter_mut().enumerate() {
            *p = re[k] * re[k] + im[k] * im[k];
        }
        for (m, filt) in fb.iter().enumerate() {
            let e: f64 = filt.iter().zip(&power).map(|(a, b)| a * b).sum();
            // power_to_db: clamp then 10*log10 (reference=1.0, no db_range clipping).
            out[t * N_MELS + m] = (10.0 * e.max(MIN_POWER).log10()) as f32;
        }
    }
    (frames, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hand-rolled FFT must agree with a naive DFT — the foundation everything else sits on.
    #[test]
    fn fft_matches_naive_dft() {
        const N: usize = 64;
        let sig: Vec<f64> = (0..N)
            .map(|i| ((i * 7) as f64).sin() + 0.3 * ((i * 3) as f64).cos())
            .collect();
        let (mut re, mut im) = (sig.clone(), vec![0.0; N]);
        fft(&mut re, &mut im);
        for k in 0..N {
            let (mut dr, mut di) = (0.0f64, 0.0f64);
            for (n, &x) in sig.iter().enumerate() {
                let a = -2.0 * std::f64::consts::PI * (k * n) as f64 / N as f64;
                dr += x * a.cos();
                di += x * a.sin();
            }
            assert!(
                (re[k] - dr).abs() < 1e-9 && (im[k] - di).abs() < 1e-9,
                "bin {k}: fft=({}, {}) dft=({dr}, {di})",
                re[k],
                im[k]
            );
        }
    }

    /// Slaney scale anchors: the piecewise break sits at exactly 1000 Hz ⇒ 15 mel, the linear
    /// region is 3f/200, and the transform round-trips.
    #[test]
    fn slaney_mel_scale_anchors_and_roundtrip() {
        assert!((hz_to_mel(1000.0) - 15.0).abs() < 1e-9);
        assert!((hz_to_mel(200.0) - 3.0).abs() < 1e-9);
        for f in [50.0, 400.0, 999.0, 1000.0, 4000.0, 14000.0] {
            assert!((mel_to_hz(hz_to_mel(f)) - f).abs() < 1e-6, "roundtrip {f}");
        }
    }

    /// Filterbank shape: triangles are non-negative, each has energy, and their peaks march
    /// monotonically up the spectrum.
    #[test]
    fn filterbank_is_ordered_triangles() {
        let fb = mel_filterbank();
        assert_eq!(fb.len(), N_MELS);
        let mut last_peak = 0usize;
        for (j, row) in fb.iter().enumerate() {
            assert_eq!(row.len(), N_FREQ_BINS);
            assert!(row.iter().all(|&x| x >= 0.0), "filter {j} has negatives");
            let peak = row
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
                .map(|(i, _)| i)
                .unwrap();
            assert!(row[peak] > 0.0, "filter {j} is entirely zero");
            assert!(peak >= last_peak, "filter {j} peak went backwards");
            last_peak = peak;
        }
    }

    /// The end-to-end check that catches a wrong mel scale or frequency mapping: a pure tone
    /// must light up the mel band that actually contains its frequency.
    #[test]
    fn pure_tone_peaks_in_the_correct_mel_band() {
        for tone_hz in [500.0f64, 2000.0, 8000.0] {
            let samples: Vec<f32> = (0..SAMPLE_RATE as usize)
                .map(|i| {
                    (2.0 * std::f64::consts::PI * tone_hz * i as f64 / SAMPLE_RATE as f64).sin()
                        as f32
                })
                .collect();
            let (frames, data) = log_mel_spectrogram(&samples);
            assert!(frames > 10);
            // Average each mel bin over the middle frames (avoids edge/padding effects).
            let mid = frames / 2;
            let mut avg = vec![0.0f64; N_MELS];
            for t in (mid - 5)..(mid + 5) {
                for m in 0..N_MELS {
                    avg[m] += data[t * N_MELS + m] as f64;
                }
            }
            let peak = avg
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
                .map(|(i, _)| i)
                .unwrap();

            // Which band *should* contain this tone? Band j spans filter edges [f_j, f_j+2].
            let (mel_min, mel_max) = (hz_to_mel(F_MIN), hz_to_mel(F_MAX));
            let edge = |i: usize| {
                mel_to_hz(mel_min + (mel_max - mel_min) * i as f64 / (N_MELS + 1) as f64)
            };
            let expected = (0..N_MELS)
                .find(|&j| tone_hz >= edge(j) && tone_hz <= edge(j + 2))
                .expect("tone inside the filterbank range");
            assert!(
                peak.abs_diff(expected) <= 1,
                "{tone_hz} Hz peaked in mel bin {peak}, expected ~{expected}"
            );
        }
    }

    /// Shape + padding contract the encoder depends on: 10 s → 1001 frames × 64, and a short
    /// clip is repeat-padded to exactly the same shape (never a ragged tensor).
    #[test]
    fn output_shape_is_fixed_regardless_of_input_length() {
        let long: Vec<f32> = vec![0.1; MAX_SAMPLES];
        let (f_long, d_long) = log_mel_spectrogram(&long);
        assert_eq!(f_long, 1001, "10s at hop 480 with centring");
        assert_eq!(d_long.len(), f_long * N_MELS);

        let short: Vec<f32> = (0..1000).map(|i| (i as f32 * 0.01).sin()).collect();
        let (f_short, d_short) = log_mel_spectrogram(&short);
        assert_eq!(f_short, f_long, "repeatpad fills to the same length");
        assert_eq!(d_short.len(), d_long.len());

        // Silence must be finite (the 1e-10 clamp), never -inf/NaN.
        let (_, quiet) = log_mel_spectrogram(&vec![0.0; 4800]);
        assert!(
            quiet.iter().all(|x| x.is_finite()),
            "silence produced non-finite dB"
        );
    }
}
