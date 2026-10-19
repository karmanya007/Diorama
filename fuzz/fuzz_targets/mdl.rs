#![no_main]

use libfuzzer_sys::fuzz_target;
use lumen_we::mdl::Mdl;

fuzz_target!(|data: &[u8]| {
    let _ = Mdl::parse(data);
});
