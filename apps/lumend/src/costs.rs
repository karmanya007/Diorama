//! What each wallpaper costs to show (P12): Lumen's own load (this daemon,
//! web hosts and their browser processes) sampled while one wallpaper plays
//! alone, averaged per wallpaper and kept in `<cache dir>/costs.json`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use lumen_ipc::Cost;
use lumen_platform::{ProcessLoad, ProcessMonitor};

const FILE: &str = "costs.json";
pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(2);
const SAVE_INTERVAL: Duration = Duration::from_secs(60);
/// Samples of the same showing skipped first (loading, shader compiles).
const SETTLE_SAMPLES: u32 = 3;
/// The average follows the last ~5 minutes rather than all time.
const MAX_WEIGHT: f32 = 150.0;

/// One wallpaper playing alone on every monitor that shows anything.
#[derive(Debug, Clone, PartialEq)]
pub struct Showing {
    pub path: PathBuf,
    pub pixels: u64,
    pub fps: u32,
}

pub struct Costs {
    monitor: Option<ProcessMonitor>,
    costs: Vec<Cost>,
    file: Option<PathBuf>,
    next_sample: Instant,
    saved: Instant,
    dirty: bool,
    /// The showing seen at the last samples, and how many in a row.
    streak: Option<(Showing, u32)>,
}

impl Costs {
    pub fn load(cache_dir: Option<&Path>) -> Self {
        let file = cache_dir.map(|d| d.join(FILE));
        let costs = file
            .as_deref()
            .and_then(|f| std::fs::read(f).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let monitor = ProcessMonitor::new()
            .inspect_err(|e| tracing::info!("wallpaper costs not measured: {e}"))
            .ok();
        Self {
            monitor,
            costs,
            file,
            next_sample: Instant::now() + SAMPLE_INTERVAL,
            saved: Instant::now(),
            dirty: false,
            streak: None,
        }
    }

    pub fn list(&self) -> Vec<Cost> {
        self.costs.clone()
    }

    /// When the next sample is due.
    pub fn next_sample(&self) -> Instant {
        self.next_sample
    }

    /// Samples the load of `processes` and the process trees of `trees` if
    /// due; it counts for `showing` once that has lasted a few samples.
    pub fn sample(&mut self, showing: Option<Showing>, processes: &[u32], trees: &[u32]) {
        let now = Instant::now();
        if now < self.next_sample {
            return;
        }
        let elapsed = SAMPLE_INTERVAL + now.duration_since(self.next_sample);
        self.next_sample = now + SAMPLE_INTERVAL;
        let Some(monitor) = &mut self.monitor else {
            return;
        };
        // Sampled even when it will not count: CPU time is a difference.
        let load = monitor.sample(processes, trees);
        let streak = match (self.streak.take(), showing) {
            (Some((last, n)), Some(s)) if last == s => Some((s, n + 1)),
            (_, Some(s)) => Some((s, 1)),
            (_, None) => None,
        };
        if let Some((s, n)) = &streak
            && *n > SETTLE_SAMPLES
        {
            add_sample(&mut self.costs, s, load, elapsed);
            self.dirty = true;
        }
        self.streak = streak;
        if self.dirty && now.duration_since(self.saved) >= SAVE_INTERVAL {
            self.save();
        }
    }

    pub fn save(&mut self) {
        self.saved = Instant::now();
        let Some(file) = &self.file else { return };
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let written = file
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| {
                let bytes =
                    serde_json::to_vec_pretty(&self.costs).map_err(std::io::Error::other)?;
                std::fs::write(file, bytes)
            });
        if let Err(e) = written {
            tracing::warn!("cannot save {}: {e}", file.display());
        }
    }
}

/// Folds one sample into `showing`'s average; a different monitor area or
/// frame rate cap starts the average again.
fn add_sample(costs: &mut Vec<Cost>, showing: &Showing, load: ProcessLoad, elapsed: Duration) {
    let i = match costs.iter().position(|c| c.path == showing.path) {
        Some(i) => i,
        None => {
            costs.push(empty(showing));
            costs.len() - 1
        }
    };
    let cost = &mut costs[i];
    if cost.pixels != showing.pixels || cost.fps != showing.fps {
        *cost = empty(showing);
    }
    let samples = cost.seconds as f32 / SAMPLE_INTERVAL.as_secs_f32();
    let w = 1.0 / (samples.min(MAX_WEIGHT) + 1.0);
    let mix = |old: f32, new: f32| old + (new - old) * w;
    let mib = |bytes: u64| (bytes / (1 << 20)) as f32;
    cost.cpu = mix(cost.cpu, load.cpu);
    cost.gpu = mix(cost.gpu, load.gpu);
    cost.vram_mb = mix(cost.vram_mb as f32, mib(load.vram)).round() as u32;
    cost.ram_mb = mix(cost.ram_mb as f32, mib(load.ram)).round() as u32;
    cost.seconds += elapsed.as_secs() as u32;
}

fn empty(showing: &Showing) -> Cost {
    Cost {
        path: showing.path.clone(),
        cpu: 0.0,
        gpu: 0.0,
        vram_mb: 0,
        ram_mb: 0,
        pixels: showing.pixels,
        fps: showing.fps,
        seconds: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn showing(path: &str, fps: u32) -> Showing {
        Showing {
            path: PathBuf::from(path),
            pixels: 1920 * 1080,
            fps,
        }
    }

    fn load(cpu: f32, gpu: f32) -> ProcessLoad {
        ProcessLoad {
            cpu,
            gpu,
            vram: 200 << 20,
            ram: 300 << 20,
            processes: 1,
        }
    }

    #[test]
    fn averages_per_wallpaper_and_restarts_on_other_conditions() {
        let mut costs = Vec::new();
        let a = showing("a", 30);
        add_sample(&mut costs, &a, load(2.0, 10.0), SAMPLE_INTERVAL);
        add_sample(&mut costs, &a, load(4.0, 20.0), SAMPLE_INTERVAL);
        add_sample(
            &mut costs,
            &showing("b", 30),
            load(50.0, 50.0),
            SAMPLE_INTERVAL,
        );
        assert_eq!(costs.len(), 2);
        assert!((costs[0].cpu - 3.0).abs() < 1e-4);
        assert!((costs[0].gpu - 15.0).abs() < 1e-4);
        assert_eq!(
            (costs[0].vram_mb, costs[0].ram_mb, costs[0].seconds),
            (200, 300, 4)
        );

        // A different frame rate cap measures again from scratch.
        add_sample(
            &mut costs,
            &showing("a", 60),
            load(8.0, 40.0),
            SAMPLE_INTERVAL,
        );
        assert_eq!((costs[0].cpu, costs[0].fps, costs[0].seconds), (8.0, 60, 2));
    }

    #[test]
    fn long_averages_still_follow_changes() {
        let mut costs = Vec::new();
        let a = showing("a", 30);
        for _ in 0..1000 {
            add_sample(&mut costs, &a, load(10.0, 10.0), SAMPLE_INTERVAL);
        }
        add_sample(&mut costs, &a, load(160.0, 10.0), SAMPLE_INTERVAL);
        // Weight 1/151 at most: one outlier moves it by about 1.
        assert!((costs[0].cpu - 11.0).abs() < 0.01, "{}", costs[0].cpu);
    }
}
