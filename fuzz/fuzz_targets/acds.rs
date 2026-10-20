//! The R2013+ data storage section (AcDs).
#![no_main]

use libfuzzer_sys::fuzz_target;
use opendwg::section::DataStorage;

fuzz_target!(|data: &[u8]| {
    if let Ok(ds) = DataStorage::parse(data) {
        let _ = ds.encode();
    }
    let _ = DataStorage::stored_search(data);
});
