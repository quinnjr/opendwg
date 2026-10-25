//! R2007 Reed-Solomon decoding (ODS section 5.13): de-interleave the 255-byte codewords and
//! correct byte errors. [`encode`] is the inverse, for tests and fuzz inputs (the writer emits
//! R2018 only).
//!
//! Code parameters (ODS 5.13). Both codes are RS(255, k) over GF(2^8) with generator roots
//! `alpha^1 ..= alpha^(255 - k)` (Simon Rockliff's convention, which ODS names as the reference
//! implementation) and `alpha = 2`:
//! - `k = 239` (system pages and the file header): field polynomial coefficients
//!   `(1, 0, 0, 1, 0, 1, 1, 0)`, i.e. `x^8 + x^6 + x^5 + x^3 + 1` (0x169); corrects 8 bytes.
//! - `k = 251` (data pages): coefficients `(1, 0, 1, 1, 1, 0, 0, 0)`, i.e.
//!   `x^8 + x^4 + x^3 + x^2 + 1` (0x11D); corrects 2 bytes.
//!
//! ODS lists the coefficients for `x^0 .. x^7` (Rockliff's `pp` array); `x^8` is implied.
//!
//! Byte layout of one codeword: the `k` data bytes, then the `255 - k` parity bytes. File byte `j`
//! is the coefficient of `x^j`. Rockliff stores parity at the low degrees instead; the file layout
//! is that codeword cyclically shifted by `k`, which is still a codeword because RS codes are
//! cyclic. Both codes were checked against a real AC1021 file (ACadSharp's `sample_AC1021.dwg`):
//! every codeword of the file header, the system pages and the encoding-4 data pages has all-zero
//! syndromes under these parameters.
//!
//! `factor` is the interleave stride, not the file header's `CorrectionFactor`. That header
//! field (`PagesMapCorrectionFactor`, `SectionsMapCorrectionFactor`) is a repeat count: a system
//! page stores its 8-byte-aligned data `CorrectionFactor` times before encoding (ODS 5.3). The
//! caller derives the block count from it, `ceil(aligned_size * repeat / k)` (ODS 5.3, with
//! `repeat = 1` for data pages), and passes that block count as `factor` for the interleaved
//! layout.
//!
//! Block layouts (ODS 5.13.1 / 5.13.2), selected by `factor`:
//! - `factor == block_count` (interleaved): byte `j` of codeword `i` is at `j * factor + i`.
//!   This matches ACadSharp (MIT) `DwgReader.reedSolomonDecoding`, consulted to confirm the
//!   reading of ODS 5.13.2 (see `THIRD_PARTY.md`); ACadSharp does not correct errors.
//! - `factor == 1` (non-interleaved): all data blocks back to back, then all parity blocks in the
//!   same order ("the first parity block follows the last data block"). With one block both
//!   layouts are the same.
//!
//! Policy choices (ODS is silent):
//! - Bytes after the `block_count * 255` encoded bytes are ignored (page padding).
//! - Any other `factor`, or a `data_size_per_block` other than 239/251, is `Error::Corrupt`.
//! - A codeword with more errors than the code corrects is `Error::Corrupt` when detected. RS can
//!   miscorrect past its limit: for `k = 239` that is rare (about 1 in 40,000 random 9-byte error
//!   patterns), but for `k = 251` about half of all 3-byte error patterns decode to a wrong
//!   codeword. The page CRCs (checked by the container reader) catch those.

use crate::error::{Error, Result};

/// Codeword length `n`.
const N: usize = 255;
/// Most parity bytes of either code (`255 - 239`).
const MAX_ROOTS: usize = 16;

fn corrupt(msg: &str) -> Error {
    Error::Corrupt(format!("R2007 Reed-Solomon: {msg}"))
}

/// GF(2^8) log/antilog tables for one field polynomial.
struct Field {
    /// `exp[i] = alpha^i`, doubled so a sum of two logs needs no reduction.
    exp: [u8; 2 * N],
    /// `log[alpha^i] = i`; `log[0]` is unused.
    log: [u8; 256],
}

impl Field {
    const fn new(poly: u16) -> Self {
        let mut exp = [0u8; 2 * N];
        let mut log = [0u8; 256];
        let mut x: u16 = 1;
        let mut i = 0;
        while i < N {
            exp[i] = x as u8;
            exp[i + N] = x as u8;
            log[x as usize] = i as u8;
            x <<= 1;
            if x & 0x100 != 0 {
                x ^= poly;
            }
            i += 1;
        }
        Field { exp, log }
    }

