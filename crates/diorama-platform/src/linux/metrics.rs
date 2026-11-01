//! Machine and process load from `/proc` and sysfs, for wallpapers that
//! show it and for the per-wallpaper cost report.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

use crate::{ProcessLoad, Result, SystemMetrics};

fn read(path: impl AsRef<Path>) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// Busy and total jiffies of each `cpu` line of `/proc/stat` (the total
/// first, then each processor).
fn cpu_times(stat: &str) -> Vec<(u64, u64)> {
    stat.lines()
        .filter(|l| l.starts_with("cpu"))
        .map(|l| {
            // user nice system idle iowait irq softirq steal (guest time is
            // already in user and nice).
            let v: Vec<u64> = l
                .split_whitespace()
                .skip(1)
                .take(8)
                .filter_map(|n| n.parse().ok())
                .collect();
            let total: u64 = v.iter().sum();
            let idle = v.get(3).copied().unwrap_or(0) + v.get(4).copied().unwrap_or(0);
            (total.saturating_sub(idle), total)
        })
        .collect()
}

/// `MemTotal` and `MemAvailable` from `/proc/meminfo`, in bytes.
fn memory(meminfo: &str) -> Option<(u64, u64)> {
    let field = |name: &str| {
        meminfo
            .lines()
            .find_map(|l| l.strip_prefix(name))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|kb| kb.parse::<u64>().ok())
            .map(|kb| kb * 1024)
    };
    Some((field("MemTotal:")?, field("MemAvailable:")?))
}

/// Bytes received and sent by the interfaces `keep` accepts, from
/// `/proc/net/dev`.
fn network(dev: &str, keep: impl Fn(&str) -> bool) -> (u64, u64) {
    dev.lines()
        .skip(2)
        .filter_map(|l| l.split_once(':'))
        .filter(|(name, _)| keep(name.trim()))
        .map(|(_, rest)| {
            let v: Vec<u64> = rest
                .split_whitespace()
                .filter_map(|n| n.parse().ok())
                .collect();
            (
                v.first().copied().unwrap_or(0),
                v.get(8).copied().unwrap_or(0),
            )
        })
        .fold((0, 0), |(r, s), (r1, s1)| (r + r1, s + s1))
}

/// Physical interfaces have a device behind them (not `lo`, bridges, VPNs).
fn physical(interface: &str) -> bool {
    Path::new("/sys/class/net")
        .join(interface)
        .join("device")
        .exists()
}

/// The GPU with the most video memory that reports its load (amdgpu does,
/// in sysfs): its `device` folder.
fn amd_gpu() -> Option<std::path::PathBuf> {
    std::fs::read_dir("/sys/class/drm")
        .ok()?
        .flatten()
        .map(|e| e.path().join("device"))
        .filter(|d| d.join("gpu_busy_percent").exists())
        .max_by_key(|d| {
            read(d.join("mem_info_vram_total"))
                .and_then(|v| v.trim().parse::<u64>().ok())
                .unwrap_or(0)
        })
}

/// Reads the machine's load; rates (CPU, network) cover the time since the
/// previous [`SystemMonitor::sample`], so the first sample reads 0 there.
#[derive(Debug)]
pub struct SystemMonitor {
    cpu: Vec<(u64, u64)>,
    network: Option<(Instant, u64, u64)>,
    gpu: Option<std::path::PathBuf>,
}

impl SystemMonitor {
    pub fn new() -> Result<Self> {
        Ok(Self {
            cpu: read("/proc/stat")
                .map(|s| cpu_times(&s))
                .unwrap_or_default(),
            network: None,
            gpu: amd_gpu(),
        })
    }

    pub fn sample(&mut self) -> SystemMetrics {
        let mut m = SystemMetrics::default();
        let now = read("/proc/stat")
            .map(|s| cpu_times(&s))
            .unwrap_or_default();
        let percent = |(b1, t1): (u64, u64), (b0, t0): (u64, u64)| {
            let total = t1.saturating_sub(t0);
            if total == 0 {
                0.0
            } else {
                (b1.saturating_sub(b0) as f64 / total as f64 * 100.0).clamp(0.0, 100.0) as f32
            }
        };
        let mut rates = now.iter().zip(&self.cpu).map(|(a, b)| percent(*a, *b));
        m.cpu = rates.next().unwrap_or(0.0);
        m.cpu_per_core = rates.collect();
        self.cpu = now;

        if let Some((total, available)) = read("/proc/meminfo").and_then(|s| memory(&s)) {
            m.memory_total = total;
            m.memory_used = total.saturating_sub(available);
        }

        if let Some(gpu) = &self.gpu {
            let value =
                |name: &str| read(gpu.join(name)).and_then(|v| v.trim().parse::<u64>().ok());
            m.gpu = value("gpu_busy_percent").map(|p| p.min(100) as f32);
            m.vram_used = value("mem_info_vram_used");
            m.vram_total = value("mem_info_vram_total");
        }

        // Disks: the wallpaper API names them by Windows drive letter.

        if let Some(dev) = read("/proc/net/dev") {
            let (received, sent) = network(&dev, physical);
            let now = Instant::now();
            if let Some((then, r0, s0)) = self.network {
                let secs = now.duration_since(then).as_secs_f64().max(1e-3);
                m.download = received.saturating_sub(r0) as f64 / secs;
                m.upload = sent.saturating_sub(s0) as f64 / secs;
            }
            self.network = Some((now, received, sent));
        }
        m
    }
}

