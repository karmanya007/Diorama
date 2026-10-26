//! Per-user system integration: config location, autostart, console, OS wallpaper.

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::System::Console::FreeConsole;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RRF_RT_REG_SZ, RegCloseKey, RegDeleteValueW,
    RegGetValueW, RegOpenKeyExW, RegSetValueExW,
};
use windows::Win32::UI::Shell::{DWPOS_FILL, DWPOS_SPAN, DesktopWallpaper, IDesktopWallpaper};
use windows::core::{HSTRING, PCWSTR, w};

use crate::{Error, Result};

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("Lumen");

fn win32(call: &'static str, code: u32) -> Error {
    Error::Os {
        call,
        message: windows::core::Error::from_hresult(windows::core::HRESULT::from_win32(code))
            .message(),
    }
}

/// `%APPDATA%\Lumen`.
pub fn config_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("Lumen"))
}

/// The user's display language as a tag such as `en-US`.
pub fn ui_language() -> Option<String> {
    use windows::Win32::Globalization::{GetUserDefaultUILanguage, LCIDToLocaleName};
    let mut name = [0u16; 85];
    // SAFETY: plain queries; the buffer is LOCALE_NAME_MAX_LENGTH long.
    let len =
        unsafe { LCIDToLocaleName(u32::from(GetUserDefaultUILanguage()), Some(&mut name), 0) };
    let len = usize::try_from(len).ok().filter(|&n| n > 1)?;
    Some(String::from_utf16_lossy(&name[..len - 1]))
}

/// Fraction of the local day elapsed, 0..1 (`g_Daytime`).
pub fn day_fraction() -> f32 {
    // SAFETY: plain query of the local wall-clock time.
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    let seconds = f32::from(t.wHour) * 3600.0
        + f32::from(t.wMinute) * 60.0
        + f32::from(t.wSecond)
        + f32::from(t.wMilliseconds) / 1000.0;
    seconds / 86_400.0
}

/// `%LOCALAPPDATA%\Lumen\cache`: machine-local, safe to delete (shader cache, thumbnails).
pub fn cache_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Lumen").join("cache"))
}

/// Per-user `HKCU\…\Run` entry: no admin rights, removable by the user in Task Manager.
pub fn set_autostart(command: Option<&str>) -> Result<()> {
    // SAFETY: registry calls with valid key handles and buffers; key closed below.
    unsafe {
        let mut key = HKEY::default();
        let err = RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, KEY_SET_VALUE, &mut key);
        if err != ERROR_SUCCESS {
            return Err(win32("RegOpenKeyExW(Run)", err.0));
        }
        let err = match command {
            Some(cmd) => {
                let wide: Vec<u16> = cmd.encode_utf16().chain(std::iter::once(0)).collect();
                let bytes = std::slice::from_raw_parts(wide.as_ptr().cast::<u8>(), wide.len() * 2);
                RegSetValueExW(key, RUN_VALUE, None, REG_SZ, Some(bytes))
            }
            None => match RegDeleteValueW(key, RUN_VALUE) {
                // Already absent is fine.
                e if e.0 == 2 => ERROR_SUCCESS,
                e => e,
            },
        };
        let _ = RegCloseKey(key);
        if err != ERROR_SUCCESS {
            return Err(win32("RegSetValueExW/RegDeleteValueW(Lumen)", err.0));
        }
        Ok(())
    }
}

/// The registered autostart command, if any.
pub fn autostart() -> Result<Option<String>> {
    // SAFETY: two-call RegGetValueW pattern with correctly sized buffers.
    unsafe {
        let mut len = 0u32;
        let err = RegGetValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            RUN_VALUE,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut len),
        );
        if err.0 == 2 {
            return Ok(None); // ERROR_FILE_NOT_FOUND
        }
        if err != ERROR_SUCCESS {
            return Err(win32("RegGetValueW(Lumen)", err.0));
        }
        let mut buf = vec![0u16; (len as usize).div_ceil(2)];
        let err = RegGetValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            RUN_VALUE,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut len),
        );
        if err != ERROR_SUCCESS {
            return Err(win32("RegGetValueW(Lumen)", err.0));
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Ok(Some(String::from_utf16_lossy(&buf[..end])))
    }
}

/// Detaches from the console so a background daemon leaves no window behind.
pub fn detach_console() {
    // SAFETY: no arguments; harmless if there is no console.
    unsafe {
        let _ = FreeConsole();
    }
}

/// Sets the Windows desktop background (all monitors) to `image`: shown when
/// Lumen is not running, on the lock screen transition and at sign-in (P14).
pub fn set_os_wallpaper(image: &Path, span: bool) -> Result<()> {
    // SAFETY: COM on the calling thread; RPC_E_CHANGED_MODE (already initialised
    // differently) is fine because the call below works in either apartment.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dw: IDesktopWallpaper =
            CoCreateInstance(&DesktopWallpaper, None, CLSCTX_ALL).map_err(|e| Error::Os {
                call: "CoCreateInstance(DesktopWallpaper)",
                message: e.message(),
            })?;
        dw.SetPosition(if span { DWPOS_SPAN } else { DWPOS_FILL })
            .map_err(|e| Error::Os {
                call: "IDesktopWallpaper::SetPosition",
                message: e.message(),
            })?;
        dw.SetWallpaper(PCWSTR::null(), &HSTRING::from(image.as_os_str()))
            .map_err(|e| Error::Os {
                call: "IDesktopWallpaper::SetWallpaper",
                message: e.message(),
            })?;
    }
    Ok(())
}
