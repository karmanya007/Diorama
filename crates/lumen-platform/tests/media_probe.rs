//! Prints what the OS media session reports (manual check, needs a player):
//! `cargo test -p lumen-platform --test media_probe -- --ignored --nocapture`

#[test]
#[ignore = "depends on what is playing on this machine"]
fn print_now_playing() {
    let watcher = lumen_platform::MediaWatcher::start();
    std::thread::sleep(std::time::Duration::from_secs(2));
    let mut info = watcher.latest();
    let thumbnail = info.thumbnail.take().map(|t| t.len());
    println!("{info:#?}\nthumbnail bytes: {thumbnail:?}");
}