    fn mul(&self, a: u8, b: u8) -> u8 {
        if a == 0 || b == 0 {
            0
        } else {
            self.exp[usize::from(self.log[usize::from(a)]) + usize::from(self.log[usize::from(b)])]
        }
    }

    /// `a / b`; `b` must be non-zero.
    fn div(&self, a: u8, b: u8) -> u8 {
        if a == 0 {
            0
        } else {
            let la = usize::from(self.log[usize::from(a)]);
            let lb = usize::from(self.log[usize::from(b)]);
            self.exp[la + N - lb]
        }
    }

    /// `alpha^e` for any `e`.
    fn alpha_pow(&self, e: usize) -> u8 {
        self.exp[e % N]
    }

    /// Evaluate `p(x) = sum p[i] x^i` at `x`.
    fn eval(&self, p: &[u8], x: u8) -> u8 {
        p.iter().rev().fold(0, |acc, &c| self.mul(acc, x) ^ c)
    }
}

/// ODS 5.13 system-page code, pp = (1, 0, 0, 1, 0, 1, 1, 0).
static FIELD_239: Field = Field::new(0x169);
/// ODS 5.13 data-page code, pp = (1, 0, 1, 1, 1, 0, 0, 0).
static FIELD_251: Field = Field::new(0x11D);

fn field_for(k: usize) -> Result<&'static Field> {
    match k {
        239 => Ok(&FIELD_239),
        251 => Ok(&FIELD_251),
        _ => Err(corrupt("data size per block must be 239 or 251")),
    }
}

/// Syndromes `S_i = c(alpha^i)`, `i = 1 ..= roots`, stored at `s[i - 1]`. True if all are zero.
fn syndromes(f: &Field, cw: &[u8; N], roots: usize, s: &mut [u8; MAX_ROOTS]) -> bool {
    let mut clean = true;
    for (i, si) in s.iter_mut().enumerate().take(roots) {
        *si = f.eval(cw, f.alpha_pow(i + 1));
        clean &= *si == 0;
    }
    clean
}

/// Correct one codeword in place (file byte `j` = coefficient of `x^j`).
fn correct(f: &Field, cw: &mut [u8; N], roots: usize) -> Result<()> {
    let mut s = [0u8; MAX_ROOTS];
    if syndromes(f, cw, roots, &mut s) {
        return Ok(());
    }

    // Berlekamp-Massey: error locator `lambda(x) = prod (1 - X_k x)`, degree `l`.
    let mut lambda = [0u8; MAX_ROOTS + 1];
    let mut prev = [0u8; MAX_ROOTS + 1];
    lambda[0] = 1;
    prev[0] = 1;
    let mut l = 0usize;
    let mut m = 1usize;
    let mut b = 1u8;
    for n in 0..roots {
        let mut d = s[n];
        for i in 1..=l {
            d ^= f.mul(lambda[i], s[n - i]);
        }
        if d == 0 {
            m += 1;
            continue;
        }
        let coef = f.div(d, b);
        let saved = lambda;
        for i in m..=MAX_ROOTS {
            lambda[i] ^= f.mul(coef, prev[i - m]);
        }
        if 2 * l <= n {
            l = n + 1 - l;
            prev = saved;
            b = d;
            m = 1;
        } else {
            m += 1;
        }
    }
    if l > roots / 2 {
        return Err(corrupt("uncorrectable codeword"));
    }

    // Chien search: position `j` is in error when `lambda(alpha^-j) = 0`.
    let mut positions = [0usize; MAX_ROOTS / 2];
    let mut found = 0;
    for j in 0..N {
        if f.eval(&lambda[..=l], f.alpha_pow(N - j)) == 0 {
            let slot = positions
                .get_mut(found)
                .ok_or_else(|| corrupt("uncorrectable codeword"))?;
            *slot = j;
            found += 1;
        }
    }
    if found != l {
        return Err(corrupt("uncorrectable codeword"));
    }

    // Forney (first root alpha^1): e = omega(X^-1) / lambda'(X^-1), with
    // omega(x) = S(x) lambda(x) mod x^roots and S(x) = sum S_(i+1) x^i.
    let mut omega = [0u8; MAX_ROOTS];
    for i in 0..roots {
        for j in 0..=i.min(l) {
            omega[i] ^= f.mul(s[i - j], lambda[j]);
        }
    }
    // Formal derivative: only odd powers survive in characteristic 2.
    let mut dlambda = [0u8; MAX_ROOTS];
    for i in (1..=l).step_by(2) {
        dlambda[i - 1] = lambda[i];
    }
    for &j in &positions[..found] {
        let x_inv = f.alpha_pow(N - j);
        let den = f.eval(&dlambda[..l.max(1)], x_inv);
        if den == 0 {
            return Err(corrupt("uncorrectable codeword"));
        }
        cw[j] ^= f.div(f.eval(&omega[..roots], x_inv), den);
    }

    if syndromes(f, cw, roots, &mut s) {
        Ok(())
    } else {
        Err(corrupt("uncorrectable codeword"))
    }
}

