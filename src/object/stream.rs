//! Object streams (ODS 20.1): an object's data, string and handle streams.
//!
//! On disk an object is `MS size`, (R2010+) `MC handle stream size in bits`, then
//! `size` bytes and an `RS` CRC. The `size` bytes start with the data stream
//! (object type, R2000-R2007 `RL` data size in bits, then the fields), and the
//! handle stream follows the data stream's last bit. From R2007 the data stream
//! ends with a string stream, located backwards from its end:
//!
//! ```text
//! ... data ... | strings | [hi RS] | lo RS | flag B | handles ... | pad | CRC
//! ```
//!
//! `flag` says a string stream is present; `lo` is its size in bits, and when
//! `lo` has 0x8000 set, `hi` (just before it) supplies bits 15.. of the size.
//! These positions follow ACadSharp (MIT), `DwgObjectReader.getEntityType` and
//! `DwgStreamReaderBase.SetPositionByFlag`: sizes count from the first byte
//! after the `MS` (R2000-R2007) or after the `MC` (R2010+).
//!
//! ODS 20.1 says the end bit is decremented "by 16 bytes" for each size short;
//! it means 16 *bits* (one `RS`), as its own "endbit - 128 (bits)" wording is
//! also off: the size short sits immediately before the flag bit. The string
//! stream size is likewise in bits. Corpus objects of every version confirm
//! this (see `tests/objects_common.rs` and the golden tests in `common.rs`).

use crate::bits::{BitReader, BitWriter, Color, cmc_from_parts, cmc_parts};
use crate::codec::crc16;
use crate::diag::{Diagnostic, Severity};
use crate::error::{Error, Result};
use crate::handle::{Handle, HandleRef};
use crate::strings;
use crate::version::Version;

/// CRC-16 seed for objects (ODS 20.1).
const CRC_SEED: u16 = 0xC0C1;

fn corrupt(msg: &str) -> Error {
    Error::Corrupt(format!("object: {msg}"))
}

/// A value that cannot be encoded (see [`Error::Encode`]).
fn encode_err(msg: &str) -> Error {
    Error::Encode(format!("object: {msg}"))
}

/// Sizes and type read from an object's header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct ObjectPreamble {
    pub type_code: u16,
    /// The `MS` size: bytes after the size fields, not counting the CRC.
    pub size_bytes: u32,
    /// Bits before the handle stream, counted from the start of the `size_bytes`.
    pub data_bits: u64,
    /// Bits of handle stream, including its end padding.
    pub handle_stream_bits: u64,
}

/// Reads one object: the data, string (R2007+) and handle streams.
///
/// Text and handle helpers record problems that do not stop decoding, such as a
/// lossy text decode, in `diagnostics`.
#[derive(Debug, Clone)]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct ObjReader<'a> {
    pub data: BitReader<'a>,
    /// The string stream; `Some` from R2007 (empty when the object has none).
    pub strings: Option<BitReader<'a>>,
    pub handles: BitReader<'a>,
    pub version: Version,
    /// DWG code page index for R2000/R2004 text.
    pub codepage: u16,
    /// The object's own handle, set by the common-data decoders; relative
    /// handle references resolve against it.
    pub handle: Handle,
    pub diagnostics: Vec<Diagnostic>,
    /// Set by [`super::decode_object`] for a pre-R2010 multileader style
    /// that has ACAD_MLEADERVER extended data (see
    /// [`super::DecodeCtx::mleader_style_marker`]).
    pub(crate) mleader_new_format: bool,
    /// Set by [`super::decode_object`] for an object whose extended data
    /// names `AcDbSavedByObjectVersion`: AutoCAD saved it in an earlier
    /// version of its type's layout (see [`super::saved_by_object_version`]).
    pub(crate) saved_by_object_version: bool,
    crc_ok: bool,
}

