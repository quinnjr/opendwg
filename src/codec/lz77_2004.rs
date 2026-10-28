//! R2004-family LZ77 (ODS section 4.7), used for the R2004 through R2018 page and section data.
//!
//! Stream layout: an optional first literal run, then a series of match opcodes, each carrying
//! 0-3 trailing literals inline (or a literal length that follows), ended by opcode `0x11`.
//!
//! Where ODS is ambiguous this follows ACadSharp (MIT), `DwgLZ77AC18Decompressor`; see
//! `THIRD_PARTY.md`:
//! - Every back-reference distance is `compOffset + 1`. ODS lists the `0x10`-`0x1F` forms as
//!   "+ 0x3FFF", which is the same thing: `0x3FFF + 1 = 0x4000`.
//! - In opcodes `0x10`-`0x1F` only the low 3 bits give the length (`0` = long form, `+9`;
//!   otherwise `+ 2`), and bit `0x08` adds a further `0x4000` to the distance. ODS writes the
//!   length as `(opcode & 0x0F) + 2`, which is only right for `0x12`-`0x17`. With bit `0x08` the
//!   distance reaches `0xBFFF`, the window of this format.
//!
//! Policy choices (ODS is silent):
//! - `decompress` produces exactly `expected_len` bytes. More is `Error::Corrupt`; fewer at the
//!   terminator is `Error::Corrupt`; input ending first is `Error::Truncated`. The terminator may
//!   be absent only when the output is already full. Bytes after the terminator are ignored
//!   (pages are padded).
//! - A literal run of 1-3 bytes cannot be the first run (the shortest encodable one is 4), so
//!   `compress` of a 1-3 byte input emits a 4-byte first run padded with zeros, and `decompress`
//!   accepts that one case: a first run of exactly 4 bytes with `expected_len < 4` is cut to
//!   `expected_len`.
//! - `compress` only emits distances up to 0x7FFF (ODS-documented forms); see `WINDOW`.
//! - `compress` is deterministic (greedy match, bounded hash chain, no randomness).

use crate::error::{Error, Result};

/// Largest distance the decoder accepts (`0x4000 + 0x3FFF + 0x4000`).
const DECODE_WINDOW: usize = 0xBFFF;
/// Largest distance `compress` emits. Distances above this need the opcode 0x18-0x1F forms
/// (bit 0x08), which ODS does not document, so the matcher never uses them. `encode` can still
/// write them (tests) and `decompress` reads them.
const WINDOW: usize = 0x7FFF;
const _: () = assert!(WINDOW <= DECODE_WINDOW);
/// Chain candidates examined per position.
const MAX_CHAIN: usize = 48;
const HASH_BITS: u32 = 15;

fn truncated() -> Error {
    Error::Truncated {
        context: "R2004 LZ77 stream",
    }
}

fn corrupt(msg: &str) -> Error {
    Error::Corrupt(format!("R2004 LZ77: {msg}"))
}

struct Reader<'a> {
    src: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn byte(&mut self) -> Result<u8> {
        let b = *self.src.get(self.pos).ok_or_else(truncated)?;
        self.pos += 1;
        Ok(b)
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    /// ODS "Literal Length", given its first byte (`< 0x10`).
    fn literal_length(&mut self, first: u8) -> Result<usize> {
        if first != 0 {
            return Ok(usize::from(first) + 3);
        }
        let mut total = 0x0F_usize;
        loop {
            let b = self.byte()?;
            if b != 0 {
                return Ok(total + usize::from(b) + 3);
            }
            total = total
                .checked_add(0xFF)
                .ok_or_else(|| corrupt("length overflow"))?;
        }
    }

    /// ODS "Long Compression Offset".
    fn long_count(&mut self) -> Result<usize> {
        let b = self.byte()?;
        if b != 0 {
            return Ok(usize::from(b));
        }
        let mut total = 0xFF_usize;
        loop {
            let b = self.byte()?;
            if b != 0 {
                return Ok(total + usize::from(b));
            }
            total = total
                .checked_add(0xFF)
                .ok_or_else(|| corrupt("length overflow"))?;
        }
    }

    /// ODS "Two Byte Offset": (offset field, inline literal count 0-3).
    fn two_byte_offset(&mut self) -> Result<(usize, usize)> {
        let b0 = self.byte()?;
        let b1 = self.byte()?;
        Ok((
            (usize::from(b0) >> 2) | (usize::from(b1) << 6),
            usize::from(b0 & 3),
        ))
    }
}

