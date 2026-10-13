//! Deterministic mutation testing: corrupt real WE files and check the parsers
//! return errors instead of panicking or allocating unbounded memory.
//!
//! Complements the libFuzzer targets in `fuzz/` (Linux CI) with a stable-Rust
//! check that runs everywhere. Uses the local WE install when present, else
//! only the synthetic seeds below.

#![allow(
    clippy::unwrap_used,
    reason = "test harness: I/O failures should fail the test loudly"
)]

use std::path::PathBuf;

use lumen_core::Limits;
use lumen_we::{Pkg, Tex, mip_to_rgba8};

const DEFAULT_INSTALL: &str = r"C:\Program Files (x86)\Steam\steamapps\common\wallpaper_engine";
const MUTATIONS_PER_SEED: usize = 300;

/// xorshift64*: tiny, deterministic, good enough to pick byte positions.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn mutate(seed: &[u8], rng: &mut Rng) -> Vec<u8> {
    let mut data = seed.to_vec();
    match rng.below(4) {
        0 => data.truncate(rng.below(seed.len())),
        1 => {
            for _ in 0..1 + rng.below(8) {
                let i = rng.below(data.len());
                data[i] = rng.next() as u8;
            }
        }
        2 => {
            // Overwrite a 4-byte field with an extreme value.
            let i = rng.below(data.len().saturating_sub(4));
            let v = [0u32, 1, 0x7fff_ffff, 0xffff_ffff, 0x8000_0000][rng.below(5)];
            if let Some(slot) = data.get_mut(i..i + 4) {
                slot.copy_from_slice(&v.to_le_bytes());
            }
        }
        _ => {
            let i = rng.below(data.len());
            data.insert(i, rng.next() as u8);
        }
    }
    data
}

fn small_seed_files(ext: &str, max_len: u64, max_count: usize) -> Vec<Vec<u8>> {
    let install = std::env::var_os("LUMEN_WE_INSTALL")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_INSTALL));
    if !install.is_dir() {
        return Vec::new();
    }
    let mut stack = vec![install.join("assets")];
    let mut out = Vec::new();
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == ext)
                && entry.metadata().is_ok_and(|m| m.len() <= max_len)
            {
                out.push(std::fs::read(&path).unwrap());
                if out.len() >= max_count {
                    return out;
                }
            }
        }
    }
    out
}

fn fuzz_limits() -> Limits {
    Limits {
        max_decompressed_bytes: 64 << 20,
        max_texture_dim: 4096,
        ..Limits::default()
    }
}

#[test]
fn mutated_tex_files_never_panic() {
    let seeds = small_seed_files("tex", 64 * 1024, 40);
    let limits = fuzz_limits();
    let mut rng = Rng(0x5eed_1e55);
    let mut parsed_ok = 0usize;
    for seed in &seeds {
        for _ in 0..MUTATIONS_PER_SEED {
            let data = mutate(seed, &mut rng);
            if let Ok(tex) = Tex::parse(&data, &limits) {
                parsed_ok += 1;
                if let Some(mip) = tex.first_mip() {
                    let _ = mip_to_rgba8(&tex, mip);
                }
            }
        }
    }
    eprintln!(
        "{} tex seeds × {MUTATIONS_PER_SEED} mutations, {parsed_ok} still parsed",
        seeds.len()
    );
}

#[test]
fn mutated_pkg_bytes_never_panic() {
    // Synthetic seed: a valid two-entry archive.
    let mut seed = Vec::new();
    let put = |out: &mut Vec<u8>, s: &[u8]| {
        out.extend_from_slice(&(s.len() as u32).to_le_bytes());
        out.extend_from_slice(s);
    };
    put(&mut seed, b"PKGV0001");
    seed.extend_from_slice(&2u32.to_le_bytes());
    for (name, offset, size) in [("scene.json", 0u32, 2u32), ("a/b.tex", 2, 4)] {
        put(&mut seed, name.as_bytes());
        seed.extend_from_slice(&offset.to_le_bytes());
        seed.extend_from_slice(&size.to_le_bytes());
    }
    seed.extend_from_slice(b"{}TEXV");

    let limits = fuzz_limits();
    let mut rng = Rng(0xfeed_beef);
    for _ in 0..5_000 {
        let data = mutate(&seed, &mut rng);
        if let Ok(pkg) = Pkg::parse(&data, &limits) {
            for entry in pkg.entries() {
                assert!(
                    pkg.data(entry).is_some(),
                    "validated entry must be readable"
                );
            }
        }
    }
}
