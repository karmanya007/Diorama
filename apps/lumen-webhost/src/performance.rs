//! Answers `http://127.0.0.1:<port>/performance` (and `/status`) the way the
//! PerformanceMonitor companion app does, which widgets such as 3343374776
//! poll for CPU, RAM, disk, GPU and network figures. Lumen reads Windows' own
//! counters instead, so no extra program runs and no port is opened: the
//! WebView hands these requests to us before they reach the network.
//!
//! The format follows the app's documented response: `psutil` numbers
//! rounded to one decimal, disks and memory as `"<used> GB/<total> GB"`
//! (GiB), network in kilobits per second, a UNIX `timestamp`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use lumen_platform::{SystemMetrics, SystemMonitor};
use serde_json::{Map, Value, json};

/// The sampler stops when no page asked for this long.
const IDLE: Duration = Duration::from_secs(30);
const INTERVAL: Duration = Duration::from_secs(1);
const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// Latest figures, sampled on a thread that runs only while pages ask.
#[derive(Clone, Default)]
pub struct Performance {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    latest: Mutex<Option<SystemMetrics>>,
    asked: Mutex<Option<Instant>>,
    running: AtomicBool,
}

impl Performance {
    /// Response body for `path` (`/performance` or `/status`), `None` for
    /// anything else.
    pub fn respond(&self, path: &str, port: u16) -> Option<Vec<u8>> {
        let body = match path {
            "/performance" => {
                self.wake();
                let latest = self.inner.latest.lock().ok()?.clone().unwrap_or_default();
                json(&latest, unix_now())
            }
            "/status" => {
                self.wake();
                let gpu = self
                    .inner
                    .latest
                    .lock()
                    .ok()?
                    .as_ref()
                    .is_some_and(|m| m.gpu.is_some());
                json!({
                    "status": "running",
                    "service": "Lumen",
                    "port": port,
                    "gpu_available": gpu,
                    "timestamp": unix_now(),
                })
            }
            _ => return None,
        };
        Some(body.to_string().into_bytes())
    }

    /// Notes the request and starts the sampler if it is not running.
    fn wake(&self) {
        if let Ok(mut asked) = self.inner.asked.lock() {
            *asked = Some(Instant::now());
        }
        if self.inner.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let inner = Arc::clone(&self.inner);
        std::thread::spawn(move || {
            match SystemMonitor::new() {
                Ok(mut monitor) => loop {
                    let sample = monitor.sample();
                    if let Ok(mut latest) = inner.latest.lock() {
                        *latest = Some(sample);
                    }
                    let idle = inner
                        .asked
                        .lock()
                        .map(|a| a.is_none_or(|t| t.elapsed() > IDLE))
                        .unwrap_or(true);
                    if idle {
                        break;
                    }
                    std::thread::sleep(INTERVAL);
                },
                Err(e) => tracing::warn!("system figures unavailable: {e}"),
            }
            inner.running.store(false, Ordering::SeqCst);
        });
    }
}

fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn gib_pair(used: u64, total: u64) -> String {
    format!("{:.1} GB/{:.1} GB", used as f64 / GIB, total as f64 / GIB)
}

fn percent(used: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        round1(used as f64 * 100.0 / total as f64)
    }
}

/// The companion app's response for `m` (numbers the page expects are
/// always present: 0 when unknown).
pub fn json(m: &SystemMetrics, timestamp: f64) -> Value {
    let mut p = Map::new();
    p.insert("cpu".into(), json!(round1(f64::from(m.cpu))));
    p.insert(
        "cpu_percore".into(),
        json!(
            m.cpu_per_core
                .iter()
                .map(|&c| round1(f64::from(c)))
                .collect::<Vec<_>>()
        ),
    );
    p.insert(
        "memory".into(),
        json!(percent(m.memory_used, m.memory_total)),
    );
    p.insert(
        "memory_gb".into(),
        json!(gib_pair(m.memory_used, m.memory_total)),
    );
    p.insert(
        "gpu_usage".into(),
        json!(round1(f64::from(m.gpu.unwrap_or(0.0)))),
    );
    let (vram_used, vram_total) = (m.vram_used.unwrap_or(0), m.vram_total.unwrap_or(0));
    p.insert("vram_usage".into(), json!(percent(vram_used, vram_total)));
    p.insert(
        "vram_gb".into(),
        if vram_total > 0 {
            json!(gib_pair(vram_used, vram_total))
        } else {
            Value::Null
        },
    );
    // Not available without vendor libraries.
    p.insert("gpu_temp".into(), Value::Null);
    p.insert(
        "upload_speed".into(),
        json!(round1(m.upload * 8.0 / 1000.0)),
    );
    p.insert(
        "download_speed".into(),
        json!(round1(m.download * 8.0 / 1000.0)),
    );
    for d in &m.disks {
        p.insert(
            format!("{}_disk", d.letter.to_ascii_lowercase()),
            json!(gib_pair(d.used, d.total)),
        );
    }
    json!({ "hwinfo": [], "psutil": p, "timestamp": timestamp })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumen_platform::DiskUsage;

    #[test]
    fn matches_the_companion_apps_format() {
        let m = SystemMetrics {
            cpu: 6.14,
            cpu_per_core: vec![3.06, 9.0],
            memory_used: 8 << 30,
            memory_total: 32 << 30,
            gpu: Some(12.345),
            vram_used: Some(3 << 29),
            vram_total: Some(8 << 30),
            disks: vec![DiskUsage {
                letter: 'C',
                used: 120 << 30,
                total: 476 << 30,
            }],
            upload: 1000.0,
            download: 12_500.0,
        };
        let v = json(&m, 1.5);
        let p = &v["psutil"];
        assert_eq!(p["cpu"], 6.1);
        assert_eq!(p["cpu_percore"], json!([3.1, 9.0]));
        assert_eq!(p["memory"], 25.0);
        assert_eq!(p["memory_gb"], "8.0 GB/32.0 GB");
        assert_eq!(p["gpu_usage"], 12.3);
        assert_eq!(p["vram_usage"], 18.8);
        assert_eq!(p["vram_gb"], "1.5 GB/8.0 GB");
        assert_eq!(p["upload_speed"], 8.0);
        assert_eq!(p["download_speed"], 100.0);
        // What 3343374776 does with a disk: parseFloat of each side.
        assert_eq!(p["c_disk"], "120.0 GB/476.0 GB");
        assert_eq!(v["timestamp"], 1.5);
        assert!(v["hwinfo"].is_array());
    }

    #[test]
    fn unknown_gpu_still_gives_numbers() {
        let p = json(&SystemMetrics::default(), 0.0)["psutil"].clone();
        assert_eq!(p["gpu_usage"], 0.0);
        assert_eq!(p["vram_usage"], 0.0);
        assert_eq!(p["memory"], 0.0);
    }

    #[test]
    fn only_known_paths_answer() {
        let perf = Performance::default();
        assert!(perf.respond("/other", 5000).is_none());
        let status: Value =
            serde_json::from_slice(&perf.respond("/status", 5000).unwrap()).unwrap();
        assert_eq!(status["port"], 5000);
    }
}
