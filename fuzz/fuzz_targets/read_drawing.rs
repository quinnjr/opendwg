//! The full reader: any bytes either read or fail with an error.
#![no_main]

use libfuzzer_sys::fuzz_target;
use opendwg::Drawing;

fuzz_target!(|data: &[u8]| {
    let _ = Drawing::from_bytes(data);
});
