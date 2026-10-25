//! Checksums and header crypto shared by the DWG container readers and writers.

mod checksum;
mod crc;
mod crc64;
mod crypt;
pub mod lz77_2004;
pub mod lz77_2007;
pub mod rs2007;

pub use checksum::{page_checksum, page_checksum_2007};
pub use crc::{crc16, crc32};
pub use crc64::{crc64, crc64_mirrored, crc64_seed1, crc64_seed2};
pub use crypt::{page_header_mask, xor_r2004_file_header};

/// Append the `len` bytes that start `dist` bytes before the end of `out`
/// (an LZ77 back-reference; `1 <= dist <= out.len()`). An overlapping copy
/// (`len > dist`) repeats the pattern: what is copied is periodic, so each
/// pass copies everything from the start written so far, doubling it.
pub(crate) fn copy_back(out: &mut Vec<u8>, dist: usize, len: usize) {
    out.reserve(len);
    let start = out.len() - dist;
    let mut left = len;
    while left > 0 {
        let n = left.min(out.len() - start);
        out.extend_from_within(start..start + n);
        left -= n;
    }
}

#[cfg(test)]
mod tests {
    /// The chunked copy gives what a byte-by-byte copy gives, overlapping
    /// or not.
    #[test]
    fn copy_back_matches_a_byte_loop() {
        let base: Vec<u8> = (1..=9).collect();
        for dist in 1..=base.len() {
            for len in 0..40 {
                let mut want = base.clone();
                let start = want.len() - dist;
                for i in 0..len {
                    let b = want[start + i];
                    want.push(b);
                }
                let mut got = base.clone();
                super::copy_back(&mut got, dist, len);
                assert_eq!(got, want, "dist {dist} len {len}");
            }
        }
    }
}
