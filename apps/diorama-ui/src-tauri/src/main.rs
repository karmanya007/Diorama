//! `diorama-ui` — Diorama's desktop UI (ADR-006). The Svelte front end talks
//! to dioramad through `ipc` (the same requests as the `diorama` CLI), reads
//! the library itself and starts the daemon when it is not running.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use diorama_ipc::{Request, Response};
use diorama_library::{Item, Roots};
use tauri::Manager;

/// One request to dioramad, starting it first if needed.
#[tauri::command]
async fn ipc(request: Request) -> Result<Response, String> {
    blocking(move || match diorama_ipc::call(&request) {
        Err(diorama_ipc::Error::NotRunning) => {
            start_daemon()?;
            diorama_ipc::call(&request).map_err(|e| e.to_string())
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
        let mut roots = diorama_platform::steam_dir()
            .map(|d| Roots::from_steam(&d))
            .unwrap_or_default();
        roots.folders = folders;
        let scope = app.asset_protocol_scope();
        for dir in readable_dirs(&roots) {
            if dir.is_dir()
                && let Err(e) = scope.allow_directory(&dir, true)
            {
                eprintln!(
                    "diorama-ui: cannot show previews from {}: {e}",
                    dir.display()
                );
            }
        }
        Ok(diorama_library::scan(&roots))
    })
    .await
}

/// The policy presets, in the order to list them.
#[tauri::command]
fn presets() -> Vec<(String, diorama_ipc::Policy)> {
    diorama_ipc::Policy::PRESETS
        .iter()
        .map(|(name, policy)| ((*name).to_owned(), *policy))
        .collect()
}

/// Whether dioramad starts at sign-in.
#[tauri::command]
fn autostart() -> Result<bool, String> {
    diorama_platform::autostart()
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
    diorama_platform::set_autostart(command.as_deref()).map_err(|e| e.to_string())
}

/// Folders whose files the page may show (previews, images).
fn readable_dirs(roots: &Roots) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for lib in &roots.steam_libraries {
        let apps = lib.join("steamapps");
        dirs.push(
            apps.join("workshop")
                .join("content")
                .join(diorama_library::WE_APP_ID),
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

/// `dioramad` next to this program.
fn daemon_exe() -> PathBuf {
    std::env::current_exe()
        .unwrap_or_default()
        .with_file_name(format!("dioramad{}", std::env::consts::EXE_SUFFIX))
}

/// Starts dioramad in the background and waits until it answers.
fn start_daemon() -> Result<(), String> {
    let exe = daemon_exe();
    let mut cmd = std::process::Command::new(&exe);
    diorama_platform::no_console(&mut cmd);
    cmd.arg("--background")
        .spawn()
        .map_err(|e| format!("starting {}: {e}", exe.display()))?;
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        if diorama_ipc::call(&Request::Status).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err("dioramad did not start (see dioramad.log in the Diorama config folder)".into())
}

/// Runs `f` off the UI thread.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
}

fn quit_asked(args: &[String]) -> bool {
    args.iter().skip(1).any(|a| a == "--quit")
}

fn main() {
    let quit = quit_asked(&std::env::args().collect::<Vec<_>>());
    let mut context = tauri::generate_context!();
    if quit {
        // Only to hand `--quit` to a running UI: no window of its own.
        context.config_mut().app.windows.clear();
    }
    let result = tauri::Builder::default()
        // A second start (e.g. dioramad's tray "Open Diorama") shows this window;
        // with `--quit` (the tray's "Quit") it closes it.
        .plugin(tauri_plugin_single_instance::init(|app, args, _| {
            if quit_asked(&args) {
                return app.exit(0);
            }
            if let Some(window) = app.webview_windows().values().next() {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .setup(move |_| {
            // `--quit` with no UI running: nothing to close.
            if quit {
                std::process::exit(0);
            }
            Ok(())
        })
        .plugin(tauri_plugin_dialog::init())
        // Settings → "Check for updates": signed installers from GitHub Releases.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .invoke_handler(tauri::generate_handler![
            ipc,
            library,
            presets,
            autostart,
            set_autostart
        ])
        .run(context);
    if let Err(e) = result {
        eprintln!("diorama-ui: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn quit_is_a_flag_not_the_program_name() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(quit_asked(&args(&["diorama-ui", "--quit"])));
        assert!(!quit_asked(&args(&["diorama-ui"])));
        assert!(!quit_asked(&args(&["--quit"])));
    }

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
