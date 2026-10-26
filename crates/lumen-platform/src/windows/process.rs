//! Load of Lumen's own processes (the daemon, web hosts and the WebView2
//! processes they start): CPU from process times, GPU and video memory from
//! PDH's per-process counters, as Task Manager shows them.

use std::collections::{HashMap, HashSet};
use std::mem::size_of;
use std::time::Instant;

use windows::Win32::Foundation::{CloseHandle, FILETIME};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Performance::{
    PDH_HCOUNTER, PDH_HQUERY, PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData,
    PdhOpenQueryW,
};
use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
};
use windows::core::{HSTRING, PCWSTR};

use super::metrics::{counter_array, pdh};
use crate::{ProcessLoad, Result};

/// Samples the load of process trees; rates cover the time since the
/// previous [`ProcessMonitor::sample`].
#[derive(Debug)]
pub struct ProcessMonitor {
    query: PDH_HQUERY,
    gpu_engines: Option<PDH_HCOUNTER>,
    gpu_memory: Option<PDH_HCOUNTER>,
    /// CPU time (100 ns units) per process at the previous sample.
    cpu_times: HashMap<u32, u64>,
    sampled: Instant,
    processors: u32,
}

impl ProcessMonitor {
    pub fn new() -> Result<Self> {
        let mut query = PDH_HQUERY::default();
        // SAFETY: plain PDH calls with out-pointers to locals.
        unsafe {
            pdh(
                "PdhOpenQueryW",
                PdhOpenQueryW(PCWSTR::null(), 0, &mut query),
            )?;
            let add = |path: &str| {
                let mut counter = PDH_HCOUNTER::default();
                let status = PdhAddEnglishCounterW(query, &HSTRING::from(path), 0, &mut counter);
                (status == 0).then_some(counter)
            };
            let gpu_engines = add(r"\GPU Engine(*)\Utilization Percentage");
            let gpu_memory = add(r"\GPU Process Memory(*)\Dedicated Usage");
            PdhCollectQueryData(query);
            let mut info = SYSTEM_INFO::default();
            GetSystemInfo(&mut info);
            Ok(Self {
                query,
                gpu_engines,
                gpu_memory,
                cpu_times: HashMap::new(),
                sampled: Instant::now(),
                processors: info.dwNumberOfProcessors.max(1),
            })
        }
    }

    /// Load of `processes`, and of `trees` with every process they started
    /// (and so on).
    pub fn sample(&mut self, processes: &[u32], trees: &[u32]) -> ProcessLoad {
        // SAFETY: the query belongs to this monitor.
        unsafe { PdhCollectQueryData(self.query) };
        let mut pids = descendants(trees);
        pids.extend(processes);
        let now = Instant::now();
        let elapsed = now.duration_since(self.sampled).as_secs_f64().max(1e-3);
        self.sampled = now;

        let mut load = ProcessLoad {
            processes: pids.len() as u32,
            ..ProcessLoad::default()
        };
        let mut cpu_100ns = 0u64;
        let mut times = HashMap::new();
        for &pid in &pids {
            let Some((cpu, ram)) = process_usage(pid) else {
                continue;
            };
            load.ram += ram;
            if let Some(before) = self.cpu_times.get(&pid) {
                cpu_100ns += cpu.saturating_sub(*before);
            }
            times.insert(pid, cpu);
        }
        self.cpu_times = times;
        load.cpu = (cpu_100ns as f64 / 1e7 / elapsed / f64::from(self.processors) * 100.0)
            .clamp(0.0, 100.0) as f32;

        let ours = |name: &str| {
            name.strip_prefix("pid_")
                .and_then(|rest| rest.split('_').next())
                .and_then(|pid| pid.parse().ok())
                .is_some_and(|pid: u32| pids.contains(&pid))
        };
        if let Some(counter) = self.gpu_engines {
            // Per engine (summed over our processes), the busiest one.
            let mut engines: HashMap<String, f64> = HashMap::new();
            for (name, value) in counter_array(counter) {
                if !ours(&name) {
                    continue;
                }
                let engine = name
                    .split_once("_luid_")
                    .map(|(_, e)| e.split("_engtype_").next().unwrap_or_default().to_owned())
                    .unwrap_or_default();
                *engines.entry(engine).or_default() += value;
            }
            load.gpu = engines
                .values()
                .copied()
                .fold(0.0, f64::max)
                .clamp(0.0, 100.0) as f32;
        }
        if let Some(counter) = self.gpu_memory {
            load.vram = counter_array(counter)
                .iter()
                .filter(|(name, _)| ours(name))
                .map(|(_, v)| v.max(0.0) as u64)
                .sum();
        }
        load
    }
}

impl Drop for ProcessMonitor {
    fn drop(&mut self) {
        // SAFETY: closes the query this monitor opened (and its counters).
        unsafe { PdhCloseQuery(self.query) };
    }
}

/// `roots` and their descendants among running processes.
fn descendants(roots: &[u32]) -> HashSet<u32> {
    let mut parents: Vec<(u32, u32)> = Vec::new();
    // SAFETY: snapshot walk with a correctly sized entry; handle closed.
    unsafe {
        if let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
            let mut entry = PROCESSENTRY32W {
                dwSize: size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };
            let mut ok = Process32FirstW(snapshot, &mut entry).is_ok();
            while ok {
                parents.push((entry.th32ProcessID, entry.th32ParentProcessID));
                ok = Process32NextW(snapshot, &mut entry).is_ok();
            }
            let _ = CloseHandle(snapshot);
        }
    }
    let mut set: HashSet<u32> = roots.iter().copied().collect();
    // Parents come before children only by chance: repeat until stable.
    loop {
        let before = set.len();
        for &(pid, parent) in &parents {
            if pid != parent && set.contains(&parent) {
                set.insert(pid);
            }
        }
        if set.len() == before {
            return set;
        }
    }
}

/// CPU time so far (100 ns units) and working set (bytes) of `pid`.
fn process_usage(pid: u32) -> Option<(u64, u64)> {
    // SAFETY: handle opened for querying only and closed below.
    unsafe {
        let process = OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ,
            false,
            pid,
        )
        .or_else(|_| OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid))
        .ok()?;
        let (mut created, mut exited, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        let times = GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user);
        let mut memory = PROCESS_MEMORY_COUNTERS {
            cb: size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            ..Default::default()
        };
        let ram = if K32GetProcessMemoryInfo(process, &mut memory, memory.cb).as_bool() {
            memory.WorkingSetSize as u64
        } else {
            0
        };
        let _ = CloseHandle(process);
        times.ok()?;
        let ticks = |t: FILETIME| u64::from(t.dwHighDateTime) << 32 | u64::from(t.dwLowDateTime);
        Some((ticks(kernel) + ticks(user), ram))
    }
}

#[cfg(test)]
mod tests {
    use super::ProcessMonitor;

    #[test]
    fn measures_this_process() {
        let mut monitor = ProcessMonitor::new().unwrap();
        let me = std::process::id();
        monitor.sample(&[me], &[]);
        // Keep one core busy for a while.
        let start = std::time::Instant::now();
        let mut x = 0u64;
        while start.elapsed() < std::time::Duration::from_millis(300) {
            x = std::hint::black_box(x.wrapping_mul(31).wrapping_add(7));
        }
        let load = monitor.sample(&[me], &[]);
        assert_eq!(load.processes, 1);
        assert!(load.cpu > 0.0, "{load:?}");
        assert!(load.ram > 1 << 20, "{load:?}");
    }
}
