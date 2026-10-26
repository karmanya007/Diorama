//! Machine load from Windows' own counters: PDH for CPU and GPU, the memory
//! status, free space per fixed drive, DXGI for video memory sizes and the IP
//! helper's interface counters for network speed.

use std::collections::HashMap;
use std::mem::size_of;
use std::time::{Duration, Instant};

use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1,
};
use windows::Win32::NetworkManagement::IpHelper::{
    FreeMibTable, GetIfTable2, IF_TYPE_SOFTWARE_LOOPBACK, MIB_IF_TABLE2,
};
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives};
use windows::Win32::System::Performance::{
    PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA,
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhOpenQueryW,
};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::WindowsProgramming::DRIVE_FIXED;
use windows::core::{HSTRING, PCWSTR};

use crate::{DiskUsage, Error, Result, SystemMetrics};

/// Free space changes slowly and asking may wake a sleeping disk.
const DISK_INTERVAL: Duration = Duration::from_secs(10);

/// Reads the machine's load; rates (CPU, GPU, network) cover the time since
/// the previous [`SystemMonitor::sample`], so the first sample reads 0 there.
#[derive(Debug)]
pub struct SystemMonitor {
    query: PDH_HQUERY,
    cpu: PDH_HCOUNTER,
    gpu_engines: Option<PDH_HCOUNTER>,
    gpu_memory: Option<PDH_HCOUNTER>,
    /// The adapter shown as "the GPU": the one with the most dedicated
    /// memory, as PDH names it (`luid_0x…_0x…`), and that memory in bytes.
    adapter: Option<(String, u64)>,
    network: Option<(Instant, u64, u64)>,
    disks: Vec<DiskUsage>,
    disks_read: Option<Instant>,
}

impl SystemMonitor {
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
            let Some(cpu) = add(r"\Processor(*)\% Processor Time") else {
                PdhCloseQuery(query);
                return Err(Error::Os {
                    call: "PdhAddEnglishCounterW",
                    message: "no processor counters".into(),
                });
            };
            let gpu_engines = add(r"\GPU Engine(*)\Utilization Percentage");
            let gpu_memory = add(r"\GPU Adapter Memory(*)\Dedicated Usage");
            // Rates need a first collection to compare against.
            PdhCollectQueryData(query);
            Ok(Self {
                query,
                cpu,
                gpu_engines,
                gpu_memory,
                adapter: main_adapter(),
                network: None,
                disks: Vec::new(),
                disks_read: None,
            })
        }
    }

    pub fn sample(&mut self) -> SystemMetrics {
        // SAFETY: the query and counters belong to this monitor.
        unsafe { PdhCollectQueryData(self.query) };
        let mut m = SystemMetrics::default();

        let mut cores: Vec<(u32, f32)> = Vec::new();
        for (name, value) in counter_array(self.cpu) {
            let value = value.clamp(0.0, 100.0) as f32;
            match name.parse() {
                Ok(i) => cores.push((i, value)),
                Err(_) if name == "_Total" => m.cpu = value,
                Err(_) => {}
            }
        }
        cores.sort_by_key(|c| c.0);
        m.cpu_per_core = cores.into_iter().map(|c| c.1).collect();

        let mut status = MEMORYSTATUSEX {
            dwLength: size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        // SAFETY: `status` is sized as the call requires.
        if unsafe { GlobalMemoryStatusEx(&mut status) }.is_ok() {
            m.memory_total = status.ullTotalPhys;
            m.memory_used = status.ullTotalPhys.saturating_sub(status.ullAvailPhys);
        }

        if let Some((luid, total)) = &self.adapter {
            // Task Manager's GPU figure: the busiest engine, summed over processes.
            if let Some(counter) = self.gpu_engines {
                m.gpu =
                    Some(busiest_engine(&counter_array(counter), luid).clamp(0.0, 100.0) as f32);
            }
            if let Some(counter) = self.gpu_memory {
                let used: f64 = counter_array(counter)
                    .iter()
                    .filter(|(name, _)| name.starts_with(luid.as_str()))
                    .map(|(_, v)| v)
                    .sum();
                m.vram_used = Some(used.max(0.0) as u64);
                m.vram_total = Some(*total);
            }
        }

        if self.disks_read.is_none_or(|t| t.elapsed() >= DISK_INTERVAL) {
            self.disks = fixed_disks();
            self.disks_read = Some(Instant::now());
        }
        m.disks = self.disks.clone();

        if let Some((received, sent)) = network_octets() {
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

impl Drop for SystemMonitor {
    fn drop(&mut self) {
        // SAFETY: closes the query this monitor opened (and its counters).
        unsafe { PdhCloseQuery(self.query) };
    }
}

pub(super) fn pdh(call: &'static str, status: u32) -> Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(Error::Os {
            call,
            message: format!("PDH status {status:#x}"),
        })
    }
}

/// (instance name, value) for every instance of a wildcard counter; empty
/// while the counter has no data yet.
pub(super) fn counter_array(counter: PDH_HCOUNTER) -> Vec<(String, f64)> {
    let (mut bytes, mut count) = (0u32, 0u32);
    // SAFETY: the first call sizes the buffer, the second fills it; item
    // names point into the same buffer, read before it is dropped.
    unsafe {
        if PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut bytes, &mut count, None)
            != PDH_MORE_DATA
        {
            return Vec::new();
        }
        let item = size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>();
        let mut buf: Vec<PDH_FMT_COUNTERVALUE_ITEM_W> =
            vec![Default::default(); (bytes as usize).div_ceil(item)];
        if PdhGetFormattedCounterArrayW(
            counter,
            PDH_FMT_DOUBLE,
            &mut bytes,
            &mut count,
            Some(buf.as_mut_ptr()),
        ) != 0
        {
            return Vec::new();
        }
        buf.iter()
            .take(count as usize)
            .filter(|i| i.FmtValue.CStatus == 0)
            .map(|i| {
                (
                    i.szName.to_string().unwrap_or_default(),
                    i.FmtValue.Anonymous.doubleValue,
                )
            })
            .collect()
    }
}

