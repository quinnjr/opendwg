//! R2007 Reed-Solomon decoding. The first three bytes give the block count,
//! the data size per block (239 or 251 for real pages) and the layout. The
//! rest is decoded, and (up to 2 KiB of it) encoded: decoding the encoding
//! must give the data back.
#![no_main]

use libfuzzer_sys::fuzz_target;
use opendwg::codec::rs2007;

fuzz_target!(|data: &[u8]| {
    let Some(([count, k, layout], src)) = data.split_first_chunk::<3>() else {
        return;
    };
    let block_count = usize::from(*count);
    let interleaved = layout & 1 == 1;
    let factor = if interleaved { block_count } else { 1 };
    let _ = rs2007::decode(src, block_count, usize::from(*k), factor);

    // Encoding is costly: a prefix is enough to check the round trip.
    let src = &src[..src.len().min(2048)];
    let k = if layout & 2 == 0 { 239 } else { 251 };
    let encoded = rs2007::encode(src, k, interleaved).unwrap();
    let n = src.len().div_ceil(k).max(1);
    let decoded = rs2007::decode(&encoded, n, k, if interleaved { n } else { 1 }).unwrap();
    assert_eq!(&decoded[..src.len()], src);
    assert!(decoded[src.len()..].iter().all(|&b| b == 0));
});
