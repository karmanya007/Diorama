//! Audio spectrum for audio-reactive wallpapers: what WE exposes as
//! `g_AudioSpectrum{16,32,64}{Left,Right}` and script audio buffers.
//!
//! WE's exact band mapping and smoothing are not known; this follows the
//! common shape: a windowed FFT of the last samples per channel, 64 bands
//! spaced logarithmically from 30 Hz to 16 kHz, magnitudes mapped from
//! −60..0 dB to 0..1, instant rise and an exponential fall.

use std::collections::VecDeque;
use std::f32::consts::PI;

pub const BANDS: usize = 64;
const FFT_SIZE: usize = 2048;
const LOW_HZ: f32 = 30.0;
const HIGH_HZ: f32 = 16_000.0;
const FLOOR_DB: f32 = -60.0;
/// Fall speed of a band (per second, exponential).
const DECAY: f32 = 6.0;

/// Per-channel band levels 0..1, bass first.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioSpectrum {
    pub left: [f32; BANDS],
    pub right: [f32; BANDS],
}

impl Default for AudioSpectrum {
    fn default() -> Self {
        Self {
            left: [0.0; BANDS],
            right: [0.0; BANDS],
        }
    }
}

impl AudioSpectrum {
    /// The 64 bands folded to `n` (16, 32 or 64) by averaging neighbours.
    pub fn bands(&self, n: usize, right: bool) -> Vec<f32> {
        let src = if right { &self.right } else { &self.left };
        let n = n.clamp(1, BANDS);
        let group = BANDS / n;
        src.chunks(group.max(1))
            .take(n)
            .map(|c| c.iter().sum::<f32>() / c.len() as f32)
            .collect()
    }

    /// Mean of both channels' bands: a single loudness 0..1.
    pub fn level(&self) -> f32 {
        (self.left.iter().sum::<f32>() + self.right.iter().sum::<f32>()) / (2 * BANDS) as f32
    }
}

/// Keeps recent samples and turns them into smoothed band levels.
#[derive(Debug, Clone)]
pub struct SpectrumAnalyzer {
    rate: f32,
    left: VecDeque<f32>,
    right: VecDeque<f32>,
    smoothed: AudioSpectrum,
    window: Vec<f32>,
}

impl SpectrumAnalyzer {
    pub fn new(sample_rate: u32) -> Self {
        let window = (0..FFT_SIZE)
            .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / (FFT_SIZE - 1) as f32).cos())
            .collect();
        Self {
            rate: sample_rate.max(1) as f32,
            left: VecDeque::with_capacity(FFT_SIZE),
            right: VecDeque::with_capacity(FFT_SIZE),
            smoothed: AudioSpectrum::default(),
            window,
        }
    }

    /// Adds interleaved samples (mono is used for both channels).
    pub fn push(&mut self, samples: &[f32], channels: usize) {
        let channels = channels.max(1);
        for frame in samples.chunks(channels) {
            let l = frame.first().copied().unwrap_or(0.0);
            let r = frame.get(1).copied().unwrap_or(l);
            for (buf, v) in [(&mut self.left, l), (&mut self.right, r)] {
                if buf.len() == FFT_SIZE {
                    buf.pop_front();
                }
                buf.push_back(v);
            }
        }
    }

    /// Band levels now; `dt` seconds since the last call drives the fall.
    pub fn analyze(&mut self, dt: f32) -> AudioSpectrum {
        let fall = (-DECAY * dt.max(0.0)).exp();
        let left = self.bands(true);
        let right = self.bands(false);
        for (s, new) in self.smoothed.left.iter_mut().zip(left) {
            *s = new.max(*s * fall);
        }
        for (s, new) in self.smoothed.right.iter_mut().zip(right) {
            *s = new.max(*s * fall);
        }
        self.smoothed
    }

    fn bands(&self, left: bool) -> [f32; BANDS] {
        let samples = if left { &self.left } else { &self.right };
        let mut out = [0.0; BANDS];
        if samples.len() < FFT_SIZE {
            return out;
        }
        let mut re: Vec<f32> = samples
            .iter()
            .zip(&self.window)
            .map(|(s, w)| s * w)
            .collect();
        let mut im = vec![0.0; FFT_SIZE];
        fft(&mut re, &mut im);
        // Hann window halves the amplitude; full-scale sine → 0 dB.
        let norm = 4.0 / FFT_SIZE as f32;
        let hz_per_bin = self.rate / FFT_SIZE as f32;
        for (band, o) in out.iter_mut().enumerate() {
            let lo = LOW_HZ * (HIGH_HZ / LOW_HZ).powf(band as f32 / BANDS as f32);
            let hi = LOW_HZ * (HIGH_HZ / LOW_HZ).powf((band + 1) as f32 / BANDS as f32);
            let first = ((lo / hz_per_bin) as usize).clamp(1, FFT_SIZE / 2 - 1);
            let last = ((hi / hz_per_bin) as usize).clamp(first, FFT_SIZE / 2 - 1);
            let peak = (first..=last)
                .map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt() * norm)
                .fold(0.0f32, f32::max);
            let db = 20.0 * peak.max(1e-9).log10();
            *o = ((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0);
        }
        out
    }
}

/// In-place radix-2 FFT (length must be a power of two).
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
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
    let mut len = 2;
    while len <= n {
        let angle = -2.0 * PI / len as f32;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (s, c) = (angle * k as f32).sin_cos();
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * c - im[b] * s;
                let ti = re[b] * s + im[b] * c;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
        }
        len <<= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f32, amp: f32, rate: u32) -> Vec<f32> {
        (0..FFT_SIZE * 2)
            .flat_map(|i| {
                let v = amp * (2.0 * PI * hz * i as f32 / rate as f32).sin();
                [v, 0.0]
            })
            .collect()
    }

    #[test]
    fn a_tone_lights_its_band_on_its_channel() {
        let mut a = SpectrumAnalyzer::new(48_000);
        a.push(&tone(1000.0, 0.5, 48_000), 2);
        let s = a.analyze(0.0);
        let (band, peak) = s
            .left
            .iter()
            .enumerate()
            .fold((0, 0.0), |m, (i, v)| if *v > m.1 { (i, *v) } else { m });
        // 1 kHz on a 30 Hz – 16 kHz log scale of 64 bands → band 35.
        assert_eq!(band, 35, "{:?}", s.left);
        assert!(peak > 0.85, "{peak}");
        assert!(s.right.iter().all(|v| *v == 0.0));
        assert!(s.left[5] < 0.3, "{}", s.left[5]);
    }

    #[test]
    fn silence_decays_and_folds() {
        let mut a = SpectrumAnalyzer::new(48_000);
        a.push(&tone(100.0, 1.0, 48_000), 2);
        let loud = a.analyze(0.0);
        a.push(&vec![0.0; FFT_SIZE * 2], 2);
        let later = a.analyze(0.5);
        assert!(later.level() < loud.level() * 0.1);
        assert_eq!(loud.bands(16, false).len(), 16);
        assert_eq!(loud.bands(32, true).len(), 32);
    }
}