/// Copy `n` literal bytes to `out`, which may grow to `end` bytes.
fn copy_literals(r: &mut Reader, out: &mut Vec<u8>, n: usize, end: usize) -> Result<()> {
    if n > end - out.len() {
        return Err(corrupt("output exceeds expected length"));
    }
    let end = r.pos.checked_add(n).ok_or_else(truncated)?;
    let lit = r.src.get(r.pos..end).ok_or_else(truncated)?;
    out.extend_from_slice(lit);
    r.pos = end;
    Ok(())
}

/// Decompress an R2004-family LZ77 stream into exactly `expected_len` bytes.
///
/// Peak output is bounded by `expected_len`, not by the input size (a few bytes can expand to
/// `expected_len`), so callers must validate `expected_len` against the page header or the
/// section's maximum page size before calling.
pub fn decompress(src: &[u8], expected_len: usize) -> Result<Vec<u8>> {
    // Reserve conservatively: `expected_len` comes from the file, the output grows with real data.
    let mut out = Vec::with_capacity(expected_len.min(src.len().saturating_mul(8)));
    decompress_into(src, expected_len, &mut out)?;
    Ok(out)
}

/// [`decompress`], appending the `expected_len` bytes to `out` (a section
/// built page by page). Back-references reach only into this stream's
/// output, not into what `out` held before. On error `out` may hold part of
/// the output.
pub(crate) fn decompress_into(src: &[u8], expected_len: usize, out: &mut Vec<u8>) -> Result<()> {
    let mut r = Reader { src, pos: 0 };
    let base = out.len();
    let end = base
        .checked_add(expected_len)
        .ok_or_else(|| corrupt("output length overflow"))?;

    let mut opcode = r.byte()?;
    if opcode < 0x10 {
        let n = r.literal_length(opcode)?;
        if n == 4 && expected_len < 4 {
            // The padded first run `compress` writes for 1-3 byte inputs.
            copy_literals(&mut r, out, 4, base + 4)?;
            out.truncate(end);
        } else {
            copy_literals(&mut r, out, n, end)?;
        }
        match r.peek() {
            Some(_) => opcode = r.byte()?,
            None if out.len() == end => return Ok(()),
            None => return Err(truncated()),
        }
    }

    loop {
        if opcode == 0x11 {
            break;
        }
        let (len, dist, mut lit) = match opcode {
            0x00..=0x0F => return Err(corrupt("literal length where an opcode is required")),
            0x10..=0x1F => {
                let len = match opcode & 7 {
                    0 => r.long_count()? + 9,
                    n => usize::from(n) + 2,
                };
                let (off, lit) = r.two_byte_offset()?;
                (len, off + 0x4000 + (usize::from(opcode & 8) << 11), lit)
            }
            0x20 => {
                let len = r.long_count()? + 0x21;
                let (off, lit) = r.two_byte_offset()?;
                (len, off + 1, lit)
            }
            0x21..=0x3F => {
                let (off, lit) = r.two_byte_offset()?;
                (usize::from(opcode) - 0x1E, off + 1, lit)
            }
            0x40..=0xFF => {
                let op2 = usize::from(r.byte()?);
                let off = (op2 << 2) | usize::from((opcode & 0x0C) >> 2);
                (
                    usize::from(opcode >> 4) - 1,
                    off + 1,
                    usize::from(opcode & 3),
                )
            }
        };

        if dist > out.len() - base {
            return Err(corrupt("back-reference before start of output"));
        }
        if len > end - out.len() {
            return Err(corrupt("output exceeds expected length"));
        }
        super::copy_back(out, dist, len);

        if lit == 0 {
            // A byte with a high nibble is already the next opcode (literal length 0).
            match r.peek() {
                Some(b) if b < 0x10 => {
                    r.pos += 1;
                    lit = r.literal_length(b)?;
                }
                _ => {}
            }
        }
        copy_literals(&mut r, out, lit, end)?;

        match r.peek() {
            Some(_) => opcode = r.byte()?,
            None if out.len() == end => return Ok(()),
            None => return Err(truncated()),
        }
    }

    if out.len() == end {
        Ok(())
    } else {
        Err(corrupt("stream ended before expected length"))
    }
}

