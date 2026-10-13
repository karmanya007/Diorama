#![no_main]

use libfuzzer_sys::fuzz_target;
use lumen_core::Limits;
use lumen_we::{Tex, mip_to_rgba8};

fuzz_target!(|data: &[u8]| {
    let limits = Limits {
        max_decompressed_bytes: 64 << 20,
        max_texture_dim: 4096,
        ..Limits::default()
    };
    if let Ok(tex) = Tex::parse(data, &limits) {
        if let Some(mip) = tex.first_mip() {
            let _ = mip_to_rgba8(&tex, mip);
        }
    }
});
