//! Fixed-key obfuscation used by R2004+ headers (ODS 4.1 and 4.6).

/// XORs `buf` with the R2004 file-header keystream (ODS 4.1).
///
/// The keystream is an LCG started from seed 1 (`seed = seed * 0x343FD +
/// 0x269EC3`, byte = `seed >> 16`). Applying this twice restores the input.
pub fn xor_r2004_file_header(buf: &mut [u8]) {
    let mut seed: u32 = 1;
    for b in buf {
        seed = seed.wrapping_mul(0x343FD).wrapping_add(0x269EC3);
        *b ^= (seed >> 16) as u8;
    }
}

/// Mask for the 32-byte masked data page header (ODS 4.6).
///
/// `page_offset` is the page's absolute file address; only its low 32 bits
/// are used. XOR each of the header's eight little-endian u32 words with the
/// returned value to mask or unmask it.
#[must_use]
pub fn page_header_mask(page_offset: u64) -> u32 {
    0x4164_536B ^ (page_offset as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xor_twice_is_identity() {
        let orig: Vec<u8> = (0..0x6C).collect();
        let mut buf = orig.clone();
        xor_r2004_file_header(&mut buf);
        assert_ne!(buf, orig);
        xor_r2004_file_header(&mut buf);
        assert_eq!(buf, orig);
    }

    #[test]
    fn keystream_first_bytes() {
        // Hand computation, seed starts at 1:
        //   s1 = 1 * 0x343FD + 0x269EC3 = 0x0029E2C0 -> (s1 >> 16) & 0xFF = 0x29
        //   s2 = s1 * 0x343FD + 0x269EC3 (mod 2^32) = 0xC823F683 -> 0x23
        //   s3 = 0x18BE873A -> 0xBE
        //   s4 = 0xE7847115 -> 0x84
        // These match the first bytes of the ODS magic sequence
        // (29 23 BE 84 E1 6C D6 AE ...).
        let mut buf = [0u8; 4];
        xor_r2004_file_header(&mut buf);
        assert_eq!(buf, [0x29, 0x23, 0xBE, 0x84]);
    }

    #[test]
    fn page_mask_uses_low_32_bits() {
        assert_eq!(page_header_mask(0), 0x4164_536B);
        assert_eq!(page_header_mask(0x100), 0x4164_526B);
        assert_eq!(page_header_mask(0x1_0000_0100), page_header_mask(0x100));
    }
}