/// Generator `g(x) = prod_(i=1..=roots) (x - alpha^i)`, `g[i]` = coefficient of `x^i`.
fn generator(f: &Field, roots: usize) -> Vec<u8> {
    let mut g = vec![1u8];
    for i in 1..=roots {
        let a = f.alpha_pow(i);
        let mut next = vec![0u8; g.len() + 1];
        for (d, &c) in g.iter().enumerate() {
            next[d + 1] ^= c;
            next[d] ^= f.mul(c, a);
        }
        g = next;
    }
    g
}

/// One systematic codeword (Rockliff's LFSR, same field and generator as [`decode`]): parity =
/// `(sum data[j] x^(j + roots)) mod g(x)`; the layout is data then parity. `data` is `k` bytes.
fn encode_block(f: &Field, g: &[u8], data: &[u8], k: usize) -> [u8; N] {
    let roots = N - k;
    let mut reg = vec![0u8; roots];
    for &d in data.iter().rev() {
        let fb = d ^ reg[roots - 1];
        for i in (1..roots).rev() {
            reg[i] = reg[i - 1] ^ f.mul(fb, g[i]);
        }
        reg[0] = f.mul(fb, g[0]);
    }
    let mut cw = [0u8; N];
    cw[..k].copy_from_slice(data);
    cw[k..].copy_from_slice(&reg);
    cw
}

/// Encode `data` as RS(255, `k`) codewords, the inverse of [`decode`]: `data` is split into
/// `k`-byte blocks (the last one zero-padded); `interleaved` selects the layout (see the module
/// docs). The result holds `ceil(len / k)` blocks of 255 bytes, at least one. `k` must be 239
/// or 251. The writer (R2018 only) does not use it; it makes test and fuzz inputs.
#[doc(hidden)]
pub fn encode(data: &[u8], k: usize, interleaved: bool) -> Result<Vec<u8>> {
    let f = field_for(k)?;
    let g = generator(f, N - k);
    let blocks: Vec<[u8; N]> = if data.is_empty() {
        vec![encode_block(f, &g, &vec![0; k], k)]
    } else {
        data.chunks(k)
            .map(|c| {
                let mut d = c.to_vec();
                d.resize(k, 0);
                encode_block(f, &g, &d, k)
            })
            .collect()
    };
    let n = blocks.len();
    let mut out = vec![0u8; n * N];
    if interleaved {
        for (i, b) in blocks.iter().enumerate() {
            for (j, &x) in b.iter().enumerate() {
                out[j * n + i] = x;
            }
        }
    } else {
        let roots = N - k;
        for (i, b) in blocks.iter().enumerate() {
            out[i * k..(i + 1) * k].copy_from_slice(&b[..k]);
            let parity = n * k + i * roots;
            out[parity..parity + roots].copy_from_slice(&b[k..]);
        }
    }
    Ok(out)
}

/// [`encode`] with the interleaved layout, for tests.
#[cfg(test)]
pub(crate) fn encode_interleaved(data: &[u8], k: usize) -> Vec<u8> {
    encode(data, k, true).unwrap()
}

/// De-interleave and error-correct `block_count` RS(255, `data_size_per_block`) codewords and
/// return their data bytes, block after block (`block_count * data_size_per_block` bytes).
///
/// `factor` selects the layout: `block_count` for interleaved, `1` for non-interleaved (see the
/// module docs). `src` must hold at least `block_count * 255` bytes; the rest is ignored.
pub fn decode(
    src: &[u8],
    block_count: usize,
    data_size_per_block: usize,
    factor: usize,
) -> Result<Vec<u8>> {
    decode_first(src, block_count, data_size_per_block, factor, block_count)
}