impl<'a> ObjReader<'a> {
    /// Split an object starting at `object_bytes[0]` into its streams. The slice
    /// may extend past the object; the object and its CRC must fit in it.
    ///
    /// The returned reader's `data` is positioned after the type (and R2000-R2007
    /// `RL` data size), at the object's handle.
    pub fn split(
        object_bytes: &'a [u8],
        version: Version,
        codepage: u16,
    ) -> Result<(ObjReader<'a>, ObjectPreamble)> {
        let mut head = BitReader::new(object_bytes);
        let size_bytes = head.read_ms()?;
        let handle_size = if version >= Version::R2010 {
            Some(head.read_umc()?)
        } else {
            None
        };
        // MS and MC are whole bytes, so `start` is byte aligned.
        let start = head.bit_pos();
        let start_byte = (start / 8) as usize;
        let end_byte = usize::try_from(size_bytes)
            .ok()
            .and_then(|n| start_byte.checked_add(n))
            .ok_or(Error::Truncated { context: "object" })?;
        let crc_bytes = object_bytes
            .get(end_byte..end_byte.saturating_add(2))
            .filter(|c| c.len() == 2)
            .ok_or(Error::Truncated { context: "object" })?;
        let body = &object_bytes[..end_byte];
        let crc_ok = crc16(CRC_SEED, body) == u16::from_le_bytes([crc_bytes[0], crc_bytes[1]]);
        let end = (end_byte as u64) * 8;

        let mut data = BitReader::with_bit_limit(body, end);
        data.seek_bit(start)?;
        let type_code = data.read_ot(version)?;
        let data_end = match handle_size {
            Some(bits) => end
                .checked_sub(bits)
                .filter(|&e| e >= data.bit_pos())
                .ok_or_else(|| corrupt("handle stream size exceeds the object"))?,
            None => {
                let rl = u64::from(data.read_rl()? as u32);
                start
                    .checked_add(rl)
                    .filter(|&e| e >= data.bit_pos() && e <= end)
                    .ok_or_else(|| corrupt("data size in bits exceeds the object"))?
            }
        };
        let header_end = data.bit_pos();

        let (data_limit, strings) = if version >= Version::R2007 {
            let (limit, s) = locate_strings(body, header_end, data_end)?;
            (limit, Some(s))
        } else {
            (data_end, None)
        };
        let mut data = BitReader::with_bit_limit(body, data_limit);
        data.seek_bit(header_end)?;
        let mut handles = BitReader::with_bit_limit(body, end);
        handles.seek_bit(data_end)?;

        let preamble = ObjectPreamble {
            type_code,
            size_bytes,
            data_bits: data_end - start,
            handle_stream_bits: end - data_end,
        };
        let reader = ObjReader {
            data,
            strings,
            handles,
            version,
            codepage,
            handle: Handle(0),
            diagnostics: Vec::new(),
            mleader_new_format: false,
            saved_by_object_version: false,
            crc_ok,
        };
        Ok((reader, preamble))
    }

    /// A reader over streams the caller has already located, for sections laid
    /// out like an object (the R2007+ `AcDb:Header`). Before R2007 `strings`
    /// is `None`. The CRC is the caller's to check.
    pub(crate) fn from_streams(
        data: BitReader<'a>,
        strings: Option<BitReader<'a>>,
        handles: BitReader<'a>,
        version: Version,
        codepage: u16,
    ) -> ObjReader<'a> {
        ObjReader {
            data,
            strings,
            handles,
            version,
            codepage,
            handle: Handle(0),
            diagnostics: Vec::new(),
            mleader_new_format: false,
            saved_by_object_version: false,
            crc_ok: true,
        }
    }

    /// Whether the object's stored CRC matched.
    pub fn crc_ok(&self) -> bool {
        self.crc_ok
    }

    fn warn(&mut self, message: String) {
        self.diagnostics.push(Diagnostic {
            severity: Severity::Warning,
            handle: Some(self.handle),
            message,
        });
    }

    /// A text field (`TV`): code page bytes from the data stream before R2007,
    /// UTF-16 from the string stream from R2007.
    ///
    /// Trailing NULs are removed: AutoCAD does not store a terminator, but some
    /// other writers (e.g. ACadSharp) count one in the length.
    /// Text policy: decoding is lossy. Malformed code page bytes and unpaired
    /// UTF-16 surrogates become U+FFFD and add a warning to `diagnostics`, since
    /// such text cannot be written back byte for byte.
    pub fn text(&mut self) -> Result<String> {
        let mut text = if self.version < Version::R2007 {
            let raw = self.data.read_tv_raw()?;
            let (text, malformed) = strings::decode_checked(self.codepage, &raw);
            if malformed {
                self.warn(format!(
                    "text is not valid in code page {}; malformed bytes became U+FFFD",
                    self.codepage
                ));
            }
            text
        } else {
            let s = self
                .strings
                .as_mut()
                .ok_or_else(|| corrupt("no string stream"))?;
            let (text, replaced) = s.read_tu_checked()?;
            if replaced {
                self.warn(
                    "text has an unpaired UTF-16 surrogate; it became U+FFFD and will not \
                     round-trip exactly"
                        .into(),
                );
            }
            text
        };
        text.truncate(text.trim_end_matches('\0').len());
        Ok(text)
    }

    /// The next handle reference in the handle stream.
    pub fn href(&mut self) -> Result<HandleRef> {
        self.handles.read_handle_ref()
    }

    /// The next handle reference, resolved against [`Self::handle`]. Codes 0-5
    /// are absolute (ODS 2.13 defines 2-5; code 0 is used for null references
    /// and the object's own handle, and ACadSharp treats 0-5 alike).
    pub fn href_abs(&mut self) -> Result<Handle> {
        let r = self.href()?;
        resolve(r, self.handle)
    }

    /// A `CMC`. The R2004+ color and book names are text fields: code page
    /// bytes in the data stream before R2007, and in the string stream from
    /// R2007 (ODS 20.1: all Unicode strings of an object are there).
    pub fn read_cmc(&mut self) -> Result<Color> {
        self.read_cmc_layer().map(|(c, _)| c)
    }

    /// A LAYER's `CMC`, also returning the layer-off flag (see
    /// [`BitReader::read_cmc_layer`]).
    pub fn read_cmc_layer(&mut self) -> Result<(Color, bool)> {
        if self.version < Version::R2004 {
            return self.data.read_cmc_layer(self.version);
        }
        let (rgb, flags) = self.data.read_cmc_head()?;
        let color_name = if flags & 1 != 0 {
            self.text()?
        } else {
            String::new()
        };
        let book_name = if flags & 2 != 0 {
            self.text()?
        } else {
            String::new()
        };
        Ok((cmc_from_parts(rgb, flags, color_name, book_name)?, false))
    }
}

