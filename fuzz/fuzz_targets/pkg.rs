#![no_main]

use libfuzzer_sys::fuzz_target;
use lumen_core::Limits;
use lumen_we::Pkg;

fuzz_target!(|data: &[u8]| {
    if let Ok(pkg) = Pkg::parse(data, &Limits::default()) {
        for entry in pkg.entries() {
            let _ = pkg.data(entry);
        }
    }
});
