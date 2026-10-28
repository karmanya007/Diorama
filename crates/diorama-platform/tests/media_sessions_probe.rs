//! Lists every media session Windows reports (manual check):
//! `cargo test -p diorama-platform --test media_sessions_probe -- --ignored --nocapture`
#![cfg(windows)]
#![allow(clippy::unwrap_used, reason = "probe")]

use windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager as Manager;

#[test]
#[ignore = "depends on what is playing on this machine"]
fn list_sessions() {
    let m = Manager::RequestAsync().unwrap().join().unwrap();
    let current = m
        .GetCurrentSession()
        .ok()
        .map(|s| s.SourceAppUserModelId().unwrap().to_string_lossy());
    println!("current: {current:?}");
    for s in m.GetSessions().unwrap() {
        let app = s.SourceAppUserModelId().unwrap().to_string_lossy();
        let p = s.TryGetMediaPropertiesAsync().unwrap().join().unwrap();
        let status = s.GetPlaybackInfo().unwrap().PlaybackStatus().unwrap();
        println!(
            "{app}: {:?} / {:?} — {status:?}",
            p.Title().unwrap().to_string_lossy(),
            p.Artist().unwrap().to_string_lossy()
        );
    }
}