/// Resolve a handle reference against `base`, treating codes 0 and 1 as
/// absolute like 2-5.
pub(crate) fn resolve(r: HandleRef, base: Handle) -> Result<Handle> {
    match r.code {
        0 | 1 => Some(Handle(r.value)),
        _ => r.resolve(base),
    }
    .ok_or_else(|| {
        corrupt(&format!(
            "unresolvable handle reference (code {}, value {:X}) from {base}",
            r.code, r.value
        ))
    })
}

/// Locate the R2007+ string stream that ends the data stream at `data_end`.
/// Returns the end of the remaining object data and the string stream reader.
pub(crate) fn locate_strings(
    body: &[u8],
    header_end: u64,
    data_end: u64,
) -> Result<(u64, BitReader<'_>)> {
    let bad = || corrupt("string stream size exceeds the object data");
    let flag_pos = data_end
        .checked_sub(1)
        .filter(|&p| p >= header_end)
        .ok_or_else(|| corrupt("no room for the string stream flag"))?;
    let mut r = BitReader::with_bit_limit(body, data_end);
    r.seek_bit(flag_pos)?;
    if !r.read_b()? {
        return Ok((flag_pos, BitReader::with_bit_limit(body, 0)));
    }
    let mut pos = flag_pos
        .checked_sub(16)
        .filter(|&p| p >= header_end)
        .ok_or_else(bad)?;
    r.seek_bit(pos)?;
    let lo = r.read_rs()? as u16;
    let mut size = u64::from(lo);
    if lo & 0x8000 != 0 {
        pos = pos
            .checked_sub(16)
            .filter(|&p| p >= header_end)
            .ok_or_else(bad)?;
        r.seek_bit(pos)?;
        let hi = r.read_rs()? as u16;
        size = u64::from(lo & 0x7FFF) | u64::from(hi) << 15;
    }
    let str_start = pos
        .checked_sub(size)
        .filter(|&p| p >= header_end)
        .ok_or_else(bad)?;
    let mut s = BitReader::with_bit_limit(body, pos);
    s.seek_bit(str_start)?;
    Ok((str_start, s))
}

/// Builds one object in R2018 form; the mirror of [`ObjReader`].
#[derive(Debug, Clone)]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct ObjWriter {
    /// The data stream after the object type.
    pub data: BitWriter,
    strings: BitWriter,
    handles: BitWriter,
    pub handle: Handle,
    /// Every reference passed to [`ObjWriter::href`], when recording (see
    /// [`ObjWriter::recording`]).
    refs: Option<Vec<HandleRef>>,
}

impl ObjWriter {
    /// The only version objects are written in.
    pub const VERSION: Version = Version::R2018;

    /// A writer for the object with handle `handle`.
    pub fn new(handle: Handle) -> Self {
        ObjWriter {
            data: BitWriter::new(),
            strings: BitWriter::new(),
            handles: BitWriter::new(),
            handle,
            refs: None,
        }
    }