/// Parent and CPU time (user + system, clock ticks) from `/proc/<pid>/stat`.
fn process_stat(stat: &str) -> Option<(u32, u64)> {
    // The command name is in parentheses and may contain anything; fields
    // after it are space-separated: state ppid … utime(14) stime(15).
    let rest = &stat[stat.rfind(')')? + 1..];
    let f: Vec<&str> = rest.split_whitespace().collect();
    let parent = f.get(1)?.parse().ok()?;
    let utime: u64 = f.get(11)?.parse().ok()?;
    let stime: u64 = f.get(12)?.parse().ok()?;
    Some((parent, utime + stime))
}

/// Samples the load of process trees; rates cover the time since the
/// previous [`ProcessMonitor::sample`]. GPU figures need vendor tools and
/// stay 0.
#[derive(Debug)]
pub struct ProcessMonitor {
    /// CPU time (clock ticks) per process at the previous sample.
    cpu_times: HashMap<u32, u64>,
    sampled: Instant,
    processors: u32,
    ticks_per_second: u64,
    page_size: u64,
}

impl ProcessMonitor {
    pub fn new() -> Result<Self> {
        // SAFETY: plain configuration queries.
        let (ticks, page) = unsafe {
            (
                libc::sysconf(libc::_SC_CLK_TCK),
                libc::sysconf(libc::_SC_PAGESIZE),
            )
        };
        Ok(Self {
            cpu_times: HashMap::new(),
            sampled: Instant::now(),
            processors: std::thread::available_parallelism().map_or(1, |n| n.get() as u32),
            ticks_per_second: u64::try_from(ticks).unwrap_or(100).max(1),
            page_size: u64::try_from(page).unwrap_or(4096).max(1),
        })
    }

    /// Load of `processes`, and of `trees` with every process they started
    /// (and so on).
    pub fn sample(&mut self, processes: &[u32], trees: &[u32]) -> ProcessLoad {
        let stats: Vec<(u32, u32, u64)> = std::fs::read_dir("/proc")
            .map(|d| {
                d.flatten()
                    .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
                    .filter_map(|pid| {
                        let (parent, cpu) = process_stat(&read(format!("/proc/{pid}/stat"))?)?;
                        Some((pid, parent, cpu))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let parents: Vec<(u32, u32)> = stats.iter().map(|&(p, parent, _)| (p, parent)).collect();
        let mut pids = descendants(trees, &parents);
        pids.extend(processes);

        let now = Instant::now();
        let elapsed = now.duration_since(self.sampled).as_secs_f64().max(1e-3);
        self.sampled = now;
        let mut load = ProcessLoad {
            processes: pids.len() as u32,
            ..ProcessLoad::default()
        };
        let mut ticks = 0u64;
        let mut times = HashMap::new();
        for &(pid, _, cpu) in stats.iter().filter(|s| pids.contains(&s.0)) {
            if let Some(before) = self.cpu_times.get(&pid) {
                ticks += cpu.saturating_sub(*before);
            }
            times.insert(pid, cpu);
            // statm: size resident shared … (pages).
            load.ram += read(format!("/proc/{pid}/statm"))
                .and_then(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok())
                .map_or(0, |pages| pages * self.page_size);
        }
        self.cpu_times = times;
        load.cpu =
            (ticks as f64 / self.ticks_per_second as f64 / elapsed / f64::from(self.processors)
                * 100.0)
                .clamp(0.0, 100.0) as f32;
        load
    }
}

/// `roots` and their descendants among `(pid, parent)` pairs.
fn descendants(roots: &[u32], parents: &[(u32, u32)]) -> HashSet<u32> {
    let mut set: HashSet<u32> = roots.iter().copied().collect();
    // Parents come before children only by chance: repeat until stable.
    loop {
        let before = set.len();
        for &(pid, parent) in parents {
            if pid != parent && set.contains(&parent) {
                set.insert(pid);
            }
        }
        if set.len() == before {
            return set;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_proc_files() {
        let stat = "cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 60 0 20 400 20 0 0 0 0 0\n\
                    cpu1 40 0 30 400 30 0 0 0 0 0\nintr 1 2\n";
        assert_eq!(cpu_times(stat), vec![(150, 1000), (80, 500), (70, 500)]);
        let meminfo = "MemTotal:       16000000 kB\nMemFree:  1 kB\nMemAvailable:   4000000 kB\n";
        assert_eq!(memory(meminfo), Some((16_000_000 * 1024, 4_000_000 * 1024)));
        let dev = "Inter-|   Receive |  Transmit\n face |bytes packets|bytes\n\
                   lo: 500 5 0 0 0 0 0 0 500 5 0 0 0 0 0 0\n\
                   eth0: 1000 10 0 0 0 0 0 0 300 3 0 0 0 0 0 0\n";
        assert_eq!(network(dev, |i| i != "lo"), (1000, 300));
        // A command name with spaces and parentheses.
        let pstat = "42 (my (odd) app) S 7 42 42 0 -1 4194304 1 0 0 0 25 5 0 0 20 0 1 0";
        assert_eq!(process_stat(pstat), Some((7, 30)));
    }

    #[test]
    fn trees_include_grandchildren() {
        let parents = [(10, 1), (11, 10), (12, 11), (13, 1), (1, 1)];
        let set = descendants(&[10], &parents);
        assert_eq!(
            set.into_iter().collect::<std::collections::BTreeSet<_>>(),
            [10, 11, 12].into()
        );
    }
}
