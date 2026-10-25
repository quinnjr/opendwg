//! The small informational sections: summary information (ODS 13,
//! `AcDb:SummaryInfo`), the preview image (ODS 14, `AcDb:Preview`) and the
//! application information (ODS 16, `AcDb:AppInfo`).
//!
//! The data storage section (AcDs) is [`super::acds`]. Other sections
//! (FileDepList, RevHistory, Security, Template, ObjFreeSpace, AuxHeader,
//! ...) are kept as read in [`crate::Drawing::extra_sections`].

use crate::diag::{Diagnostic, Severity};
use crate::error::{Error, Result};
use crate::object::AsRead;
use crate::section::{HeaderVars, JulianDate};
use crate::strings;
use crate::version::Version;

/// Summary information (ODS 13): title, author and the like.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct SummaryInfo {
    pub title: String,
    pub subject: String,
    pub author: String,
    pub keywords: String,
    pub comments: String,
    pub last_saved_by: String,
    pub revision_number: String,
    pub hyperlink_base: String,
    /// A duration: days and milliseconds (ODS: "ODA writes two zero
    /// Int32's"; AutoCAD stores the editing time). A copy of the header's
    /// `$TDINDWG`, like `create` of `$TDCREATE` and `modified` of
    /// `$TDUPDATE` (equal in every corpus file): see
    /// [`SummaryInfo::written_dates`].
    pub total_edit_time: JulianDate,
    pub create: JulianDate,
    pub modified: JulianDate,
    /// Custom properties: name and value.
    pub custom: Vec<(String, String)>,
    /// The two Int32s that end the section (ODS: unknown, write 0).
    pub unknown: [i32; 2],
    /// The header's `$TDINDWG`, `$TDCREATE` and `$TDUPDATE` when the summary
    /// was read with its drawing (see [`SummaryInfo::written_dates`]).
    #[cfg_attr(feature = "serde", serde(skip))]
    pub read_header_dates: AsRead<[JulianDate; 3]>,
}

/// The preview images (ODS 14.2): the header data and the images, kept as
/// stored.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Preview {
    /// The header data (code 1).
    pub header: Vec<u8>,
    /// A BMP image (code 2).
    pub bmp: Option<Vec<u8>>,
    /// A WMF image (code 3).
    pub wmf: Option<Vec<u8>>,
    /// A PNG image (code 6).
    pub png: Option<Vec<u8>>,
}

/// One string of the application information, with the 16 bytes before it
/// (ODS: a checksum, ODA writes zeroes).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct AppInfoEntry {
    pub checksum: [u8; 16],
    pub text: String,
}

/// The application that wrote the file (ODS 16).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct AppInfo {
    /// The first `UInt32` (AutoCAD writes 3; ODS: ODA writes 2).
    pub class_version: u32,
    /// "AppInfoDataList".
    pub name: String,
    pub entries: Vec<AppInfoEntry>,
    /// ODS's trailing "App info version" string, which AutoCAD does not
    /// write.
    pub version: Option<String>,
}

/// Reads little-endian values from a section's bytes; every length is
/// checked against the bytes left before anything is allocated.
struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
    context: &'static str,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8], context: &'static str) -> Self {
        Cursor {
            bytes,
            pos: 0,
            context,
        }
    }

    fn left(&self) -> usize {
        self.bytes.len() - self.pos
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.bytes.len())
            .ok_or(Error::Truncated {
                context: self.context,
            })?;
        let s = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut a = [0; N];
        a.copy_from_slice(self.take(N)?);
        Ok(a)
    }

    fn u16(&mut self) -> Result<u16> {
        self.array().map(u16::from_le_bytes)
    }

    fn u32(&mut self) -> Result<u32> {
        self.array().map(u32::from_le_bytes)
    }

    fn i32(&mut self) -> Result<i32> {
        self.array().map(i32::from_le_bytes)
    }

    fn date(&mut self) -> Result<JulianDate> {
        Ok(JulianDate {
            day: self.i32()?,
            ms: self.i32()?,
        })
    }

    /// A string: a 16-bit length, then that many UTF-16 units (`wide`) or
    /// code page bytes, the terminating NUL included. Trailing NULs are
    /// dropped. Returns whether the text decoded lossily.
    fn text(&mut self, wide: bool, codepage: u16) -> Result<(String, bool)> {
        let n = usize::from(self.u16()?);
        let (mut s, lossy) = if wide {
            strings::decode_utf16le_checked(self.take(n * 2)?)
        } else {
            strings::decode_checked(codepage, self.take(n)?)
        };
        s.truncate(s.trim_end_matches('\0').len());
        Ok((s, lossy))
    }

    fn end(&self) -> Result<()> {
        if self.left() == 0 {
            Ok(())
        } else {
            Err(Error::Corrupt(format!(
                "{}: {} bytes after the end",
                self.context,
                self.left()
            )))
        }
    }
}