/// A back-reference chosen by the matcher: copy `len` bytes from `dist` back, at `pos`.
struct Match {
    pos: usize,
    len: usize,
    dist: usize,
}

fn hash3(b: &[u8]) -> usize {
    let v = u32::from(b[0]) | (u32::from(b[1]) << 8) | (u32::from(b[2]) << 16);
    (v.wrapping_mul(0x9E37_79B1) >> (32 - HASH_BITS)) as usize
}

/// Is a match of this length/distance worth emitting? Splitting a literal run costs up to a
/// length byte, so the shortest matches are skipped. The encoder itself handles lengths from 3
/// (4 beyond distance 0x4000, where `0x11` is the terminator).
fn usable(len: usize, dist: usize) -> bool {
    if dist <= 0x400 { len >= 4 } else { len >= 5 }
}

/// The matcher's hash chains: the last position of each hash, and each
/// position's previous one with the same hash. Kept by a [`Compressor`] so
/// that compressing many pages allocates them once.
#[derive(Debug, Default)]
struct Chains {
    head: Vec<u32>,
    prev: Vec<u32>,
}

impl Chains {
    /// Empty chains for an input of `n` bytes. `prev` needs no clearing: a
    /// position's entry is set when the position is inserted, before it is
    /// read.
    fn reset(&mut self, n: usize) -> (&mut [u32], &mut [u32]) {
        self.head.clear();
        self.head.resize(1 << HASH_BITS, u32::MAX);
        if self.prev.len() < n {
            self.prev.resize(n, u32::MAX);
        }
        (&mut self.head, &mut self.prev[..n])
    }
}

fn find_matches(src: &[u8], chains: &mut Chains) -> Vec<Match> {
    let n = src.len();
    let mut matches = Vec::new();
    if n < 8 {
        return matches;
    }
    debug_assert!(n < u32::MAX as usize, "positions are stored as u32");
    let (head, prev) = chains.reset(n);
    let insert = |head: &mut [u32], prev: &mut [u32], p: usize| {
        if p + 3 <= n {
            let h = hash3(&src[p..]);
            prev[p] = head[h];
            head[h] = p as u32;
        }
    };

    let mut pos = 0;
    while pos < n {
        let mut best: Option<(usize, usize)> = None; // (len, dist)
        // The first literal run must be empty or at least 4 bytes, so no match before pos 4.
        if pos >= 4 && pos + 3 <= n {
            let mut cand = head[hash3(&src[pos..])];
            let mut steps = 0;
            while cand != u32::MAX && steps < MAX_CHAIN {
                let c = cand as usize;
                let dist = pos - c;
                if dist > WINDOW {
                    break;
                }
                let mut l = 0;
                while pos + l < n && src[c + l] == src[pos + l] {
                    l += 1;
                }
                if usable(l, dist) && best.is_none_or(|(bl, _)| l > bl) {
                    best = Some((l, dist));
                    if pos + l == n {
                        break;
                    }
                }
                cand = prev[c];
                steps += 1;
            }
        }
        match best {
            Some((len, dist)) => {
                matches.push(Match { pos, len, dist });
                for p in pos..pos + len {
                    insert(head, prev, p);
                }
                pos += len;
            }
            None => {
                insert(head, prev, pos);
                pos += 1;
            }
        }
    }
    matches
}

fn put_literal_length(out: &mut Vec<u8>, n: usize) {
    debug_assert!(n >= 4);
    if n <= 0x12 {
        out.push((n - 3) as u8);
    } else {
        out.push(0);
        let rem = n - 0x12; // >= 1
        let zeros = (rem - 1) / 0xFF;
        out.extend(std::iter::repeat_n(0, zeros));
        out.push((rem - zeros * 0xFF) as u8);
    }
}

