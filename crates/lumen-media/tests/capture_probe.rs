//! Prints the live spectrum (manual check, play some audio first):
//! `cargo test -p lumen-media --test capture_probe -- --ignored --nocapture`

#[test]
#[ignore = "depends on what is playing on this machine"]
fn print_spectrum() {
    let Ok(capture) = lumen_media::AudioCapture::start() else {
        println!("no capture");
        return;
    };
    for _ in 0..5 {
        std::thread::sleep(std::time::Duration::from_millis(200));
        let s = capture.spectrum(0.2);
        let bars: String = s
            .bands(16, false)
            .iter()
            .map(|v| char::from(b" .:-=+*#%@"[(v * 9.0).round().clamp(0.0, 9.0) as usize]))
            .collect();
        println!("level {:.2} |{bars}|", s.level());
    }
}
