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

/// Prints the loudest and mean web level (what web pages receive) every
/// 500 ms for `LUMEN_PROBE_SECS` (default 5) seconds:
/// `cargo test -p lumen-media --test capture_probe log_web_levels -- --ignored --nocapture`
#[test]
#[ignore = "depends on what is playing on this machine"]
fn log_web_levels() {
    let secs: u64 = std::env::var("LUMEN_PROBE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);
    let Ok(capture) = lumen_media::AudioCapture::start() else {
        println!("no capture");
        return;
    };
    let start = std::time::Instant::now();
    while start.elapsed().as_secs() < secs {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let s = capture.web_spectrum();
        let all = s.left.iter().chain(&s.right);
        let max = all.clone().copied().fold(0.0, f32::max);
        let mean = all.sum::<f32>() / 128.0;
        println!(
            "{:.1}s max {max:.4} mean {mean:.4}",
            start.elapsed().as_secs_f32()
        );
    }
}

/// Logs 16-band levels (left, right; `t,l0..l15,r0..r15`) every 100 ms for
/// `LUMEN_PROBE_SECS` (default 10) seconds, to compare with WE's
/// `g_AudioSpectrum16*` in a capture taken over the same music:
/// `LUMEN_PROBE_SECS=90 cargo test -p lumen-media --test capture_probe log_bands -- --ignored --nocapture`
#[test]
#[ignore = "depends on what is playing on this machine"]
fn log_bands() {
    let secs: u64 = std::env::var("LUMEN_PROBE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let Ok(capture) = lumen_media::AudioCapture::start() else {
        println!("no capture");
        return;
    };
    let start = std::time::Instant::now();
    while start.elapsed().as_secs() < secs {
        std::thread::sleep(std::time::Duration::from_millis(100));
        let s = capture.spectrum(0.1);
        let fmt = |v: Vec<f32>| {
            v.iter()
                .map(|x| format!("{x:.3}"))
                .collect::<Vec<_>>()
                .join(",")
        };
        println!(
            "{:.1},{},{}",
            start.elapsed().as_secs_f32(),
            fmt(s.bands(16, false)),
            fmt(s.bands(16, true))
        );
    }
}

/// Records the default output's loopback audio for `LUMEN_PROBE_SECS`
/// (default 10) seconds to `LUMEN_PCM_OUT` (interleaved f32 LE) plus
/// `LUMEN_PCM_OUT.idx` (`rate,channels` header, then `unix_ms,frames_so_far`
/// per callback), to replay against captured WE frames offline. Stays on
/// this machine (`scratch/`); delete it after use.
#[test]
#[ignore = "records what is playing on this machine"]
fn record_pcm() {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use std::io::Write;
    let Ok(out) = std::env::var("LUMEN_PCM_OUT") else {
        println!("set LUMEN_PCM_OUT");
        return;
    };
    let secs: u64 = std::env::var("LUMEN_PROBE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let device = cpal::default_host()
        .default_output_device()
        .expect("output device");
    let config = device.default_output_config().expect("config").config();
    let channels = usize::from(config.channels);
    let mut pcm = std::io::BufWriter::new(std::fs::File::create(&out).expect("pcm file"));
    let mut idx =
        std::io::BufWriter::new(std::fs::File::create(format!("{out}.idx")).expect("idx file"));
    writeln!(idx, "{},{}", config.sample_rate, channels).expect("idx");
    let (tx, rx) = std::sync::mpsc::channel::<(u128, Vec<f32>)>();
    let stream = device
        .build_input_stream::<f32, _, _>(
            config,
            move |data: &[f32], _| {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis());
                let _ = tx.send((now, data.to_vec()));
            },
            |e| println!("stream: {e}"),
            None,
        )
        .expect("stream");
    stream.play().expect("play");
    let start = std::time::Instant::now();
    let mut frames = 0usize;
    while start.elapsed().as_secs() < secs {
        if let Ok((ms, data)) = rx.recv_timeout(std::time::Duration::from_millis(200)) {
            for v in &data {
                pcm.write_all(&v.to_le_bytes()).expect("write");
            }
            frames += data.len() / channels.max(1);
            writeln!(idx, "{ms},{frames}").expect("idx");
        }
    }
    println!("{frames} frames → {out}");
}

/// Replays a `record_pcm` recording through [`SpectrumAnalyzer`] and scores
/// its 16 bands against WE's captured `g_AudioSpectrum16*` at the same
/// moments (`LUMEN_PCM_OUT` = the recording, `LUMEN_WE_SPECTRA` = a JSON
/// list of `{trigger_ms, L: [16], R: [16]}`): prints mean absolute error and
/// mean per-band correlation.
#[test]
#[ignore = "needs a local recording and WE captures"]
fn replay_against_we() {
    let (Ok(pcm_path), Ok(we_path)) = (
        std::env::var("LUMEN_PCM_OUT"),
        std::env::var("LUMEN_WE_SPECTRA"),
    ) else {
        println!("set LUMEN_PCM_OUT and LUMEN_WE_SPECTRA");
        return;
    };
    let idx = std::fs::read_to_string(format!("{pcm_path}.idx")).expect("idx");
    let mut lines = idx.lines();
    let header: Vec<u32> = lines
        .next()
        .expect("header")
        .split(',')
        .map(|v| v.parse().expect("number"))
        .collect();
    let (rate, channels) = (header[0], header[1] as usize);
    let chunks: Vec<(u64, usize)> = lines
        .filter_map(|l| {
            let (ms, frames) = l.split_once(',')?;
            Some((ms.parse().ok()?, frames.parse().ok()?))
        })
        .collect();
    let bytes = std::fs::read(&pcm_path).expect("pcm");
    let pcm: Vec<f32> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect();
    let we: Vec<serde_json::Value> =
        serde_json::from_str(&std::fs::read_to_string(&we_path).expect("we")).expect("json");
    // WE's frame follows its capture trigger by ~50 ms (best alignment).
    let mut targets: Vec<(u64, usize)> = we
        .iter()
        .enumerate()
        .filter_map(|(i, f)| Some((f["trigger_ms"].as_u64()? + 50, i)))
        .collect();
    targets.sort_unstable();
    let mut analyzer = lumen_media::spectrum::SpectrumAnalyzer::new(rate);
    let (mut ours, mut theirs) = (vec![Vec::new(); 16], vec![Vec::new(); 16]);
    let (mut prev_frames, mut prev_ms, mut t) = (0usize, None::<u64>, 0usize);
    for &(ms, frames) in &chunks {
        analyzer.push(&pcm[prev_frames * channels..frames * channels], channels);
        let dt = prev_ms.map_or(0.0, |p| (ms - p) as f32 / 1000.0);
        let s = analyzer.analyze(dt);
        (prev_frames, prev_ms) = (frames, Some(ms));
        while t < targets.len() && targets[t].0 <= ms {
            let f = &we[targets[t].1];
            for (right, side) in [(false, "L"), (true, "R")] {
                for (b, v) in s.bands(16, right).into_iter().enumerate() {
                    ours[b].push(v);
                    theirs[b].push(f[side][b].as_f64().unwrap_or(0.0) as f32);
                }
            }
            t += 1;
        }
    }
    let corr = |x: &[f32], y: &[f32]| {
        let n = x.len() as f32;
        let (mx, my) = (x.iter().sum::<f32>() / n, y.iter().sum::<f32>() / n);
        let sxy: f32 = x.iter().zip(y).map(|(a, b)| (a - mx) * (b - my)).sum();
        let sxx: f32 = x.iter().map(|a| (a - mx).powi(2)).sum();
        let syy: f32 = y.iter().map(|b| (b - my).powi(2)).sum();
        sxy / (sxx * syy).sqrt().max(1e-9)
    };
    let n: usize = ours.iter().map(Vec::len).sum();
    let mae: f32 = ours
        .iter()
        .zip(&theirs)
        .flat_map(|(a, b)| a.iter().zip(b).map(|(x, y)| (x - y).abs()))
        .sum::<f32>()
        / n as f32;
    let c: f32 = (0..16).map(|b| corr(&ours[b], &theirs[b])).sum::<f32>() / 16.0;
    let mean = |v: &[Vec<f32>]| v.iter().flatten().sum::<f32>() / n as f32;
    println!(
        "{} samples: MAE {mae:.3}, corr {c:.3}, our mean {:.2}, WE mean {:.2}",
        n,
        mean(&ours),
        mean(&theirs)
    );
}