/// A string in the R2018 form: a 16-bit count of UTF-16 units with the
/// terminating NUL, the units and the NUL.
fn write_text(out: &mut Vec<u8>, s: &str, context: &str) -> Result<()> {
    let units: Vec<u16> = s.encode_utf16().chain([0]).collect();
    let n = u16::try_from(units.len())
        .map_err(|_| Error::Encode(format!("{context}: text longer than 65534 units")))?;
    out.extend_from_slice(&n.to_le_bytes());
    out.extend(units.iter().flat_map(|u| u.to_le_bytes()));
    Ok(())
}

fn lossy_warning(context: &str) -> Diagnostic {
    Diagnostic {
        severity: Severity::Warning,
        handle: None,
        message: format!("{context}: text is malformed; it became U+FFFD"),
    }
}

impl SummaryInfo {
    /// Decode the section's bytes. Strings are UTF-16 from R2007, in the
    /// drawing's code page `codepage` before (as ACadSharp (MIT)
    /// `DwgSummaryInfoReader` reads them). Lossy text adds a warning to
    /// `diagnostics`.
    pub fn decode(
        bytes: &[u8],
        version: Version,
        codepage: u16,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Result<SummaryInfo> {
        const CONTEXT: &str = "summary info";
        let wide = version >= Version::R2007;
        let mut c = Cursor::new(bytes, CONTEXT);
        let mut lossy = false;
        let mut text = |c: &mut Cursor| -> Result<String> {
            let (s, l) = c.text(wide, codepage)?;
            lossy |= l;
            Ok(s)
        };
        let mut fields: [String; 8] = Default::default();
        for f in &mut fields {
            *f = text(&mut c)?;
        }
        let total_edit_time = c.date()?;
        let create = c.date()?;
        let modified = c.date()?;
        let n = c.u16()? as i16;
        // Each property takes at least two 2-byte lengths.
        let n = usize::try_from(n)
            .ok()
            .filter(|&n| n * 4 <= c.left())
            .ok_or_else(|| Error::Corrupt(format!("{CONTEXT}: property count {n}")))?;
        let mut custom = Vec::with_capacity(n);
        for _ in 0..n {
            let name = text(&mut c)?;
            custom.push((name, text(&mut c)?));
        }
        let unknown = [c.i32()?, c.i32()?];
        c.end()?;
        if lossy {
            diagnostics.push(lossy_warning(CONTEXT));
        }
        let [
            title,
            subject,
            author,
            keywords,
            comments,
            last_saved_by,
            revision_number,
            hyperlink_base,
        ] = fields;
        Ok(SummaryInfo {
            read_header_dates: AsRead::default(),
            title,
            subject,
            author,
            keywords,
            comments,
            last_saved_by,
            revision_number,
            hyperlink_base,
            total_edit_time,
            create,
            modified,
            custom,
            unknown,
        })
    }

    /// The header's `$TDINDWG`, `$TDCREATE` and `$TDUPDATE`.
    fn header_dates(header: &HeaderVars) -> [JulianDate; 3] {
        [header.tdindwg, header.tdcreate, header.tdupdate]
    }

    /// Record `header`'s dates as the ones read with the summary.
    pub(crate) fn set_read_header(&mut self, header: &HeaderVars) {
        self.read_header_dates = AsRead::new(Self::header_dates(header));
    }

    /// The total editing time, creation and modification dates written:
    /// each as the summary holds it while the header variable it copies
    /// (`$TDINDWG`, `$TDCREATE`, `$TDUPDATE`) is unchanged since the summary
    /// was read with it; otherwise, and for a summary not read from a file,
    /// the header variable's value.
    pub fn written_dates(&self, header: &HeaderVars) -> [JulianDate; 3] {
        let now = Self::header_dates(header);
        let kept = [self.total_edit_time, self.create, self.modified];
        let read = self.read_header_dates.get();
        std::array::from_fn(|i| match read {
            Some(r) if r[i] == now[i] => kept[i],
            _ => now[i],
        })
    }

    /// The section's bytes in the R2018 form, with the dates of
    /// [`SummaryInfo::written_dates`].
    pub fn encode_r2018(&self, header: &HeaderVars) -> Result<Vec<u8>> {
        const CONTEXT: &str = "summary info";
        let mut out = Vec::new();
        for s in [
            &self.title,
            &self.subject,
            &self.author,
            &self.keywords,
            &self.comments,
            &self.last_saved_by,
            &self.revision_number,
            &self.hyperlink_base,
        ] {
            write_text(&mut out, s, CONTEXT)?;
        }
        for d in self.written_dates(header) {
            out.extend_from_slice(&d.day.to_le_bytes());
            out.extend_from_slice(&d.ms.to_le_bytes());
        }
        let n = i16::try_from(self.custom.len())
            .map_err(|_| Error::Encode(format!("{CONTEXT}: too many properties")))?;
        out.extend_from_slice(&n.to_le_bytes());
        for (name, value) in &self.custom {
            write_text(&mut out, name, CONTEXT)?;
            write_text(&mut out, value, CONTEXT)?;
        }
        for u in self.unknown {
            out.extend_from_slice(&u.to_le_bytes());
        }
        Ok(out)
    }
}

/// The preview sentinels (ODS 14.2).
const PREVIEW_START: [u8; 16] = [
    0x1F, 0x25, 0x6D, 0x07, 0xD4, 0x36, 0x28, 0x28, 0x9D, 0x57, 0xCA, 0x3F, 0x9D, 0x44, 0x10, 0x2B,
];
const PREVIEW_END: [u8; 16] = [
    0xE0, 0xDA, 0x92, 0xF8, 0x2B, 0xC9, 0xD7, 0xD7, 0x62, 0xA8, 0x35, 0xC0, 0x62, 0xBB, 0xEF, 0xD4,
];

/// The preview entry codes: header data, BMP, WMF and PNG (ODS gives 1-3;
/// 6 for PNG as ACadSharp (MIT) `DwgPreview` names it, and as the corpus's
/// R2013+ files store their PNG).
const CODE_HEADER: u8 = 1;
const CODE_BMP: u8 = 2;
const CODE_WMF: u8 = 3;
const CODE_PNG: u8 = 6;

impl Preview {
    /// Decode the section, sentinels included. The entries locate their data
    /// by file address; `address` is the file address of the start sentinel
    /// (the file header's preview address). Without it the data is taken to
    /// start right after the entry table, where AutoCAD writes it.
    pub fn decode(bytes: &[u8], address: Option<u64>) -> Result<Preview> {
        const CONTEXT: &str = "preview";
        let corrupt = |m: &str| Error::Corrupt(format!("{CONTEXT}: {m}"));
        let mut c = Cursor::new(bytes, CONTEXT);
        if c.take(16)? != PREVIEW_START {
            return Err(Error::BadSentinel("preview start"));
        }
        let overall = c.u32()?;
        // The overall size counts the bytes between itself and the end
        // sentinel. AutoCAD can leave stale bytes after the sentinel (the
        // section keeps the size of an older, larger preview), which are
        // ignored.
        let end = usize::try_from(overall)
            .ok()
            .and_then(|n| c.pos.checked_add(n))
            .ok_or_else(|| corrupt(&format!("overall size {overall}")))?;
        match bytes.get(end..).and_then(|b| b.get(..16)) {
            None => return Err(Error::Truncated { context: CONTEXT }),
            Some(s) if s != PREVIEW_END => {
                return Err(if bytes.ends_with(&PREVIEW_END) {
                    corrupt(&format!("overall size {overall}"))
                } else {
                    Error::BadSentinel("preview end")
                });
            }
            Some(_) => {}
        }
        let n = usize::from(c.array::<1>()?[0]);
        let mut entries = Vec::with_capacity(n);
        for _ in 0..n {
            let code = c.array::<1>()?[0];
            entries.push((code, u64::from(c.u32()?), u64::from(c.u32()?)));
        }
        let table_end = c.pos;
        let base = match address {
            Some(a) => a,
            None => match entries.iter().map(|e| e.1).min() {
                Some(first) => first
                    .checked_sub(table_end as u64)
                    .ok_or_else(|| corrupt("entry before the section"))?,
                None => 0,
            },
        };
        let mut p = Preview::default();
        let mut seen = Vec::new();
        for (code, start, size) in entries {
            if seen.contains(&code) {
                return Err(corrupt(&format!("code {code} twice")));
            }
            seen.push(code);
            let data = start
                .checked_sub(base)
                .filter(|&s| s >= table_end as u64)
                .and_then(|s| Some((s, s.checked_add(size)?)))
                .filter(|&(_, e)| e <= end as u64)
                .map(|(s, e)| bytes[s as usize..e as usize].to_vec())
                .ok_or_else(|| corrupt(&format!("code {code} data outside the section")))?;
            match code {
                CODE_HEADER => p.header = data,
                CODE_BMP => p.bmp = Some(data),
                CODE_WMF => p.wmf = Some(data),
                CODE_PNG => p.png = Some(data),
                _ => return Err(corrupt(&format!("unknown code {code}"))),
            }
        }
        Ok(p)
    }

