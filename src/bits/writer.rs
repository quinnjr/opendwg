//! `BitWriter`: MSB-first bit sink, the inverse of `BitReader`.
//!
//! Every `write_*` picks the shortest encoding the reader accepts. Values the
//! format cannot represent (over-long strings, out-of-range modular numbers,
//! colors that need a newer version) return `Error::Encode` instead of
//! writing something different from what was asked.

use super::Color;
use super::color::{METHOD_ACI, METHOD_BY_BLOCK, METHOD_BY_LAYER, METHOD_NONE, METHOD_RGB};
use crate::error::{Error, Result};
use crate::handle::HandleRef;
use crate::version::Version;

/// An [`Error::Encode`], whose message gets its own "cannot encode: "
/// prefix from the error's display.
fn unencodable(what: &str) -> Error {
    Error::Encode(format!("bitstream: {what}"))
}

/// Writes DWG bit-coded primitives. Bits are MSB-first within each byte and
/// multi-byte raw values are little-endian.
#[derive(Debug, Clone, Default)]
pub struct BitWriter {
    buf: Vec<u8>,
    bit_len: u64,
}

impl BitWriter {
    /// An empty writer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of bits written so far.
    pub fn bit_len(&self) -> u64 {
        self.bit_len
    }

    /// The written bits, zero-padded to a whole byte.
    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    /// Append the low `n` (<= 64) bits of `value`, MSB-first.
    fn put_bits(&mut self, value: u64, n: u32) {
        debug_assert!(n <= 64);
        let mut left = n;
        while left > 0 {
            let used = (self.bit_len % 8) as u32;
            if used == 0 {
                self.buf.push(0);
            }
            let take = (8 - used).min(left);
            let chunk = ((value >> (left - take)) & ((1u64 << take) - 1)) as u8;
            if let Some(last) = self.buf.last_mut() {
                *last |= chunk << (8 - used - take);
            }
            self.bit_len += u64::from(take);
            left -= take;
        }
    }

    /// Copy the first `bit_len` bits of `data` (for splicing raw streams).
    pub fn write_bits_from(&mut self, data: &[u8], bit_len: u64) -> Result<()> {
        if bit_len > (data.len() as u64) * 8 {
            return Err(unencodable("bit length exceeds the source data"));
        }
        let whole = (bit_len / 8) as usize;
        self.write_bytes(&data[..whole]);
        let rest = (bit_len % 8) as u32;
        if rest > 0 {
            self.put_bits(u64::from(data[whole] >> (8 - rest)), rest);
        }
        Ok(())
    }

    /// Overwrite 4 already-written, byte-aligned bytes at `byte_pos` with an RL.
    pub fn patch_rl_at(&mut self, byte_pos: usize, v: i32) -> Result<()> {
        self.patch(byte_pos, &v.to_le_bytes())
    }

    /// Overwrite 2 already-written, byte-aligned bytes at `byte_pos` with an RS.
    pub fn patch_rs_at(&mut self, byte_pos: usize, v: i16) -> Result<()> {
        self.patch(byte_pos, &v.to_le_bytes())
    }

    fn patch(&mut self, byte_pos: usize, bytes: &[u8]) -> Result<()> {
        let end = byte_pos
            .checked_add(bytes.len())
            .ok_or_else(|| unencodable("patch position overflow"))?;
        // Only fully written bytes may be patched, never a partial last byte.
        if (end as u64) * 8 > self.bit_len {
            return Err(unencodable("patch beyond written data"));
        }
        self.buf[byte_pos..end].copy_from_slice(bytes);
        Ok(())
    }

    // ---- bit-coded primitives ----

    /// B: one bit.
    pub fn write_b(&mut self, v: bool) {
        self.put_bits(u64::from(v), 1);
    }

    /// BB: the low two bits of `v`.
    pub fn write_bb(&mut self, v: u8) {
        self.put_bits(u64::from(v & 3), 2);
    }

    /// 3B. Only 0, 2, 6 and 7 exist (`0`, `10`, `110`, `111`).
    pub fn write_3b(&mut self, v: u8) -> Result<()> {
        let (bits, n) = match v {
            0 => (0b0, 1),
            2 => (0b10, 2),
            6 => (0b110, 3),
            7 => (0b111, 3),
            _ => return Err(unencodable("3B value must be 0, 2, 6 or 7")),
        };
        self.put_bits(bits, n);
        Ok(())
    }

    /// BS: a bitshort.
    pub fn write_bs(&mut self, v: i16) {
        match v {
            0 => self.write_bb(2),
            256 => self.write_bb(3),
            1..=255 => {
                self.write_bb(1);
                self.write_rc(v as u8);
            }
            _ => {
                self.write_bb(0);
                self.write_rs(v);
            }
        }
    }

