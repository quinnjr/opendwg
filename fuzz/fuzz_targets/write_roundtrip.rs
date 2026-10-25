//! Read, and when that succeeds write R2018 (at a fixed save time, so a
//! crash replays) and read the result again: a written file must read back,
//! and validate with no error.
#![no_main]

use libfuzzer_sys::fuzz_target;
use opendwg::section::JulianDate;
use opendwg::{Drawing, Severity};

const SAVED: JulianDate = JulianDate {
    day: 2_461_300,
    ms: 0,
};

fuzz_target!(|data: &[u8]| {
    let Ok(d) = Drawing::from_bytes(data) else {
        return;
    };
    if let Ok(bytes) = d.to_bytes_at(SAVED) {
        let back = Drawing::from_bytes(&bytes).expect("written file reads back");
        let errors: Vec<_> = back
            .validate()
            .into_iter()
            .filter(|x| x.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "written file has errors: {errors:?}");
    }
});
