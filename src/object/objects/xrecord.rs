//! Extended records (ODS 20.4.104, XRECORD).

use crate::error::Result;
use crate::error::encode_error as encode_err;
use crate::handle::{Handle, HandleRef};
use crate::object::common::AsRead;
use crate::object::entities::dim::{check_fits, count, write_count};
use crate::object::raw::{HandleTail, trailing_refs};
use crate::object::stream::{ObjReader, ObjWriter};
use crate::strings;
use crate::version::Version;

/// The value type of an extended record item, by DXF group code (the DXF
/// reference's group code ranges, as ACadSharp (MIT) `GroupCodeValue
/// .TransformValue` lists them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum GroupCodeType {
    /// `RS` length and the text: code page bytes after an `RC` code page
    /// before R2007, UTF-16 units from R2007.
    Text,
    /// 3 `RD`.
    Point,
    /// `RD`.
    Real,
    /// `RS`.
    Int16,
    /// `RL`.
    Int32,
    /// 8 bytes.
    Int64,
    /// `RC`.
    Byte,
    /// `RC`, 0 or 1.
    Bool,
    /// A handle written as hexadecimal text (a [`GroupCodeType::Text`]).
    HandleText,
    /// `RC` length and the bytes.
    Binary,
    /// 8 bytes, little-endian.
    ObjectId,
}

/// The value type of group code `code`, or `None` for codes outside the
/// DXF ranges.
pub fn group_code_type(code: i16) -> Option<GroupCodeType> {
    use GroupCodeType::*;
    Some(match code {
        5 | 105 | 320..=329 | 480..=481 => HandleText,
        0..=9 | 100..=102 | 300..=309 | 410..=419 | 430..=439 | 470..=479 | 999..=1003 => Text,
        10..=39 => Point,
        40..=59 | 110..=149 | 210..=239 | 460..=469 | 1010..=1059 => Real,
        60..=79 | 170..=179 | 270..=279 | 370..=389 | 400..=409 | 1060..=1070 => Int16,
        90..=99 | 420..=429 | 440..=459 | 1071 => Int32,
        160..=169 => Int64,
        280..=289 => Byte,
        290..=299 => Bool,
        310..=319 | 1004 => Binary,
        330..=369 | 390..=399 | 1005..=1009 => ObjectId,
        _ => return None,
    })
}

/// Whether an item of group code `code` written as hexadecimal text is a
/// reference: 480-481, "hard-pointer handle; same as 340-349" (DXF
/// reference). The DWG stores them as text, as ACadSharp (MIT) reads and
/// writes them (`GroupCodeValueType.Handle`, `ReadTextUnicode`). 320-329,
/// "arbitrary object handles ... not translated during INSERT and XREF
/// operations", and 5 and 105 (an object's own handle as DXF writes it)
/// are not followed.
pub(crate) fn handle_text_is_reference(code: i16) -> bool {
    matches!(code, 480..=481)
}

/// The handle a [`XRecordValue::HandleText`] holds: hexadecimal digits
/// (`None` for any other text, the empty text included).
pub(crate) fn parse_handle_text(s: &str) -> Option<Handle> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u64::from_str_radix(s, 16).ok().map(Handle)
}

/// `h` as hexadecimal text in the form of `like`: lower case when `like`
/// has lower-case digits only, zero-padded to `like`'s length when `like`
/// starts with a zero.
pub(crate) fn format_handle_text(h: Handle, like: &str) -> String {
    let lower = like.bytes().any(|b| b.is_ascii_lowercase())
        && !like.bytes().any(|b| b.is_ascii_uppercase());
    let digits = if lower {
        format!("{:x}", h.0)
    } else {
        format!("{:X}", h.0)
    };
    let width = if like.starts_with('0') { like.len() } else { 0 };
    format!("{digits:0>width$}")
}

/// The value of one extended record item.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub enum XRecordValue {
    Text(String),
    Point([f64; 3]),
    Real(f64),
    Int16(i16),
    Int32(i32),
    Int64(i64),
    Byte(u8),
    Bool(bool),
    /// A handle as hexadecimal text, kept as stored.
    HandleText(String),
    Binary(Vec<u8>),
    ObjectId(Handle),
    /// The rest of the data bytes from the start of an item whose group
    /// code has no known type or whose value could not be read as its type
    /// (R2007+ text that is not UTF-16 included), its group code included (or a lone trailing byte, with no room for a
    /// code). The pair's code is the code read (0 for a lone byte) and is
    /// not written again: it must match the bytes' first two. It can only
    /// be the last item.
    Unknown(Vec<u8>),
}

