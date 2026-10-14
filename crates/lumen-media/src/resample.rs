//! Streaming linear resampler + channel mapping for wallpaper audio.
//!
//! Linear interpolation is enough for ambient wallpaper sound; swap for a
//! windowed-sinc resampler if quality complaints appear.

#[derive(Debug, Clone)]
pub struct Resampler {
    src_channels: usize,
    dst_channels: usize,
    /// Source frames per destination frame.
    step: f64,
    /// Position between `prev` (0.0) and the next source frame (1.0).
    t: f64,
    prev: Option<Vec<f32>>,
}

impl Resampler {
    pub fn new(src_rate: u32, src_channels: usize, dst_rate: u32, dst_channels: usize) -> Self {
        Self {
            src_channels: src_channels.max(1),
            dst_channels: dst_channels.max(1),
            step: f64::from(src_rate.max(1)) / f64::from(dst_rate.max(1)),
            t: 0.0,
            prev: None,
        }
    }

    /// Maps one source frame to the destination channel layout.
    fn map_channels(&self, frame: &[f32]) -> Vec<f32> {
        match (self.src_channels, self.dst_channels) {
            (s, d) if s == d => frame.to_vec(),
            // Mono → every channel.
            (1, d) => vec![frame[0]; d],
            // Anything → mono: average.
            (s, 1) => vec![frame.iter().sum::<f32>() / s as f32],
            // Otherwise keep the first min(s, d) channels; silence the rest.
            (_, d) => (0..d)
                .map(|i| frame.get(i).copied().unwrap_or(0.0))
                .collect(),
        }
    }

    /// Feeds interleaved source samples, appending interleaved output to `out`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        for frame in input.chunks_exact(self.src_channels) {
            let cur = self.map_channels(frame);
            let Some(prev) = self.prev.take() else {
                self.prev = Some(cur);
                continue;
            };
            while self.t < 1.0 {
                let t = self.t as f32;
                out.extend(prev.iter().zip(&cur).map(|(a, b)| a + (b - a) * t));
                self.t += self.step;
            }
            self.t -= 1.0;
            self.prev = Some(cur);
        }
    }

    /// Forget history (after a seek/loop restart).
    pub fn reset(&mut self) {
        self.prev = None;
        self.t = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_rate_passes_through() {
        let mut r = Resampler::new(48_000, 2, 48_000, 2);
        let mut out = Vec::new();
        r.process(&[0.0, 0.0, 1.0, -1.0, 0.5, -0.5], &mut out);
        // First frame is held back as history; each later frame emits the previous one.
        assert_eq!(out, [0.0, 0.0, 1.0, -1.0]);
    }

    #[test]
    fn upsampling_doubles_frames_and_interpolates() {
        let mut r = Resampler::new(24_000, 1, 48_000, 1);
        let mut out = Vec::new();
        r.process(&[0.0, 1.0, 0.0], &mut out);
        assert_eq!(out, [0.0, 0.5, 1.0, 0.5]);
    }

    #[test]
    fn rate_ratio_holds_over_long_input() {
        let mut r = Resampler::new(44_100, 2, 48_000, 2);
        let input = vec![0.25f32; 44_100 * 2];
        let mut out = Vec::new();
        r.process(&input, &mut out);
        let frames = out.len() / 2;
        assert!((47_990..=48_000).contains(&frames), "{frames}");
        assert!(out.iter().all(|&s| (s - 0.25).abs() < 1e-6));
    }

    #[test]
    fn channel_mapping() {
        let mono_to_stereo = Resampler::new(1, 1, 1, 2);
        assert_eq!(mono_to_stereo.map_channels(&[0.5]), [0.5, 0.5]);
        let stereo_to_mono = Resampler::new(1, 2, 1, 1);
        assert_eq!(stereo_to_mono.map_channels(&[1.0, 0.0]), [0.5]);
        let six_to_stereo = Resampler::new(1, 6, 1, 2);
        assert_eq!(
            six_to_stereo.map_channels(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]),
            [1.0, 2.0]
        );
        let stereo_to_quad = Resampler::new(1, 2, 1, 4);
        assert_eq!(
            stereo_to_quad.map_channels(&[1.0, 2.0]),
            [1.0, 2.0, 0.0, 0.0]
        );
    }
}
