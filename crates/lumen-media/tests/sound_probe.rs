//! Decodes a sound file from memory at volume 0 (silent) and checks it keeps
//! playing (decode errors end playback):
//! `LUMEN_SOUND_PROBE=<absolute path to .mp3|.wav> [LUMEN_SOUND_FILE=1] cargo test -p lumen-media --test sound_probe -- --ignored --nocapture`

#[test]
#[ignore = "needs a local sound file and an audio device"]
fn memory_sound_decodes() {
    tracing_subscriber::fmt().with_test_writer().init();
    let Ok(path) = std::env::var("LUMEN_SOUND_PROBE") else {
        println!("set LUMEN_SOUND_PROBE");
        return;
    };
    // Tests run in the crate directory: pass an absolute path.
    let data: std::sync::Arc<[u8]> = match std::fs::read(&path) {
        Ok(d) => d.into(),
        Err(e) => panic!("{path}: {e}"),
    };
    let name = path
        .rsplit(['/', std::path::MAIN_SEPARATOR])
        .next()
        .unwrap_or(&path)
        .to_owned();
    let source = if std::env::var("LUMEN_SOUND_FILE").is_ok() {
        lumen_media::MediaSource::File(path.clone().into())
    } else {
        lumen_media::MediaSource::Memory { data, name }
    };
    let player = match lumen_media::AudioPlayer::spawn_playlist(
        vec![source],
        lumen_media::PlaybackMode::Once,
        0.0,
    ) {
        Ok(p) => p,
        Err(e) => {
            println!("no player: {e}");
            return;
        }
    };
    player.set_paused(false);
    std::thread::sleep(std::time::Duration::from_secs(1));
    assert!(player.is_playing(), "decoding failed (see log)");
    println!("playing");
}
