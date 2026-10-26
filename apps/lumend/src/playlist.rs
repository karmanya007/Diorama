//! Which wallpaper of a playlist plays when.

use lumen_ipc::{Order, Playlist, Schedule};

/// Minutes since local midnight, 0..1440.
pub fn minute_of_day() -> u32 {
    (lumen_platform::day_fraction() * 1440.0) as u32 % 1440
}

/// The item a time-of-day playlist shows at `minute`: the one with the
/// latest start not after it, else (before the first start) the latest of
/// the day before.
pub fn by_time(starts: &[u32], minute: u32) -> Option<usize> {
    let latest = |pred: &dyn Fn(u32) -> bool| {
        starts
            .iter()
            .enumerate()
            .filter(|(_, s)| pred(**s))
            .max_by_key(|(_, s)| **s)
            .map(|(i, _)| i)
    };
    latest(&|s| s <= minute).or_else(|| latest(&|_| true))
}

/// Where a playlist starts.
pub fn first(playlist: &Playlist, rng: &mut Rng) -> usize {
    match &playlist.schedule {
        Schedule::TimeOfDay { starts } => by_time(starts, minute_of_day()).unwrap_or(0),
        Schedule::Interval { .. } => match playlist.order {
            Order::Sequential => 0,
            Order::Shuffle => rng.below(playlist.items.len()),
        },
    }
}

/// The item after `current` (`Next`, or an interval passing).
pub fn after(playlist: &Playlist, current: usize, rng: &mut Rng) -> usize {
    let n = playlist.items.len().max(1);
    match playlist.order {
        Order::Shuffle if n > 1 => {
            // Any other item.
            let step = 1 + rng.below(n - 1);
            (current + step) % n
        }
        _ => (current + 1) % n,
    }
}

/// A small xorshift generator; shuffling needs no more.
pub struct Rng(u64);

impl Rng {
    pub fn from_time() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |d| d.as_nanos() as u64);
        Self(nanos | 1)
    }

    /// 0..n (0 when n is 0).
    pub fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        if n == 0 {
            0
        } else {
            (self.0 % n as u64) as usize
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn playlist(order: Order, n: usize) -> Playlist {
        Playlist {
            name: "p".into(),
            items: (0..n).map(|i| PathBuf::from(format!("{i}.png"))).collect(),
            order,
            schedule: Schedule::Interval { minutes: 10 },
        }
    }

    #[test]
    fn time_of_day_picks_the_latest_start() {
        // 07:00 morning, 18:30 evening, 22:00 night.
        let starts = [420, 1110, 1320];
        assert_eq!(by_time(&starts, 600), Some(0));
        assert_eq!(by_time(&starts, 1110), Some(1));
        assert_eq!(by_time(&starts, 1439), Some(2));
        assert_eq!(by_time(&starts, 60), Some(2), "after midnight: still night");
        assert_eq!(by_time(&[], 60), None);
    }

    #[test]
    fn sequential_wraps_and_shuffle_never_repeats() {
        let mut rng = Rng(42);
        let p = playlist(Order::Sequential, 3);
        assert_eq!(after(&p, 2, &mut rng), 0);
        let p = playlist(Order::Shuffle, 4);
        for current in 0..4 {
            for _ in 0..50 {
                let next = after(&p, current, &mut rng);
                assert!(next != current && next < 4);
            }
        }
        assert_eq!(after(&playlist(Order::Shuffle, 1), 0, &mut rng), 0);
    }
}