fn put_long_count(out: &mut Vec<u8>, v: usize) {
    debug_assert!(v >= 1);
    if v <= 0xFF {
        out.push(v as u8);
    } else {
        out.push(0);
        let rem = v - 0xFF; // >= 1
        let zeros = (rem - 1) / 0xFF;
        out.extend(std::iter::repeat_n(0, zeros));
        out.push((rem - zeros * 0xFF) as u8);
    }
}

fn put_two_byte(out: &mut Vec<u8>, off: usize, inline: usize) {
    out.push((((off & 0x3F) << 2) | inline) as u8);
    out.push((off >> 6) as u8);
}

/// Serialize `src` using the given (ascending, non-overlapping) matches.
fn encode(src: &[u8], matches: &[Match]) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len() / 2 + 16);
    let first_end = matches.first().map_or(src.len(), |m| m.pos);
    let first = &src[..first_end];
    match first.len() {
        0 => {}
        1..=3 => {
            // Not encodable on its own; pad to the 4-byte minimum (see module docs).
            out.push(1);
            out.extend_from_slice(first);
            out.extend(std::iter::repeat_n(0, 4 - first.len()));
        }
        n => {
            put_literal_length(&mut out, n);
            out.extend_from_slice(first);
        }
    }

    for (i, m) in matches.iter().enumerate() {
        let lit_end = matches.get(i + 1).map_or(src.len(), |next| next.pos);
        let lit = &src[m.pos + m.len..lit_end];
        let inline = if lit.len() <= 3 { lit.len() } else { 0 };

        if m.dist <= 0x400 && m.len <= 14 {
            let o = m.dist - 1;
            out.push((((m.len + 1) << 4) | ((o & 3) << 2) | inline) as u8);
            out.push((o >> 2) as u8);
        } else if m.dist <= 0x4000 {
            if m.len <= 33 {
                out.push((m.len + 0x1E) as u8);
            } else {
                out.push(0x20);
                put_long_count(&mut out, m.len - 0x21);
            }
            put_two_byte(&mut out, m.dist - 1, inline);
        } else {
            let mut off = m.dist - 0x4000;
            let mut op = 0x10;
            if off >= 0x4000 {
                op |= 8;
                off -= 0x4000;
            }
            if m.len <= 9 {
                out.push(op | (m.len - 2) as u8);
            } else {
                out.push(op);
                put_long_count(&mut out, m.len - 9);
            }
            put_two_byte(&mut out, off, inline);
        }

        if lit.len() > 3 {
            put_literal_length(&mut out, lit.len());
        }
        out.extend_from_slice(lit);
    }
    out.push(0x11);
    out
}

/// Compress into the R2004 LZ77 format. Deterministic.
pub fn compress(src: &[u8]) -> Vec<u8> {
    Compressor::new().compress(src)
}

/// A compressor whose matcher buffers (a 128 KiB hash table and a chain
/// entry per input byte) are reused across inputs: the R2018 writer
/// compresses every page of a file with one. Its output is
/// [`compress`]'s, byte for byte.
#[derive(Debug, Default)]
pub struct Compressor {
    chains: Chains,
}