    /// BL: a bitlong.
    pub fn write_bl(&mut self, v: i32) {
        match v {
            0 => self.write_bb(2),
            1..=255 => {
                self.write_bb(1);
                self.write_rc(v as u8);
            }
            _ => {
                self.write_bb(0);
                self.write_rl(v);
            }
        }
    }

    /// BITLONGLONG: a 3-bit byte count, then the bytes (see
    /// `BitReader::read_bll`). Values of 2^56 or more need 8 bytes, which the
    /// count cannot express, and are rejected.
    pub fn write_bll(&mut self, v: u64) -> Result<()> {
        let n = (u64::BITS - v.leading_zeros()).div_ceil(8);
        if n > 7 {
            return Err(unencodable("BITLONGLONG needs more than 7 bytes"));
        }
        self.put_bits(u64::from(n), 3);
        for i in 0..n {
            self.write_rc((v >> (8 * i)) as u8);
        }
        Ok(())
    }

    /// BD: a bitdouble.
    pub fn write_bd(&mut self, v: f64) {
        match v.to_bits() {
            b if b == 1.0f64.to_bits() => self.write_bb(1),
            0 => self.write_bb(2),
            _ => {
                self.write_bb(0);
                self.write_rd(v);
            }
        }
    }

    // ---- raw primitives ----

    /// RC: a raw byte.
    pub fn write_rc(&mut self, v: u8) {
        self.put_bits(u64::from(v), 8);
    }

    /// RS: a raw little-endian short.
    pub fn write_rs(&mut self, v: i16) {
        self.write_bytes(&v.to_le_bytes());
    }

    /// RL: a raw little-endian long.
    pub fn write_rl(&mut self, v: i32) {
        self.write_bytes(&v.to_le_bytes());
    }

    /// RD: a raw little-endian double.
    pub fn write_rd(&mut self, v: f64) {
        self.write_bytes(&v.to_le_bytes());
    }

