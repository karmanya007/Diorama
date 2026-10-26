//! `lumen-ui` — Lumen's desktop UI (ADR-006). The Svelte front end talks
//! to lumend through `ipc` (the same requests as the `lumen` CLI), reads
//! the library itself and starts the daemon when it is not running.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use lumen_ipc::{Request, Response};
use lumen_library::{Item, Roots};
use tauri::Manager;

/// One request to lumend, starting it first if needed.
#[tauri::command]
async fn ipc(request: Request) -> Result<Response, String> {
    blocking(move || match lumen_ipc::call(&request) {
        Err(lumen_ipc::Error::NotRunning) => {
            start_daemon()?;
            lumen_ipc::call(&request).map_err(|e| e.to_string())
        }
        other => other.map_err(|e| e.to_string()),
    })
    .await
}

/// The user's wallpapers: Steam's and those in `folders`. Their previews
/// become readable by the page.
#[tauri::command]
async fn library(app: tauri::AppHandle, folders: Vec<PathBuf>) -> Result<Vec<Item>, String> {
    blocking(move || {
        let mut roots = lumen_platform::steam_dir()
            .map(|d| Roots::from_steam(&d))
            .unwrap_or_default();
        roots.folders = folders;
        let scope = app.asset_protocol_scope();
        for dir in readable_dirs(&roots) {
            if dir.is_dir()
                && let Err(e) = scope.allow_directory(&dir, true)
            {
                eprintln!("lumen-ui: cannot show previews from {}: {e}", dir.display());
            }
        }
        Ok(lumen_library::scan(&roots))
    })
    .await
}

/// The policy presets, in the order to list them.
#[tauri::command]
fn presets() -> Vec<(String, lumen_ipc::Policy)> {
    lumen_ipc::Policy::PRESETS
        .iter()
        .map(|(name, policy)| ((*name).to_owned(), *policy))
        .collect()
}

/// Whether lumend starts at sign-in.
#[tauri::command]
fn autostart() -> Result<bool, String> {
    lumen_platform::autostart()
        .map(|c| c.is_some())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn set_autostart(on: bool) -> Result<(), String> {
    let command = if on {
        let daemon = daemon_exe();
        if !daemon.is_file() {
            return Err(format!("{} not found", daemon.display()));
        }
        Some(format!("\"{}\" --background", daemon.display()))
    } else {
        None
    };
    lumen_platform::set_autostart(command.as_deref()).map_err(|e| e.to_string())
}

/// Folders whose files the page may show (previews, images).
fn readable_dirs(roots: &Roots) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for lib in &roots.steam_libraries {
        let apps = lib.join("steamapps");
        dirs.push(
            apps.join("workshop")
                .join("content")
                .join(lumen_library::WE_APP_ID),
        );
        dirs.push(
            apps.join("common")
                .join("wallpaper_engine")
                .join("projects"),
        );
    }
    dirs.extend(roots.folders.iter().cloned());
    dirs
}

/// `lumend` next to this program.
fn daemon_exe() -> PathBuf {
    std::env::current_exe()
        .unwrap_or_default()
        .with_file_name(format!("lumend{}", std::env::consts::EXE_SUFFIX))
}

/// Starts lumend in the background and waits until it answers.
fn start_daemon() -> Result<(), String> {
    let exe = daemon_exe();
    let mut cmd = std::process::Command::new(&exe);
    lumen_platform::no_console(&mut cmd);
    cmd.arg("--background")
        .spawn()
        .map_err(|e| format!("starting {}: {e}", exe.display()))?;
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        if lumen_ipc::call(&Request::Status).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err("lumend did not start (see lumend.log in the Lumen config folder)".into())
}

/// Runs `f` off the UI thread.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
}

fn main() {
    let result = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            ipc,
            library,
            presets,
            autostart,
            set_autostart
        ])
        .run(tauri::generate_context!());
    if let Err(e) = result {
        eprintln!("lumen-ui: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn previews_are_readable_only_from_library_folders() {
        let roots = Roots {
            steam_libraries: vec![PathBuf::from("S")],
            folders: vec![PathBuf::from("F")],
        };
        let dirs = readable_dirs(&roots);
        assert_eq!(dirs.len(), 3);
        assert!(dirs[0].ends_with(Path::new("steamapps/workshop/content/431960")));
        assert!(dirs[1].ends_with(Path::new("wallpaper_engine/projects")));
        assert_eq!(dirs[2], PathBuf::from("F"));
    }
}
