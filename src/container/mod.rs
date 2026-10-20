//! Container model: the raw byte sections of a DWG file, before any object decoding.
//!
//! [`read`] dispatches on the version magic. Each version module slices the file into
//! [`RawSections`]; later stages decode the section contents.

use std::collections::BTreeMap;

use crate::error::{Error, Result};
use crate::version::Version;

mod r2000;
mod r2004;
mod r2007;
mod write;

pub use write::{r2018_preview_address, write_r2018};

/// A named data section. Names follow ODS chapter 4.5 (R2004+ section map); R2000 files have
/// no names, so the reader maps its section locators onto the same variants.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[allow(missing_docs)] // field by field, as ODS names them
pub enum SectionName {
    Header,
    Classes,
    /// `AcDb:Handles`, the object map.
    Handles,
    /// `AcDb:AcDbObjects`.
    Objects,
    AuxHeader,
    Preview,
    SummaryInfo,
    AppInfo,
    AppInfoHistory,
    FileDepList,
    RevHistory,
    Security,
    ObjFreeSpace,
    Template,
    AcDsPrototype,
    Signature,
    VbaProject,
    Other(String),
}

impl SectionName {
    /// Map an R2004+ section name (e.g. `"AcDb:Header"`) to its variant. R2007 uses the same
    /// names (stored as UTF-16). Unknown names are kept verbatim in [`SectionName::Other`].
    pub fn from_r2004_name(name: &str) -> SectionName {
        match name {
            "AcDb:Header" => SectionName::Header,
            "AcDb:Classes" => SectionName::Classes,
            "AcDb:Handles" => SectionName::Handles,
            // ODS spells it AcDbObjects in the section table; some text says AcDb:Objects.
            "AcDb:AcDbObjects" | "AcDb:Objects" => SectionName::Objects,
            "AcDb:AuxHeader" => SectionName::AuxHeader,
            "AcDb:Preview" => SectionName::Preview,
            "AcDb:SummaryInfo" => SectionName::SummaryInfo,
            "AcDb:AppInfo" => SectionName::AppInfo,
            "AcDb:AppInfoHistory" => SectionName::AppInfoHistory,
            "AcDb:FileDepList" => SectionName::FileDepList,
            "AcDb:RevHistory" => SectionName::RevHistory,
            "AcDb:Security" => SectionName::Security,
            "AcDb:ObjFreeSpace" => SectionName::ObjFreeSpace,
            "AcDb:Template" => SectionName::Template,
            "AcDb:AcDsPrototype_1b" | "AcDb:AcDsPrototype" => SectionName::AcDsPrototype,
            "AcDb:Signature" => SectionName::Signature,
            "AcDb:VBAProject" => SectionName::VbaProject,
            other => SectionName::Other(other.to_owned()),
        }
    }

    /// The R2004-family section map name of this section, the inverse of
    /// [`SectionName::from_r2004_name`]. Where a reader accepts two spellings this is the one
    /// AutoCAD writes (`AcDb:AcDbObjects`, `AcDb:AcDsPrototype_1b`).
    pub fn r2004_name(&self) -> &str {
        match self {
            SectionName::Header => "AcDb:Header",
            SectionName::Classes => "AcDb:Classes",
            SectionName::Handles => "AcDb:Handles",
            SectionName::Objects => "AcDb:AcDbObjects",
            SectionName::AuxHeader => "AcDb:AuxHeader",
            SectionName::Preview => "AcDb:Preview",
            SectionName::SummaryInfo => "AcDb:SummaryInfo",
            SectionName::AppInfo => "AcDb:AppInfo",
            SectionName::AppInfoHistory => "AcDb:AppInfoHistory",
            SectionName::FileDepList => "AcDb:FileDepList",
            SectionName::RevHistory => "AcDb:RevHistory",
            SectionName::Security => "AcDb:Security",
            SectionName::ObjFreeSpace => "AcDb:ObjFreeSpace",
            SectionName::Template => "AcDb:Template",
            SectionName::AcDsPrototype => "AcDb:AcDsPrototype_1b",
            SectionName::Signature => "AcDb:Signature",
            SectionName::VbaProject => "AcDb:VBAProject",
            SectionName::Other(name) => name,
        }
    }
}

/// With the `serde` feature a section name is its R2004 name, a string, so
/// that it can key a JSON object.
#[cfg(feature = "serde")]
impl serde::Serialize for SectionName {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(self.r2004_name())
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for SectionName {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let name = <std::borrow::Cow<'de, str>>::deserialize(d)?;
        Ok(SectionName::from_r2004_name(&name))
    }
}

/// Section map metadata of one R2004-family section (ODS 4.5), as read. The R2018 writer uses
/// it for sections it has no rule for ([`SectionName::Other`]), so they are written back as
/// they were.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionMeta {
    /// Compressed flag (stored as 2 = yes, 1 = no).
    pub compressed: bool,
    /// 0 = no, 1 = password-encrypted, 2 = unknown (ODS 4.5).
    pub encrypted: u32,
    /// Max decompressed size of a page of this section.
    pub max_page_size: u32,
    /// The description's field at 0x10 (ODS: unknown; AutoCAD writes 1).
    pub unknown: u32,
}