    /// The section's bytes with its start sentinel at file address
    /// `address`: the header entry (when there is a header or an image),
    /// then the BMP, WMF and PNG, their data in that order.
    pub fn encode(&self, address: u64) -> Result<Vec<u8>> {
        let mut parts: Vec<(u8, &[u8])> = Vec::new();
        let images = [
            (CODE_BMP, &self.bmp),
            (CODE_WMF, &self.wmf),
            (CODE_PNG, &self.png),
        ];
        if !self.header.is_empty() || images.iter().any(|(_, i)| i.is_some()) {
            parts.push((CODE_HEADER, &self.header));
        }
        for (code, image) in images {
            if let Some(i) = image {
                parts.push((code, i));
            }
        }
        let too_large = || Error::Encode("preview: too large".into());
        let table_end = 16 + 4 + 1 + 9 * parts.len();
        let data_len: usize = parts.iter().map(|(_, d)| d.len()).sum();
        let overall = u32::try_from(table_end - 20 + data_len).map_err(|_| too_large())?;
        let mut out = PREVIEW_START.to_vec();
        out.extend_from_slice(&overall.to_le_bytes());
        out.push(parts.len() as u8);
        let mut at = address
            .checked_add(table_end as u64)
            .ok_or_else(too_large)?;
        for (code, data) in &parts {
            out.push(*code);
            let start = u32::try_from(at).map_err(|_| too_large())?;
            out.extend_from_slice(&start.to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            at += data.len() as u64;
        }
        u32::try_from(at).map_err(|_| too_large())?;
        for (_, data) in &parts {
            out.extend_from_slice(data);
        }
        out.extend_from_slice(&PREVIEW_END);
        Ok(out)
    }
}

impl AppInfo {
    /// The application information opendwg writes: "opendwg" and the crate
    /// version, laid out as ACadSharp (MIT) `DwgAppInfoWriter` writes its
    /// own (class version 3, then version, comment and product strings with
    /// zero checksums).
    pub fn opendwg() -> AppInfo {
        let version = env!("CARGO_PKG_VERSION");
        let entry = |text: String| AppInfoEntry {
            checksum: [0; 16],
            text,
        };
        AppInfo {
            class_version: 3,
            name: "AppInfoDataList".into(),
            entries: vec![
                entry(version.into()),
                entry(format!("opendwg {version}")),
                entry(format!(
                    "<ProductInformation name =\"opendwg\" build_version=\"{version}\" \
                     registry_version=\"{version}\" install_id_string=\"opendwg\" \
                     registry_localeID=\"1033\"/>"
                )),
            ],
            version: None,
        }
    }

