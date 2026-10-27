//! The notification-area icon's picture and its "Open Lumen" choice.

use std::process::Stdio;

use anyhow::{Context, Result};

/// Lumen's icon (the UI's) as `size`×`size` RGBA.
pub fn icon(size: u32) -> Result<Vec<u8>> {
    let png = include_bytes!("../../lumen-ui/src-tauri/icons/icon.png");
    let image = image::load_from_memory(png).context("decoding the tray icon")?;
    Ok(image
        .resize_exact(size, size, image::imageops::FilterType::Lanczos3)
        .to_rgba8()
        .into_raw())
}

/// Starts `lumen-ui`, which sits next to lumend.
pub fn open_ui() {
    let exe = std::env::current_exe()
        .unwrap_or_default()
        .with_file_name(format!("lumen-ui{}", std::env::consts::EXE_SUFFIX));
    let started = std::process::Command::new(&exe)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Err(e) = started {
        tracing::warn!("cannot start {}: {e}", exe.display());
    }
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
