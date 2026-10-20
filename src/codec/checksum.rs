//! Section page checksums: R2004 (ODS 4.2) and R2007 (ODS 5.4.1), both Adler-32 variants.

use super::crc64::ods_order;

/// Largest run that can be summed before the modulus must be applied.
const CHUNK: usize = 0x15B0;
const MODULUS: u64 = 0xFFF1;

/// Computes the system/data page checksum with the given `seed`.
///
/// System pages (ODS 4.3): seed 0 over the header, then that result as the seed over the data.
/// Data pages (ODS 4.6): seed 0 over the data, then that result as the seed over the header.
///
/// The sums are kept in `u64`, so no seed can overflow them; for every seed the result equals
/// the ODS `u32` pseudocode (which itself stays below `u32::MAX` within a 0x15B0-byte chunk).
#[must_use]
pub fn page_checksum(seed: u32, data: &[u8]) -> u32 {
    let mut sum1 = u64::from(seed & 0xFFFF);
    let mut sum2 = u64::from(seed >> 16);
    for chunk in data.chunks(CHUNK) {
        for &b in chunk {
            sum1 += u64::from(b);
            sum2 += sum1;
        }
        sum1 %= MODULUS;
        sum2 %= MODULUS;
    }
    // Both sums are below 0xFFF1 after any chunk; with no data they are the seed halves.
    ((sum2 as u32) << 16) | (sum1 as u32 & 0xFFFF)
}

/// R2007 data page checksum (ODS 5.4.1) of the page's uncompressed data.
///
/// Differs from [`page_checksum`] in two ways: the 64-bit `seed` is first mixed with the length
/// (`(seed + len) * 0x343FD + 0x269EC3`, of which the low 32 bits seed the two sums), and the
/// bytes are summed in the ODS 5.12 block order (`6, 7, 4, 5, 2, 3, 0, 1` per 8 bytes). The
/// corpus confirms it with seed 0 on every page.
#[must_use]
pub fn page_checksum_2007(seed: u64, data: &[u8]) -> u32 {
    let seed = seed
        .wrapping_add(data.len() as u64)
        .wrapping_mul(0x343FD)
        .wrapping_add(0x26_9EC3);
    let mut sum1 = seed & 0xFFFF;
    let mut sum2 = (seed >> 16) & 0xFFFF;
    // CHUNK is a multiple of 8, so chunking the reordered stream equals chunking the data.
    let mut n = 0;
    for b in ods_order(data) {
        sum1 += u64::from(b);
        sum2 += sum1;
        n += 1;
        if n == CHUNK {
            sum1 %= MODULUS;
            sum2 %= MODULUS;
            n = 0;
        }
    }
    sum1 %= MODULUS;
    sum2 %= MODULUS;
    ((sum2 as u32) << 16) | (sum1 as u32 & 0xFFFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_returns_seed() {
        assert_eq!(page_checksum(0x1234_5678, b""), 0x1234_5678);
    }

    #[test]
    fn small_hand_computed() {
        // sum1: 1, 3, 6; sum2: 1, 4, 10.
        assert_eq!(page_checksum(0, &[1, 2, 3]), 0x000A_0006);
    }

    /// Reference with u64 arithmetic and a reduction after every byte.
    fn reference(seed: u32, data: &[u8]) -> u32 {
        let (mut s1, mut s2) = (u64::from(seed & 0xFFFF), u64::from(seed >> 16));
        for &b in data {
            s1 = (s1 + u64::from(b)) % 0xFFF1;
            s2 = (s2 + s1) % 0xFFF1;
        }
        ((s2 as u32) << 16) | s1 as u32
    }

    #[test]
    fn matches_wide_reference_across_chunks() {
        let data: Vec<u8> = (0..20_000u32).map(|i| (i * 31 + 7) as u8).collect();
        assert_eq!(page_checksum(0, &data), reference(0, &data));
    }

    #[test]
    fn all_ff_does_not_overflow() {
        // Worst case for the per-chunk sums: maximal seed halves and all-0xFF data.
        let data = vec![0xFFu8; 0x15B0 * 3];
        for seed in [0xFFF0_FFF0, 0xFFFF_FFFF] {
            assert_eq!(
                page_checksum(seed, &data),
                reference(seed, &data),
                "{seed:#x}"
            );
        }
    }

    #[test]
    fn r2007_known_values() {
        // Reference values from a direct transcription of the ODS 5.4.1 pseudocode.
        let data: Vec<u8> = (1..=20).collect();
        assert_eq!(page_checksum_2007(0, &data), 0xAAAD_EF59);
        assert_eq!(page_checksum_2007(7, &data), 0xD8F3_CB44);
        let big: Vec<u8> = (0..20_000u32).map(|i| (i * 31 + 7) as u8).collect();
        assert_eq!(page_checksum_2007(0, &big), 0x0436_1F9C);
    }

    #[test]
    fn r2007_order_matters() {
        // Swapping bytes 0 and 6 of a block changes sum2: the order is not file order.
        let a = [1, 0, 0, 0, 0, 0, 2, 0];
        let b = [2, 0, 0, 0, 0, 0, 1, 0];
        assert_ne!(page_checksum_2007(0, &a), page_checksum_2007(0, &b));
    }

    // Real-file checks (system page, data page) are corpus tests in tests/container.rs:
    // `r2004_real_page_checksums`.
}