    /// Append raw bytes (at any bit alignment).
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        if self.bit_len % 8 == 0 {
            self.buf.extend_from_slice(bytes);
            self.bit_len += (bytes.len() as u64) * 8;
        } else {
            for &b in bytes {
                self.write_rc(b);
            }
        }
    }

    // ---- modular ----

    /// Modular char (canonical: fewest bytes). Magnitudes must be below 2^62.
    pub fn write_mc(&mut self, v: i64) -> Result<()> {
        let mut m = v.unsigned_abs();
        if m >= 1 << 62 {
            return Err(unencodable("modular char magnitude must be below 2^62"));
        }
        while m >= 0x40 {
            self.write_rc((m & 0x7F) as u8 | 0x80);
            m >>= 7;
        }
        self.write_rc(m as u8 | if v < 0 { 0x40 } else { 0 });
        Ok(())
    }

    /// Unsigned modular char: 7 bits per byte, no sign.
    pub fn write_umc(&mut self, mut v: u64) {
        while v >= 0x80 {
            self.write_rc((v & 0x7F) as u8 | 0x80);
            v >>= 7;
        }
        self.write_rc(v as u8);
    }

    /// Modular short: 15 bits per little-endian short.
    pub fn write_ms(&mut self, mut v: u32) {
        while v >= 0x8000 {
            self.write_rs((v & 0x7FFF | 0x8000) as u16 as i16);
            v >>= 15;
        }
        self.write_rs(v as u16 as i16);
    }

    // ---- composite types ----

    /// BitDouble with default (ODS 2.9): 00 (same), 01 (low 4 bytes differ),
    /// 10 (low 6 bytes differ), 11 (full RD).
    pub fn write_dd(&mut self, v: f64, default: f64) {
        let (b, d) = (v.to_le_bytes(), default.to_le_bytes());
        if b == d {
            self.write_bb(0);
        } else if b[4..] == d[4..] {
            self.write_bb(1);
            self.write_bytes(&b[..4]);
        } else if b[6..] == d[6..] {
            self.write_bb(2);
            self.write_bytes(&b[4..6]);
            self.write_bytes(&b[..4]);
        } else {
            self.write_bb(3);
            self.write_bytes(&b);
        }
    }

    /// BitThickness (R2000+): a set bit means 0.0, otherwise a BD follows.
    pub fn write_bt(&mut self, v: f64) {
        if v.to_bits() == 0 {
            self.write_b(true);
        } else {
            self.write_b(false);
            self.write_bd(v);
        }
    }

    /// BitExtrusion (R2000+): a set bit means (0,0,1), otherwise 3BD follows.
    pub fn write_be(&mut self, v: [f64; 3]) {
        if v.map(f64::to_bits) == [0, 0, 1.0f64.to_bits()] {
            self.write_b(true);
        } else {
            self.write_b(false);
            self.write_3bd(v);
        }
    }

    /// 2RD: two raw doubles.
    pub fn write_2rd(&mut self, v: [f64; 2]) {
        v.into_iter().for_each(|x| self.write_rd(x));
    }

    /// 2DD: two doubles, each coded against `default`.
    pub fn write_2dd(&mut self, v: [f64; 2], default: [f64; 2]) {
        self.write_dd(v[0], default[0]);
        self.write_dd(v[1], default[1]);
    }

    /// 3BD: three bitdoubles.
    pub fn write_3bd(&mut self, v: [f64; 3]) {
        v.into_iter().for_each(|x| self.write_bd(x));
    }

    /// 3RD: three raw doubles.
    pub fn write_3rd(&mut self, v: [f64; 3]) {
        v.into_iter().for_each(|x| self.write_rd(x));
    }

    /// Object type (ODS 2.12): BS before R2010, a bit pair plus 1-2 bytes after.
    pub fn write_ot(&mut self, v: u16, version: Version) {
        if version < Version::R2010 {
            self.write_bs(v as i16);
        } else if v <= 0xFF {
            self.write_bb(0);
            self.write_rc(v as u8);
        } else if (0x1F0..=0x1F0 + 0xFF).contains(&v) {
            self.write_bb(1);
            self.write_rc((v - 0x1F0) as u8);
        } else {
            self.write_bb(2);
            self.write_bytes(&v.to_le_bytes());
        }
    }

    /// Handle reference: RC (code << 4 | counter) then big-endian bytes.
    pub fn write_handle_ref(&mut self, h: HandleRef) -> Result<()> {
        if h.code > 0x0F {
            return Err(unencodable("handle code must fit in 4 bits"));
        }
        let counter = (u64::BITS - h.value.leading_zeros()).div_ceil(8) as u8;
        self.write_rc(h.code << 4 | counter);
        let be = h.value.to_be_bytes();
        self.write_bytes(&be[8 - usize::from(counter)..]);
        Ok(())
    }

    // ---- strings ----

    /// Text bytes: BS length then the raw bytes (already encoded).
    pub fn write_tv_raw(&mut self, bytes: &[u8]) -> Result<()> {
        let len =
            i16::try_from(bytes.len()).map_err(|_| unencodable("text longer than 32767 bytes"))?;
        self.write_bs(len);
        self.write_bytes(bytes);
        Ok(())
    }

    /// Unicode text: BS UTF-16 unit count then UTF-16LE code units.
    pub fn write_tu(&mut self, s: &str) -> Result<()> {
        let units: Vec<u16> = s.encode_utf16().collect();
        let len = i16::try_from(units.len())
            .map_err(|_| unencodable("text longer than 32767 UTF-16 units"))?;
        self.write_bs(len);
        for u in units {
            self.write_bytes(&u.to_le_bytes());
        }
        Ok(())
    }

    fn write_color_text(&mut self, s: &str, version: Version) -> Result<()> {
        if version >= Version::R2007 {
            return self.write_tu(s);
        }
        // Mirrors the reader: one byte per character (Latin-1 range).
        let raw: Vec<u8> = s
            .chars()
            .map(|c| {
                u8::try_from(u32::from(c)).map_err(|_| unencodable("color name is not Latin-1"))
            })
            .collect::<Result<_>>()?;
        self.write_tv_raw(&raw)
    }

    // ---- colors ----

    /// CMC. Before R2004 a BS ACI index, afterwards BS 0, a BL RGB carrying
    /// the color-method byte, and RC name flags. Mirrors `read_cmc`.
    pub fn write_cmc(&mut self, color: &Color, version: Version) -> Result<()> {
        self.write_cmc_layer(color, false, version)
    }

    /// CMC as stored on a LAYER. Before R2004 `layer_off` is stored as a
    /// negative ACI index; R2004+ CMC cannot carry it.
    pub fn write_cmc_layer(
        &mut self,
        color: &Color,
        layer_off: bool,
        version: Version,
    ) -> Result<()> {
        if version < Version::R2004 {
            let aci = match color {
                Color::ByLayer => 256,
                Color::ByBlock => 0,
                Color::Index(n) if *n >= 1 => i16::from(*n),
                _ => return Err(unencodable("color needs R2004 or later")),
            };
            if layer_off && aci == 0 {
                return Err(unencodable("a BYBLOCK layer color cannot be marked off"));
            }
            self.write_bs(if layer_off { -aci } else { aci });
            return Ok(());
        }
        if layer_off {
            return Err(unencodable("layer-off is not stored in R2004+ CMC"));
        }
        let (rgb, color_name, book_name) = cmc_parts(color)?;
        self.write_cmc_head(rgb, color_name, book_name);
        if !color_name.is_empty() {
            self.write_color_text(color_name, version)?;
        }
        if !book_name.is_empty() {
            self.write_color_text(book_name, version)?;
        }
        Ok(())
    }

    /// The R2004+ CMC fields before the names (see `cmc_parts`): BS 0, the BL
    /// RGB long and the RC name flags.
    pub(crate) fn write_cmc_head(&mut self, rgb: u32, color_name: &str, book_name: &str) {
        let flags = u8::from(!color_name.is_empty()) | u8::from(!book_name.is_empty()) << 1;
        self.write_bs(0);
        self.write_bl(rgb as i32);
        self.write_rc(flags);
    }

    /// ENC. Mirrors `read_enc`: a flag word (0x8000 RGB, 0x4000 book handle
    /// follows, 0x2000 transparency, low 9 bits ACI), then the BL RGB and BL
    /// transparency when flagged. A book color is written as 0xC000 plus an
    /// ACI color, with no RGB long. Before R2004 only a plain ACI index exists.
    pub fn write_enc(
        &mut self,
        color: &Color,
        transparency: Option<u32>,
        book_handle_follows: bool,
        version: Version,
    ) -> Result<()> {
        self.write_enc_with_aci(color, transparency, book_handle_follows, 0, version)
    }

    /// [`Self::write_enc`] with the ACI index to store beside a true color
    /// (see `BitReader::read_enc_with_aci`); ignored for other colors. The
    /// RGB long carries the true-color method byte 0xC2, as AutoCAD writes it.
    pub fn write_enc_with_aci(
        &mut self,
        color: &Color,
        transparency: Option<u32>,
        book_handle_follows: bool,
        rgb_aci: u16,
        version: Version,
    ) -> Result<()> {
        if matches!(color, Color::Rgb { .. }) && rgb_aci > 0x01FF {
            return Err(unencodable("an ENC color index is 9 bits"));
        }
        let aci = match color {
            Color::ByLayer => Some(256u16),
            Color::ByBlock => Some(0),
            Color::Index(n) if *n >= 1 => Some(u16::from(*n)),
            Color::Rgb { .. } => None,
            _ => return Err(unencodable("color cannot be written as an entity color")),
        };
        if version < Version::R2004 {
            let Some(aci) = aci else {
                return Err(unencodable("true color needs R2004 or later"));
            };
            if transparency.is_some() || book_handle_follows {
                return Err(unencodable(
                    "transparency and color books need R2004 or later",
                ));
            }
            self.write_bs(aci as i16);
            return Ok(());
        }
        let mut word = aci.unwrap_or(0x8000 | rgb_aci);
        if book_handle_follows {
            // As AutoCAD writes it: 0x8000 too, but no RGB long (see read_enc).
            if aci.is_none() {
                return Err(unencodable(
                    "a book color's entity color must be an ACI color",
                ));
            }
            word |= 0xC000;
        }
        if transparency.is_some() {
            word |= 0x2000;
        }
        self.write_bs(word as i16);
        if let Color::Rgb { r, g, b } = color {
            self.write_bl(rgb_long(*r, *g, *b) as i32);
        }
        if let Some(t) = transparency {
            self.write_bl(t as i32);
        }
        Ok(())
    }
}

