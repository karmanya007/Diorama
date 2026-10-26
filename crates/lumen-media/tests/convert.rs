//! Converts a real video (`LUMEN_TEST_VIDEO`, never committed) and checks
//! the copy decodes. `cargo test -p lumen-media --test convert -- --ignored`.

use std::path::PathBuf;

#[test]
#[ignore = "needs LUMEN_TEST_VIDEO"]
fn converts_to_h264_that_decodes_on_the_gpu() {
    let src = PathBuf::from(std::env::var("LUMEN_TEST_VIDEO").expect("LUMEN_TEST_VIDEO"));
    let before = lumen_media::probe(&src).unwrap();
    println!("source: {before:?}");
    // `LUMEN_TEST_OUT` keeps the copy there for a look.
    let keep = std::env::var("LUMEN_TEST_OUT").ok().map(PathBuf::from);
    let dst = keep.clone().unwrap_or_else(|| {
        std::env::temp_dir().join(format!("lumen-convert-{}.mp4", std::process::id()))
    });
    let size = lumen_media::converted_size((before.width, before.height), &[(640, 360)]);
    let mut last = 0.0;
    let start = std::time::Instant::now();
    lumen_media::convert(&src, &dst, size, &mut |p| {
        assert!(p >= last, "progress went back: {p} < {last}");
        last = p;
        true
    })
    .unwrap();
    println!("converted in {:?}", start.elapsed());
    assert!((last - 1.0).abs() < 1e-6);
    let after = lumen_media::probe(&dst).unwrap();
    println!("copy: {after:?}");
    assert_eq!(
        (after.codec.as_str(), after.width, after.height),
        ("H.264", size.0, size.1)
    );
    let mut decoder = lumen_media::open(&dst).unwrap();
    let mut frame = decoder.next_frame().unwrap().expect("a frame");
    for _ in 0..90 {
        frame = decoder.next_frame().unwrap().expect("a frame");
    }
    if let Some(keep) = &keep {
        std::fs::write(keep.with_extension("rgba"), frame.to_rgba()).unwrap();
    }
    drop(decoder);
    if keep.is_none() {
        std::fs::remove_file(&dst).unwrap();
    }
}

#[test]
fn stopping_early_is_cancelled() {
    let Ok(src) = std::env::var("LUMEN_TEST_VIDEO") else {
        return;
    };
    let dst = std::env::temp_dir().join(format!("lumen-cancel-{}.mp4", std::process::id()));
    let result = lumen_media::convert(src.as_ref(), &dst, (320, 180), &mut |_| false);
    assert!(
        matches!(result, Err(lumen_media::Error::Cancelled)),
        "{result:?}"
    );
    let _ = std::fs::remove_file(&dst);
}
