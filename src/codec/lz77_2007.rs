//! R2007 LZ77 decompression (ODS section 5.10), used for the AC1021 file header, system pages and
//! data pages. R2007 is read-only here, so there is no compressor.
//!
//! Stream layout: a literal chunk, then one or more back-reference ("decompressed") chunks, then
//! a literal chunk, and so on until the input ends. There is no terminator opcode, so the caller
//! passes exactly the compressed bytes. The last chunk may be a literal chunk.
//!
//! - First byte: the first opcode. If its high nibble is 2, bytes 1-2 are skipped, the first
//!   literal length is `byte[3] & 7` and the literals start at byte 4. ODS 5.10 ("advanced 2
//!   bytes, and a length is read from the next byte ... 3 bytes in total") can be read as
//!   `byte[2]` or `byte[3]`; ACadSharp (MIT) `DwgLZ77AC21Decompressor.Decompress` advances the
//!   source index by 3 past the opcode and reads the byte before it, i.e. `byte[3]`.
//! - Literal length (when not already known): `opcode + 8`; `0x17` means a byte follows and is
//!   added, and `0xFF` there means little-endian 16-bit values follow and are added while they
//!   are `0xFFFF`.
//! - Literals are not copied in order: 32-byte blocks have their four 8-byte groups reversed, and
//!   a 1-31 byte remainder uses the per-length piece order of the ODS 5.10.1 table (`REMAINDER`).
//! - Back-reference opcodes follow ODS 5.10.2 `ReadInstructions`. After each copy the low 3 bits
//!   of the last opcode byte are the next literal length; if they are 0 another opcode follows,
//!   and an opcode below `0x10` ends the back-reference run (it is then the literal length
//!   opcode). Inside a run, high nibble `0xF` is treated as nibble 0.
//!
//! ODS gives the algorithm as pseudocode; ACadSharp (MIT) `DwgLZ77AC21Decompressor` was consulted
//! to confirm the reading of it (see `THIRD_PARTY.md`). Its copy table and opcode handling agree
//! with ODS.
//!
//! Policy choices (ODS is silent):
//! - `decompress` produces exactly `expected_len` bytes. A copy that would go past it is
//!   `Error::Corrupt`; input that ends before the output is full, or in the middle of a chunk or
//!   opcode, is `Error::Truncated`.
//! - A back-reference to before the start of the output is `Error::Corrupt`.
//! - An empty input decodes to an empty output (`Truncated` if `expected_len > 0`).

use crate::error::{Error, Result};

fn truncated() -> Error {
    Error::Truncated {
        context: "R2007 LZ77 stream",
    }
}

fn corrupt(msg: &str) -> Error {
    Error::Corrupt(format!("R2007 LZ77: {msg}"))
}

/// ODS 5.10.1 table: for a remainder of `n` literal bytes (index `n`), the pieces in output
/// order as `(size, source offset)`. Piece sizes copy as: 1 = one byte; 2 and 3 = bytes in
/// reverse order; 4 and 8 = bytes in order; 16 = bytes 8-15 then 0-7.
const REMAINDER: [&[(u8, u8)]; 32] = [
    &[],
    &[(1, 0)],
    &[(2, 0)],
    &[(3, 0)],
    &[(4, 0)],
    &[(1, 4), (4, 0)],
    &[(1, 5), (4, 1), (1, 0)],
    &[(2, 5), (4, 1), (1, 0)],
    &[(8, 0)],
    &[(1, 8), (8, 0)],
    &[(1, 9), (8, 1), (1, 0)],
    &[(2, 9), (8, 1), (1, 0)],
    &[(4, 8), (8, 0)],
    &[(1, 12), (4, 8), (8, 0)],
    &[(1, 13), (4, 9), (8, 1), (1, 0)],
    &[(2, 13), (4, 9), (8, 1), (1, 0)],
    &[(16, 0)],
    &[(8, 9), (1, 8), (8, 0)],
    &[(1, 17), (16, 1), (1, 0)],
    &[(3, 16), (16, 0)],
    &[(4, 16), (16, 0)],
    &[(1, 20), (4, 16), (16, 0)],
    &[(2, 20), (4, 16), (16, 0)],
    &[(3, 20), (4, 16), (16, 0)],
    &[(8, 16), (16, 0)],
    &[(8, 17), (1, 16), (16, 0)],
    &[(1, 25), (8, 17), (1, 16), (16, 0)],
    &[(2, 25), (8, 17), (1, 16), (16, 0)],
    &[(4, 24), (8, 16), (16, 0)],
    &[(1, 28), (4, 24), (8, 16), (16, 0)],
    &[(2, 28), (4, 24), (8, 16), (16, 0)],
    &[(1, 30), (4, 26), (8, 18), (16, 2), (2, 0)],
];

