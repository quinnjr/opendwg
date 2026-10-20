//! R2004 page decompression. The first two bytes give the expected size.
#![no_main]

use libfuzzer_sys::fuzz_target;
use opendwg::codec::lz77_2004;

fuzz_target!(|data: &[u8]| {
    let Some((len, src)) = data.split_first_chunk::<2>() else {
        return;
    };
    let _ = lz77_2004::decompress(src, usize::from(u16::from_le_bytes(*len)));
});
