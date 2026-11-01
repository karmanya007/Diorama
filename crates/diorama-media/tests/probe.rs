//! Probes a real video (`DIORAMA_TEST_VIDEO`, never committed) and decodes
//! its first frames. `cargo test -p diorama-media --test probe -- --ignored`.

use std::path::PathBuf;

#[test]
#[ignore = "needs DIORAMA_TEST_VIDEO"]
fn probe_matches_what_decodes() {
    let path = PathBuf::from(std::env::var("DIORAMA_TEST_VIDEO").expect("DIORAMA_TEST_VIDEO"));
    let probe = diorama_media::probe(&path).unwrap();
    println!("{probe:?}");
    assert!(probe.decodable);
    let mut decoder = diorama_media::open(&path).unwrap();
    let info = decoder.info();
    assert_eq!((info.width, info.height), (probe.width, probe.height));
    for _ in 0..30 {
        let frame = decoder.next_frame().unwrap().expect("a frame");
        assert_eq!((frame.width, frame.height), (probe.width, probe.height));
    }
    decoder.restart().unwrap();
    let first = decoder
        .next_frame()
        .unwrap()
        .expect("a frame after restart");
    assert!(first.pts < std::time::Duration::from_millis(100));
}