    /// Decode the section's bytes. AutoCAD writes the R21 layout (ODS 16.2:
    /// UTF-16 strings) in every corpus file, R2004 ones included; the second
    /// `UInt32` counts the checksummed strings that follow (two in AutoCAD's
    /// files, three in ODA's, as ACadSharp (MIT) `DwgAppInfoWriter` names
    /// it "num strings"). ODS's trailing 8-bit "App info version" is read
    /// when present. The R18 layout (ODS 16.1: 8-bit strings, starting with
    /// the name) is recognised by its name (`AppInfoDataList`, or
    /// `AppInfoData` as AutoCAD 2004 writes it); its three strings become entries
    /// with zero checksums, the last one the version.
    pub fn decode(bytes: &[u8], diagnostics: &mut Vec<Diagnostic>) -> Result<AppInfo> {
        const CONTEXT: &str = "app info";
        // ODS names the R18 record "AppInfoDataList"; AutoCAD 2004 writes
        // "AppInfoData" (`arx-03` of the extended corpus).
        let name = b"AppInfoData";
        let mut c = Cursor::new(bytes, CONTEXT);
        let mut lossy = false;
        let r18 = bytes.get(2..2 + name.len()) == Some(&name[..]);
        let info = if r18 {
            let mut text = |c: &mut Cursor| -> Result<String> {
                let (s, l) = c.text(false, crate::strings::DEFAULT_CODEPAGE)?;
                lossy |= l;
                Ok(s)
            };
            let name = text(&mut c)?;
            let class_version = c.u32()?;
            let entries = (0..2)
                .map(|_| {
                    Ok(AppInfoEntry {
                        checksum: [0; 16],
                        text: text(&mut c)?,
                    })
                })
                .collect::<Result<_>>()?;
            AppInfo {
                class_version,
                name,
                entries,
                version: Some(text(&mut c)?),
            }
        } else {
            let class_version = c.u32()?;
            let (name, l) = c.text(true, 0)?;
            lossy |= l;
            let n = c.u32()?;
            // Each entry takes at least 18 bytes.
            let n = usize::try_from(n)
                .ok()
                .filter(|&n| n.saturating_mul(18) <= c.left())
                .ok_or_else(|| Error::Corrupt(format!("{CONTEXT}: string count {n}")))?;
            let mut entries = Vec::with_capacity(n);
            for _ in 0..n {
                let checksum = c.array()?;
                let (text, l) = c.text(true, 0)?;
                lossy |= l;
                entries.push(AppInfoEntry { checksum, text });
            }
            let version = if c.left() > 0 {
                let (v, l) = c.text(false, crate::strings::DEFAULT_CODEPAGE)?;
                lossy |= l;
                Some(v)
            } else {
                None
            };
            AppInfo {
                class_version,
                name,
                entries,
                version,
            }
        };
        c.end()?;
        if lossy {
            diagnostics.push(lossy_warning(CONTEXT));
        }
        Ok(info)
    }

