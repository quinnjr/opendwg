//! `BitReader`: MSB-first bit cursor over a byte slice.

use crate::error::{Error, Result};
use crate::handle::HandleRef;
use crate::version::Version;

const CTX: &str = "bitstream";

fn trunc() -> Error {
    Error::Truncated { context: CTX }
}

/// Reads DWG bit-coded primitives. Bits are MSB-first within each byte and
/// multi-byte raw values are little-endian. Every read is bounds-checked.
#[derive(Debug, Clone)]
pub struct BitReader<'a> {
    data: &'a [u8],
    pos: u64,
    limit: u64,
}

impl<'a> BitReader<'a> {
    /// A reader at the first bit of `data`.
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            limit: (data.len() as u64) * 8,
        }
    }

    /// Like `new`, but reads stop after `bits` bits (capped at the slice length).
    pub fn with_bit_limit(data: &'a [u8], bits: u64) -> Self {
        let mut r = Self::new(data);
        r.limit = r.limit.min(bits);
        r
    }

    /// The position, in bits from the start.
    pub fn bit_pos(&self) -> u64 {
        self.pos
    }

    /// Stop reads `bits` bits after the current position (never past the
    /// current limit). For truncation tests.
    #[cfg(test)]
    pub(crate) fn limit_remaining(&mut self, bits: u64) {
        self.limit = self.limit.min(self.pos.saturating_add(bits));
    }

    /// Move to bit `pos`; past the end is [`crate::Error::Truncated`].
    pub fn seek_bit(&mut self, pos: u64) -> Result<()> {
        if pos > self.limit {
            return Err(trunc());
        }
        self.pos = pos;
        Ok(())
    }

    /// The bits left to read.
    pub fn remaining_bits(&self) -> u64 {
        self.limit.saturating_sub(self.pos)
    }

    /// Advance to the next byte boundary (clamped to the limit).
    pub fn align_byte(&mut self) {
        self.pos = ((self.pos + 7) & !7).min(self.limit);
    }

    /// Read `n` (<= 64) bits, MSB-first, as an unsigned value.
    fn read_bits(&mut self, n: u32) -> Result<u64> {
        debug_assert!(n <= 64);
        if u64::from(n) > self.remaining_bits() {
            return Err(trunc());
        }
        let mut left = n;
        let mut out: u64 = 0;
        while left > 0 {
            let byte = *self.data.get((self.pos / 8) as usize).ok_or_else(trunc)?;
            let used = (self.pos % 8) as u32;
            let take = (8 - used).min(left);
            let chunk = (u32::from(byte) >> (8 - used - take)) & ((1u32 << take) - 1);
            out = (out << take) | u64::from(chunk);
            self.pos += u64::from(take);
            left -= take;
        }
        Ok(out)
    }

    fn read_le<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut buf = [0u8; N];
        for b in &mut buf {
            *b = self.read_rc()?;
        }
        Ok(buf)
    }

    // ---- bit-coded primitives ----

    /// B: one bit.
    pub fn read_b(&mut self) -> Result<bool> {
        Ok(self.read_bits(1)? == 1)
    }

    /// BB: two bits.
    pub fn read_bb(&mut self) -> Result<u8> {
        Ok(self.read_bits(2)? as u8)
    }

    /// 3B: 1 to 3 bits, stopping at the first zero bit.
    pub fn read_3b(&mut self) -> Result<u8> {
        let mut v = 0;
        for _ in 0..3 {
            v <<= 1;
            if self.read_b()? {
                v |= 1;
            } else {
                break;
            }
        }
        Ok(v)
    }

    /// BS: a bitshort.
    pub fn read_bs(&mut self) -> Result<i16> {
        match self.read_bb()? {
            0 => self.read_rs(),
            1 => Ok(i16::from(self.read_rc()?)),
            2 => Ok(0),
            _ => Ok(256),
        }
    }

    /// BL: a bitlong.
    pub fn read_bl(&mut self) -> Result<i32> {
        match self.read_bb()? {
            0 => self.read_rl(),
            1 => Ok(i32::from(self.read_rc()?)),
            2 => Ok(0),
            _ => Err(Error::Corrupt("invalid BITLONG code 11".into())),
        }
    }

    /// BITLONGLONG: a 3-bit byte count (0-7), then that many little-endian
    /// bytes.
    ///
    /// ODS 2.4 says the count takes "1-3 bits" like a 3B. The R2013 and R2018
    /// corpus headers ($REQUIREDVERSIONS, the only known BLL) show a plain
    /// 3-bit count: with it the next field starts at bit 3 and the whole
    /// header is consumed exactly, while a 3B count misaligns it. ACadSharp
    /// (MIT, `DwgStreamReaderBase.ReadBitLongLong`) reads 3 bits too.
    pub fn read_bll(&mut self) -> Result<u64> {
        let n = self.read_bits(3)? as u8;
        let mut v: u64 = 0;
        for i in 0..n {
            v |= u64::from(self.read_rc()?) << (8 * u32::from(i));
        }
        Ok(v)
    }

    /// BD: a bitdouble.
    pub fn read_bd(&mut self) -> Result<f64> {
        match self.read_bb()? {
            0 => self.read_rd(),
            1 => Ok(1.0),
            2 => Ok(0.0),
            _ => Err(Error::Corrupt("invalid BITDOUBLE code 11".into())),
        }
    }

    // ---- raw primitives ----

    /// RC: a raw byte.
    pub fn read_rc(&mut self) -> Result<u8> {
        Ok(self.read_bits(8)? as u8)
    }

    /// RS: a raw little-endian short.
    pub fn read_rs(&mut self) -> Result<i16> {
        Ok(i16::from_le_bytes(self.read_le()?))
    }

    /// RL: a raw little-endian long.
    pub fn read_rl(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.read_le()?))
    }

    /// RD: a raw little-endian double.
    pub fn read_rd(&mut self) -> Result<f64> {
        Ok(f64::from_le_bytes(self.read_le()?))
    }

    /// Read `n` raw bytes; `n` is checked against the remaining input first.
    pub fn read_bytes(&mut self, n: usize) -> Result<Vec<u8>> {
        let bits = (n as u64).checked_mul(8).ok_or_else(trunc)?;
        if bits > self.remaining_bits() {
            return Err(trunc());
        }
        if self.pos % 8 == 0 {
            let start = (self.pos / 8) as usize;
            let slice = self.data.get(start..start + n).ok_or_else(trunc)?;
            self.pos += bits;
            return Ok(slice.to_vec());
        }
        (0..n).map(|_| self.read_rc()).collect()
    }

    // ---- modular ----

    /// Modular char: 7 bits per byte, little-endian groups, high bit =
    /// continue. In the final byte bit 0x40 is the sign and only 6 value
    /// bits remain.
    pub fn read_mc(&mut self) -> Result<i64> {
        let mut v: u64 = 0;
        let mut shift = 0u32;
        loop {
            let b = self.read_rc()?;
            if b & 0x80 != 0 {
                v |= u64::from(b & 0x7F) << shift;
                shift += 7;
                if shift > 56 {
                    return Err(Error::Corrupt("modular char too long".into()));
                }
            } else {
                v |= u64::from(b & 0x3F) << shift;
                let v = v as i64;
                return Ok(if b & 0x40 != 0 { -v } else { v });
            }
        }
    }

    /// Unsigned modular char (object-map handle/location offsets): 7 bits per
    /// byte, no sign.
    pub fn read_umc(&mut self) -> Result<u64> {
        let mut v: u64 = 0;
        let mut shift = 0u32;
        loop {
            let b = self.read_rc()?;
            if shift > 63 {
                return Err(Error::Corrupt("modular char too long".into()));
            }
            let part = u64::from(b & 0x7F);
            if shift > 0 && (part << shift) >> shift != part {
                return Err(Error::Corrupt("modular char overflow".into()));
            }
            v |= part << shift;
            if b & 0x80 == 0 {
                return Ok(v);
            }
            shift += 7;
        }
    }

    /// Modular short: 15 bits per little-endian short, high bit = continue.
    pub fn read_ms(&mut self) -> Result<u32> {
        let mut v: u64 = 0;
        let mut shift = 0u32;
        loop {
            let w = u16::from_le_bytes(self.read_le()?);
            v |= u64::from(w & 0x7FFF) << shift;
            if w & 0x8000 == 0 {
                return u32::try_from(v)
                    .map_err(|_| Error::Corrupt("modular short overflow".into()));
            }
            shift += 15;
            if shift > 30 {
                return Err(Error::Corrupt("modular short too long".into()));
            }
        }
    }

    // ---- composite types ----

    /// BitDouble with default (ODS 2.9).
    pub fn read_dd(&mut self, default: f64) -> Result<f64> {
        let mut b = default.to_le_bytes();
        match self.read_bb()? {
            0 => {}
            1 => {
                let d = self.read_le::<4>()?;
                b[..4].copy_from_slice(&d);
            }
            2 => {
                let d = self.read_le::<6>()?;
                b[4..6].copy_from_slice(&d[..2]);
                b[..4].copy_from_slice(&d[2..]);
            }
            _ => return self.read_rd(),
        }
        Ok(f64::from_le_bytes(b))
    }

    /// BitThickness (R2000+): a set bit means 0.0, otherwise a BD follows.
    pub fn read_bt(&mut self) -> Result<f64> {
        if self.read_b()? {
            Ok(0.0)
        } else {
            self.read_bd()
        }
    }

    /// BitExtrusion (R2000+): a set bit means (0,0,1), otherwise 3BD follows.
    pub fn read_be(&mut self) -> Result<[f64; 3]> {
        if self.read_b()? {
            Ok([0.0, 0.0, 1.0])
        } else {
            self.read_3bd()
        }
    }

    /// 2RD: two raw doubles.
    pub fn read_2rd(&mut self) -> Result<[f64; 2]> {
        Ok([self.read_rd()?, self.read_rd()?])
    }

    /// 2DD: two doubles, each coded against `default`.
    pub fn read_2dd(&mut self, default: [f64; 2]) -> Result<[f64; 2]> {
        let x = self.read_dd(default[0])?;
        let y = self.read_dd(default[1])?;
        Ok([x, y])
    }

    /// 3BD: three bitdoubles.
    pub fn read_3bd(&mut self) -> Result<[f64; 3]> {
        Ok([self.read_bd()?, self.read_bd()?, self.read_bd()?])
    }

    /// 3RD: three raw doubles.
    pub fn read_3rd(&mut self) -> Result<[f64; 3]> {
        Ok([self.read_rd()?, self.read_rd()?, self.read_rd()?])
    }

    /// Object type (ODS 2.12): BS before R2010, a bit pair plus 1-2 bytes after.
    pub fn read_ot(&mut self, version: Version) -> Result<u16> {
        if version < Version::R2010 {
            return Ok(self.read_bs()? as u16);
        }
        match self.read_bb()? {
            0 => Ok(u16::from(self.read_rc()?)),
            1 => Ok(u16::from(self.read_rc()?) + 0x1f0),
            _ => Ok(u16::from_le_bytes(self.read_le()?)),
        }
    }

    /// Handle reference: RC (code << 4 | counter) then `counter` big-endian bytes.
    pub fn read_handle_ref(&mut self) -> Result<HandleRef> {
        let head = self.read_rc()?;
        let (code, counter) = (head >> 4, head & 0x0F);
        if counter > 8 {
            return Err(Error::Corrupt(
                "handle reference longer than 8 bytes".into(),
            ));
        }
        let mut value: u64 = 0;
        for _ in 0..counter {
            value = (value << 8) | u64::from(self.read_rc()?);
        }
        Ok(HandleRef { code, value })
    }

    // ---- strings ----

    /// A BS string length; negative values are corrupt.
    fn read_len(&mut self) -> Result<usize> {
        usize::try_from(self.read_bs()?)
            .map_err(|_| Error::Corrupt("negative string length".into()))
    }

    /// Text bytes: BS length then that many raw bytes (decoded elsewhere).
    pub fn read_tv_raw(&mut self) -> Result<Vec<u8>> {
        let len = self.read_len()?;
        self.read_bytes(len)
    }

    /// Unicode text: BS character count then UTF-16LE code units. Unpaired
    /// surrogates become U+FFFD; see [`Self::read_tu_checked`].
    pub fn read_tu(&mut self) -> Result<String> {
        self.read_tu_checked().map(|(s, _)| s)
    }

    /// Like [`Self::read_tu`], also reporting whether an unpaired surrogate was
    /// replaced with U+FFFD (the text then cannot be written back exactly).
    pub fn read_tu_checked(&mut self) -> Result<(String, bool)> {
        let chars = self.read_len()?;
        let bytes = self.read_bytes(chars * 2)?;
        let (text, replaced) = crate::strings::decode_utf16le_checked(&bytes);
        Ok((text, replaced))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::{bits, byte_bits};

    #[test]
    fn bs_codes() {
        assert_eq!(BitReader::new(&bits("10")).read_bs().unwrap(), 0);
        assert_eq!(BitReader::new(&bits("11")).read_bs().unwrap(), 256);
        assert_eq!(BitReader::new(&bits("01 00001111")).read_bs().unwrap(), 15);
        assert_eq!(
            BitReader::new(&bits("00 00000001 00000001"))
                .read_bs()
                .unwrap(),
            0x0101
        );
        assert_eq!(
            BitReader::new(&bits("00 11111111 11111111"))
                .read_bs()
                .unwrap(),
            -1
        );
    }

    #[test]
    fn bs_ods_sequence() {
        let d = bits("0000000001000000011011010000111110");
        let mut rd = BitReader::new(&d);
        let got: Vec<i16> = (0..5).map(|_| rd.read_bs().unwrap()).collect();
        assert_eq!(got, [257, 0, 256, 15, 0]);
    }

    #[test]
    fn bd_codes() {
        assert_eq!(BitReader::new(&bits("01")).read_bd().unwrap(), 1.0);
        assert_eq!(BitReader::new(&bits("10")).read_bd().unwrap(), 0.0);
        let s = format!("00{}", byte_bits(&3.5f64.to_le_bytes()));
        assert_eq!(BitReader::new(&bits(&s)).read_bd().unwrap(), 3.5);
        assert!(matches!(
            BitReader::new(&bits("11")).read_bd(),
            Err(Error::Corrupt(_))
        ));
    }

    #[test]
    fn bl_codes() {
        assert_eq!(BitReader::new(&bits("10")).read_bl().unwrap(), 0);
        assert_eq!(BitReader::new(&bits("01 11111111")).read_bl().unwrap(), 255);
        let s = format!("00{}", byte_bits(&0x1234_5678i32.to_le_bytes()));
        assert_eq!(BitReader::new(&bits(&s)).read_bl().unwrap(), 0x1234_5678);
        let s = format!("00{}", byte_bits(&(-1i32).to_le_bytes()));
        assert_eq!(BitReader::new(&bits(&s)).read_bl().unwrap(), -1);
        assert!(BitReader::new(&bits("11")).read_bl().is_err());
    }

    #[test]
    fn bl_ods_sequence() {
        let d = bits("000000000100000001000000000000000010010000111110");
        let mut rd = BitReader::new(&d);
        let got: Vec<i32> = (0..3).map(|_| rd.read_bl().unwrap()).collect();
        assert_eq!(got, [257, 0, 15]);
    }

    #[test]
    fn bll_codes() {
        // A plain 3-bit byte count (not a 3B; see `read_bll`).
        assert_eq!(BitReader::new(&bits("000")).read_bll().unwrap(), 0);
        assert_eq!(
            BitReader::new(&bits("001 00110100")).read_bll().unwrap(),
            0x34
        );
        assert_eq!(
            BitReader::new(&bits("010 00110100 00010010"))
                .read_bll()
                .unwrap(),
            0x1234
        );
        let s = format!("111{}", byte_bits(&[1, 0, 0, 0, 0, 0, 0x80]));
        assert_eq!(
            BitReader::new(&bits(&s)).read_bll().unwrap(),
            0x80_0000_0000_0001
        );
        // The count always takes 3 bits, even for 0.
        let d = bits("000 1");
        let mut r = BitReader::new(&d);
        assert_eq!(r.read_bll().unwrap(), 0);
        assert_eq!(r.bit_pos(), 3);
    }

    #[test]
    fn three_b_codes() {
        assert_eq!(BitReader::new(&bits("0")).read_3b().unwrap(), 0);
        assert_eq!(BitReader::new(&bits("10")).read_3b().unwrap(), 2);
        assert_eq!(BitReader::new(&bits("110")).read_3b().unwrap(), 6);
        assert_eq!(BitReader::new(&bits("111")).read_3b().unwrap(), 7);
        let d = bits("10111");
        let mut rd = BitReader::new(&d);
        assert_eq!(rd.read_3b().unwrap(), 2);
        assert_eq!(rd.bit_pos(), 2);
    }

    #[test]
    fn mc_spec_example() {
        // 0x82 0x24 => 2 + 36*128 = 4610
        assert_eq!(BitReader::new(&[0x82, 0x24]).read_mc().unwrap(), 4610);
    }

    #[test]
    fn mc_negative() {
        assert_eq!(BitReader::new(&[0x41]).read_mc().unwrap(), -1);
        assert_eq!(
            BitReader::new(&[0x82, 0x64]).read_mc().unwrap(),
            -(2 + 36 * 128)
        );
        assert!(BitReader::new(&[0x80; 12]).read_mc().is_err());
    }

    #[test]
    fn mc_ods_worked_examples() {
        // ODS 2.6 examples
        assert_eq!(
            BitReader::new(&[0xE9, 0x97, 0xE6, 0x35]).read_mc().unwrap(),
            112_823_273
        );
        assert_eq!(BitReader::new(&[0x85, 0x4B]).read_mc().unwrap(), -1413);
    }

    #[test]
    fn umc_codes() {
        assert_eq!(BitReader::new(&[0x7F]).read_umc().unwrap(), 127);
        assert_eq!(BitReader::new(&[0x82, 0x24]).read_umc().unwrap(), 4610);
        assert_eq!(BitReader::new(&[0x80, 0x01]).read_umc().unwrap(), 128);
        // 0x40 is a value bit here, not a sign
        assert_eq!(BitReader::new(&[0x41]).read_umc().unwrap(), 0x41);
        let mut over = vec![0xFF; 9];
        over.push(0x7F);
        assert!(BitReader::new(&over).read_umc().is_err());
    }

    #[test]
    fn ms_codes() {
        assert_eq!(BitReader::new(&[0x05, 0x00]).read_ms().unwrap(), 5);
        // 0x7FFF with continue bit, then 1 => 0x7FFF + (1 << 15)
        assert_eq!(
            BitReader::new(&[0xFF, 0xFF, 0x01, 0x00]).read_ms().unwrap(),
            0xFFFF
        );
        // ODS 2.7 example bytes: (0xF431 & 0x7FFF) | (0x008D << 15)
        assert_eq!(
            BitReader::new(&[0x31, 0xF4, 0x8D, 0x00]).read_ms().unwrap(),
            0x7431 | (0x8D << 15)
        );
        assert!(
            BitReader::new(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00])
                .read_ms()
                .is_err()
        );
    }

    #[test]
    fn handle_ref_parse() {
        assert_eq!(
            BitReader::new(&[0x51, 0x2A]).read_handle_ref().unwrap(),
            HandleRef {
                code: 5,
                value: 0x2A
            }
        );
        // ODS 2.13: 5.2.05.E7 => 0x5E7
        assert_eq!(
            BitReader::new(&[0x52, 0x05, 0xE7])
                .read_handle_ref()
                .unwrap(),
            HandleRef {
                code: 5,
                value: 0x5E7
            }
        );
        assert_eq!(
            BitReader::new(&[0x60]).read_handle_ref().unwrap(),
            HandleRef { code: 6, value: 0 }
        );
        assert!(BitReader::new(&[0x59; 10]).read_handle_ref().is_err());
        assert!(BitReader::new(&[0x52, 0x05]).read_handle_ref().is_err());
    }

    #[test]
    fn dd_patch_first_four() {
        let patch = [0xAA, 0xBB, 0xCC, 0xDD];
        let s = format!("01{}", byte_bits(&patch));
        let mut want = 1.0f64.to_le_bytes();
        want[..4].copy_from_slice(&patch);
        assert_eq!(
            BitReader::new(&bits(&s)).read_dd(1.0).unwrap(),
            f64::from_le_bytes(want)
        );
    }

    #[test]
    fn dd_other_codes() {
        assert_eq!(BitReader::new(&bits("00")).read_dd(2.5).unwrap(), 2.5);
        // code 10: data = [p4, p5, p0, p1, p2, p3]
        let data = [0x11, 0x22, 0xAA, 0xBB, 0xCC, 0xDD];
        let s = format!("10{}", byte_bits(&data));
        let mut want = 1.0f64.to_le_bytes();
        want[4] = 0x11;
        want[5] = 0x22;
        want[..4].copy_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);
        assert_eq!(
            BitReader::new(&bits(&s)).read_dd(1.0).unwrap(),
            f64::from_le_bytes(want)
        );
        let s = format!("11{}", byte_bits(&7.25f64.to_le_bytes()));
        assert_eq!(BitReader::new(&bits(&s)).read_dd(1.0).unwrap(), 7.25);
    }

    #[test]
    fn two_dd_uses_per_axis_defaults() {
        assert_eq!(
            BitReader::new(&bits("0000")).read_2dd([1.5, 2.5]).unwrap(),
            [1.5, 2.5]
        );
    }

    #[test]
    fn bt_and_be_defaults() {
        assert_eq!(BitReader::new(&bits("1")).read_bt().unwrap(), 0.0);
        assert_eq!(
            BitReader::new(&bits("1")).read_be().unwrap(),
            [0.0, 0.0, 1.0]
        );
        assert_eq!(BitReader::new(&bits("0 01")).read_bt().unwrap(), 1.0);
        assert_eq!(
            BitReader::new(&bits("0 01 10 01")).read_be().unwrap(),
            [1.0, 0.0, 1.0]
        );
    }

    #[test]
    fn raw_types() {
        let mut d = vec![0x34, 0x12, 0x78, 0x56, 0x34, 0x12];
        d.extend(2.0f64.to_le_bytes());
        let mut rd = BitReader::new(&d);
        assert_eq!(rd.read_rs().unwrap(), 0x1234);
        assert_eq!(rd.read_rl().unwrap(), 0x1234_5678);
        assert_eq!(rd.read_rd().unwrap(), 2.0);
        assert_eq!(rd.remaining_bits(), 0);
    }

    #[test]
    fn two_and_three_component_types() {
        let mut d = Vec::new();
        for v in [1.0f64, 2.0, 3.0, 4.0, 5.0] {
            d.extend(v.to_le_bytes());
        }
        let mut rd = BitReader::new(&d);
        assert_eq!(rd.read_2rd().unwrap(), [1.0, 2.0]);
        assert_eq!(rd.read_3rd().unwrap(), [3.0, 4.0, 5.0]);
        assert_eq!(
            BitReader::new(&bits("01 10 01")).read_3bd().unwrap(),
            [1.0, 0.0, 1.0]
        );
    }

    #[test]
    fn ot_versions() {
        assert_eq!(
            BitReader::new(&bits("01 00001111"))
                .read_ot(Version::R2000)
                .unwrap(),
            15
        );
        assert_eq!(
            BitReader::new(&bits("00 00101010"))
                .read_ot(Version::R2010)
                .unwrap(),
            0x2A
        );
        assert_eq!(
            BitReader::new(&bits("01 00000001"))
                .read_ot(Version::R2010)
                .unwrap(),
            0x1f1
        );
        assert_eq!(
            BitReader::new(&bits("10 00000001 00000010"))
                .read_ot(Version::R2013)
                .unwrap(),
            0x0201
        );
        assert_eq!(
            BitReader::new(&bits("11 00000001 00000010"))
                .read_ot(Version::R2018)
                .unwrap(),
            0x0201
        );
    }

    #[test]
    fn unaligned_reads() {
        let d = bits("1 10101010 1");
        let mut rd = BitReader::new(&d);
        assert!(rd.read_b().unwrap());
        assert_eq!(rd.read_rc().unwrap(), 0xAA);
        assert_eq!(rd.bit_pos(), 9);
        let d = bits("0 00000011 00000100 0");
        let mut rd = BitReader::new(&d);
        rd.read_b().unwrap();
        assert_eq!(rd.read_bytes(2).unwrap(), vec![3, 4]);
    }

    #[test]
    fn seek_align_and_limit() {
        let d = [0xFF, 0x00, 0xAB];
        let mut rd = BitReader::new(&d);
        rd.seek_bit(3).unwrap();
        rd.align_byte();
        assert_eq!(rd.bit_pos(), 8);
        rd.align_byte();
        assert_eq!(rd.bit_pos(), 8);
        assert_eq!(rd.remaining_bits(), 16);
        assert!(rd.seek_bit(25).is_err());
        let mut lim = BitReader::with_bit_limit(&d, 12);
        assert_eq!(lim.remaining_bits(), 12);
        assert!(lim.read_bytes(2).is_err());
        lim.seek_bit(12).unwrap();
        assert!(lim.read_b().is_err());
        assert_eq!(BitReader::with_bit_limit(&d, 999).remaining_bits(), 24);
    }

    #[test]
    fn truncation_errors_not_panics() {
        assert!(matches!(
            BitReader::new(&[0x00]).read_bd(),
            Err(Error::Truncated { .. })
        ));
        assert!(matches!(
            BitReader::new(&[]).read_b(),
            Err(Error::Truncated { .. })
        ));
        assert!(BitReader::new(&[0x00; 3]).read_rl().is_err());
        assert!(BitReader::new(&[0x80]).read_mc().is_err());
        assert!(BitReader::new(&[0x80]).read_ms().is_err());
        assert!(BitReader::new(&[0x00]).read_bytes(usize::MAX).is_err());
        // failed read must not move the cursor past the data
        let mut rd = BitReader::new(&[0x00]);
        assert!(rd.read_bits(9).is_err());
        assert_eq!(rd.bit_pos(), 0);
    }

    #[test]
    fn tv_raw_bounded() {
        // BS length 3 then bytes
        let d = bits("01 00000011 01100001 01100010 01100011");
        assert_eq!(BitReader::new(&d).read_tv_raw().unwrap(), b"abc");
        // huge claimed length with no data
        assert!(
            BitReader::new(&bits("00 11111111 11111111"))
                .read_tv_raw()
                .is_err()
        );
        // negative length (-1) is corrupt, not a 65535-byte read
        assert!(matches!(
            BitReader::new(&bits("00 11111111 11111111")).read_tv_raw(),
            Err(Error::Corrupt(_))
        ));
        assert!(matches!(
            BitReader::new(&bits("00 11111111 11111111")).read_tu(),
            Err(Error::Corrupt(_))
        ));
    }

    #[test]
    fn tu_reads_utf16() {
        let d = bits("01 00000010 01000010 00110000 01000001 00000000");
        assert_eq!(BitReader::new(&d).read_tu().unwrap(), "あA");
        // surrogate pair U+1F600
        let d = bits("01 00000010 00111101 11011000 00000000 11011110");
        assert_eq!(BitReader::new(&d).read_tu().unwrap(), "\u{1F600}");
        assert!(BitReader::new(&bits("01 00001000 0000")).read_tu().is_err());
    }

    #[test]
    fn tu_checked_flags_unpaired_surrogate() {
        let d = bits("01 00000010 00111101 11011000 01000001 00000000");
        assert_eq!(
            BitReader::new(&d).read_tu_checked().unwrap(),
            ("\u{FFFD}A".to_string(), true)
        );
        let d = bits("01 00000001 01000001 00000000");
        assert_eq!(
            BitReader::new(&d).read_tu_checked().unwrap(),
            ("A".to_string(), false)
        );
    }
}