/// Length of the unencrypted start of an R2004-family file header (ODS 4.1).
pub const R2004_PLAIN_HEADER_LEN: usize = 0x80;

/// The sections of a DWG file as raw bytes.
#[derive(Debug, Clone)]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct RawSections {
    pub version: Version,
    /// Maintenance release version from the file header.
    pub maintenance: u8,
    /// Windows code page number from the file header (R2000: offset 0x13).
    pub codepage: u16,
    pub sections: BTreeMap<SectionName, Vec<u8>>,
    /// Objects section base offset used by the object map (R2000: file offset 0; R2004+:
    /// section-relative).
    pub objects_base: u64,
    pub preview_offset: Option<u64>,
    /// Section map metadata by section (R2004 family only; empty for other versions).
    pub section_meta: BTreeMap<SectionName, SectionMeta>,
    /// The unencrypted file header bytes `0x00..0x80` (R2004 family only). The R2018 writer
    /// keeps the fields it does not derive from the layout, such as the application version
    /// at 0x11 and the undocumented longs from 0x34 that AutoCAD fills in.
    pub file_header: Option<[u8; R2004_PLAIN_HEADER_LEN]>,
    /// Problems the reader tolerated, such as a data page checksum mismatch (R2004+) or an
    /// unreadable optional section. Later stages turn these into diagnostics.
    pub warnings: Vec<String>,
}

/// Little-endian u32 at `at`, or `Error::Truncated` with `context`.
fn u32_at(b: &[u8], at: usize, context: &'static str) -> Result<u32> {
    b.get(at..at.saturating_add(4))
        .and_then(|s| <[u8; 4]>::try_from(s).ok())
        .map(u32::from_le_bytes)
        .ok_or(Error::Truncated { context })
}

/// Little-endian u64 at `at`, or `Error::Truncated` with `context`.
fn u64_at(b: &[u8], at: usize, context: &'static str) -> Result<u64> {
    b.get(at..at.saturating_add(8))
        .and_then(|s| <[u8; 8]>::try_from(s).ok())
        .map(u64::from_le_bytes)
        .ok_or(Error::Truncated { context })
}

/// The bytes `[address, address + len)`, or `None` when out of range.
fn span(bytes: &[u8], address: u64, len: u64) -> Option<&[u8]> {
    let start = usize::try_from(address).ok()?;
    let end = start.checked_add(usize::try_from(len).ok()?)?;
    bytes.get(start..end)
}

/// Header, Classes, Handles and Objects: a reader fails when one of them cannot be read, while
/// other sections that fail are skipped with a warning.
fn is_required(name: &SectionName) -> bool {
    matches!(
        name,
        SectionName::Header | SectionName::Classes | SectionName::Handles | SectionName::Objects
    )
}

/// `Error::Corrupt` naming the first required section missing from `sections`.
fn check_required(sections: &BTreeMap<SectionName, Vec<u8>>) -> Result<()> {
    for (name, what) in [
        (SectionName::Header, "AcDb:Header"),
        (SectionName::Classes, "AcDb:Classes"),
        (SectionName::Handles, "AcDb:Handles"),
        (SectionName::Objects, "AcDb:AcDbObjects"),
    ] {
        if !sections.contains_key(&name) {
            return Err(Error::Corrupt(format!("missing section {what}")));
        }
    }
    Ok(())
}

/// Slice `bytes` into [`RawSections`], dispatching on the version magic.
pub fn read(bytes: &[u8]) -> Result<RawSections> {
    let magic: [u8; 6] =
        bytes
            .get(..6)
            .and_then(|m| m.try_into().ok())
            .ok_or(Error::Truncated {
                context: "file magic",
            })?;
    match Version::from_magic(&magic)? {
        Version::R2000 => r2000::read(bytes),
        v @ (Version::R2004 | Version::R2010 | Version::R2013 | Version::R2018) => {
            r2004::read(bytes, v)
        }
        Version::R2007 => r2007::read(bytes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_names_map() {
        assert_eq!(
            SectionName::from_r2004_name("AcDb:Header"),
            SectionName::Header
        );
        assert_eq!(
            SectionName::from_r2004_name("AcDb:AcDbObjects"),
            SectionName::Objects
        );
        assert_eq!(
            SectionName::from_r2004_name("AcDb:Foo"),
            SectionName::Other("AcDb:Foo".into())
        );
    }

    #[test]
    fn bad_magic() {
        let r = read(b"AC1099\0\0\0\0");
        assert!(matches!(r, Err(Error::UnsupportedVersion(s)) if s == "AC1099"));
    }

    #[test]
    fn short_input() {
        assert!(matches!(read(b"AC10"), Err(Error::Truncated { .. })));
    }
}