/// The busiest engine of adapter `luid`: engine instances are named
/// `pid_<pid>_luid_<hi>_<lo>_phys_<n>_eng_<n>_engtype_<type>`, one per
/// process, so usage is summed per engine first.
fn busiest_engine(instances: &[(String, f64)], luid: &str) -> f64 {
    let mut engines: HashMap<&str, f64> = HashMap::new();
    for (name, value) in instances {
        if let Some(at) = name.find(luid) {
            let engine = name[at..].split("_engtype_").next().unwrap_or_default();
            *engines.entry(engine).or_default() += value;
        }
    }
    engines.values().copied().fold(0.0, f64::max)
}

/// The hardware adapter with the most dedicated memory, as PDH's
/// `luid_0x<high>_0x<low>` prefix, and that memory.
fn main_adapter() -> Option<(String, u64)> {
    // SAFETY: DXGI enumeration; descriptors are plain data.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        let mut best: Option<(String, u64)> = None;
        let mut i = 0;
        while let Ok(adapter) = factory.EnumAdapters1(i) {
            i += 1;
            let Ok(desc) = adapter.GetDesc1() else {
                continue;
            };
            if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                continue;
            }
            let memory = desc.DedicatedVideoMemory as u64;
            if best.as_ref().is_none_or(|b| memory > b.1) {
                let luid = format!(
                    "luid_0x{:08X}_0x{:08X}",
                    desc.AdapterLuid.HighPart as u32, desc.AdapterLuid.LowPart
                );
                best = Some((luid, memory));
            }
        }
        best
    }
}

fn fixed_disks() -> Vec<DiskUsage> {
    // SAFETY: drive queries on root paths built here.
    let mask = unsafe { GetLogicalDrives() };
    (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .filter_map(|i| {
            let letter = char::from(b'A' + i);
            let root = HSTRING::from(format!("{letter}:\\"));
            // SAFETY: as above.
            unsafe {
                if GetDriveTypeW(&root) != DRIVE_FIXED {
                    return None;
                }
                let (mut total, mut free) = (0u64, 0u64);
                GetDiskFreeSpaceExW(&root, None, Some(&mut total), Some(&mut free)).ok()?;
                Some(DiskUsage {
                    letter,
                    used: total.saturating_sub(free),
                    total,
                })
            }
        })
        .collect()
}

/// Bytes received and sent so far by the physical network interfaces that are up.
fn network_octets() -> Option<(u64, u64)> {
    let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    // SAFETY: the table is allocated by GetIfTable2, read within its
    // `NumEntries`, then freed.
    unsafe {
        if GetIfTable2(&mut table).is_err() || table.is_null() {
            return None;
        }
        let rows =
            std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize);
        let (mut received, mut sent) = (0u64, 0u64);
        for row in rows {
            // Bit 0: a hardware interface (not a filter or virtual layer
            // stacked on one, which would count the same bytes again).
            let hardware = row.InterfaceAndOperStatusFlags._bitfield & 1 != 0;
            if hardware && row.Type != IF_TYPE_SOFTWARE_LOOPBACK && row.OperStatus == IfOperStatusUp
            {
                received += row.InOctets;
                sent += row.OutOctets;
            }
        }
        FreeMibTable(table.cast());
        Some((received, sent))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busiest_engine_sums_processes_per_engine() {
        let luid = "luid_0x00000000_0x0000D2C5";
        let i = |s: &str, v| (s.to_owned(), v);
        let instances = [
            i(
                "pid_1_luid_0x00000000_0x0000D2C5_phys_0_eng_0_engtype_3D",
                20.0,
            ),
            i(
                "pid_2_luid_0x00000000_0x0000D2C5_phys_0_eng_0_engtype_3D",
                15.0,
            ),
            i(
                "pid_2_luid_0x00000000_0x0000D2C5_phys_0_eng_3_engtype_VideoDecode",
                30.0,
            ),
            i(
                "pid_3_luid_0x00000000_0x0000AAAA_phys_0_eng_0_engtype_3D",
                90.0,
            ),
        ];
        assert_eq!(busiest_engine(&instances, luid), 35.0);
    }

    #[test]
    fn samples_this_machine() {
        let mut m = SystemMonitor::new().expect("PDH");
        std::thread::sleep(Duration::from_millis(300));
        let s = m.sample();
        assert!((0.0..=100.0).contains(&s.cpu));
        assert!(!s.cpu_per_core.is_empty());
        assert!(s.memory_total > 0 && s.memory_used <= s.memory_total);
        assert!(s.disks.iter().any(|d| d.total > 0 && d.used <= d.total));
    }

    /// Prints a few samples to compare with Task Manager:
    /// `cargo test -p lumen-platform print_samples -- --ignored --nocapture`
    #[test]
    #[ignore = "manual comparison with Task Manager"]
    fn print_samples() {
        let mut m = SystemMonitor::new().expect("PDH");
        println!("adapter {:?}", m.adapter);
        for _ in 0..3 {
            std::thread::sleep(Duration::from_secs(1));
            println!("{:#?}", m.sample());
        }
    }
}