impl XRecordValue {
    fn type_of(&self) -> Option<GroupCodeType> {
        use GroupCodeType as T;
        Some(match self {
            XRecordValue::Text(_) => T::Text,
            XRecordValue::Point(_) => T::Point,
            XRecordValue::Real(_) => T::Real,
            XRecordValue::Int16(_) => T::Int16,
            XRecordValue::Int32(_) => T::Int32,
            XRecordValue::Int64(_) => T::Int64,
            XRecordValue::Byte(_) => T::Byte,
            XRecordValue::Bool(_) => T::Bool,
            XRecordValue::HandleText(_) => T::HandleText,
            XRecordValue::Binary(_) => T::Binary,
            XRecordValue::ObjectId(_) => T::ObjectId,
            XRecordValue::Unknown(_) => return None,
        })
    }
}

/// An extended record (ODS 20.4.104, XRECORD): DXF group code and value
/// pairs, stored as bytes in the data stream.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct XRecord {
    pub items: Vec<(i16, XRecordValue)>,
    /// DXF 280: the duplicate record cloning flag.
    pub cloning: i16,
    /// The object ids after the common references, kept as stored ("as many
    /// as you can read until you run out of data").
    pub object_ids: Vec<HandleRef>,
    /// Handle stream bits after the last reference that do not parse as
    /// one, kept as stored (see [`crate::object::HandleTail`]).
    #[cfg_attr(feature = "serde", serde(default))]
    pub handle_tail: HandleTail,
    /// The version the record was read from (see [`XRecord::is_convertible`]).
    #[cfg_attr(feature = "serde", serde(skip))]
    pub read_version: AsRead<Version>,
}

/// Reads extended record values from the record's data bytes.
struct ItemReader<'a> {
    bytes: &'a [u8],
    pos: usize,
    version: Version,
}

impl ItemReader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.bytes.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(s)
    }

    fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N)?.try_into().ok()
    }

    fn f64(&mut self) -> Option<f64> {
        self.array().map(f64::from_le_bytes)
    }

    /// A text value, and whether it decoded lossily; `None` for R2007+ text
    /// that is not UTF-16 (a lone surrogate), which is kept as bytes (see
    /// [`XRecordValue::Unknown`]): unlike older text, which converts to
    /// R2018 anyway, it can be written back unchanged.
    fn text(&mut self) -> Option<(String, bool)> {
        let n = usize::from(u16::from_le_bytes(self.array()?));
        if self.version >= Version::R2007 {
            let bytes = self.take(n.checked_mul(2)?)?;
            match strings::decode_utf16le_checked(bytes) {
                (_, true) => None,
                text => Some(text),
            }
        } else {
            let codepage = self.array::<1>()?[0];
            let bytes = self.take(n)?;
            Some(strings::decode_checked(u16::from(codepage), bytes))
        }
    }

    /// One value of type `t`, and whether it decoded lossily; `None` when
    /// the bytes do not hold one.
    fn value(&mut self, t: GroupCodeType) -> Option<(XRecordValue, bool)> {
        use GroupCodeType as T;
        let v = match t {
            T::Text => {
                let (s, lossy) = self.text()?;
                return Some((XRecordValue::Text(s), lossy));
            }
            T::HandleText => {
                let (s, lossy) = self.text()?;
                return Some((XRecordValue::HandleText(s), lossy));
            }
            T::Point => XRecordValue::Point([self.f64()?, self.f64()?, self.f64()?]),
            T::Real => XRecordValue::Real(self.f64()?),
            T::Int16 => XRecordValue::Int16(i16::from_le_bytes(self.array()?)),
            T::Int32 => XRecordValue::Int32(i32::from_le_bytes(self.array()?)),
            T::Int64 => XRecordValue::Int64(i64::from_le_bytes(self.array()?)),
            T::Byte => XRecordValue::Byte(self.array::<1>()?[0]),
            T::Bool => match self.array::<1>()?[0] {
                0 => XRecordValue::Bool(false),
                1 => XRecordValue::Bool(true),
                _ => return None,
            },
            T::Binary => {
                let n = usize::from(self.array::<1>()?[0]);
                XRecordValue::Binary(self.take(n)?.to_vec())
            }
            T::ObjectId => XRecordValue::ObjectId(Handle(u64::from_le_bytes(self.array()?))),
        };
        Some((v, false))
    }
}