impl Compressor {
    /// A compressor with no buffers yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Compress `src` (see [`compress`]).
    pub fn compress(&mut self, src: &[u8]) -> Vec<u8> {
        encode(src, &find_matches(src, &mut self.chains))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// Deterministic pseudo-random bytes (xorshift) for incompressible filler.
    fn noise(n: usize, mut s: u32) -> Vec<u8> {
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                (s >> 8) as u8
            })
            .collect()
    }

    /// A reused compressor gives the output of a fresh one, whatever the
    /// inputs before (longer, shorter, alike or not).
    #[test]
    fn reused_compressor_is_byte_identical() {
        let text: Vec<u8> = b"the quick brown fox jumps over the lazy dog. "
            .iter()
            .copied()
            .cycle()
            .take(0x7400)
            .collect();
        let mut mixed = noise(0x3000, 7);
        mixed.extend_from_slice(&text[..0x2000]);
        let inputs = [
            text.clone(),
            noise(0x7400, 1),
            text[..100].to_vec(),
            mixed,
            vec![0; 0x7400],
            noise(5, 3),
            text[7..0x5000].to_vec(),
        ];
        let mut c = Compressor::new();
        for x in &inputs {
            assert_eq!(c.compress(x), compress(x), "len {}", x.len());
        }
    }

    fn rt(x: &[u8]) {
        let c = compress(x);
        assert_eq!(decompress(&c, x.len()).unwrap(), x, "len {}", x.len());
    }

    proptest! {
        #[test]
        fn roundtrip_random(x in proptest::collection::vec(any::<u8>(), 0..65536)) {
            rt(&x);
        }

        #[test]
        fn roundtrip_low_entropy(x in proptest::collection::vec(0u8..4, 0..65536)) {
            rt(&x);
        }

        #[test]
        fn decompress_never_panics(
            x in proptest::collection::vec(any::<u8>(), 0..512),
            n in 0usize..4096,
        ) {
            let _ = decompress(&x, n);
        }
    }

    #[test]
    fn zeros_compress_small() {
        let z = vec![0u8; 0x7400];
        let c = compress(&z);
        assert!(c.len() < 200, "{}", c.len());
        assert_eq!(decompress(&c, z.len()).unwrap(), z);
    }

    #[test]
    fn tiny_inputs_roundtrip() {
        for n in 0..40 {
            rt(&noise(n, 7 + n as u32));
            rt(&vec![0x55; n]);
        }
    }

    #[test]
    fn compress_is_deterministic() {
        let mut x = noise(5000, 3);
        x.extend_from_within(100..3000);
        assert_eq!(compress(&x), compress(&x));
    }

    #[test]
    fn truncated_stream_errors() {
        let mut x = noise(300, 9);
        x.extend_from_within(0..200);
        let c = compress(&x);
        for cut in 0..c.len() - 1 {
            assert!(decompress(&c[..cut], x.len()).is_err(), "cut {cut}");
        }
    }

    #[test]
    fn backref_beyond_output_is_corrupt() {
        // Literal run of 4, then a 0x40-form opcode: len 3, offset field (1 << 2) = 4, so the
        // distance is 5, more than the 4 bytes produced.
        let s = [0x01, 1, 2, 3, 4, 0x41, 0x01, 0x11];
        assert!(matches!(decompress(&s, 7), Err(Error::Corrupt(_))));
    }

    #[test]
    fn invalid_opcode_is_corrupt() {
        let s = [0x01, 1, 2, 3, 4, 0x05, 0x11];
        assert!(matches!(decompress(&s, 4), Err(Error::Corrupt(_))));
    }

    #[test]
    fn output_over_expected_is_corrupt() {
        let c = compress(&[7u8; 100]);
        assert!(matches!(decompress(&c, 99), Err(Error::Corrupt(_))));
    }

    #[test]
    fn output_short_is_error() {
        let c = compress(&noise(100, 1));
        assert!(matches!(decompress(&c, 101), Err(Error::Corrupt(_))));
    }

    #[test]
    fn missing_terminator_ok_only_when_full() {
        let x = noise(50, 5);
        let mut c = compress(&x);
        assert_eq!(c.pop(), Some(0x11));
        assert_eq!(decompress(&c, 50).unwrap(), x);
        assert!(decompress(&c, 51).is_err());
    }

    /// Build `prefix(P) ++ match(dist, len)` and encode it with one forced match.
    fn forced(dist: usize, len: usize, trailing: usize) -> (Vec<u8>, Vec<u8>) {
        let p = dist.max(4);
        let mut src = noise(p, 99);
        for _ in 0..len {
            let b = src[src.len() - dist];
            src.push(b);
        }
        src.extend(noise(trailing, 1234));
        let enc = encode(&src, &[Match { pos: p, len, dist }]);
        (src, enc)
    }

    /// Opcode byte of the (single) match: it follows the initial literal run.
    fn first_opcode(enc: &[u8], p: usize) -> u8 {
        let lit_hdr = if p < 19 { 1 } else { 2 + (p - 19) / 255 };
        enc[lit_hdr + p]
    }

    #[test]
    fn opcode_form_boundaries() {
        // (dist, len, expected opcode)
        let cases: &[(usize, usize, u8)] = &[
            (1, 3, 0x40),
            (0x400, 3, 0x40 | (3 << 2)),
            (0x400, 14, 0xF0 | (3 << 2)),
            (0x400, 15, 0x21 + 12),
            (0x401, 3, 0x21),
            (0x401, 33, 0x3F),
            (0x401, 34, 0x20),
            (0x4000, 3, 0x21),
            (0x4000, 34, 0x20),
            (0x4001, 4, 0x12),
            (0x4001, 9, 0x17),
            (0x4001, 10, 0x10),
            (0x7FFF, 10, 0x10),
            (0x8000, 4, 0x1A),
            (0x8001, 4, 0x1A),
            (0x8001, 9, 0x1F),
            (0xBFFF, 4, 0x1A),
            (0xBFFF, 10, 0x18),
        ];
        for &(d, l, op) in cases {
            let (src, enc) = forced(d, l, 0);
            assert_eq!(first_opcode(&enc, d.max(4)), op, "dist {d:#x} len {l}");
            assert_eq!(
                decompress(&enc, src.len()).unwrap(),
                src,
                "dist {d:#x} len {l}"
            );
        }
    }

    #[test]
    fn match_edge_cases_roundtrip() {
        let dists = [
            1usize, 2, 3, 4, 5, 0x3FF, 0x400, 0x401, 0x402, 0x3FFF, 0x4000, 0x4001, 0x4002, 0x7FFF,
            0x8000, 0x8001, 0xBFFE, 0xBFFF,
        ];
        let lens = [
            3usize,
            4,
            5,
            8,
            9,
            10,
            11,
            14,
            15,
            16,
            33,
            34,
            35,
            9 + 254,
            9 + 255,
            9 + 256,
            9 + 510,
            9 + 511,
            0x21 + 254,
            0x21 + 255,
            0x21 + 256,
            0x21 + 600,
        ];
        let trailing = [0usize, 1, 2, 3, 4, 18, 19, 20, 18 + 255, 18 + 256, 18 + 511];
        for &d in &dists {
            for &l in &lens {
                if d > 0x4000 && l < 4 {
                    continue;
                }
                for &t in &trailing {
                    let (src, enc) = forced(d, l, t);
                    let got = decompress(&enc, src.len())
                        .unwrap_or_else(|e| panic!("d {d:#x} l {l} t {t}: {e}"));
                    assert_eq!(got, src, "d {d:#x} l {l} t {t}");
                }
            }
        }
    }

    #[test]
    fn initial_literal_lengths_roundtrip() {
        for n in [0usize, 4, 5, 18, 19, 20, 273, 274, 275, 528, 529, 1000] {
            let mut src = noise(n, 5);
            src.extend_from_within(0..n.min(8));
            rt(&src);
        }
    }

    #[test]
    fn compressor_finds_far_matches() {
        for d in [0x400usize, 0x401, 0x4000, 0x4001, 0x7FFF] {
            let pat = noise(32, 77);
            let mut src = pat.clone();
            src.extend(noise(d - 32, 4242));
            let base = compress(&src).len();
            src.extend(&pat);
            let c = compress(&src);
            // The repeated 32 bytes must cost one match (<= 6 bytes), not 32 literals.
            assert!(c.len() <= base + 6, "dist {d:#x}: {} vs {base}", c.len());
            assert_eq!(decompress(&c, src.len()).unwrap(), src);
        }
    }

    #[test]
    fn matcher_never_exceeds_ods_window() {
        // A repeat at 0xBFFF is only reachable with the undocumented forms: must stay literal.
        let pat = noise(32, 77);
        let mut src = pat.clone();
        src.extend(noise(0xBFFF - 32, 4242));
        src.extend(&pat);
        assert!(
            find_matches(&src, &mut Chains::default())
                .iter()
                .all(|m| m.dist <= 0x7FFF)
        );
        rt(&src);
    }

    #[test]
    fn mixed_realistic_data() {
        let mut x = Vec::new();
        for i in 0..4000u32 {
            x.extend_from_slice(&(i / 7).to_le_bytes());
            x.extend_from_slice(b"LINE");
        }
        rt(&x);
        assert!(compress(&x).len() < x.len() / 2);
    }
}