/// An R2004+ CMC color as its RGB long (with color-method byte) and names.
pub(crate) fn cmc_parts(color: &Color) -> Result<(u32, &str, &str)> {
    Ok(match color {
        Color::ByLayer => (u32::from(METHOD_BY_LAYER) << 24, "", ""),
        Color::ByBlock => (u32::from(METHOD_BY_BLOCK) << 24, "", ""),
        Color::None => (u32::from(METHOD_NONE) << 24, "", ""),
        Color::Index(n) if *n >= 1 => (u32::from(METHOD_ACI) << 24 | u32::from(*n), "", ""),
        Color::Index(_) => return Err(unencodable("ACI index 0 is BYBLOCK")),
        Color::Rgb { r, g, b } => (rgb_long(*r, *g, *b), "", ""),
        Color::Book {
            rgb,
            color_name,
            book_name,
        } => {
            if color_name.is_empty() && book_name.is_empty() {
                return Err(unencodable("book color needs a color or book name"));
            }
            (
                u32::from(METHOD_RGB) << 24 | (rgb & 0x00FF_FFFF),
                color_name.as_str(),
                book_name.as_str(),
            )
        }
    })
}

/// 0xC2 method byte plus 24-bit RGB.
fn rgb_long(r: u8, g: u8, b: u8) -> u32 {
    u32::from(METHOD_RGB) << 24 | u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::{BitReader, bits, byte_bits};
    use proptest::prelude::*;

    fn written(f: impl FnOnce(&mut BitWriter)) -> String {
        let mut w = BitWriter::new();
        f(&mut w);
        let n = w.bit_len() as usize;
        byte_bits(&w.into_bytes())[..n].to_string()
    }

    #[test]
    fn exact_bits() {
        assert_eq!(written(|w| w.write_bs(256)), "11");
        assert_eq!(written(|w| w.write_bs(0)), "10");
        assert_eq!(written(|w| w.write_bs(15)), "0100001111");
        assert_eq!(
            written(|w| w.write_bs(257)),
            format!("00{}", byte_bits(&257i16.to_le_bytes()))
        );
        assert_eq!(written(|w| w.write_bd(1.0)), "01");
        assert_eq!(written(|w| w.write_bd(0.0)), "10");
        assert_eq!(written(|w| w.write_bl(0)), "10");
        assert_eq!(written(|w| w.write_bl(255)), "0111111111");
        assert_eq!(written(|w| w.write_bt(0.0)), "1");
        assert_eq!(written(|w| w.write_be([0.0, 0.0, 1.0])), "1");
        assert_eq!(written(|w| w.write_bll(0).unwrap()), "000");
        assert_eq!(written(|w| w.write_bll(0x34).unwrap()), "00100110100");
        assert_eq!(
            written(|w| w.write_bll(0x1234).unwrap()),
            "0100011010000010010"
        );
        assert_eq!(
            written(|w| w.write_bll(0x12_3456).unwrap()),
            format!("011{}", byte_bits(&[0x56, 0x34, 0x12]))
        );
    }

    #[test]
    fn exact_bytes() {
        let mut w = BitWriter::new();
        w.write_mc(4610).unwrap();
        assert_eq!(w.into_bytes(), [0x82, 0x24]);
        for (v, want) in [
            (-1, vec![0x41]),
            (-1413, vec![0x85, 0x4B]),
            (112_823_273, vec![0xE9, 0x97, 0xE6, 0x35]),
        ] {
            let mut w = BitWriter::new();
            w.write_mc(v).unwrap();
            assert_eq!(w.into_bytes(), want);
        }
        let mut w = BitWriter::new();
        w.write_handle_ref(HandleRef {
            code: 5,
            value: 0x2A,
        })
        .unwrap();
        assert_eq!(w.into_bytes(), [0x51, 0x2A]);
        let mut w = BitWriter::new();
        w.write_handle_ref(HandleRef { code: 5, value: 0 }).unwrap();
        assert_eq!(w.into_bytes(), [0x50]);
        let mut w = BitWriter::new();
        w.write_handle_ref(HandleRef {
            code: 5,
            value: 0x5E7,
        })
        .unwrap();
        assert_eq!(w.into_bytes(), [0x52, 0x05, 0xE7]);
        let mut w = BitWriter::new();
        w.write_umc(128);
        w.write_ms(0xFFFF);
        assert_eq!(w.into_bytes(), [0x80, 0x01, 0xFF, 0xFF, 0x01, 0x00]);
    }

    #[test]
    fn dd_shortest_forms() {
        assert_eq!(written(|w| w.write_dd(2.5, 2.5)), "00");
        let mut v = 1.0f64.to_le_bytes();
        v[..4].copy_from_slice(&[1, 2, 3, 4]);
        assert_eq!(
            written(|w| w.write_dd(f64::from_le_bytes(v), 1.0)).len(),
            2 + 32
        );
        v[4] = 9;
        assert_eq!(
            written(|w| w.write_dd(f64::from_le_bytes(v), 1.0)).len(),
            2 + 48
        );
        v[7] = 9;
        assert_eq!(
            written(|w| w.write_dd(f64::from_le_bytes(v), 1.0)).len(),
            2 + 64
        );
    }

    #[test]
    fn unencodable_values_error() {
        let mut w = BitWriter::new();
        assert!(w.write_mc(1 << 62).is_err());
        assert!(w.write_mc(i64::MIN).is_err());
        assert!(w.write_bll(1 << 56).is_err());
        assert!(w.write_3b(1).is_err());
        assert!(w.write_tv_raw(&vec![0; 32768]).is_err());
        assert!(w.write_tu(&"a".repeat(32768)).is_err());
        assert!(w.write_tv_raw(&vec![0; 32767]).is_ok());
        assert!(w.write_tu(&"a".repeat(32767)).is_ok());
        assert!(
            w.write_handle_ref(HandleRef { code: 16, value: 1 })
                .is_err()
        );
        assert!(w.write_bits_from(&[0], 9).is_err());
        assert!(
            w.write_cmc(&Color::Rgb { r: 1, g: 2, b: 3 }, Version::R2000)
                .is_err()
        );
        assert!(
            w.write_cmc_layer(&Color::ByLayer, true, Version::R2018)
                .is_err()
        );
        assert!(
            w.write_cmc_layer(&Color::ByBlock, true, Version::R2000)
                .is_err()
        );
        let empty_book = Color::Book {
            rgb: 1,
            color_name: String::new(),
            book_name: String::new(),
        };
        assert!(w.write_cmc(&empty_book, Version::R2018).is_err());
        assert!(
            w.write_enc(&Color::ByLayer, Some(1), false, Version::R2000)
                .is_err()
        );
        assert!(
            w.write_enc(&empty_book, None, false, Version::R2018)
                .is_err()
        );
        assert!(
            w.write_cmc(
                &Color::Book {
                    rgb: 0,
                    color_name: "é€".into(),
                    book_name: String::new()
                },
                Version::R2004
            )
            .is_err()
        );
        // The message has one "cannot encode" prefix, the error's own.
        let e = w.write_mc(1 << 62).unwrap_err().to_string();
        assert_eq!(e.matches("cannot encode").count(), 1, "{e}");
    }

    /// The ACI index stored beside a true color is range checked; for other
    /// colors it is ignored, as documented.
    #[test]
    fn enc_aci_is_checked_only_for_true_color() {
        let mut w = BitWriter::new();
        let rgb = Color::Rgb { r: 1, g: 2, b: 3 };
        assert!(
            w.write_enc_with_aci(&rgb, None, false, 0x200, Version::R2018)
                .is_err()
        );
        assert!(
            w.write_enc_with_aci(&rgb, None, false, 0x1FF, Version::R2018)
                .is_ok()
        );
        for color in [Color::ByLayer, Color::ByBlock, Color::Index(7)] {
            let mut ignored = BitWriter::new();
            ignored
                .write_enc_with_aci(&color, None, false, 0xFFFF, Version::R2018)
                .unwrap();
            let mut plain = BitWriter::new();
            plain
                .write_enc(&color, None, false, Version::R2018)
                .unwrap();
            assert_eq!(ignored.into_bytes(), plain.into_bytes(), "{color:?}");
        }
    }

    #[test]
    fn patch_and_splice() {
        let mut w = BitWriter::new();
        w.write_rl(0);
        w.write_rs(0);
        w.write_b(true);
        w.patch_rl_at(0, 0x0102_0304).unwrap();
        w.patch_rs_at(4, -2).unwrap();
        assert!(w.patch_rs_at(5, 0).is_err()); // would touch the partial byte
        assert!(w.patch_rl_at(usize::MAX, 0).is_err());
        assert_eq!(w.bit_len(), 49);
        assert_eq!(w.into_bytes(), [4, 3, 2, 1, 0xFE, 0xFF, 0x80]);

        let src = bits("1011 0110 111");
        let mut w = BitWriter::new();
        w.write_b(false);
        w.write_bits_from(&src, 11).unwrap();
        assert_eq!(w.bit_len(), 12);
        assert_eq!(byte_bits(&w.into_bytes()), "0101101101110000");
    }

    // ---- round trips ----

    fn f64s() -> impl Strategy<Value = f64> {
        prop_oneof![
            any::<u64>().prop_map(f64::from_bits),
            Just(0.0),
            Just(-0.0),
            Just(1.0),
            Just(f64::NAN),
            any::<f64>(),
        ]
    }

    /// Start with 0..8 junk bits so every value is exercised unaligned.
    fn start(pre: u8) -> BitWriter {
        let mut w = BitWriter::new();
        for i in 0..(pre % 8) {
            w.write_b(i % 2 == 0);
        }
        w
    }

    fn reader<'a>(bytes: &'a [u8], pre: u8) -> BitReader<'a> {
        let mut r = BitReader::new(bytes);
        r.seek_bit(u64::from(pre % 8)).unwrap();
        r
    }

    fn same(a: f64, b: f64) -> bool {
        a.to_bits() == b.to_bits()
    }

    fn colors(unicode: bool) -> impl Strategy<Value = Color> {
        let text = if unicode { ".{0,6}" } else { "[ -~]{0,6}" };
        prop_oneof![
            Just(Color::ByLayer),
            Just(Color::ByBlock),
            (1u8..=255).prop_map(Color::Index),
            (any::<u8>(), any::<u8>(), any::<u8>()).prop_map(|(r, g, b)| Color::Rgb { r, g, b }),
            (0u32..0x0100_0000, text, text)
                .prop_filter("a book color needs a name", |(_, c, b)| !c.is_empty()
                    || !b.is_empty())
                .prop_map(|(rgb, color_name, book_name)| Color::Book {
                    rgb,
                    color_name,
                    book_name
                }),
        ]
    }

    proptest! {
        #[test]
        fn rt_small_ints(pre in any::<u8>(), s in any::<i16>(), l in any::<i32>(), c in any::<u8>(), b in any::<bool>(), bb in 0u8..4) {
            let mut w = start(pre);
            w.write_bs(s); w.write_bl(l); w.write_rc(c); w.write_rs(s); w.write_rl(l); w.write_b(b); w.write_bb(bb);
            let bytes = w.into_bytes();
            let mut r = reader(&bytes, pre);
            prop_assert_eq!(r.read_bs().unwrap(), s);
            prop_assert_eq!(r.read_bl().unwrap(), l);
            prop_assert_eq!(r.read_rc().unwrap(), c);
            prop_assert_eq!(r.read_rs().unwrap(), s);
            prop_assert_eq!(r.read_rl().unwrap(), l);
            prop_assert_eq!(r.read_b().unwrap(), b);
            prop_assert_eq!(r.read_bb().unwrap(), bb);
        }

        #[test]
        fn rt_every_bs_is_shortest_and_exact(s in any::<i16>()) {
            let mut w = BitWriter::new();
            w.write_bs(s);
            let want = match s { 0 | 256 => 2, 1..=255 => 10, _ => 18 };
            prop_assert_eq!(w.bit_len(), want);
        }

        #[test]
        fn rt_bll_and_3b(pre in any::<u8>(), v in any::<u64>(), t in prop::sample::select(vec![0u8, 2, 6, 7])) {
            let v = v >> (v % 64); // spread across byte widths
            let mut w = start(pre);
            if v < 1 << 56 {
                w.write_bll(v).unwrap();
            } else {
                prop_assert!(w.write_bll(v).is_err());
            }
            w.write_3b(t).unwrap();
            let bytes = w.into_bytes();
            let mut r = reader(&bytes, pre);
            if v < 1 << 56 {
                prop_assert_eq!(r.read_bll().unwrap(), v);
            }
            prop_assert_eq!(r.read_3b().unwrap(), t);
        }

        #[test]
        fn rt_doubles(pre in any::<u8>(), a in f64s(), b in f64s(), c in f64s(), d in f64s(), e in f64s(), f in f64s()) {
            let mut w = start(pre);
            w.write_bd(a); w.write_rd(b); w.write_bt(c); w.write_be([a, b, c]);
            w.write_2rd([d, e]); w.write_3rd([d, e, f]); w.write_3bd([f, e, d]);
            let bytes = w.into_bytes();
            let mut r = reader(&bytes, pre);
            prop_assert!(same(r.read_bd().unwrap(), a));
            prop_assert!(same(r.read_rd().unwrap(), b));
            prop_assert!(same(r.read_bt().unwrap(), c));
            let be = r.read_be().unwrap();
            prop_assert!(same(be[0], a) && same(be[1], b) && same(be[2], c));
            let p = r.read_2rd().unwrap();
            prop_assert!(same(p[0], d) && same(p[1], e));
            let p = r.read_3rd().unwrap();
            prop_assert!(same(p[0], d) && same(p[1], e) && same(p[2], f));
            let p = r.read_3bd().unwrap();
            prop_assert!(same(p[0], f) && same(p[1], e) && same(p[2], d));
        }

        #[test]
        fn rt_default_extrusion_and_thickness(pre in any::<u8>()) {
            let mut w = start(pre);
            w.write_be([0.0, 0.0, 1.0]); w.write_bt(0.0); w.write_bt(-0.0);
            let bytes = w.into_bytes();
            let mut r = reader(&bytes, pre);
            prop_assert_eq!(r.read_be().unwrap(), [0.0, 0.0, 1.0]);
            prop_assert!(same(r.read_bt().unwrap(), 0.0));
            prop_assert!(same(r.read_bt().unwrap(), -0.0));
        }

        #[test]
        fn rt_dd(pre in any::<u8>(), v in f64s(), def in f64s(), keep in 0usize..9, tweak in any::<u64>()) {
            // Values derived from the default exercise the 00/01/10 forms.
            let mut near = def.to_le_bytes();
            let fresh = tweak.to_le_bytes();
            near[..keep.min(8)].copy_from_slice(&fresh[..keep.min(8)]);
            let near = f64::from_le_bytes(near);
            let mut w = start(pre);
            w.write_dd(v, def); w.write_dd(near, def); w.write_2dd([v, near], [def, def]);
            let bytes = w.into_bytes();
            let mut r = reader(&bytes, pre);
            prop_assert!(same(r.read_dd(def).unwrap(), v));
            prop_assert!(same(r.read_dd(def).unwrap(), near));
            let p = r.read_2dd([def, def]).unwrap();
            prop_assert!(same(p[0], v) && same(p[1], near));
        }

        #[test]
        fn rt_modular(pre in any::<u8>(), mc in -(1i64 << 62) + 1..(1i64 << 62), small in -200i64..200, umc in any::<u64>(), ms in any::<u32>()) {
            let umc = umc >> (umc % 64);
            let mut w = start(pre);
            w.write_mc(mc).unwrap(); w.write_mc(small).unwrap(); w.write_umc(umc); w.write_ms(ms);
            let bytes = w.into_bytes();
            let mut r = reader(&bytes, pre);
            prop_assert_eq!(r.read_mc().unwrap(), mc);
            prop_assert_eq!(r.read_mc().unwrap(), small);
            prop_assert_eq!(r.read_umc().unwrap(), umc);
            prop_assert_eq!(r.read_ms().unwrap(), ms);
        }

        #[test]
        fn rt_object_type(pre in any::<u8>(), t in any::<u16>(), small in 0u16..0x400) {
            for v in [Version::R2000, Version::R2007, Version::R2010, Version::R2013, Version::R2018] {
                for t in [t, small] {
                    let mut w = start(pre);
                    w.write_ot(t, v);
                    let bytes = w.into_bytes();
                    prop_assert_eq!(reader(&bytes, pre).read_ot(v).unwrap(), t);
                }
            }
        }

        #[test]
        fn rt_handle_ref(pre in any::<u8>(), code in 0u8..16, value in any::<u64>()) {
            let value = value >> (value % 64);
            let h = HandleRef { code, value };
            let mut w = start(pre);
            w.write_handle_ref(h).unwrap();
            let bytes = w.into_bytes();
            prop_assert_eq!(reader(&bytes, pre).read_handle_ref().unwrap(), h);
        }

        #[test]
        fn rt_strings(pre in any::<u8>(), s in ".{0,40}", raw in prop::collection::vec(any::<u8>(), 0..300)) {
            let mut w = start(pre);
            w.write_tu(&s).unwrap(); w.write_tv_raw(&raw).unwrap();
            let bytes = w.into_bytes();
            let mut r = reader(&bytes, pre);
            prop_assert_eq!(r.read_tu().unwrap(), s);
            prop_assert_eq!(r.read_tv_raw().unwrap(), raw);
        }

        #[test]
        fn rt_raw_bytes_and_splice(pre in any::<u8>(), data in prop::collection::vec(any::<u8>(), 0..40), cut in 0u64..8) {
            let bit_len = (data.len() as u64 * 8).saturating_sub(cut);
            let mut w = start(pre);
            w.write_bytes(&data);
            w.write_bits_from(&data, bit_len).unwrap();
            let bytes = w.into_bytes();
            let mut r = reader(&bytes, pre);
            prop_assert_eq!(r.read_bytes(data.len()).unwrap(), data.clone());
            for i in 0..bit_len {
                let want = data[(i / 8) as usize] >> (7 - i % 8) & 1 == 1;
                prop_assert_eq!(r.read_b().unwrap(), want);
            }
        }

        #[test]
        fn rt_cmc_all_versions(pre in any::<u8>(), c in colors(false)) {
            for v in [Version::R2000, Version::R2004, Version::R2007, Version::R2010, Version::R2018] {
                let mut w = start(pre);
                let res = w.write_cmc(&c, v);
                if v < Version::R2004 && !matches!(c, Color::ByLayer | Color::ByBlock | Color::Index(_)) {
                    prop_assert!(res.is_err());
                    continue;
                }
                res.unwrap();
                let bytes = w.into_bytes();
                prop_assert_eq!(reader(&bytes, pre).read_cmc(v).unwrap(), c.clone());
            }
        }

        #[test]
        fn rt_cmc_random(pre in any::<u8>(), c in colors(true), c04 in colors(false)) {
            let mut w = start(pre);
            w.write_cmc(&c, Version::R2018).unwrap();
            w.write_cmc(&c04, Version::R2004).unwrap();
            let bytes = w.into_bytes();
            let mut r = reader(&bytes, pre);
            prop_assert_eq!(r.read_cmc(Version::R2018).unwrap(), c);
            prop_assert_eq!(r.read_cmc(Version::R2004).unwrap(), c04);
        }

        #[test]
        fn rt_cmc_layer_off(pre in any::<u8>(), n in 1u8..=255, off in any::<bool>(), by_layer in any::<bool>()) {
            let c = if by_layer { Color::ByLayer } else { Color::Index(n) };
            let mut w = start(pre);
            w.write_cmc_layer(&c, off, Version::R2000).unwrap();
            let bytes = w.into_bytes();
            prop_assert_eq!(reader(&bytes, pre).read_cmc_layer(Version::R2000).unwrap(), (c, off));
        }

        #[test]
        fn rt_enc(
            pre in any::<u8>(),
            c in prop_oneof![
                Just(Color::ByLayer), Just(Color::ByBlock),
                (1u8..=255).prop_map(Color::Index),
                (any::<u8>(), any::<u8>(), any::<u8>()).prop_map(|(r, g, b)| Color::Rgb { r, g, b }),
            ],
            t in prop::option::of(any::<u32>()),
            book in any::<bool>(),
        ) {
            for v in [Version::R2004, Version::R2010, Version::R2018] {
                let mut w = start(pre);
                if book && matches!(c, Color::Rgb { .. }) {
                    prop_assert!(w.write_enc(&c, t, book, v).is_err());
                    continue;
                }
                w.write_enc(&c, t, book, v).unwrap();
                let bytes = w.into_bytes();
                prop_assert_eq!(reader(&bytes, pre).read_enc(v).unwrap(), (c.clone(), t, book));
            }
            if !matches!(c, Color::Rgb { .. }) {
                let mut w = start(pre);
                w.write_enc(&c, None, false, Version::R2000).unwrap();
                let bytes = w.into_bytes();
                prop_assert_eq!(reader(&bytes, pre).read_enc(Version::R2000).unwrap(), (c, None, false));
            }
        }
    }
}
