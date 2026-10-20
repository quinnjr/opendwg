//! R2007 64-bit CRCs (ODS 5.12) and their seed functions.
//!
//! - [`crc64`]: the "normal" CRC (ECMA-182 polynomial `0x42F0E1EBA9EA3693`, MSB first), result
//!   inverted. Used for the file header data and its compressed form.
//! - [`crc64_mirrored`]: the "mirrored" CRC (reflected polynomial `0x95AC9329AC4BC9B5`, LSB
//!   first), result not inverted. Used for system pages and data pages.
//!
//! Both tables are built at compile time from the polynomial; they equal the tables ODS prints
//! (checked against samples below).
//!
//! Bytes are not fed in file order. ODS 5.12 processes each 8-byte block as bytes
//! `6, 7, 4, 5, 2, 3, 0, 1` and a 1-7 byte tail by its own table ([`ods_order`]). The ODS 5.4.1
//! data page checksum uses the same order. A real AC1021 file (ACadSharp's `sample_AC1021.dwg`)
//! confirms both CRCs, the order and the seed functions; see `r2007_real_crcs` in
//! `tests/container.rs`.

/// ODS 5.12.1 table (ECMA-182, MSB first).
const NORMAL_TABLE: [u64; 256] = {
    let mut table = [0u64; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = (i as u64) << 56;
        let mut k = 0;
        while k < 8 {
            c = if c >> 63 != 0 {
                (c << 1) ^ 0x42F0_E1EB_A9EA_3693
            } else {
                c << 1
            };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
};

/// ODS 5.12.2 table (reflected, LSB first).
const MIRRORED_TABLE: [u64; 256] = {
    let mut table = [0u64; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u64;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                (c >> 1) ^ 0x95AC_9329_AC4B_C9B5
            } else {
                c >> 1
            };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
};

/// Byte order within a block of `n` bytes (ODS 5.12 table, n = 1..=8).
const TAIL_ORDER: [&[usize]; 8] = [
    &[],
    &[0],
    &[0, 1],
    &[0, 1, 2],
    &[2, 3, 0, 1],
    &[2, 3, 0, 1, 4],
    &[2, 3, 0, 1, 4, 5],
    &[2, 3, 0, 1, 4, 5, 6],
];
const BLOCK_ORDER: [usize; 8] = [6, 7, 4, 5, 2, 3, 0, 1];

/// The bytes of `data` in ODS 5.12 / 5.4.1 processing order.
pub(super) fn ods_order(data: &[u8]) -> impl Iterator<Item = u8> + '_ {
    let blocks = data.chunks_exact(8);
    let tail = blocks.remainder();
    blocks
        .flat_map(|b| BLOCK_ORDER.iter().map(move |&i| b[i]))
        .chain(TAIL_ORDER[tail.len()].iter().map(move |&i| tail[i]))
}

/// ODS 5.12 normal 64-bit CRC of `data` starting from `seed` (usually from [`crc64_seed1`] or
/// [`crc64_seed2`]); the result is inverted.
#[must_use]
pub fn crc64(seed: u64, data: &[u8]) -> u64 {
    !ods_order(data).fold(seed, |crc, b| {
        NORMAL_TABLE[usize::from(b ^ (crc >> 56) as u8)] ^ (crc << 8)
    })
}

/// ODS 5.12 mirrored 64-bit CRC of `data` starting from `seed`; not inverted.
#[must_use]
pub fn crc64_mirrored(seed: u64, data: &[u8]) -> u64 {
    ods_order(data).fold(seed, |crc, b| {
        MIRRORED_TABLE[usize::from(b ^ crc as u8)] ^ (crc >> 8)
    })
}

/// ODS 5.12 `UpdateSeed1` for a CRC over `len` bytes (64-bit arithmetic, as the corpus shows).
#[must_use]
pub fn crc64_seed1(seed: u64, len: usize) -> u64 {
    let mut s = seed
        .wrapping_add(len as u64)
        .wrapping_mul(0x343FD)
        .wrapping_add(0x26_9EC3);
    s |= s.wrapping_mul(0x343FD << 32).wrapping_add(0x26_9EC3 << 32);
    !s
}

/// ODS 5.12 `UpdateSeed2` for a CRC over `len` bytes.
#[must_use]
pub fn crc64_seed2(seed: u64, len: usize) -> u64 {
    let s = seed
        .wrapping_add(len as u64)
        .wrapping_mul(0x343FD)
        .wrapping_add(0x26_9EC3);
    !s.wrapping_mul((1 << 32) + 0x343FD)
        .wrapping_add((len as u64).wrapping_add(0x26_9EC3))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DATA: [u8; 20] = [
        1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20,
    ];

    #[test]
    fn tables_match_ods_samples() {
        assert_eq!(NORMAL_TABLE[1], 0x42F0_E1EB_A9EA_3693);
        assert_eq!(NORMAL_TABLE[255], 0x9AFC_E626_CE85_B507);
        assert_eq!(MIRRORED_TABLE[1], 0x7AD8_70C8_3035_8979);
        assert_eq!(MIRRORED_TABLE[255], 0x29B7_D047_EFEC_8728);
    }

    #[test]
    fn byte_order_follows_ods_table() {
        let idx: Vec<u8> = (0..19).collect();
        let order: Vec<u8> = ods_order(&idx).collect();
        assert_eq!(
            order,
            [
                6, 7, 4, 5, 2, 3, 0, 1, 14, 15, 12, 13, 10, 11, 8, 9, 16, 17, 18
            ]
        );
        for (n, want) in TAIL_ORDER.iter().enumerate() {
            let got: Vec<usize> = ods_order(&idx[..n]).map(usize::from).collect();
            assert_eq!(&got, want, "{n}");
        }
    }

    #[test]
    fn seed_functions() {
        // Reference values from a direct transcription of the ODS 5.12 pseudocode.
        assert_eq!(crc64_seed1(0, 20), 0x9A20_50D1_FF98_1178);
        assert_eq!(crc64_seed1(5, 3), 0x91F5_313D_FFBF_4154);
        assert_eq!(crc64_seed2(0, 20), 0xFF98_1025_9A20_50BD);
    }

    #[test]
    fn known_values() {
        assert_eq!(crc64(0, &DATA), 0xE089_27EA_6A41_CAE0);
        assert_eq!(crc64(crc64_seed2(0, 20), &DATA), 0x29ED_5F58_EA9D_8769);
        assert_eq!(crc64_mirrored(0, &DATA), 0x38D0_E7E6_4EC0_40BC);
        assert_eq!(
            crc64_mirrored(crc64_seed1(0, 20), &DATA),
            0x7812_143F_2D60_B4E3
        );
    }

    #[test]
    fn empty_input() {
        assert_eq!(crc64(0x1234, b""), !0x1234);
        assert_eq!(crc64_mirrored(0x1234, b""), 0x1234);
    }
}
