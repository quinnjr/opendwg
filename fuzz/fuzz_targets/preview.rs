//! The preview section. The first byte picks whether a file address is given.
#![no_main]

use libfuzzer_sys::fuzz_target;
use opendwg::section::Preview;

fuzz_target!(|data: &[u8]| {
    let Some((&sel, rest)) = data.split_first() else {
        return;
    };
    let address = (sel & 1 != 0).then(|| u64::from(sel) * 0x100);
    if let Ok(p) = Preview::decode(rest, address) {
        let _ = p.encode(0x200);
    }
});
