//! Drawing code pages: byte text in R2000/R2004 files.
//!
//! The DWG code page is an index stored in the file header (`$DWGCODEPAGE`). ODS
//! names the field (§3.2.5) but prints no index table, so the table below follows
//! ACadSharp (MIT), `CadUtils._pageCodes`, which matches the `$DWGCODEPAGE`
//! names AutoCAD writes to DXF (e.g. 30 = ANSI_1252, 38 = ANSI_932).
//!
//! Code pages that `encoding_rs` cannot decode (the DOS OEM pages and Johab) and
//! unknown indices fall back to Windows-1252; [`is_supported`] reports this so
//! the caller can add a `Diagnostic`.

use encoding_rs::{
    BIG5, EUC_KR, Encoding, GBK, IBM866, ISO_8859_2, ISO_8859_3, ISO_8859_4, ISO_8859_5,
    ISO_8859_6, ISO_8859_7, ISO_8859_8, MACINTOSH, SHIFT_JIS, UTF_16LE, WINDOWS_874, WINDOWS_1250,
    WINDOWS_1251, WINDOWS_1252, WINDOWS_1253, WINDOWS_1254, WINDOWS_1255, WINDOWS_1256,
    WINDOWS_1257, WINDOWS_1258,
};

/// `ANSI_1252`, the fallback code page.
pub const DEFAULT_CODEPAGE: u16 = 30;

/// The `encoding_rs` encoding for a DWG code page index, or `None` when the
/// index is unknown or not supported by `encoding_rs`.
pub fn encoding_for(codepage: u16) -> Option<&'static Encoding> {
    // Where a name below differs from the encoding, the encoding is the WHATWG
    // superset: ISO 8859-1 and US-ASCII decode as Windows-1252, ISO 8859-9 as
    // Windows-1254, GB2312 as GBK, KSC5601 and ANSI_949 as EUC-KR (UHC).
    Some(match codepage {
        1 | 2 => WINDOWS_1252, // US-ASCII, ISO 8859-1
        3 => ISO_8859_2,
        4 => ISO_8859_3,
        5 => ISO_8859_4,
        6 => ISO_8859_5,
        7 => ISO_8859_6,
        8 => ISO_8859_7,
        9 => ISO_8859_8,
        10 => WINDOWS_1254, // ISO 8859-9
        // 11-21: DOS 437, 850, 852, 855, 857, 860, 861, 863, 864, 865, 869.
        22 | 38 => SHIFT_JIS, // DOS932, ANSI_932
        23 => MACINTOSH,
        24 | 41 => BIG5,   // BIG5, ANSI_950
        25 | 40 => EUC_KR, // KSC5601, ANSI_949
        // 26 and 42: Johab (ANSI_1361).
        27 => IBM866,
        28 => WINDOWS_1250,
        29 => WINDOWS_1251,
        30 => WINDOWS_1252,
        31 | 39 => GBK, // GB2312, ANSI_936
        32 => WINDOWS_1253,
        33 => WINDOWS_1254,
        34 => WINDOWS_1255,
        35 => WINDOWS_1256,
        36 => WINDOWS_1257,
        37 => WINDOWS_874,
        43 => UTF_16LE, // ANSI_1200
        44 => WINDOWS_1258,
        _ => return None,
    })
}

/// Whether `codepage` decodes as itself rather than through the 1252 fallback.
pub fn is_supported(codepage: u16) -> bool {
    encoding_for(codepage).is_some()
}

/// Decode code page bytes. Unknown code pages decode as Windows-1252; malformed
/// sequences become U+FFFD.
pub fn decode(codepage: u16, bytes: &[u8]) -> String {
    decode_checked(codepage, bytes).0
}

/// Like [`decode`], also reporting whether any byte sequence was malformed.
pub(crate) fn decode_checked(codepage: u16, bytes: &[u8]) -> (String, bool) {
    let enc = encoding_for(codepage).unwrap_or(WINDOWS_1252);
    let (text, malformed) = enc.decode_without_bom_handling(bytes);
    (text.into_owned(), malformed)
}

