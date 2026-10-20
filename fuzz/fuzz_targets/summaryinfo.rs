//! The summary information and application information sections.
#![no_main]

use libfuzzer_sys::fuzz_target;
use opendwg::Version;
use opendwg::section::{AppInfo, SummaryInfo, header};

const VERSIONS: [Version; 6] = [
    Version::R2000,
    Version::R2004,
    Version::R2007,
    Version::R2010,
    Version::R2013,
    Version::R2018,
];

/// The first byte picks the version (and code page), the rest is the input.
fn split(data: &[u8]) -> Option<(Version, u16, &[u8])> {
    let (&sel, rest) = data.split_first()?;
    let version = VERSIONS[usize::from(sel) % VERSIONS.len()];
    // 30 = ANSI_1252; the high bits pick a multi-byte code page (38 = ANSI_932, Shift-JIS).
    let codepage = if sel & 0x80 != 0 { 38 } else { 30 };
    Some((version, codepage, rest))
}

fuzz_target!(|data: &[u8]| {
    let Some((version, codepage, rest)) = split(data) else {
        return;
    };
    let mut diagnostics = Vec::new();
    if let Ok(s) = SummaryInfo::decode(rest, version, codepage, &mut diagnostics) {
        let _ = s.encode_r2018(&header::defaults_r2018());
    }
    if let Ok(a) = AppInfo::decode(rest, &mut diagnostics) {
        let _ = a.encode_r2018();
    }
});