/// Parse extended record data bytes into items. An item that cannot be read
/// as its type, or of a code with no type, ends the list as
/// [`XRecordValue::Unknown`] with the rest of the bytes from the item's
/// start. Returns the items
/// and whether a text decoded lossily.
fn parse_items(bytes: &[u8], version: Version) -> (Vec<(i16, XRecordValue)>, bool) {
    let mut r = ItemReader {
        bytes,
        pos: 0,
        version,
    };
    let mut items = Vec::new();
    let mut lossy = false;
    while r.pos < bytes.len() {
        let start = r.pos;
        let Some(code) = r.array().map(i16::from_le_bytes) else {
            // A single trailing byte: no room for a group code.
            items.push((0, XRecordValue::Unknown(bytes[start..].to_vec())));
            return (items, lossy);
        };
        match group_code_type(code).and_then(|t| r.value(t)) {
            Some((v, l)) => {
                lossy |= l;
                items.push((code, v));
            }
            None => {
                let rest = bytes.get(start..).unwrap_or_default().to_vec();
                items.push((code, XRecordValue::Unknown(rest)));
                return (items, lossy);
            }
        }
    }
    (items, lossy)
}

/// Write extended record items as R2018 data bytes.
fn items_bytes(items: &[(i16, XRecordValue)]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let text = |out: &mut Vec<u8>, s: &str| -> Result<()> {
        let units: Vec<u16> = s.encode_utf16().collect();
        let n = u16::try_from(units.len())
            .map_err(|_| encode_err("extended record text longer than 65535 units"))?;
        out.extend_from_slice(&n.to_le_bytes());
        out.extend(units.iter().flat_map(|u| u.to_le_bytes()));
        Ok(())
    };
    for (i, (code, value)) in items.iter().enumerate() {
        match value.type_of() {
            Some(t) if group_code_type(*code) != Some(t) => {
                return Err(encode_err(format!(
                    "extended record group code {code} cannot hold a {t:?} value"
                )));
            }
            None if i + 1 != items.len() => {
                return Err(encode_err(
                    "an unknown extended record value must be the last item",
                ));
            }
            _ => {}
        }
        if let XRecordValue::Unknown(b) = value {
            let stored = match b.as_slice() {
                [lo, hi, ..] => i16::from_le_bytes([*lo, *hi]),
                _ => 0,
            };
            if stored != *code {
                return Err(encode_err(format!(
                    "an unknown extended record item's bytes start with code {stored}, not {code}"
                )));
            }
            out.extend_from_slice(b);
            continue;
        }
        out.extend_from_slice(&code.to_le_bytes());
        match value {
            XRecordValue::Text(s) | XRecordValue::HandleText(s) => text(&mut out, s)?,
            XRecordValue::Point(p) => {
                for c in p {
                    out.extend_from_slice(&c.to_le_bytes());
                }
            }
            XRecordValue::Real(v) => out.extend_from_slice(&v.to_le_bytes()),
            XRecordValue::Int16(v) => out.extend_from_slice(&v.to_le_bytes()),
            XRecordValue::Int32(v) => out.extend_from_slice(&v.to_le_bytes()),
            XRecordValue::Int64(v) => out.extend_from_slice(&v.to_le_bytes()),
            XRecordValue::Byte(v) => out.push(*v),
            XRecordValue::Bool(v) => out.push(u8::from(*v)),
            XRecordValue::Binary(b) => {
                let n = u8::try_from(b.len())
                    .map_err(|_| encode_err("extended record binary chunk over 255 bytes"))?;
                out.push(n);
                out.extend_from_slice(b);
            }
            XRecordValue::ObjectId(h) => out.extend_from_slice(&h.0.to_le_bytes()),
            XRecordValue::Unknown(_) => unreachable!("written above"),
        }
    }
    Ok(out)
}