    /// A writer that also records every handle reference written (see
    /// [`ObjWriter::recorded_refs`]). Every encoder writes its references
    /// through [`ObjWriter::href`], so encoding an object with it lists the
    /// object's references.
    pub(crate) fn recording(handle: Handle) -> Self {
        ObjWriter {
            refs: Some(Vec::new()),
            ..ObjWriter::new(handle)
        }
    }

    /// The references recorded so far, in write order (empty when not
    /// recording).
    pub(crate) fn recorded_refs(&self) -> &[HandleRef] {
        self.refs.as_deref().unwrap_or_default()
    }

    /// A text field: `TU` in the string stream. As in AutoCAD files, the
    /// length does not count a terminator and none is written.
    pub fn text(&mut self, s: &str) -> Result<()> {
        self.strings.write_tu(s)
    }

    /// A `CMC`: the fields in the data stream, the names in the string stream.
    pub fn write_cmc(&mut self, color: &Color) -> Result<()> {
        let (rgb, color_name, book_name) = cmc_parts(color)?;
        self.data.write_cmc_head(rgb, color_name, book_name);
        for name in [color_name, book_name] {
            if !name.is_empty() {
                self.text(name)?;
            }
        }
        Ok(())
    }

    /// Append string stream bits as kept (a raw object's unread strings).
    pub(crate) fn write_raw_strings(&mut self, bytes: &[u8], bits: u64) -> Result<()> {
        self.strings.write_bits_from(bytes, bits)
    }

    /// The handle stream, for sections that put plain values after the
    /// handles (the R2007+ header's last fields).
    pub(crate) fn handle_stream(&mut self) -> &mut BitWriter {
        &mut self.handles
    }

    /// A handle reference in the handle stream.
    pub fn href(&mut self, h: HandleRef) -> Result<()> {
        if let Some(refs) = &mut self.refs {
            refs.push(h);
        }
        self.handles.write_handle_ref(h)
    }

    /// Assemble the object: `MS` size, `MC` handle stream size, `OT` type, the
    /// data and string streams with the string flag, the handle stream padded
    /// to a byte with one bits, and the CRC.
    pub fn finish(self, type_code: u16) -> Result<Vec<u8>> {
        let mut main = BitWriter::new();
        main.write_ot(type_code, Self::VERSION);
        let data_end = self.write_streams(&mut main)?;
        // Pad the handle stream to a byte with one bits, as AutoCAD does
        // (the padding counts in the handle stream size).
        while main.bit_len() % 8 != 0 {
            main.write_b(true);
        }
        let body = main.into_bytes();
        let size = u32::try_from(body.len()).map_err(|_| encode_err("object too large"))?;
        let mut out = BitWriter::new();
        out.write_ms(size);
        out.write_umc((body.len() as u64) * 8 - data_end);
        out.write_bytes(&body);
        let mut bytes = out.into_bytes();
        let crc = crc16(CRC_SEED, &bytes);
        bytes.extend_from_slice(&crc.to_le_bytes());
        Ok(bytes)
    }

