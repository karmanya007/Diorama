//! Values of layers that move: between keys, or with the sound the system
//! plays. Evaluated here (no scripts); the host gives the results to the
//! renderer.

/// Bands of an audio spectrum, bass first (50 Hz to 12 kHz, evenly by pitch).
pub const BANDS: usize = 64;

/// One, two, three or four numbers (the rest zero).
pub type Numbers = [f32; 4];

/// The moving values of one layer.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerMotion {
    /// Index into [`crate::Scene::layers`]; it has a node.
    pub layer: usize,
    /// Its colour without its opacity, and that: each may move on its own.
    pub color: [f32; 4],
    pub opacity: f32,
    pub tracks: Vec<(MotionTarget, Track)>,
}

/// What moves.
#[derive(Debug, Clone, PartialEq)]
pub enum MotionTarget {
    /// The node's origin (x, y), scale (x, y) and turn (radians).
    Position,
    Scale,
    Rotation,
    /// Red, green, blue, alpha.
    Color,
    Opacity,
    /// A uniform of all passes of one effect, and how many numbers it takes.
    Setting {
        effect: usize,
        uniform: String,
        numbers: usize,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Track {
    Keys(Keys),
    Audio(AudioTrack),
}

impl Track {
    /// Every value it is made from.
    pub fn values(&self) -> impl Iterator<Item = &Numbers> {
        let (keys, ends): (&[Key], Vec<&Numbers>) = match self {
            Self::Keys(k) => (&k.keys, Vec::new()),
            Self::Audio(a) => (&[], vec![&a.from, &a.to]),
        };
        keys.iter().map(|k| &k.value).chain(ends)
    }

    /// The same movement of values made by `change`.
    pub fn map(&self, change: impl Fn(Numbers) -> Numbers) -> Self {
        match self {
            Self::Keys(k) => Self::Keys(Keys {
                keys: k
                    .keys
                    .iter()
                    .map(|key| Key {
                        value: change(key.value),
                        ..*key
                    })
                    .collect(),
                ..*k
            }),
            Self::Audio(a) => Self::Audio(AudioTrack {
                from: change(a.from),
                to: change(a.to),
                ..*a
            }),
        }
    }
}

/// Values at times, and what happens between them.
#[derive(Debug, Clone, PartialEq)]
pub struct Keys {
    /// By time; at least one.
    keys: Vec<Key>,
    /// Seconds until it repeats or ends.
    length: f32,
    repeat: Repeat,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Key {
    /// Seconds from the start.
    pub time: f32,
    pub value: Numbers,
    /// How it goes on to the next.
    pub ease: Ease,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Repeat {
    /// Starts over; from the last key it goes on to the first one.
    #[default]
    Loop,
    /// There and back.
    Mirror,
    Once,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Ease {
    Linear,
    /// Starts slowly.
    In,
    /// Ends slowly.
    Out,
    #[default]
    InOut,
    /// Stays until the next key.
    Hold,
}

impl Ease {
    fn at(self, u: f32) -> f32 {
        match self {
            Self::Linear => u,
            Self::In => u * u,
            Self::Out => u * (2.0 - u),
            Self::InOut => u * u * (3.0 - 2.0 * u),
            Self::Hold => 0.0,
        }
    }
}

impl Keys {
    /// `None` without keys. `length`: at least up to the last key.
    pub fn new(mut keys: Vec<Key>, length: Option<f32>, repeat: Repeat) -> Option<Self> {
        keys.sort_by(|a, b| a.time.total_cmp(&b.time));
        let last = keys.last()?.time;
        Some(Self {
            keys,
            length: length.map_or(last, |l| l.max(last)),
            repeat,
        })
    }

    /// The value at `time` seconds.
    pub fn at(&self, time: f32) -> Numbers {
        let (Some(first), Some(last)) = (self.keys.first(), self.keys.last()) else {
            return [0.0; 4];
        };
        let length = self.length;
        if length <= 0.0 {
            return last.value;
        }
        let time = match self.repeat {
            Repeat::Loop => time.rem_euclid(length),
            Repeat::Mirror => {
                let t = time.rem_euclid(2.0 * length);
                if t > length { 2.0 * length - t } else { t }
            }
            Repeat::Once => time.clamp(0.0, length),
        };
        let next = self.keys.partition_point(|k| k.time <= time);
        // Around the end a loop goes from the last key to the first.
        let (from, to, start, end) = match (next.checked_sub(1), self.keys.get(next)) {
            (Some(i), Some(to)) => (&self.keys[i], to, self.keys[i].time, to.time),
            (None, _) if self.repeat == Repeat::Loop => {
                (last, first, last.time - length, first.time)
            }
            (None, _) => return first.value,
            (Some(_), None) if self.repeat == Repeat::Loop => {
                (last, first, last.time, first.time + length)
            }
            (Some(_), None) => return last.value,
        };
        if end <= start {
            return from.value;
        }
        let k = from
            .ease
            .at(((time - start) / (end - start)).clamp(0.0, 1.0));
        mix(from.value, to.value, k)
    }
}

fn mix(a: Numbers, b: Numbers, k: f32) -> Numbers {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * k)
}

/// A value between two that follows the sound.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioTrack {
    pub range: AudioRange,
    /// The value in silence and at its loudest.
    pub from: Numbers,
    pub to: Numbers,
    /// Seconds it takes to follow.
    pub smoothing: f32,
    /// Levels up to this count as silence (0..1).
    pub threshold: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AudioRange {
    #[default]
    All,
    /// Up to about 250 Hz.
    Bass,
    /// Up to about 4 kHz.
    Middle,
    Treble,
}

impl AudioRange {
    fn bands(self) -> std::ops::Range<usize> {
        match self {
            Self::All => 0..BANDS,
            Self::Bass => 0..19,
            Self::Middle => 19..51,
            Self::Treble => 51..BANDS,
        }
    }
}

impl AudioTrack {
    /// How loud it is: 0 to 1.
    pub fn level(&self, spectrum: &[[f32; BANDS]; 2]) -> f32 {
        let bands = self.range.bands();
        let count = (2 * bands.len()) as f32;
        let sum: f32 = spectrum.iter().flat_map(|s| &s[bands.clone()]).sum();
        let level = (sum / count).clamp(0.0, 1.0);
        ((level - self.threshold) / (1.0 - self.threshold).max(0.01)).clamp(0.0, 1.0)
    }

    /// Where a level of `now` is `dt` seconds later, the sound being `level`.
    pub fn follow(&self, now: f32, level: f32, dt: f32) -> f32 {
        if self.smoothing <= 0.0 {
            return level;
        }
        let next = level + (now - level) * (-dt.max(0.0) / self.smoothing).exp();
        // It comes to rest, so that nothing is drawn for nothing.
        if (next - level).abs() < 1e-3 {
            level
        } else {
            next
        }
    }

    pub fn at(&self, level: f32) -> Numbers {
        mix(self.from, self.to, level)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(time: f32, value: f32, ease: Ease) -> Key {
        Key {
            time,
            value: [value, 0.0, 0.0, 0.0],
            ease,
        }
    }

    fn at(keys: &Keys, time: f32) -> f32 {
        keys.at(time)[0]
    }

    #[test]
    fn keys_are_passed_in_order_whatever_the_order_they_came_in() {
        let k = Keys::new(
            vec![key(2.0, 10.0, Ease::Linear), key(0.0, 0.0, Ease::Linear)],
            None,
            Repeat::Once,
        )
        .unwrap();
        for (t, v) in [
            (-1.0, 0.0),
            (0.0, 0.0),
            (0.5, 2.5),
            (2.0, 10.0),
            (9.0, 10.0),
        ] {
            assert_eq!(at(&k, t), v, "at {t}");
        }
        assert!(Keys::new(Vec::new(), None, Repeat::Loop).is_none());
    }

    #[test]
    fn loops_go_from_the_last_key_to_the_first() {
        let keys = vec![key(1.0, 0.0, Ease::Linear), key(2.0, 10.0, Ease::Linear)];
        let k = Keys::new(keys.clone(), Some(5.0), Repeat::Loop).unwrap();
        // From 2 s (10) to 6 s (0), which is 1 s again.
        for (t, v) in [
            (1.0, 0.0),
            (1.5, 5.0),
            (2.0, 10.0),
            (4.0, 5.0),
            (5.5, 1.25),
            (0.5, 1.25),
        ] {
            assert!((at(&k, t) - v).abs() < 1e-4, "{} at {t}", at(&k, t));
        }
        // Without more length it jumps back.
        let keys = vec![key(0.0, 0.0, Ease::Linear), key(2.0, 10.0, Ease::Linear)];
        let k = Keys::new(keys, None, Repeat::Loop).unwrap();
        assert!((at(&k, 3.0) - 5.0).abs() < 1e-4);
        assert!(at(&k, 2.001) < 0.1);
    }

    #[test]
    fn mirrors_go_there_and_back() {
        let keys = vec![key(0.0, 0.0, Ease::Linear), key(2.0, 10.0, Ease::Linear)];
        let k = Keys::new(keys, Some(3.0), Repeat::Mirror).unwrap();
        for (t, v) in [
            (1.0, 5.0),
            (2.5, 10.0),
            (3.5, 10.0),
            (5.0, 5.0),
            (6.0, 0.0),
            (7.0, 5.0),
        ] {
            assert!((at(&k, t) - v).abs() < 1e-4, "{} at {t}", at(&k, t));
        }
    }

    #[test]
    fn easing() {
        for (ease, half) in [
            (Ease::Linear, 0.5),
            (Ease::In, 0.25),
            (Ease::Out, 0.75),
            (Ease::InOut, 0.5),
            (Ease::Hold, 0.0),
        ] {
            let k = Keys::new(
                vec![key(0.0, 0.0, ease), key(1.0, 1.0, ease)],
                None,
                Repeat::Once,
            );
            let k = k.unwrap();
            assert_eq!(at(&k, 0.5), half, "{ease:?}");
            assert_eq!(at(&k, 1.0), 1.0, "{ease:?}");
        }
        assert!(Ease::InOut.at(0.1) < 0.1);
    }

    #[test]
    fn keys_at_one_time_do_not_divide_by_zero() {
        let k = Keys::new(
            vec![key(0.0, 1.0, Ease::InOut), key(0.0, 2.0, Ease::InOut)],
            None,
            Repeat::Loop,
        );
        assert_eq!(at(&k.unwrap(), 3.0), 2.0);
        let one = Keys::new(vec![key(1.0, 4.0, Ease::InOut)], None, Repeat::Loop).unwrap();
        assert_eq!(at(&one, 0.3), 4.0);
    }

    #[test]
    fn sound_levels() {
        let mut track = AudioTrack {
            range: AudioRange::Bass,
            from: [1.0, 0.0, 0.0, 0.0],
            to: [3.0, 0.0, 0.0, 0.0],
            smoothing: 0.0,
            threshold: 0.0,
        };
        let mut spectrum = [[0.0; BANDS]; 2];
        spectrum[0][..19].fill(1.0);
        assert_eq!(track.level(&spectrum), 0.5, "one of two channels");
        assert_eq!(track.at(0.5)[0], 2.0);
        track.threshold = 0.5;
        assert_eq!(track.level(&spectrum), 0.0);
        spectrum[1][..19].fill(0.5);
        assert!((track.level(&spectrum) - 0.5).abs() < 1e-5);
        track.range = AudioRange::Treble;
        assert_eq!(track.level(&spectrum), 0.0);

        track.smoothing = 0.1;
        let slow = track.follow(0.0, 1.0, 0.05);
        assert!(slow > 0.3 && slow < 0.5, "{slow}");
        assert_eq!(track.follow(0.0005, 0.0, 0.01), 0.0, "comes to rest");
        assert_eq!(track.follow(0.2, 0.2, -1.0), 0.2);
    }
}
