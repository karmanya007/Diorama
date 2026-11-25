#![no_main]

use diorama_core::Limits;
use diorama_project::{Memory, NoHost};
use libfuzzer_sys::fuzz_target;

// The document, then (after a zero byte) an image and a font it may use.
fuzz_target!(|data: &[u8]| {
    let mut parts = data.splitn(3, |b| *b == 0);
    let mut files = Memory::default();
    for name in ["wallpaper.json", "assets/a.png", "assets/a.ttf"] {
        if let Some(part) = parts.next() {
            files.0.insert(name.to_owned(), part.to_vec());
        }
    }
    let limits = Limits {
        max_texture_dim: 2048,
        max_decompressed_bytes: 32 << 20,
        ..Limits::default()
    };
    let _ = diorama_project::compile(&files, &Default::default(), &NoHost, &limits);
});