/// Append one table piece of `lit` (sizes as documented on `REMAINDER`).
fn push_piece(out: &mut Vec<u8>, lit: &[u8], size: usize, off: usize) {
    let p = &lit[off..off + size];
    match size {
        2 | 3 => out.extend(p.iter().rev()),
        16 => {
            out.extend_from_slice(&p[8..]);
            out.extend_from_slice(&p[..8]);
        }
        _ => out.extend_from_slice(p),
    }
}

/// Append `lit` in the ODS 5.10.1 shuffled order.
fn push_literals(out: &mut Vec<u8>, lit: &[u8]) {
    let mut blocks = lit.chunks_exact(32);
    for b in &mut blocks {
        for g in (0..4).rev() {
            out.extend_from_slice(&b[g * 8..g * 8 + 8]);
        }
    }
    let rest = blocks.remainder();
    for &(size, off) in REMAINDER[rest.len()] {
        push_piece(out, rest, usize::from(size), usize::from(off));
    }
}

struct Decoder<'a> {
    src: &'a [u8],
    pos: usize,
    out: Vec<u8>,
    expected_len: usize,
}

impl Decoder<'_> {
    fn byte(&mut self) -> Result<u8> {
        let b = *self.src.get(self.pos).ok_or_else(truncated)?;
        self.pos += 1;
        Ok(b)
    }

    fn at_end(&self) -> bool {
        self.pos >= self.src.len()
    }

    fn room(&self, n: usize) -> Result<()> {
        if n > self.expected_len - self.out.len() {
            Err(corrupt("output exceeds expected length"))
        } else {
            Ok(())
        }
    }

    /// ODS `ReadLiteralLength`.
    fn literal_length(&mut self, opcode: u8) -> Result<usize> {
        let mut len = usize::from(opcode) + 8;
        if len == 0x17 {
            let n = self.byte()?;
            len += usize::from(n);
            if n == 0xFF {
                loop {
                    let n = u16::from_le_bytes([self.byte()?, self.byte()?]);
                    len = len
                        .checked_add(usize::from(n))
                        .ok_or_else(|| corrupt("length overflow"))?;
                    if n != 0xFFFF {
                        break;
                    }
                }
            }
        }
        Ok(len)
    }

    fn copy_literals(&mut self, n: usize) -> Result<()> {
        self.room(n)?;
        let end = self.pos.checked_add(n).ok_or_else(truncated)?;
        let lit = self.src.get(self.pos..end).ok_or_else(truncated)?;
        push_literals(&mut self.out, lit);
        self.pos = end;
        Ok(())
    }

    /// ODS `ReadInstructions`: returns `(distance, length)` and updates `opcode` to the last
    /// opcode byte read (whose low 3 bits are the next literal length).
    fn instruction(&mut self, opcode: &mut u8) -> Result<(usize, usize)> {
        let op = *opcode;
        Ok(match op >> 4 {
            0 => {
                let off = usize::from(self.byte()?);
                *opcode = self.byte()?;
                let len = usize::from(op & 0x0F) + 0x13 + usize::from((*opcode >> 3) & 0x10);
                (off + (usize::from(*opcode & 0x78) << 5) + 1, len)
            }
            1 => {
                let off = usize::from(self.byte()?);
                *opcode = self.byte()?;
                let len = usize::from(op & 0x0F) + 3;
                (off + (usize::from(*opcode & 0xF8) << 5) + 1, len)
            }
            2 => {
                let mut off = usize::from(u16::from_le_bytes([self.byte()?, self.byte()?]));
                let mut len = usize::from(op & 7);
                if op & 8 == 0 {
                    *opcode = self.byte()?;
                    len += usize::from(*opcode & 0xF8);
                } else {
                    off += 1;
                    len += usize::from(self.byte()?) << 3;
                    *opcode = self.byte()?;
                    len += (usize::from(*opcode & 0xF8) << 8) + 0x100;
                }
                (off, len)
            }
            _ => {
                let len = usize::from(op >> 4);
                let low = usize::from(op & 0x0F);
                *opcode = self.byte()?;
                (low + (usize::from(*opcode & 0xF8) << 1) + 1, len)
            }
        })
    }

    fn copy_back(&mut self, dist: usize, len: usize) -> Result<()> {
        if dist == 0 || dist > self.out.len() {
            return Err(corrupt("back-reference before start of output"));
        }
        self.room(len)?;
        super::copy_back(&mut self.out, dist, len);
        Ok(())
    }

    /// ODS `CopyDecompressedChunks`. The opcode is one byte, as in ACadSharp's
    /// `copyDecompressedChunks`; the ODS pseudocode's second `inputIndex++` is a typo. Returns
    /// the next literal length (0 = read it from the stream using the returned opcode).
    fn back_references(&mut self) -> Result<(u8, usize)> {
        let mut opcode = self.byte()?;
        let (dist, len) = self.instruction(&mut opcode)?;
        self.copy_back(dist, len)?;
        loop {
            let lit = usize::from(opcode & 7);
            if lit != 0 || self.at_end() {
                return Ok((opcode, lit));
            }
            opcode = self.byte()?;
            if opcode >> 4 == 0 {
                return Ok((opcode, 0));
            }
            if opcode >> 4 == 0x0F {
                opcode &= 0x0F;
            }
            let (dist, len) = self.instruction(&mut opcode)?;
            self.copy_back(dist, len)?;
        }
    }
}

