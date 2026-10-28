#![no_main]

use libfuzzer_sys::fuzz_target;
use diorama_we::mdl::{Mdl, MdlMesh};

fuzz_target!(|data: &[u8]| {
    let _ = Mdl::parse(data);
    let _ = MdlMesh::parse(data);
});