/// [`decode`], correcting and returning only the first `wanted` codewords
/// (at most `block_count`): a system page repeats its data, and only the
/// first copy is used.
pub(crate) fn decode_first(
    src: &[u8],
    block_count: usize,
    data_size_per_block: usize,
    factor: usize,
    wanted: usize,
) -> Result<Vec<u8>> {
    let wanted = wanted.min(block_count);
    let k = data_size_per_block;
    let f = field_for(k)?;
    let roots = N - k;
    let interleaved = factor == block_count;
    if !interleaved && factor != 1 {
        return Err(corrupt("factor must be 1 or the block count"));
    }
    let encoded_len = block_count
        .checked_mul(N)
        .ok_or_else(|| corrupt("block count overflow"))?;
    if src.len() < encoded_len {
        return Err(Error::Truncated {
            context: "R2007 Reed-Solomon blocks",
        });
    }

    let mut out = Vec::with_capacity(wanted * k);
    let mut cw = [0u8; N];
    for i in 0..wanted {
        if interleaved {
            for (j, b) in cw.iter_mut().enumerate() {
                *b = src[j * block_count + i];
            }
        } else {
            let data = i * k;
            cw[..k].copy_from_slice(&src[data..data + k]);
            let parity = block_count * k + i * roots;
            cw[k..].copy_from_slice(&src[parity..parity + roots]);
        }
        correct(f, &mut cw, roots)?;
        out.extend_from_slice(&cw[..k]);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// One codeword of `data` (`k` bytes).
    fn encode_block(data: &[u8], k: usize) -> [u8; N] {
        let f = field_for(k).unwrap();
        super::encode_block(f, &generator(f, N - k), data, k)
    }

    fn interleave(blocks: &[[u8; N]]) -> Vec<u8> {
        let n = blocks.len();
        let mut out = vec![0u8; n * N];
        for (i, b) in blocks.iter().enumerate() {
            for (j, &x) in b.iter().enumerate() {
                out[j * n + i] = x;
            }
        }
        out
    }

    #[test]
    fn encode_both_layouts_roundtrip() {
        for k in [239, 251] {
            for len in [0, 1, k - 1, k, k + 1, 3 * k + 5] {
                let data = noise(len, len as u32 + 3);
                let n = len.div_ceil(k).max(1);
                let mut padded = data.clone();
                padded.resize(n * k, 0);
                for (interleaved, factor) in [(true, n), (false, 1)] {
                    let src = encode(&data, k, interleaved).unwrap();
                    assert_eq!(src.len(), n * N);
                    assert_eq!(decode(&src, n, k, factor).unwrap(), padded, "{k} {len}");
                }
            }
        }
        assert!(encode(b"x", 200, true).is_err());
    }

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

    /// Flip `count` distinct bytes (spread over data and parity) of a codeword.
    fn damage(cw: &mut [u8; N], count: usize, seed: usize) {
        for e in 0..count {
            let pos = (seed * 37 + e * 31) % N;
            cw[pos] ^= (e as u8).wrapping_mul(29) | 1;
        }
    }

    #[test]
    fn encoder_output_is_a_codeword() {
        for k in [239, 251] {
            let cw = encode_block(&noise(k, 7), k);
            let mut s = [0u8; MAX_ROOTS];
            assert!(syndromes(field_for(k).unwrap(), &cw, N - k, &mut s));
        }
    }

    #[test]
    fn clean_roundtrip() {
        for k in [239, 251] {
            let data = noise(3 * k, 11);
            let blocks: Vec<_> = data.chunks(k).map(|c| encode_block(c, k)).collect();
            assert_eq!(decode(&interleave(&blocks), 3, k, 3).unwrap(), data);
        }
    }

    /// Decoding the first codewords gives the first blocks' data, whatever
    /// the later codewords hold (here: damaged beyond correction).
    #[test]
    fn decode_first_codewords_only() {
        let k = 239;
        let data = noise(3 * k, 5);
        let mut blocks: Vec<_> = data.chunks(k).map(|c| encode_block(c, k)).collect();
        damage(&mut blocks[2], 40, 9);
        let src = interleave(&blocks);
        assert!(decode(&src, 3, k, 3).is_err());
        assert_eq!(decode_first(&src, 3, k, 3, 2).unwrap(), data[..2 * k]);
        assert_eq!(decode_first(&src, 3, k, 3, 0).unwrap(), []);
    }

    #[test]
    fn corrects_eight_errors_per_codeword_239() {
        let k = 239;
        let data = noise(3 * k, 3);
        let mut blocks: Vec<_> = data.chunks(k).map(|c| encode_block(c, k)).collect();
        for (i, b) in blocks.iter_mut().enumerate() {
            damage(b, 8, i + 1);
        }
        assert_eq!(decode(&interleave(&blocks), 3, k, 3).unwrap(), data);
    }

    #[test]
    fn corrects_two_errors_per_codeword_251() {
        let k = 251;
        let data = noise(2 * k, 5);
        let mut blocks: Vec<_> = data.chunks(k).map(|c| encode_block(c, k)).collect();
        for (i, b) in blocks.iter_mut().enumerate() {
            damage(b, 2, i + 4);
        }
        assert_eq!(decode(&interleave(&blocks), 2, k, 2).unwrap(), data);
    }

    #[test]
    fn corrects_errors_in_parity_only() {
        let k = 239;
        let data = noise(k, 9);
        let mut cw = encode_block(&data, k);
        for b in &mut cw[k..k + 8] {
            *b ^= 0x5A;
        }
        assert_eq!(decode(&cw, 1, k, 1).unwrap(), data);
    }

    #[test]
    fn nine_errors_is_err() {
        let k = 239;
        let data = noise(3 * k, 13);
        let mut blocks: Vec<_> = data.chunks(k).map(|c| encode_block(c, k)).collect();
        damage(&mut blocks[1], 9, 2);
        assert!(matches!(
            decode(&interleave(&blocks), 3, k, 3),
            Err(Error::Corrupt(_))
        ));
    }

    #[test]
    fn interleaving_three_blocks_restores_order() {
        // Blocks filled with 0xA0, 0xB0, 0xC0 so a stride mix-up shows in the output.
        let k = 239;
        let blocks: Vec<_> = [0xA0u8, 0xB0, 0xC0]
            .iter()
            .map(|&v| encode_block(&vec![v; k], k))
            .collect();
        let src = interleave(&blocks);
        assert_eq!(&src[..3], &[0xA0, 0xB0, 0xC0]);
        let out = decode(&src, 3, k, 3).unwrap();
        assert!(out[..k].iter().all(|&b| b == 0xA0));
        assert!(out[k..2 * k].iter().all(|&b| b == 0xB0));
        assert!(out[2 * k..].iter().all(|&b| b == 0xC0));
    }

    #[test]
    fn non_interleaved_layout_data_then_parity() {
        let k = 251;
        let data = noise(3 * k, 17);
        let blocks: Vec<_> = data.chunks(k).map(|c| encode_block(c, k)).collect();
        let mut src = Vec::new();
        for b in &blocks {
            src.extend_from_slice(&b[..k]);
        }
        for b in &blocks {
            src.extend_from_slice(&b[k..]);
        }
        src[5] ^= 0xFF; // one error in block 0 data
        src[3 * k + 4 + 1] ^= 0x10; // one error in block 1 parity
        assert_eq!(decode(&src, 3, k, 1).unwrap(), data);
    }

    #[test]
    fn trailing_padding_is_ignored() {
        let k = 239;
        let data = noise(k, 21);
        let mut src = encode_block(&data, k).to_vec();
        src.extend_from_slice(&[0xEE; 40]);
        assert_eq!(decode(&src, 1, k, 1).unwrap(), data);
    }

    #[test]
    fn short_input_is_truncated() {
        let k = 239;
        let blocks: Vec<_> = (0..3).map(|i| encode_block(&noise(k, i + 1), k)).collect();
        let src = interleave(&blocks);
        assert!(matches!(
            decode(&src[..src.len() - 1], 3, k, 3),
            Err(Error::Truncated { .. })
        ));
    }

    #[test]
    fn bad_parameters_are_errors() {
        let src = [0u8; 4 * N];
        assert!(matches!(decode(&src, 1, 240, 1), Err(Error::Corrupt(_))));
        assert!(matches!(decode(&src, 4, 239, 2), Err(Error::Corrupt(_))));
        assert!(matches!(
            decode(&src, usize::MAX, 239, 1),
            Err(Error::Corrupt(_))
        ));
        assert_eq!(decode(&src, 0, 239, 0).unwrap(), Vec::<u8>::new());
    }

    proptest! {
        #[test]
        fn corrects_any_pattern_within_capacity(
            seed in any::<u32>(),
            use_239 in any::<bool>(),
            errs in proptest::collection::btree_map(0usize..N, 1u8..=255, 0..=8),
        ) {
            let k = if use_239 { 239 } else { 251 };
            let t = (N - k) / 2;
            let data = noise(k, seed | 1);
            let mut cw = encode_block(&data, k);
            for (&pos, &v) in errs.iter().take(t) {
                cw[pos] ^= v;
            }
            prop_assert_eq!(decode(&cw, 1, k, 1).unwrap(), data);
        }

        #[test]
        fn never_panics(src in proptest::collection::vec(any::<u8>(), 0..800), blocks in 0usize..4) {
            let _ = decode(&src, blocks, 239, blocks);
            let _ = decode(&src, blocks, 251, 1);
        }
    }
}