impl XRecord {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let n = count(r.data.read_bl()?, "extended record byte")?;
        check_fits(r, n, 8, "extended record data")?;
        let bytes = r.data.read_bytes(n)?;
        let (items, lossy) = parse_items(&bytes, r.version);
        if lossy {
            r.diagnostics.push(crate::diag::Diagnostic {
                severity: crate::diag::Severity::Warning,
                handle: Some(r.handle),
                message: "extended record text is malformed; it became U+FFFD".into(),
            });
        }
        let cloning = r.data.read_bs()?;
        let (object_ids, handle_tail) = trailing_refs(r)?;
        Ok(XRecord {
            items,
            cloning,
            object_ids,
            handle_tail,
            read_version: AsRead::new(r.version),
        })
    }

    /// Whether the record can be written as R2018: not when it was read
    /// before R2007 and keeps the bytes of an unknown item, whose text (if
    /// any) would be in the old code page form.
    pub fn is_convertible(&self) -> bool {
        let old = self.read_version.get().is_some_and(|v| *v < Version::R2007);
        !(old
            && self
                .items
                .iter()
                .any(|(_, v)| matches!(v, XRecordValue::Unknown(_))))
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        if !self.is_convertible() {
            return Err(encode_err(
                "an unknown extended record item read before R2007 cannot be written as R2018",
            ));
        }
        let bytes = items_bytes(&self.items)?;
        write_count(w, bytes.len(), "extended record bytes")?;
        w.data.write_bytes(&bytes);
        w.data.write_bs(self.cloning);
        for &h in &self.object_ids {
            w.href(h)?;
        }
        self.handle_tail.write(w)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::bits::BitWriter;

    use crate::error::Error;
    use crate::handle::{Handle, HandleRef};
    use crate::object::entities::tests::{reader, roundtrip};
    use crate::object::objects::test_util::H;

    use crate::object::stream::tests::assemble;
    use crate::object::stream::{ObjReader, ObjWriter};

    use crate::version::Version;

    /// The group code type table, pinned at the edges of every range.
    /// 480-481 handle texts are references: parsed as hexadecimal and
    /// written back in the stored form.
    #[test]
    fn handle_text_parse_and_format() {
        assert!(handle_text_is_reference(480) && handle_text_is_reference(481));
        assert!(!handle_text_is_reference(320) && !handle_text_is_reference(5));
        assert_eq!(parse_handle_text("1F"), Some(Handle(0x1F)));
        assert_eq!(parse_handle_text("00a0"), Some(Handle(0xA0)));
        assert_eq!(parse_handle_text(""), None);
        assert_eq!(parse_handle_text("-1"), None);
        assert_eq!(parse_handle_text("+1"), None);
        assert_eq!(parse_handle_text("12345678901234567"), None);
        assert_eq!(format_handle_text(Handle(0x101F), "1F"), "101F");
        assert_eq!(format_handle_text(Handle(0x10A0), "00a0"), "10a0");
        assert_eq!(format_handle_text(Handle(0xB), "000A"), "000B");
        assert_eq!(format_handle_text(Handle(0xAB), "12"), "AB");
    }

    #[test]
    fn group_code_type_table() {
        use GroupCodeType::*;
        let table = [
            (0, Some(Text)),
            (4, Some(Text)),
            (5, Some(HandleText)),
            (9, Some(Text)),
            (10, Some(Point)),
            (39, Some(Point)),
            (40, Some(Real)),
            (59, Some(Real)),
            (60, Some(Int16)),
            (79, Some(Int16)),
            (80, None),
            (89, None),
            (90, Some(Int32)),
            (99, Some(Int32)),
            (100, Some(Text)),
            (102, Some(Text)),
            (103, None),
            (105, Some(HandleText)),
            (106, None),
            (110, Some(Real)),
            (149, Some(Real)),
            (150, None),
            (160, Some(Int64)),
            (169, Some(Int64)),
            (170, Some(Int16)),
            (179, Some(Int16)),
            (180, None),
            (210, Some(Real)),
            (239, Some(Real)),
            (240, None),
            (270, Some(Int16)),
            (279, Some(Int16)),
            (280, Some(Byte)),
            (289, Some(Byte)),
            (290, Some(Bool)),
            (299, Some(Bool)),
            (300, Some(Text)),
            (309, Some(Text)),
            (310, Some(Binary)),
            (319, Some(Binary)),
            (320, Some(HandleText)),
            (329, Some(HandleText)),
            (330, Some(ObjectId)),
            (369, Some(ObjectId)),
            (370, Some(Int16)),
            (389, Some(Int16)),
            (390, Some(ObjectId)),
            (399, Some(ObjectId)),
            (400, Some(Int16)),
            (409, Some(Int16)),
            (410, Some(Text)),
            (419, Some(Text)),
            (420, Some(Int32)),
            (429, Some(Int32)),
            (430, Some(Text)),
            (439, Some(Text)),
            (440, Some(Int32)),
            (459, Some(Int32)),
            (460, Some(Real)),
            (469, Some(Real)),
            (470, Some(Text)),
            (479, Some(Text)),
            (480, Some(HandleText)),
            (481, Some(HandleText)),
            (482, None),
            (998, None),
            (999, Some(Text)),
            (1003, Some(Text)),
            (1004, Some(Binary)),
            (1005, Some(ObjectId)),
            (1009, Some(ObjectId)),
            (1010, Some(Real)),
            (1059, Some(Real)),
            (1060, Some(Int16)),
            (1070, Some(Int16)),
            (1071, Some(Int32)),
            (1072, None),
            (-1, None),
            (i16::MAX, None),
        ];
        for (code, t) in table {
            assert_eq!(group_code_type(code), t, "{code}");
        }
    }

    pub(crate) fn xrecord() -> XRecord {
        XRecord {
            items: vec![
                (1, XRecordValue::Text("Grün 日本😀".into())),
                (5, XRecordValue::HandleText("1F".into())),
                (10, XRecordValue::Point([1.0, -2.5, 3.0])),
                (40, XRecordValue::Real(0.125)),
                (70, XRecordValue::Int16(-3)),
                (90, XRecordValue::Int32(-70000)),
                (160, XRecordValue::Int64(-1 << 40)),
                (280, XRecordValue::Byte(200)),
                (290, XRecordValue::Bool(true)),
                (310, XRecordValue::Binary(vec![0, 1, 0xFF])),
                (330, XRecordValue::ObjectId(Handle(0x1234_5678_9ABC))),
                (1071, XRecordValue::Int32(7)),
            ],
            cloning: 1,
            object_ids: vec![],
            handle_tail: HandleTail::default(),
            read_version: Default::default(),
        }
    }

    #[test]
    fn roundtrip_xrecord() {
        roundtrip(&xrecord(), XRecord::encode, XRecord::decode, 79);
        // With object ids after the common references and an unknown
        // trailing item.
        let mut x = xrecord();
        x.items
            .push((2000, XRecordValue::Unknown(vec![0xD0, 0x07, 9, 8, 7])));
        x.object_ids = vec![
            HandleRef {
                code: 3,
                value: 0x91,
            },
            HandleRef { code: 5, value: 0 },
        ];
        let mut w = ObjWriter::new(H);
        x.encode(&mut w).unwrap();
        let bytes = w.finish(79).unwrap();
        assert_eq!(XRecord::decode(&mut reader(&bytes)).unwrap(), x);
    }

    /// Items that cannot be read as their type, or of a code with no type,
    /// end the list with the rest of the bytes.
    #[test]
    fn xrecord_unknown_items_keep_their_bytes() {
        let v = Version::R2018;
        // Each case: the bytes and the items they parse to; written back,
        // the same bytes.
        type Items = Vec<(i16, XRecordValue)>;
        let cases: [(&[u8], Items); 5] = [
            // R2007+ text that is not UTF-16 (a lone surrogate), as Civil
            // 3D's material records hold it (`camber-01` of the extended
            // corpus): kept as bytes, with the items after it.
            (
                &[0x2C, 0x01, 2, 0, 0x41, 0, 0x00, 0xD8, 0x46, 0x00, 5, 0],
                vec![(
                    300,
                    XRecordValue::Unknown(vec![
                        0x2C, 0x01, 2, 0, 0x41, 0, 0x00, 0xD8, 0x46, 0x00, 5, 0,
                    ]),
                )],
            ),
            // A Bool that is neither 0 nor 1.
            (
                &[0x22, 0x01, 0x02, 0x46, 0x00, 0x05, 0x00],
                vec![(
                    290,
                    XRecordValue::Unknown(vec![0x22, 0x01, 2, 0x46, 0, 5, 0]),
                )],
            ),
            // A code with no type.
            (
                &[0xD0, 0x07, 1, 2],
                vec![(2000, XRecordValue::Unknown(vec![0xD0, 0x07, 1, 2]))],
            ),
            // A value cut short.
            (
                &[0x28, 0x00, 1, 2, 3],
                vec![(40, XRecordValue::Unknown(vec![0x28, 0, 1, 2, 3]))],
            ),
            // A lone trailing byte: no room for a code.
            (
                &[0x46, 0x00, 5, 0, 0x99],
                vec![
                    (70, XRecordValue::Int16(5)),
                    (0, XRecordValue::Unknown(vec![0x99])),
                ],
            ),
        ];
        for (bytes, want) in cases {
            let (items, lossy) = parse_items(bytes, v);
            assert!(!lossy);
            assert_eq!(items, want, "{bytes:02X?}");
            assert_eq!(items_bytes(&items).unwrap(), bytes, "{bytes:02X?}");
        }
        // An unknown item's code must match its bytes.
        let x = XRecord {
            items: vec![(70, XRecordValue::Unknown(vec![0xD0, 0x07]))],
            ..XRecord::default()
        };
        assert!(x.encode(&mut ObjWriter::new(H)).is_err());
        // Encoding checks the type against the code and that an unknown
        // item is last.
        for items in [
            vec![(70, XRecordValue::Real(1.0))],
            vec![
                (0, XRecordValue::Unknown(vec![])),
                (70, XRecordValue::Int16(1)),
            ],
            vec![(310, XRecordValue::Binary(vec![0; 256]))],
        ] {
            let x = XRecord {
                items,
                ..XRecord::default()
            };
            assert!(x.encode(&mut ObjWriter::new(H)).is_err(), "{x:?}");
        }
    }

    /// Before R2007 an extended record's text is code page bytes after its
    /// own code page byte. It converts to R2018 text, unless an unknown item
    /// keeps bytes whose layout may hold such text.
    #[test]
    fn r2004_xrecord_converts() {
        let mut data = vec![0x01, 0x00, 0x02, 0x00, 38, 0x82, 0xA0]; // 1: "あ" in Shift-JIS
        data.extend_from_slice(&[0x46, 0x00, 0x05, 0x00]); // 70: 5
        data.extend_from_slice(&[0x4A, 0x01]); // 330
        data.extend_from_slice(&0x2Au64.to_le_bytes());
        let mut d = BitWriter::new();
        d.write_bl(data.len() as i32);
        d.write_bytes(&data);
        d.write_bs(1);
        let bytes = assemble(Version::R2004, 79, &d, None, &BitWriter::new());
        let (mut r, _) = ObjReader::split(&bytes, Version::R2004, 30).unwrap();
        let x = XRecord::decode(&mut r).unwrap();
        assert_eq!(
            x.items,
            [
                (1, XRecordValue::Text("あ".into())),
                (70, XRecordValue::Int16(5)),
                (330, XRecordValue::ObjectId(Handle(0x2A))),
            ]
        );
        assert!(x.is_convertible());
        let mut w = ObjWriter::new(H);
        x.encode(&mut w).unwrap();
        let out = w.finish(79).unwrap();
        assert_eq!(XRecord::decode(&mut reader(&out)).unwrap(), x);
        // With an unknown item.
        data.extend_from_slice(&[0xD0, 0x07, 1]);
        let mut d = BitWriter::new();
        d.write_bl(data.len() as i32);
        d.write_bytes(&data);
        d.write_bs(1);
        let bytes = assemble(Version::R2004, 79, &d, None, &BitWriter::new());
        let (mut r, _) = ObjReader::split(&bytes, Version::R2004, 30).unwrap();
        let x = XRecord::decode(&mut r).unwrap();
        assert!(!x.is_convertible());
        assert!(x.encode(&mut ObjWriter::new(H)).is_err());
        // The data length is checked before allocating.
        let mut d = BitWriter::new();
        d.write_bl(i32::MAX);
        let bytes = assemble(Version::R2004, 79, &d, None, &BitWriter::new());
        let (mut r, _) = ObjReader::split(&bytes, Version::R2004, 30).unwrap();
        assert!(matches!(
            XRecord::decode(&mut r),
            Err(Error::Truncated { .. })
        ));
    }
}
