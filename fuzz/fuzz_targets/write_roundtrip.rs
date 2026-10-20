//! Read, and when that succeeds write R2018 and read the result again.
#![no_main]

use libfuzzer_sys::fuzz_target;
use opendwg::Drawing;

fuzz_target!(|data: &[u8]| {
    let Ok(d) = Drawing::from_bytes(data) else {
        return;
    };
    if let Ok(bytes) = d.to_bytes() {
        let _ = Drawing::from_bytes(&bytes);
    }
});