/// Test-only encoder: a literal-only stream that [`decompress`] turns back into `plain`
/// (any length; the literal shuffle is inverted per 32-byte block and remainder).
#[cfg(test)]
pub(crate) fn literal_stream(plain: &[u8]) -> Vec<u8> {
    let mut s = Vec::new();
    match plain.len() {
        0 => return s,
        n @ 1..=0x16 if n >= 8 => s.push((n - 8) as u8),
        n if n >= 0x17 => {
            s.push(0x0F);
            let mut rest = n - 0x17;
            if rest < 0xFF {
                s.push(rest as u8);
            } else {
                s.push(0xFF);
                rest -= 0xFF;
                loop {
                    let w = rest.min(0xFFFF);
                    s.extend_from_slice(&(w as u16).to_le_bytes());
                    rest -= w;
                    if w != 0xFFFF {
                        break;
                    }
                }
            }
        }
        // 1-7 literals need the 0x2? first-opcode form: bytes 1-2 ignored, length in byte 3.
        n => s.extend_from_slice(&[0x20, 0, 0, n as u8]),
    }
    let mut order = Vec::with_capacity(plain.len());
    let idx: Vec<u32> = (0..plain.len() as u32).collect();
    let mut blocks = idx.chunks_exact(32);
    for b in &mut blocks {
        for g in (0..4).rev() {
            order.extend_from_slice(&b[g * 8..g * 8 + 8]);
        }
    }
    let rest = blocks.remainder();
    for &(size, off) in REMAINDER[rest.len()] {
        let p = &rest[usize::from(off)..usize::from(off + size)];
        match size {
            2 | 3 => order.extend(p.iter().rev()),
            16 => {
                order.extend_from_slice(&p[8..]);
                order.extend_from_slice(&p[..8]);
            }
            _ => order.extend_from_slice(p),
        }
    }
    let mut lit = vec![0u8; plain.len()];
    for (out_pos, &src_pos) in order.iter().enumerate() {
        lit[src_pos as usize] = plain[out_pos];
    }
    s.extend(lit);
    s
}