/// Decode UTF-16LE bytes (an odd trailing byte is ignored), also reporting
/// whether an unpaired surrogate was replaced with U+FFFD.
pub(crate) fn decode_utf16le_checked(bytes: &[u8]) -> (String, bool) {
    let units = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]));
    let mut replaced = false;
    let text = char::decode_utf16(units)
        .map(|c| {
            c.unwrap_or_else(|_| {
                replaced = true;
                char::REPLACEMENT_CHARACTER
            })
        })
        .collect();
    (text, replaced)
}

/// Encode to code page bytes (unmappable characters become HTML numeric
/// character references, as `encoding_rs` does): lossy, so the library's
/// writers use [`encode_exact`]. Used for test fixtures.
#[doc(hidden)]
pub fn encode(codepage: u16, s: &str) -> Vec<u8> {
    encode_with_errors(codepage, s).0
}

/// Encode to code page bytes; `None` when the code page cannot hold every
/// character of `s`.
pub(crate) fn encode_exact(codepage: u16, s: &str) -> Option<Vec<u8>> {
    let (bytes, had_errors) = encode_with_errors(codepage, s);
    (!had_errors).then_some(bytes)
}

fn encode_with_errors(codepage: u16, s: &str) -> (Vec<u8>, bool) {
    let enc = encoding_for(codepage).unwrap_or(WINDOWS_1252);
    if enc == UTF_16LE {
        // encoding_rs encodes UTF-16 as UTF-8 (WHATWG), so do it here.
        return (s.encode_utf16().flat_map(u16::to_le_bytes).collect(), false);
    }
    let (bytes, _, had_errors) = enc.encode(s);
    (bytes.into_owned(), had_errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codepage_strings() {
        assert_eq!(decode(38, &[0x82, 0xA0]), "あ");
        assert_eq!(decode(39, &[0xC4, 0xE3]), "你");
        // Unknown code page: Windows-1252, where 0x80 is the euro sign.
        assert!(!is_supported(999));
        assert_eq!(decode(999, &[0x41, 0x80]), "A€");
        assert_eq!(decode(30, &[0x41, 0x80]), "A€");
    }

    #[test]
    fn codepage_table_spot_checks() {
        assert_eq!(decode(22, &[0x82, 0xA0]), "あ"); // DOS932
        assert_eq!(decode(31, &[0xC4, 0xE3]), "你"); // GB2312
        assert_eq!(decode(40, &[0xB0, 0xA1]), "가"); // ANSI_949
        assert_eq!(decode(25, &[0xB0, 0xA1]), "가"); // KSC5601
        assert_eq!(decode(41, &[0xA4, 0x40]), "一"); // ANSI_950
        assert_eq!(decode(24, &[0xA4, 0x40]), "一"); // BIG5
        assert_eq!(decode(29, &[0xC0]), "А"); // ANSI_1251 (Cyrillic A)
        assert_eq!(decode(28, &[0x8A]), "Š"); // ANSI_1250
        assert_eq!(decode(32, &[0xC1]), "Α"); // ANSI_1253 (Greek Alpha)
        assert_eq!(decode(27, &[0x80]), "А"); // DOS866
        assert_eq!(decode(43, &[0x41, 0x00]), "A"); // ANSI_1200 (UTF-16LE)
        assert_eq!(decode(44, &[0xC3]), "Ă"); // ANSI_1258
        // DOS OEM pages and Johab are not in encoding_rs: fallback.
        for cp in [0, 11, 12, 21, 26, 42, 45] {
            assert!(!is_supported(cp), "{cp}");
        }
    }

    #[test]
    fn encode_roundtrips() {
        for (cp, s) in [
            (38, "あいう"),
            (39, "你好"),
            (40, "가"),
            (41, "一"),
            (30, "é€"),
            (43, "x😀"),
        ] {
            assert_eq!(decode(cp, &encode(cp, s)), s, "{cp}");
        }
        assert_eq!(encode(999, "é"), [0xE9]);
    }

    #[test]
    fn decode_reports_malformed() {
        // A lone Shift-JIS lead byte.
        let (s, bad) = decode_checked(38, &[0x82]);
        assert!(bad);
        assert_eq!(s, "\u{FFFD}");
        assert!(!decode_checked(38, &[0x82, 0xA0]).1);
    }
}
