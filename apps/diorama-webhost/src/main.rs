//! Hosts one web wallpaper: a WebView per output, each in a window behind
//! the desktop icons (the same windows dioramad draws into), with WE's web
//! wallpaper API injected.
//!
//! dioramad starts it and drives it with JSON lines on stdin; it exits when
//! stdin closes, so it never outlives dioramad. Run alone (stdin a terminal)
//! it shows the wallpaper with its default properties until killed.
//!
//! The WebView backends: WebView2 on Windows, WebKitGTK on Linux (in GTK
//! layer-shell windows).

#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
mod desktop;
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
mod host;
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
mod local;
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
mod performance;
mod properties;
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
mod protocol;
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
mod watch;

#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
fn main() {
    host::main();
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn main() {
    // Keeps the shared module compiled (and tested) on every platform.
    let _ = properties::user_properties;
    eprintln!("diorama-webhost: web wallpapers are not supported on this platform yet");
    std::process::exit(1);
}