    /// Append the data stream, the string stream with its size and flag, and
    /// the handle stream to `main`. Returns the bit position in `main` where
    /// the handle stream starts (the end of the data stream).
    pub(crate) fn write_streams(self, main: &mut BitWriter) -> Result<u64> {
        let data_bits = self.data.bit_len();
        main.write_bits_from(&self.data.into_bytes(), data_bits)?;
        let string_bits = self.strings.bit_len();
        if string_bits > 0 {
            main.write_bits_from(&self.strings.into_bytes(), string_bits)?;
            if string_bits < 0x8000 {
                main.write_rs(string_bits as i16);
            } else {
                let hi = u16::try_from(string_bits >> 15)
                    .map_err(|_| encode_err("string stream too large"))?;
                main.write_rs(hi as i16);
                main.write_rs(((string_bits & 0x7FFF) as u16 | 0x8000) as i16);
            }
            main.write_b(true);
        } else {
            main.write_b(false);
        }
        let data_end = main.bit_len();
        let handle_bits = self.handles.bit_len();
        main.write_bits_from(&self.handles.into_bytes(), handle_bits)?;
        Ok(data_end)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Assemble an object in any version's layout, for reader tests: `data` is
    /// everything after the type (and RL), `strings` the R2007+ string stream.
    pub(crate) fn assemble(
        version: Version,
        type_code: u16,
        data: &BitWriter,
        strings: Option<&BitWriter>,
        handles: &BitWriter,
    ) -> Vec<u8> {
        let mut tail = BitWriter::new();
        tail.write_bits_from(&data.clone().into_bytes(), data.bit_len())
            .unwrap();
        if version >= Version::R2007 {
            match strings {
                Some(s) if s.bit_len() > 0 => {
                    let n = s.bit_len();
                    tail.write_bits_from(&s.clone().into_bytes(), n).unwrap();
                    if n >= 0x8000 {
                        tail.write_rs((n >> 15) as u16 as i16);
                        tail.write_rs(((n & 0x7FFF) as u16 | 0x8000) as i16);
                    } else {
                        tail.write_rs(n as i16);
                    }
                    tail.write_b(true);
                }
                _ => tail.write_b(false),
            }
        }
        let mut main = BitWriter::new();
        if version >= Version::R2010 {
            main.write_ot(type_code, version);
        } else {
            main.write_bs(type_code as i16);
            let head = main.bit_len() + 32;
            main.write_rl((head + tail.bit_len()) as i32);
        }
        main.write_bits_from(&tail.clone().into_bytes(), tail.bit_len())
            .unwrap();
        let data_end = main.bit_len();
        main.write_bits_from(&handles.clone().into_bytes(), handles.bit_len())
            .unwrap();
        // AutoCAD pads the handle stream with one bits.
        while main.bit_len() % 8 != 0 {
            main.write_b(true);
        }
        let body = main.into_bytes();
        let mut out = BitWriter::new();
        out.write_ms(body.len() as u32);
        if version >= Version::R2010 {
            out.write_umc(body.len() as u64 * 8 - data_end);
        }
        out.write_bytes(&body);
        let mut bytes = out.into_bytes();
        let crc = crc16(CRC_SEED, &bytes);
        bytes.extend_from_slice(&crc.to_le_bytes());
        bytes
    }

    fn h(code: u8, value: u64) -> HandleRef {
        HandleRef { code, value }
    }

    #[test]
    fn object_crc_written() {
        let mut w = ObjWriter::new(Handle(0x2A));
        w.data.write_bl(7);
        w.text("abc").unwrap();
        w.href(h(5, 0x10)).unwrap();
        let bytes = w.finish(0x13).unwrap();
        let n = bytes.len();
        assert_eq!(
            u16::from_le_bytes([bytes[n - 2], bytes[n - 1]]),
            crc16(0xC0C1, &bytes[..n - 2])
        );
        let (r, _) = ObjReader::split(&bytes, Version::R2018, 30).unwrap();
        assert!(r.crc_ok());
        let mut bad = bytes.clone();
        bad[3] ^= 1;
        let (r, _) = ObjReader::split(&bad, Version::R2018, 30).unwrap();
        assert!(!r.crc_ok());
    }

    #[test]
    fn writer_reader_streams_roundtrip() {
        let mut w = ObjWriter::new(Handle(0x2A));
        w.data.write_bl(7);
        w.data.write_bd(1.5);
        w.text("Ab").unwrap();
        w.text("").unwrap();
        w.text("日本😀").unwrap();
        w.href(h(5, 0x10)).unwrap();
        w.href(h(6, 0)).unwrap();
        let bytes = w.finish(0x1F2).unwrap();
        let (mut r, pre) = ObjReader::split(&bytes, Version::R2018, 30).unwrap();
        assert_eq!(pre.type_code, 0x1F2);
        assert_eq!(
            pre.data_bits + pre.handle_stream_bits,
            u64::from(pre.size_bytes) * 8
        );
        assert_eq!(r.data.read_bl().unwrap(), 7);
        assert_eq!(r.data.read_bd().unwrap(), 1.5);
        assert_eq!(r.data.remaining_bits(), 0);
        assert_eq!(r.text().unwrap(), "Ab");
        assert_eq!(r.text().unwrap(), "");
        assert_eq!(r.text().unwrap(), "日本😀");
        assert_eq!(r.strings.as_ref().unwrap().remaining_bits(), 0);
        r.handle = Handle(0x2A);
        assert_eq!(r.href_abs().unwrap(), Handle(0x10));
        assert_eq!(r.href_abs().unwrap(), Handle(0x2B));
        assert!(r.handles.remaining_bits() < 8);
        assert!(r.diagnostics.is_empty());
    }

    #[test]
    fn empty_string_stream() {
        let mut w = ObjWriter::new(Handle(1));
        w.data.write_bs(3);
        w.href(h(4, 9)).unwrap();
        let bytes = w.finish(0x30).unwrap();
        let (mut r, pre) = ObjReader::split(&bytes, Version::R2018, 30).unwrap();
        assert_eq!(pre.type_code, 0x30);
        assert_eq!(r.data.read_bs().unwrap(), 3);
        // The flag bit is not part of the readable data.
        assert_eq!(r.data.remaining_bits(), 0);
        assert_eq!(r.strings.as_ref().unwrap().remaining_bits(), 0);
        assert!(r.text().is_err());
        assert_eq!(r.href().unwrap(), h(4, 9));
    }

    #[test]
    fn large_string_stream_uses_size_extension() {
        let big: String = "x".repeat(20_000);
        let mut w = ObjWriter::new(Handle(1));
        w.data.write_b(true);
        w.text(&big).unwrap();
        w.text(&big).unwrap();
        w.href(h(5, 0x77)).unwrap();
        let bytes = w.finish(0x2C).unwrap();
        // 2 x (BS + 20001 units) is over 0x7FFF bytes, so over 0x7FFF bits too.
        assert!(bytes.len() > 0x8000 + 0x7FFF);
        let (mut r, _) = ObjReader::split(&bytes, Version::R2018, 30).unwrap();
        assert!(r.data.read_b().unwrap());
        assert_eq!(r.data.remaining_bits(), 0);
        assert_eq!(r.text().unwrap(), big);
        assert_eq!(r.text().unwrap(), big);
        assert_eq!(r.strings.as_ref().unwrap().remaining_bits(), 0);
        assert_eq!(r.href().unwrap(), h(5, 0x77));
    }

    #[test]
    fn assemble_matches_writer_for_r2018() {
        let mut w = ObjWriter::new(Handle(1));
        w.data.write_bl(5);
        w.text("q").unwrap();
        w.href(h(5, 3)).unwrap();
        let mut data = BitWriter::new();
        data.write_bl(5);
        let mut s = BitWriter::new();
        s.write_tu("q").unwrap();
        let mut hs = BitWriter::new();
        hs.write_handle_ref(h(5, 3)).unwrap();
        assert_eq!(
            w.finish(0x13).unwrap(),
            assemble(Version::R2018, 0x13, &data, Some(&s), &hs)
        );
    }

    #[test]
    fn r2000_split_uses_rl_and_codepage_text() {
        let mut data = BitWriter::new();
        data.write_bl(-4);
        data.write_tv_raw(&[0x82, 0xA0, 0x41, 0x00]).unwrap(); // "あA\0" in Shift-JIS
        let mut hs = BitWriter::new();
        hs.write_handle_ref(h(4, 0x1F)).unwrap();
        for v in [Version::R2000, Version::R2004] {
            let bytes = assemble(v, 0x33, &data, None, &hs);
            let (mut r, pre) = ObjReader::split(&bytes, v, 38).unwrap();
            assert!(r.crc_ok());
            assert_eq!(pre.type_code, 0x33);
            assert!(r.strings.is_none());
            assert_eq!(r.data.read_bl().unwrap(), -4);
            assert_eq!(r.text().unwrap(), "あA");
            assert_eq!(r.data.remaining_bits(), 0);
            assert_eq!(r.href().unwrap(), h(4, 0x1F));
            assert!(r.handles.remaining_bits() < 8);
        }
    }

    #[test]
    fn r2007_split_rl_and_string_stream() {
        let mut data = BitWriter::new();
        data.write_bl(9);
        let mut s = BitWriter::new();
        s.write_tu("päth\0").unwrap();
        let mut hs = BitWriter::new();
        hs.write_handle_ref(h(3, 0x44)).unwrap();
        let bytes = assemble(Version::R2007, 0x31, &data, Some(&s), &hs);
        let (mut r, pre) = ObjReader::split(&bytes, Version::R2007, 30).unwrap();
        assert_eq!(pre.type_code, 0x31);
        assert_eq!(r.data.read_bl().unwrap(), 9);
        assert_eq!(r.data.remaining_bits(), 0);
        assert_eq!(r.text().unwrap(), "päth");
        assert_eq!(r.href().unwrap(), h(3, 0x44));
        // Without a string stream.
        let bytes = assemble(Version::R2007, 0x31, &data, None, &hs);
        let (mut r, _) = ObjReader::split(&bytes, Version::R2007, 30).unwrap();
        assert_eq!(r.data.read_bl().unwrap(), 9);
        assert_eq!(r.data.remaining_bits(), 0);
        assert_eq!(r.href().unwrap(), h(3, 0x44));
    }

    #[test]
    fn lossy_text_adds_diagnostics() {
        // Unpaired surrogate (R2018).
        let mut s = BitWriter::new();
        s.write_bs(1);
        s.write_bytes(&0xD800u16.to_le_bytes());
        let bytes = assemble(
            Version::R2018,
            1,
            &BitWriter::new(),
            Some(&s),
            &BitWriter::new(),
        );
        let (mut r, _) = ObjReader::split(&bytes, Version::R2018, 30).unwrap();
        r.handle = Handle(0x55);
        assert_eq!(r.text().unwrap(), "\u{FFFD}");
        assert_eq!(r.diagnostics.len(), 1);
        assert_eq!(r.diagnostics[0].handle, Some(Handle(0x55)));
        // Malformed Shift-JIS (R2000).
        let mut d = BitWriter::new();
        d.write_tv_raw(&[0x82]).unwrap();
        let bytes = assemble(Version::R2000, 1, &d, None, &BitWriter::new());
        let (mut r, _) = ObjReader::split(&bytes, Version::R2000, 38).unwrap();
        assert_eq!(r.text().unwrap(), "\u{FFFD}");
        assert_eq!(r.diagnostics.len(), 1);
    }

    #[test]
    fn r2004_book_color_names_use_codepage() {
        let mut d = BitWriter::new();
        d.write_bs(0);
        d.write_bl(0xC2_01_02_03u32 as i32);
        d.write_rc(3);
        d.write_tv_raw(&[0x82, 0xA0]).unwrap();
        d.write_tv_raw(b"BOOK").unwrap();
        let bytes = assemble(Version::R2004, 0x33, &d, None, &BitWriter::new());
        let (mut r, _) = ObjReader::split(&bytes, Version::R2004, 38).unwrap();
        assert_eq!(
            r.read_cmc().unwrap(),
            Color::Book {
                rgb: 0x010203,
                color_name: "あ".into(),
                book_name: "BOOK".into()
            }
        );
    }

    #[test]
    fn r2007_plus_cmc_names_are_in_the_string_stream() {
        let book = Color::Book {
            rgb: 0x010203,
            color_name: "あ".into(),
            book_name: "BOOK".into(),
        };
        // R2018: ObjWriter puts the fields in the data stream, the names in the
        // string stream.
        for c in [
            book.clone(),
            Color::ByLayer,
            Color::Index(9),
            Color::Rgb { r: 1, g: 2, b: 3 },
        ] {
            let mut w = ObjWriter::new(Handle(1));
            w.write_cmc(&c).unwrap();
            w.data.write_bl(5);
            let bytes = w.finish(0x33).unwrap();
            let (mut r, _) = ObjReader::split(&bytes, Version::R2018, 30).unwrap();
            assert_eq!(r.read_cmc().unwrap(), c);
            assert_eq!(r.data.read_bl().unwrap(), 5);
            assert_eq!(r.data.remaining_bits(), 0);
            assert_eq!(r.strings.as_ref().unwrap().remaining_bits(), 0);
        }
        // R2007, built by hand: BS 0, BL rgb, RC 3 in the data; names in the
        // string stream.
        let mut d = BitWriter::new();
        d.write_bs(0);
        d.write_bl(0xC2_01_02_03u32 as i32);
        d.write_rc(3);
        d.write_bl(5);
        let mut s = BitWriter::new();
        s.write_tu("あ").unwrap();
        s.write_tu("BOOK").unwrap();
        let bytes = assemble(Version::R2007, 0x33, &d, Some(&s), &BitWriter::new());
        let (mut r, _) = ObjReader::split(&bytes, Version::R2007, 30).unwrap();
        assert_eq!(r.read_cmc().unwrap(), book);
        assert_eq!(r.data.read_bl().unwrap(), 5);
        assert_eq!(r.data.remaining_bits(), 0);
    }

    #[test]
    fn href_abs_rejects_unresolvable() {
        let mut hs = BitWriter::new();
        hs.write_handle_ref(h(0xC, 0x50)).unwrap();
        hs.write_handle_ref(h(0, 0)).unwrap();
        let bytes = assemble(Version::R2018, 1, &BitWriter::new(), None, &hs);
        let (mut r, _) = ObjReader::split(&bytes, Version::R2018, 30).unwrap();
        r.handle = Handle(0x10);
        assert!(matches!(r.href_abs(), Err(Error::Corrupt(_))));
        assert_eq!(r.href_abs().unwrap(), Handle(0));
    }

    // ---- hostile input ----

    fn r2018_object() -> Vec<u8> {
        let mut w = ObjWriter::new(Handle(1));
        w.data.write_bl(1);
        w.text("abc").unwrap();
        w.href(h(5, 3)).unwrap();
        w.finish(0x13).unwrap()
    }

    #[test]
    fn hostile_ms_size_beyond_buffer() {
        // MS claims 0x7FFF bytes in a 4-byte buffer.
        let bytes = [0xFF, 0x7F, 0x00, 0x00];
        assert!(matches!(
            ObjReader::split(&bytes, Version::R2018, 30),
            Err(Error::Truncated { .. })
        ));
        // Maximal 2-short MS.
        let bytes = [0xFF, 0xFF, 0xFF, 0x7F, 0, 0];
        assert!(ObjReader::split(&bytes, Version::R2000, 30).is_err());
        // Missing CRC.
        let good = r2018_object();
        assert!(matches!(
            ObjReader::split(&good[..good.len() - 1], Version::R2018, 30),
            Err(Error::Truncated { .. })
        ));
        // Every truncation fails cleanly.
        for n in 0..good.len() - 1 {
            assert!(ObjReader::split(&good[..n], Version::R2018, 30).is_err());
        }
    }

    #[test]
    fn hostile_handle_stream_size() {
        let mut bytes = r2018_object();
        // MC handle size (byte 2 here: MS is one short) set to a huge value.
        assert_eq!(bytes[2] & 0x80, 0);
        bytes[2] = 0x7F;
        assert!(matches!(
            ObjReader::split(&bytes, Version::R2018, 30),
            Err(Error::Corrupt(_))
        ));
        let mut bytes = r2018_object();
        bytes.splice(2..3, [0xFF, 0xFF, 0xFF, 0xFF, 0x0F]);
        // MS still counts the bytes after the (now longer) MC.
        assert!(ObjReader::split(&bytes, Version::R2018, 30).is_err());
    }

    #[test]
    fn hostile_rl_data_size() {
        let mut data = BitWriter::new();
        data.write_bl(1);
        let mut bytes = assemble(Version::R2000, 1, &data, None, &BitWriter::new());
        // RL follows MS (2 bytes) and the BS type (10 bits): patch it to a huge
        // value by rewriting through a writer.
        let mut w = BitWriter::new();
        w.write_bs(1);
        w.write_rl(0x7FFF_FFFF);
        w.write_bl(1);
        let body = w.into_bytes();
        bytes.truncate(2);
        bytes[..2].copy_from_slice(&(body.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&body);
        bytes.extend_from_slice(&[0, 0]);
        assert!(matches!(
            ObjReader::split(&bytes, Version::R2000, 30),
            Err(Error::Corrupt(_))
        ));
    }

    /// An R2018 object whose data stream is exactly `d` (no flag added).
    fn r2018_raw(d: &BitWriter) -> Vec<u8> {
        let mut main = BitWriter::new();
        main.write_ot(1, Version::R2018);
        main.write_bits_from(&d.clone().into_bytes(), d.bit_len())
            .unwrap();
        let data_end = main.bit_len();
        let body = main.into_bytes();
        let mut out = BitWriter::new();
        out.write_ms(body.len() as u32);
        out.write_umc(body.len() as u64 * 8 - data_end);
        out.write_bytes(&body);
        let mut bytes = out.into_bytes();
        bytes.extend_from_slice(&[0, 0]);
        bytes
    }

    #[test]
    fn hostile_string_stream_size() {
        // lo alone too large, and lo with the 0x8000 extension and a huge hi.
        for lo in [0x7FFFu16, 0x8000 | 0x10] {
            let mut d = BitWriter::new();
            d.write_bl(1);
            d.write_rs(0x7FFF);
            d.write_rs(lo as i16);
            d.write_b(true);
            assert!(
                matches!(
                    ObjReader::split(&r2018_raw(&d), Version::R2018, 30),
                    Err(Error::Corrupt(_))
                ),
                "lo {lo:X}"
            );
        }
        // Flag set but no room for the size short.
        let mut d = BitWriter::new();
        d.write_b(true);
        assert!(matches!(
            ObjReader::split(&r2018_raw(&d), Version::R2018, 30),
            Err(Error::Corrupt(_))
        ));
        // Size exactly fills the data after the header: accepted, and empty.
        let mut d = BitWriter::new();
        d.write_rs(0);
        d.write_b(true);
        let bytes = r2018_raw(&d);
        let (r, _) = ObjReader::split(&bytes, Version::R2018, 30).unwrap();
        assert_eq!(r.data.remaining_bits(), 0);
        assert_eq!(r.strings.unwrap().remaining_bits(), 0);
    }
}
