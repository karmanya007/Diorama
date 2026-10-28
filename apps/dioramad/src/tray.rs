//! The notification-area icon's picture and its "Open Diorama" and "Quit"
//! choices for the UI.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

/// Diorama's icon (the UI's) as `size`×`size` RGBA.
pub fn icon(size: u32) -> Result<Vec<u8>> {
    let png = include_bytes!("../../diorama-ui/src-tauri/icons/icon.png");
    let image = image::load_from_memory(png).context("decoding the tray icon")?;
    Ok(image
        .resize_exact(size, size, image::imageops::FilterType::Lanczos3)
        .to_rgba8()
        .into_raw())
}

/// How long closing the UI may hold up quitting.
const CLOSE_UI_WAIT: Duration = Duration::from_secs(3);

/// `diorama-ui`, which sits next to dioramad.
fn ui_exe() -> PathBuf {
    std::env::current_exe()
        .unwrap_or_default()
        .with_file_name(format!("diorama-ui{}", std::env::consts::EXE_SUFFIX))
}

fn ui_command(exe: &PathBuf) -> Command {
    let mut cmd = Command::new(exe);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd
}

/// Starts the UI (a running one shows its window instead).
pub fn open_ui() {
    let exe = ui_exe();
    if let Err(e) = ui_command(&exe).spawn() {
        tracing::warn!("cannot start {}: {e}", exe.display());
    }
}

/// Closes a running UI: `diorama-ui --quit` passes that on to it and ends.
/// Waits for that, so the UI is told before dioramad goes (it would otherwise
/// start dioramad again).
pub fn close_ui() {
    let exe = ui_exe();
    if !exe.exists() {
        return;
    }
    let mut child = match ui_command(&exe).arg("--quit").spawn() {
        Ok(child) => child,
        Err(e) => return tracing::warn!("cannot start {}: {e}", exe.display()),
    };
    let until = Instant::now() + CLOSE_UI_WAIT;
    while Instant::now() < until {
        if !matches!(child.try_wait(), Ok(None)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    tracing::warn!("closing the UI took too long");
    let _ = child.kill();
}

#[cfg(test)]
mod tests {
    #[test]
    fn icon_is_square_rgba_and_not_blank() {
        let rgba = super::icon(20).unwrap();
        assert_eq!(rgba.len(), 20 * 20 * 4);
        assert!(rgba.chunks(4).any(|p| p[3] > 0));
    }
}