/// Decompress an R2007 LZ77 stream (exactly the compressed bytes) into exactly `expected_len`
/// bytes.
///
/// Peak output is bounded by `expected_len`, which callers must validate against the page or
/// header limits before calling.
pub fn decompress(src: &[u8], expected_len: usize) -> Result<Vec<u8>> {
    let mut d = Decoder {
        src,
        pos: 0,
        // Reserve conservatively: `expected_len` comes from the file, the output grows with
        // real data.
        out: Vec::with_capacity(expected_len.min(src.len().saturating_mul(8))),
        expected_len,
    };
    if !src.is_empty() {
        let mut opcode = d.byte()?;
        let mut lit = 0;
        if opcode >> 4 == 2 {
            // Length byte is byte[3] (ACadSharp `Decompress`: `_sourceIndex += 3`, then
            // `source[_sourceIndex - 1]`); see the module docs.
            d.pos = 3;
            lit = usize::from(d.byte()? & 7);
        }
        while !d.at_end() {
            if lit == 0 {
                lit = d.literal_length(opcode)?;
            }
            d.copy_literals(lit)?;
            if d.at_end() {
                break;
            }
            (opcode, lit) = d.back_references()?;
        }
    }
    if d.out.len() == expected_len {
        Ok(d.out)
    } else {
        Err(truncated())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Inverse of `push_literals`: the stream bytes that decode to `plain`.
    fn shuffle(plain: &[u8]) -> Vec<u8> {
        let mut order = Vec::new();
        let idx: Vec<u8> = (0..=255).collect();
        push_literals(&mut order, &idx[..plain.len().min(256)]);
        let mut stream = vec![0u8; order.len()];
        for (out_pos, &src_pos) in order.iter().enumerate() {
            stream[usize::from(src_pos)] = plain[out_pos];
        }
        stream
    }

    fn seq(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i * 7 + 1) as u8).collect()
    }

    #[test]
    fn remainder_table_is_a_permutation() {
        for (n, pieces) in REMAINDER.iter().enumerate() {
            let mut seen = vec![0u8; n];
            for &(size, off) in *pieces {
                assert!([1, 2, 3, 4, 8, 16].contains(&size), "n={n}");
                for b in &mut seen[usize::from(off)..usize::from(off + size)] {
                    *b += 1;
                }
            }
            assert!(seen.iter().all(|&c| c == 1), "n={n}: {seen:?}");
        }
    }

    #[test]
    fn literal_order_matches_ods_table() {
        let src: Vec<u8> = (0..32).collect();
        let order = |n: usize| {
            let mut o = Vec::new();
            push_literals(&mut o, &src[..n]);
            o
        };
        assert_eq!(order(1), [0]);
        assert_eq!(order(2), [1, 0]);
        assert_eq!(order(3), [2, 1, 0]);
        assert_eq!(order(4), [0, 1, 2, 3]);
        assert_eq!(order(6), [5, 1, 2, 3, 4, 0]);
        assert_eq!(order(7), [6, 5, 1, 2, 3, 4, 0]);
        assert_eq!(
            order(16),
            [8, 9, 10, 11, 12, 13, 14, 15, 0, 1, 2, 3, 4, 5, 6, 7]
        );
        assert_eq!(
            order(19),
            [
                18, 17, 16, 8, 9, 10, 11, 12, 13, 14, 15, 0, 1, 2, 3, 4, 5, 6, 7
            ]
        );
        let o31 = order(31);
        assert_eq!(&o31[..5], &[30, 26, 27, 28, 29]);
        assert_eq!(&o31[29..], &[1, 0]);
        let o32: Vec<u8> = (24..32).chain(16..24).chain(8..16).chain(0..8).collect();
        assert_eq!(order(32), o32);
    }

    #[test]
    fn literal_only_short_form() {
        // Opcode 0x00: literal length 0 + 8.
        let plain = seq(8);
        let mut s = vec![0x00];
        s.extend(shuffle(&plain));
        assert_eq!(decompress(&s, 8).unwrap(), plain);
    }

    #[test]
    fn literal_length_one_byte_extension() {
        // 0x0F -> 0x17, plus the next byte (9) = 32 bytes: one shuffled 32-byte block.
        let plain = seq(32);
        let mut s = vec![0x0F, 9];
        s.extend(shuffle(&plain[..32]));
        assert_eq!(decompress(&s, 32).unwrap(), plain);
    }

    #[test]
    fn literal_length_word_extension() {
        // 0x17 + 0xFF + 0x0100 = 0x216 bytes.
        let n = 0x17 + 0xFF + 0x100;
        let plain = seq(n);
        let mut s = vec![0x0F, 0xFF, 0x00, 0x01];
        for chunk in plain.chunks(256) {
            s.extend(shuffle(chunk));
        }
        // `shuffle` works per 256 bytes; 256 is a multiple of 32 so blocks line up.
        assert_eq!(decompress(&s, n).unwrap(), plain);
    }

    #[test]
    fn first_opcode_high_nibble_two() {
        // 0x2? opcode: skip two bytes, literal length = byte[3] & 7.
        let plain = seq(5);
        let mut s = vec![0x20, 0xEE, 0xEE, 0xF5];
        s.extend(shuffle(&plain));
        assert_eq!(decompress(&s, 5).unwrap(), plain);
    }

    /// 8 literal bytes "ABCDEFGH" followed by `tail`.
    fn with_prefix(tail: &[u8]) -> Vec<u8> {
        let mut s = vec![0x00];
        s.extend(shuffle(b"ABCDEFGH"));
        s.extend_from_slice(tail);
        s
    }

    #[test]
    fn short_back_reference_default_case() {
        // 0x41, next 0x00: length 4, distance 1 + ((0x00 & 0xF8) << 1) + 1 = 2 -> "GHGH".
        let s = with_prefix(&[0x41, 0x00]);
        assert_eq!(decompress(&s, 12).unwrap(), b"ABCDEFGHGHGH");
    }

    #[test]
    fn back_reference_then_inline_literal_count() {
        // 0x30, next 0x02: distance 1, length 3 ("HHH"), then 2 literals.
        let mut s = with_prefix(&[0x30, 0x02]);
        s.extend(shuffle(b"xy"));
        assert_eq!(decompress(&s, 13).unwrap(), b"ABCDEFGHHHHxy");
    }

    #[test]
    fn opcode_case_one() {
        // 0x12: length 2 + 3 = 5, distance byte 7 + ((0 & 0xF8) << 5) + 1 = 8 -> "ABCDE".
        let s = with_prefix(&[0x12, 0x07, 0x00]);
        assert_eq!(decompress(&s, 13).unwrap(), b"ABCDEFGHABCDE");
    }

    #[test]
    fn opcode_case_zero_and_chained_runs() {
        // Case 0 is reached inside a run via 0xF? (masked to 0x0?). First 0x30/0x00: distance 1,
        // length 3. Then 0xF0: length 0x13 (+0x10 when bit 0x80 of the next opcode is set),
        // distance byte 0 + 1 = 1. Then an opcode below 0x10 ends the run: 0x00 -> 8 literals.
        let mut s = with_prefix(&[0x30, 0x00, 0xF0, 0x00, 0x80, 0x00]);
        s.extend(shuffle(b"12345678"));
        let mut want = b"ABCDEFGH".to_vec();
        want.extend([b'H'; 3 + 0x13 + 0x10]);
        want.extend(b"12345678");
        assert_eq!(decompress(&s, want.len()).unwrap(), want);
    }

    #[test]
    fn opcode_case_two_short_and_long() {
        // 0x23: distance 0x0002, length 3 + (next & 0xF8 = 0x08) = 11.
        let s = with_prefix(&[0x23, 0x02, 0x00, 0x08]);
        assert_eq!(decompress(&s, 19).unwrap(), b"ABCDEFGHGHGHGHGHGHG");

        // 0x29: bit 8 set: distance 0x0000 + 1, length 1 + (2 << 3) + ((0 & 0xF8) << 8) + 0x100.
        let s = with_prefix(&[0x29, 0x00, 0x00, 0x02, 0x00]);
        let mut want = b"ABCDEFGH".to_vec();
        want.extend([b'H'; 1 + 16 + 0x100]);
        assert_eq!(decompress(&s, want.len()).unwrap(), want);
    }

    #[test]
    fn literal_stream_roundtrips() {
        for n in [
            1, 5, 7, 8, 0x16, 0x17, 0x18, 0x115, 0x116, 0x10115, 0x10116, 0x20000,
        ] {
            let plain = seq(n);
            assert_eq!(
                decompress(&literal_stream(&plain), n).unwrap(),
                plain,
                "{n}"
            );
        }
    }

    #[test]
    fn empty_input() {
        assert_eq!(decompress(&[], 0).unwrap(), Vec::<u8>::new());
        assert!(matches!(decompress(&[], 1), Err(Error::Truncated { .. })));
    }

    #[test]
    fn truncated_inputs_error() {
        let mut s = with_prefix(&[0x30, 0x02]);
        s.extend(shuffle(b"xy"));
        for cut in 0..s.len() {
            assert!(
                matches!(decompress(&s[..cut], 13), Err(Error::Truncated { .. })),
                "cut {cut}"
            );
        }
        // Cut inside the extended literal length and inside a case-2 instruction.
        assert!(matches!(
            decompress(&[0x0F], 32),
            Err(Error::Truncated { .. })
        ));
        assert!(matches!(
            decompress(&[0x0F, 0xFF, 0x00], 600),
            Err(Error::Truncated { .. })
        ));
        assert!(matches!(
            decompress(&with_prefix(&[0x29, 0x00, 0x00]), 300),
            Err(Error::Truncated { .. })
        ));
    }

    #[test]
    fn output_bounds() {
        // Literal run longer than expected_len.
        let s = with_prefix(&[]);
        assert!(matches!(decompress(&s, 7), Err(Error::Corrupt(_))));
        // Back-reference longer than the room left.
        assert!(matches!(
            decompress(&with_prefix(&[0x41, 0x00]), 11),
            Err(Error::Corrupt(_))
        ));
        // Back-reference before the start of the output (distance 9 > 8 bytes).
        assert!(matches!(
            decompress(&with_prefix(&[0x12, 0x08, 0x00]), 13),
            Err(Error::Corrupt(_))
        ));
    }

    #[test]
    fn huge_expected_len_does_not_preallocate() {
        let s = with_prefix(&[]);
        assert!(matches!(
            decompress(&s, usize::MAX),
            Err(Error::Truncated { .. })
        ));
    }

    proptest::proptest! {
        #[test]
        fn never_panics(src in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..512),
                        len in 0usize..4096) {
            let _ = decompress(&src, len);
        }
    }
}
