//! The object map (Handles section).
#![no_main]

use libfuzzer_sys::fuzz_target;
use opendwg::section::objmap;

fuzz_target!(|data: &[u8]| {
    let _ = objmap::decode(data);
});
