//! Monitor enumeration with stable identities.
//!
//! Geometry comes from `EnumDisplayMonitors` (physical pixels, since the
//! process is per-monitor DPI aware). Identity comes from `QueryDisplayConfig`:
//! the target's monitor device path encodes EDID manufacturer/product and the
//! connector instance, so it survives Windows renumbering `\\.\DISPLAYn` (P8).

use std::collections::HashMap;
use std::mem::size_of;

use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
    DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME, DisplayConfigGetDeviceInfo,
    GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows::Win32::Foundation::{ERROR_SUCCESS, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;
use windows::core::BOOL;

use crate::{Error, OutputId, OutputInfo, Result};

struct Monitor {
    device: String,
    rect: RECT,
    primary: bool,
}

/// Identity details for one active display path, keyed by GDI device name.
#[derive(Debug, Default)]
struct TargetInfo {
    path: String,
    name: String,
    manufacturer: String,
    product_code: u16,
    refresh_mhz: u32,
}

pub(super) fn enumerate() -> Result<Vec<OutputInfo>> {
    let monitors = monitors()?;
    let targets = targets().unwrap_or_default();

    let mut outputs: Vec<OutputInfo> = monitors
        .into_iter()
        .map(|m| {
            let t = targets.get(&m.device);
            let id = t
                .map(|t| t.path.clone())
                .filter(|p| !p.is_empty())
                // No display-config data (e.g. remote session): fall back to the volatile name.
                .unwrap_or_else(|| format!("gdi:{}", m.device));
            OutputInfo {
                id: OutputId(id),
                name: t.map(|t| t.name.clone()).unwrap_or_default(),
                manufacturer: t.map(|t| t.manufacturer.clone()).unwrap_or_default(),
                product_code: t.map_or(0, |t| t.product_code),
                device: m.device,
                x: m.rect.left,
                y: m.rect.top,
                width: (m.rect.right - m.rect.left).max(0) as u32,
                height: (m.rect.bottom - m.rect.top).max(0) as u32,
                refresh_mhz: t.map_or(0, |t| t.refresh_mhz),
                primary: m.primary,
            }
        })
        .collect();
    outputs.sort_by_key(|o| (o.x, o.y));
    Ok(outputs)
}

fn monitors() -> Result<Vec<Monitor>> {
    unsafe extern "system" fn collect(hmon: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the `&mut Vec<Monitor>` passed to EnumDisplayMonitors below,
        // which outlives the synchronous enumeration.
        let list = unsafe { &mut *(data.0 as *mut Vec<Monitor>) };
        let mut info = MONITORINFOEXW {
            monitorInfo: MONITORINFO {
                cbSize: size_of::<MONITORINFOEXW>() as u32,
                ..Default::default()
            },
            ..Default::default()
        };
        // SAFETY: MONITORINFOEXW starts with MONITORINFO and cbSize declares the extended size.
        let ok = unsafe { GetMonitorInfoW(hmon, (&raw mut info).cast::<MONITORINFO>()) };
        if ok.as_bool() {
            list.push(Monitor {
                device: wide_to_string(&info.szDevice),
                rect: info.monitorInfo.rcMonitor,
                primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
            });
        }
        BOOL::from(true)
    }

    let mut list: Vec<Monitor> = Vec::new();
    // SAFETY: callback only touches `list` through the LPARAM during this call.
    let ok =
        unsafe { EnumDisplayMonitors(None, None, Some(collect), LPARAM((&raw mut list) as isize)) };
    if !ok.as_bool() {
        return Err(Error::Os {
            call: "EnumDisplayMonitors",
            message: windows::core::Error::from_thread().message(),
        });
    }
    Ok(list)
}

fn targets() -> Result<HashMap<String, TargetInfo>> {
    let mut path_count = 0u32;
    let mut mode_count = 0u32;
    // SAFETY: out-pointers are valid u32s.
    let err = unsafe {
        GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count)
    };
    if err != ERROR_SUCCESS {
        return Err(os_err("GetDisplayConfigBufferSizes", err.0));
    }
    let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
    let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
    // SAFETY: buffers sized from GetDisplayConfigBufferSizes; counts updated on return.
    let err = unsafe {
        QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut path_count,
            paths.as_mut_ptr(),
            &mut mode_count,
            modes.as_mut_ptr(),
            None,
        )
    };
    if err != ERROR_SUCCESS {
        return Err(os_err("QueryDisplayConfig", err.0));
    }
    paths.truncate(path_count as usize);

    let mut out = HashMap::new();
    for p in &paths {
        let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                size: size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                adapterId: p.sourceInfo.adapterId,
                id: p.sourceInfo.id,
            },
            ..Default::default()
        };
        let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                size: size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
                adapterId: p.targetInfo.adapterId,
                id: p.targetInfo.id,
            },
            ..Default::default()
        };
        // SAFETY: each header's size/type match the struct it heads.
        let (rs, rt) = unsafe {
            (
                DisplayConfigGetDeviceInfo(&mut source.header),
                DisplayConfigGetDeviceInfo(&mut target.header),
            )
        };
        if rs != 0 || rt != 0 {
            continue;
        }
        let rate = p.targetInfo.refreshRate;
        let refresh_mhz = if rate.Denominator == 0 {
            0
        } else {
            (u64::from(rate.Numerator) * 1000 / u64::from(rate.Denominator)) as u32
        };
        out.insert(
            wide_to_string(&source.viewGdiDeviceName),
            TargetInfo {
                path: wide_to_string(&target.monitorDevicePath),
                name: wide_to_string(&target.monitorFriendlyDeviceName),
                manufacturer: edid_manufacturer(target.edidManufactureId),
                product_code: target.edidProductCodeId,
                refresh_mhz,
            },
        );
    }
    Ok(out)
}

/// EDID packs the manufacturer as three 5-bit letters, big-endian;
/// DISPLAYCONFIG reports the raw (byte-swapped) value.
pub(super) fn edid_manufacturer(raw: u16) -> String {
    let v = raw.swap_bytes();
    [(v >> 10) & 0x1f, (v >> 5) & 0x1f, v & 0x1f]
        .iter()
        .filter(|&&c| (1..=26).contains(&c))
        .map(|&c| char::from(b'A' + c as u8 - 1))
        .collect()
}

pub(super) fn wide_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

fn os_err(call: &'static str, code: u32) -> Error {
    Error::Os {
        call,
        message: format!("win32 error {code}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_edid_manufacturer() {
        // "GSM" (LG): G=7, S=19, M=13 → 0b0_00111_10011_01101 = 0x1E6D, stored byte-swapped.
        assert_eq!(edid_manufacturer(0x6D1E), "GSM");
        assert_eq!(edid_manufacturer(0), "");
    }

    #[test]
    fn wide_strings_stop_at_nul() {
        let mut buf = [0u16; 8];
        for (i, c) in "ABC".encode_utf16().enumerate() {
            buf[i] = c;
        }
        assert_eq!(wide_to_string(&buf), "ABC");
    }

    #[test]
    fn enumerates_at_least_one_output() {
        // Runs on any Windows host with a display (CI runners have a virtual one).
        let outputs = enumerate().unwrap();
        assert!(!outputs.is_empty());
        assert!(outputs.iter().all(|o| o.width > 0 && o.height > 0));
    }
}
