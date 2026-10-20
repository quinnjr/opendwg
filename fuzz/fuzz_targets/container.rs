//! The container layer alone: slicing a file into its sections.
#![no_main]

use libfuzzer_sys::fuzz_target;
use opendwg::container;

fuzz_target!(|data: &[u8]| {
    let _ = container::read(data);
});
