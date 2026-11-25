#![no_main]

use std::io::Cursor;

use libfuzzer_sys::fuzz_target;

// A `.diorama` file from someone else.
fuzz_target!(|data: &[u8]| {
    let dir = std::env::temp_dir().join(format!("diorama-fuzz-{}", std::process::id()));
    let _ = diorama_project::package::unpack(Cursor::new(data), &dir);
    let _ = std::fs::remove_dir_all(&dir);
});
