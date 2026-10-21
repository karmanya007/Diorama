//! Prints the live spectrum (manual check, play some audio first) and checks
//! the stream stays healthy for 6 s (xruns must not force a reopen):
//! `cargo test -p lumen-media --test capture_probe -- --ignored --nocapture`

#[test]
#[ignore = "depends on what is playing on this machine"]
fn print_spectrum() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter("debug")
        .try_init();
    let Ok(capture) = lumen_media::AudioCapture::start() else {
        println!("no capture");
        return;
    };
    for i in 0..30 {
        std::thread::sleep(std::time::Duration::from_millis(200));
        let s = capture.spectrum(0.2);
        let bars: String = s
            .bands(16, false)
            .iter()
            .map(|v| char::from(b" .:-=+*#%@"[(v * 9.0).round().clamp(0.0, 9.0) as usize]))
            .collect();
        if i % 5 == 4 {
            println!("level {:.2} |{bars}|", s.level());
        }
    }
    assert!(!capture.needs_reopen(), "capture asked to reopen");
}