    /// The section's bytes in the R21 layout, which R2018 files use.
    pub fn encode_r2018(&self) -> Result<Vec<u8>> {
        const CONTEXT: &str = "app info";
        let mut out = self.class_version.to_le_bytes().to_vec();
        write_text(&mut out, &self.name, CONTEXT)?;
        let n = u32::try_from(self.entries.len())
            .map_err(|_| Error::Encode(format!("{CONTEXT}: too many strings")))?;
        out.extend_from_slice(&n.to_le_bytes());
        for e in &self.entries {
            out.extend_from_slice(&e.checksum);
            write_text(&mut out, &e.text, CONTEXT)?;
        }
        if let Some(v) = &self.version {
            let bytes = crate::strings::encode(crate::strings::DEFAULT_CODEPAGE, v);
            let n = u16::try_from(bytes.len() + 1)
                .map_err(|_| Error::Encode(format!("{CONTEXT}: version too long")))?;
            out.extend_from_slice(&n.to_le_bytes());
            out.extend_from_slice(&bytes);
            out.push(0);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;

    fn date(day: i32, ms: i32) -> JulianDate {
        JulianDate { day, ms }
    }

    /// A header whose dates are `s`'s.
    fn header_of(s: &SummaryInfo) -> HeaderVars {
        HeaderVars {
            tdindwg: s.total_edit_time,
            tdcreate: s.create,
            tdupdate: s.modified,
            ..HeaderVars::default()
        }
    }

    fn summary() -> SummaryInfo {
        SummaryInfo {
            read_header_dates: AsRead::default(),
            title: "Plan ü".into(),
            subject: String::new(),
            author: "日本😀".into(),
            keywords: "a; b".into(),
            comments: "line 1\r\nline 2".into(),
            last_saved_by: "alber".into(),
            revision_number: "7".into(),
            hyperlink_base: "https://example.com/".into(),
            total_edit_time: date(0, 34_765_488),
            create: date(2_459_632, 37_383_000),
            modified: date(2_460_500, 34_038_232),
            custom: vec![
                ("Client".into(), "ACME".into()),
                ("Ä".into(), String::new()),
            ],
            unknown: [0, 0],
        }
    }

    /// The R2018 summary of `acadsharp-sample-ac1032.dwg`: only the last
    /// saved by field is set.
    const R2018_SUMMARY: &str = "01000000 01000000 01000000 01000000 01000000 \
        0600 6100 6c00 6200 6500 7200 0000 01000000 01000000 \
        00000000 b07c1202 f0872500 586b3a02 548d2500 d8640702 0000 00000000 00000000";

    /// The same drawing's R2004 summary: code page strings.
    const R2004_SUMMARY: &str = "010000 010000 010000 010000 010000 0600 616c62657200 \
        010000 010000 00000000 98801202 f0872500 586b3a02 548d2500 c0680702 0000 \
        00000000 00000000";

    fn hex(s: &str) -> Vec<u8> {
        let s: String = s.split_whitespace().collect();
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn summary_corpus_bytes() {
        let bytes = hex(R2018_SUMMARY);
        let mut diags = vec![];
        let s = SummaryInfo::decode(&bytes, Version::R2018, 30, &mut diags).unwrap();
        assert!(diags.is_empty());
        assert_eq!(s.last_saved_by, "alber");
        assert_eq!(s.title, "");
        assert_eq!(s.total_edit_time, date(0, 0x0212_7CB0));
        assert_eq!(s.create, date(0x0025_87F0, 0x023A_6B58));
        assert_eq!(s.modified, date(0x0025_8D54, 0x0207_64D8));
        assert!(s.custom.is_empty());
        assert_eq!(s.encode_r2018(&header_of(&s)).unwrap(), bytes);
        // The R2004 form: one byte per character in the drawing code page.
        let old = SummaryInfo::decode(&hex(R2004_SUMMARY), Version::R2004, 30, &mut diags).unwrap();
        assert_eq!(old.last_saved_by, "alber");
        assert_eq!(old.create, s.create);
        let back = SummaryInfo::decode(
            &old.encode_r2018(&header_of(&old)).unwrap(),
            Version::R2018,
            30,
            &mut diags,
        )
        .unwrap();
        assert_eq!(back, old);
        assert!(diags.is_empty());
    }

    #[test]
    fn roundtrip_summary() {
        let s = summary();
        let bytes = s.encode_r2018(&header_of(&s)).unwrap();
        let mut diags = vec![];
        assert_eq!(
            SummaryInfo::decode(&bytes, Version::R2018, 30, &mut diags).unwrap(),
            s
        );
        assert!(diags.is_empty());
        // Every truncation fails cleanly, and so do trailing bytes.
        for n in 0..bytes.len() {
            assert!(
                SummaryInfo::decode(&bytes[..n], Version::R2018, 30, &mut diags).is_err(),
                "{n}"
            );
        }
        let mut long = bytes.clone();
        long.push(0);
        assert!(SummaryInfo::decode(&long, Version::R2018, 30, &mut diags).is_err());
    }

    /// The dates copy header variables: kept as read while the header's are
    /// unchanged since, else (and for a summary built in code) taken from
    /// the header.
    #[test]
    fn summary_dates_follow_the_header() {
        let s = summary();
        let mut h = header_of(&s);
        h.tdcreate = date(1, 1);
        // Built in code: every date from the header.
        assert_eq!(s.written_dates(&h), [h.tdindwg, h.tdcreate, h.tdupdate]);
        // Read with a header whose dates differ from the summary's (finer
        // or stale copies): kept while the header is unchanged.
        let mut read = s.clone();
        read.set_read_header(&h);
        assert_eq!(
            read.written_dates(&h),
            [s.total_edit_time, s.create, s.modified]
        );
        // The header's modification date changes (an edit): that one
        // follows it.
        let mut edited = h.clone();
        edited.tdupdate = date(2_461_000, 5);
        assert_eq!(
            read.written_dates(&edited),
            [s.total_edit_time, s.create, date(2_461_000, 5)]
        );
        let bytes = read.encode_r2018(&edited).unwrap();
        let back = SummaryInfo::decode(&bytes, Version::R2018, 30, &mut vec![]).unwrap();
        assert_eq!(back.modified, date(2_461_000, 5));
        assert_eq!(back.create, s.create);
    }

    /// Non-ASCII code page text before R2007; malformed text is reported.
    #[test]
    fn summary_code_page_text() {
        let mut b = vec![];
        // Title "あ" in Shift-JIS (code page 38), then 7 empty strings.
        b.extend_from_slice(&3u16.to_le_bytes());
        b.extend_from_slice(&[0x82, 0xA0, 0]);
        for _ in 0..7 {
            b.extend_from_slice(&[1, 0, 0]);
        }
        b.extend_from_slice(&[0; 24]);
        b.extend_from_slice(&1i16.to_le_bytes());
        // A property whose name is a bad Shift-JIS byte.
        b.extend_from_slice(&[2, 0, 0x82, 0, 1, 0, 0]);
        b.extend_from_slice(&[0; 8]);
        let mut diags = vec![];
        let s = SummaryInfo::decode(&b, Version::R2004, 38, &mut diags).unwrap();
        assert_eq!(s.title, "あ");
        assert_eq!(s.custom, [("\u{FFFD}".into(), String::new())]);
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    /// Counts and lengths are checked against the bytes left.
    #[test]
    fn summary_counts_are_bounded() {
        let mut b = vec![];
        for _ in 0..8 {
            b.extend_from_slice(&[1, 0, 0, 0]);
        }
        b.extend_from_slice(&[0; 24]);
        for count in [i16::MAX, -1] {
            let mut x = b.clone();
            x.extend_from_slice(&count.to_le_bytes());
            x.extend_from_slice(&[0; 8]);
            assert!(SummaryInfo::decode(&x, Version::R2018, 30, &mut vec![]).is_err());
        }
        let x = [0xFF, 0xFF, 0, 0];
        assert!(matches!(
            SummaryInfo::decode(&x, Version::R2018, 30, &mut vec![]),
            Err(Error::Truncated { .. })
        ));
    }

    const START: [u8; 16] = [
        0x1F, 0x25, 0x6D, 0x07, 0xD4, 0x36, 0x28, 0x28, 0x9D, 0x57, 0xCA, 0x3F, 0x9D, 0x44, 0x10,
        0x2B,
    ];
    const END: [u8; 16] = [
        0xE0, 0xDA, 0x92, 0xF8, 0x2B, 0xC9, 0xD7, 0xD7, 0x62, 0xA8, 0x35, 0xC0, 0x62, 0xBB, 0xEF,
        0xD4,
    ];

    /// A preview as AutoCAD writes one: header data, then the image, at
    /// `address`.
    fn preview_bytes(address: u32, code: u8, image: &[u8]) -> Vec<u8> {
        let header = [0u8; 80];
        let mut b = START.to_vec();
        let total = 1 + 2 * 9 + header.len() + image.len();
        b.extend_from_slice(&(total as u32).to_le_bytes());
        b.push(2);
        let data = address + 16 + 4 + 1 + 18;
        b.push(1);
        b.extend_from_slice(&data.to_le_bytes());
        b.extend_from_slice(&(header.len() as u32).to_le_bytes());
        b.push(code);
        b.extend_from_slice(&(data + header.len() as u32).to_le_bytes());
        b.extend_from_slice(&(image.len() as u32).to_le_bytes());
        b.extend_from_slice(&header);
        b.extend_from_slice(image);
        b.extend_from_slice(&END);
        b
    }

    #[test]
    fn preview_corpus_layout() {
        // The R2018 sample's layout: at 0x1C0, a header and a PNG.
        let png = b"\x89PNG\r\n\x1a\n...IEND\xaeB`\x82".to_vec();
        let bytes = preview_bytes(0x1C0, 6, &png);
        let p = Preview::decode(&bytes, Some(0x1C0)).unwrap();
        assert_eq!(p.header, [0; 80]);
        assert_eq!(
            (p.png.as_deref(), &p.bmp, &p.wmf),
            (Some(&png[..]), &None, &None)
        );
        assert_eq!(p.encode(0x1C0).unwrap(), bytes);
        // Without the address, it is found from the entries.
        assert_eq!(Preview::decode(&bytes, None).unwrap(), p);
        // The R2000 sample's: a BMP at 0xDC. Written elsewhere, the offsets
        // follow.
        let bmp = vec![0x42, 0x4D, 1, 2, 3];
        let bytes = preview_bytes(0xDC, 2, &bmp);
        let p = Preview::decode(&bytes, Some(0xDC)).unwrap();
        assert_eq!(p.bmp, Some(bmp.clone()));
        assert_eq!(p.encode(0x1000).unwrap(), preview_bytes(0x1000, 2, &bmp));
    }

    #[test]
    fn roundtrip_preview() {
        let p = Preview {
            header: vec![1, 2, 3],
            bmp: Some(vec![4; 10]),
            wmf: Some(vec![]),
            png: Some(vec![5, 6]),
        };
        let bytes = p.encode(0x400).unwrap();
        assert_eq!(Preview::decode(&bytes, Some(0x400)).unwrap(), p);
        assert_eq!(Preview::decode(&bytes, None).unwrap(), p);
        for n in 0..bytes.len() {
            assert!(Preview::decode(&bytes[..n], Some(0x400)).is_err(), "{n}");
        }
        // No preview at all: an empty table.
        let empty = Preview::default();
        let bytes = empty.encode(0x400).unwrap();
        assert_eq!(bytes.len(), 16 + 4 + 1 + 16);
        assert_eq!(Preview::decode(&bytes, None).unwrap(), empty);
    }

    /// AutoCAD leaves stale bytes after the end sentinel when a preview
    /// shrinks (the section keeps the size of an older, larger one:
    /// `asp-05`, `cps-01` and ACadSharp's `sample_base.dwg` of the extended
    /// corpus). The overall size locates the end sentinel; what follows it
    /// is ignored.
    #[test]
    fn stale_bytes_after_the_end_sentinel_are_ignored() {
        let good = preview_bytes(0x1C0, 6, b"png");
        let p = Preview::decode(&good, Some(0x1C0)).unwrap();
        let mut stale = good.clone();
        stale.extend_from_slice(b"IEND\xAE\x42\x60\x82");
        stale.extend_from_slice(&PREVIEW_END);
        assert_eq!(Preview::decode(&stale, Some(0x1C0)).unwrap(), p);
        assert_eq!(Preview::decode(&stale, None).unwrap(), p);
        // The sentinel must still be where the overall size puts it.
        let mut moved = good[..good.len() - 16].to_vec();
        moved.push(0);
        moved.extend_from_slice(&PREVIEW_END);
        assert!(Preview::decode(&moved, Some(0x1C0)).is_err());
    }

    /// Entries outside the section, of unknown codes, twice the same code,
    /// or a wrong overall size are errors, never panics.
    #[test]
    fn hostile_previews() {
        let good = preview_bytes(0x1C0, 6, b"png");
        // Start before the section, size past it.
        for (at, v) in [(22, 0x10u32), (26, 0xFFFF_FFFF), (31, 0x1C0), (35, 1000)] {
            let mut b = good.clone();
            b[at..at + 4].copy_from_slice(&v.to_le_bytes());
            assert!(Preview::decode(&b, Some(0x1C0)).is_err(), "{at}");
        }
        let mut b = good.clone();
        b[30] = 9; // unknown code
        assert!(Preview::decode(&b, Some(0x1C0)).is_err());
        let mut b = good.clone();
        b[30] = 1; // a second header
        assert!(Preview::decode(&b, Some(0x1C0)).is_err());
        let mut b = good.clone();
        b[16] ^= 1; // overall size
        assert!(Preview::decode(&b, Some(0x1C0)).is_err());
        let mut b = good.clone();
        b[0] ^= 1; // sentinel
        assert!(Preview::decode(&b, Some(0x1C0)).is_err());
        for i in 0..good.len() {
            for bit in [1u8, 0x10, 0x80] {
                let mut b = good.clone();
                b[i] ^= bit;
                let _ = Preview::decode(&b, Some(0x1C0));
                let _ = Preview::decode(&b, None);
            }
        }
    }

    /// The R2018 sample's AppInfo (shortened texts): class version 3, the
    /// name, a count of 2, then two checksummed strings.
    fn corpus_app_info() -> AppInfo {
        AppInfo {
            class_version: 3,
            name: "AppInfoDataList".into(),
            entries: vec![
                AppInfoEntry {
                    checksum: [
                        0xb8, 0xd0, 0xf0, 0x25, 0xa1, 0xd7, 0x93, 0x49, 0xb2, 0xfa, 0x9b, 0xf9,
                        0x28, 0x6f, 0xa1, 0xfd,
                    ],
                    text: "Autodesk DWG.  This file is a Trusted DWG".into(),
                },
                AppInfoEntry {
                    checksum: [7; 16],
                    text: "<ProductInformation name =\"AutoCAD\"/>".into(),
                },
            ],
            version: None,
        }
    }

    fn utf16(s: &str) -> Vec<u8> {
        let mut v: Vec<u16> = s.encode_utf16().collect();
        v.push(0);
        let mut out = (v.len() as u16).to_le_bytes().to_vec();
        out.extend(v.iter().flat_map(|u| u.to_le_bytes()));
        out
    }

    #[test]
    fn app_info_layout() {
        let a = corpus_app_info();
        let mut want = 3u32.to_le_bytes().to_vec();
        want.extend(utf16("AppInfoDataList"));
        want.extend(2u32.to_le_bytes());
        for e in &a.entries {
            want.extend(e.checksum);
            want.extend(utf16(&e.text));
        }
        assert_eq!(a.encode_r2018().unwrap(), want);
        let mut diags = vec![];
        assert_eq!(AppInfo::decode(&want, &mut diags).unwrap(), a);
        assert!(diags.is_empty());
        for n in 0..want.len() {
            assert!(AppInfo::decode(&want[..n], &mut diags).is_err(), "{n}");
        }
        // ODS's trailing app info version (8-bit text).
        let mut with_version = want.clone();
        with_version.extend([8, 0]);
        with_version.extend(b"2.7.2.0\0");
        let x = AppInfo::decode(&with_version, &mut diags).unwrap();
        assert_eq!(x.version.as_deref(), Some("2.7.2.0"));
        assert_eq!(x.encode_r2018().unwrap(), with_version);
        // A count beyond the bytes left.
        let mut bad = want.clone();
        let at = 4 + utf16("AppInfoDataList").len();
        bad[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(AppInfo::decode(&bad, &mut diags).is_err());
    }

    /// ODS 16.1: the R18 layout, with 8-bit strings.
    #[test]
    fn app_info_r18_layout() {
        let mut b = vec![];
        let text = |b: &mut Vec<u8>, s: &str| {
            b.extend(((s.len() + 1) as u16).to_le_bytes());
            b.extend(s.as_bytes());
            b.push(0);
        };
        text(&mut b, "AppInfoDataList");
        b.extend(2u32.to_le_bytes());
        text(&mut b, "4001");
        text(&mut b, "<ProductInformation name =\"Teigha\"/>");
        text(&mut b, "2.7.2.0");
        let a = AppInfo::decode(&b, &mut vec![]).unwrap();
        assert_eq!(a.name, "AppInfoDataList");
        assert_eq!(a.class_version, 2);
        let texts: Vec<&str> = a.entries.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(texts, ["4001", "<ProductInformation name =\"Teigha\"/>"]);
        assert!(a.entries.iter().all(|e| e.checksum == [0; 16]));
        assert_eq!(a.version.as_deref(), Some("2.7.2.0"));
    }

    /// AutoCAD 2004's R18 layout names the record "AppInfoData" (`arx-03`
    /// of the extended corpus), not ODS's "AppInfoDataList".
    #[test]
    fn app_info_r18_autocad_2004_name() {
        let b: &[u8] = &[
            0x0C, 0, b'A', b'p', b'p', b'I', b'n', b'f', b'o', b'D', b'a', b't', b'a', 0, 0, 0, 0,
            0, 0x0A, 0, b'6', b'0', b'6', b'3', b'4', b'8', b'3', b'2', b'4', 0, 0x08, 0, b'A',
            b'u', b't', b'o', b'C', b'A', b'D', 0, 0x0A, 0, b'1', b'6', b'.', b'2', b'.', b'3',
            b'3', b'.', b'0', 0,
        ];
        let mut diags = vec![];
        let a = AppInfo::decode(b, &mut diags).unwrap();
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(a.name, "AppInfoData");
        assert_eq!(a.class_version, 0);
        let texts: Vec<&str> = a.entries.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(texts, ["606348324", "AutoCAD"]);
        assert_eq!(a.version.as_deref(), Some("16.2.33.0"));
    }

    /// The application information opendwg writes names opendwg and its
    /// version.
    #[test]
    fn app_info_opendwg() {
        let a = AppInfo::opendwg();
        let version = env!("CARGO_PKG_VERSION");
        assert_eq!(a.class_version, 3);
        assert_eq!(a.name, "AppInfoDataList");
        assert!(
            a.entries
                .iter()
                .any(|e| e.text == format!("opendwg {version}"))
        );
        assert!(
            a.entries
                .iter()
                .any(|e| e.text.contains("name =\"opendwg\"") && e.text.contains(version))
        );
        let bytes = a.encode_r2018().unwrap();
        assert_eq!(AppInfo::decode(&bytes, &mut vec![]).unwrap(), a);
    }
}
