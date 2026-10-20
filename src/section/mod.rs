//! Data sections other than objects: header variables, classes, the object
//! map, the small informational sections ([`misc`]) and the data storage
//! ([`acds`]).

pub mod acds;
pub mod classes;
pub mod header;
pub mod misc;
pub mod objmap;

pub use acds::DataStorage;
pub use classes::DxfClass;
pub use header::{DecodedHeader, HeaderVars, JulianDate, UnknownHeaderFields, UnreadBits};
pub use misc::{AppInfo, AppInfoEntry, Preview, SummaryInfo};

use crate::codec::crc16;
use crate::error::{Error, Result};
use crate::version::Version;

/// CRC-16 seed of the sentinel-framed sections (header, classes) and of the
/// object map pages.
pub(crate) const CRC_SEED: u16 = 0xC0C1;

/// The sentinels and error names of a framed section.
pub(crate) struct Frame {
    pub start: &'static [u8; 16],
    pub end: &'static [u8; 16],
    pub start_name: &'static str,
    pub end_name: &'static str,
    pub context: &'static str,
}

/// The data bytes of a sentinel-framed section (header, classes):
///
/// ```text
/// start sentinel (16) | RL size | [RL 0] | data (size bytes) | RS CRC | end sentinel (16)
/// ```
///
/// The extra `RL` (zero, perhaps the high half of a 64-bit size) is present in
/// R2018 files and in R2010/R2013 files whose maintenance version is over 3.
/// Without the maintenance version, R2010/R2013 take the layout whose end
/// sentinel sits where the size says. The CRC (seed 0xC0C1) covers the bytes
/// from the size to the end of the data. Bytes after the end sentinel (R2004+
/// files have 8) are ignored.
pub(crate) fn unframe<'a>(bytes: &'a [u8], version: Version, f: &Frame) -> Result<&'a [u8]> {
    let trunc = || Error::Truncated { context: f.context };
    let start = bytes.get(..16).ok_or_else(trunc)?;
    if start != f.start {
        return Err(Error::BadSentinel(f.start_name));
    }
    let size = bytes
        .get(16..20)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(trunc)?;
    let size = usize::try_from(size).map_err(|_| trunc())?;
    // Where the data start: after the size, or after the size and an extra
    // zero RL.
    let candidates: &[usize] = match version {
        Version::R2000 | Version::R2004 | Version::R2007 => &[20],
        Version::R2010 | Version::R2013 => &[24, 20],
        Version::R2018 => &[24],
    };
    let mut fits = false;
    for &data_start in candidates {
        let Some(data_end) = data_start.checked_add(size) else {
            continue;
        };
        let Some(tail) = data_end
            .checked_add(2)
            .and_then(|e| bytes.get(data_end..e.checked_add(16)?))
        else {
            continue;
        };
        fits = true;
        if tail[2..] != f.end[..] {
            continue;
        }
        if data_start == 24 && bytes[20..24] != [0; 4] {
            return Err(Error::Corrupt(format!("{}: size over 4 GiB", f.context)));
        }
        let crc = u16::from_le_bytes([tail[0], tail[1]]);
        if crc16(CRC_SEED, &bytes[16..data_end]) != crc {
            return Err(Error::BadCrc { context: f.context });
        }
        return Ok(&bytes[data_start..data_end]);
    }
    if fits {
        Err(Error::BadSentinel(f.end_name))
    } else {
        Err(trunc())
    }
}

/// Frame `body` in R2018 form: start sentinel, `RL` size, `RL` 0, the body,
/// the CRC and the end sentinel (see [`unframe`]).
pub(crate) fn frame_r2018(body: &[u8], f: &Frame) -> Result<Vec<u8>> {
    let size = u32::try_from(body.len())
        .map_err(|_| Error::Encode(format!("{}: data too large", f.context)))?;
    let mut out = Vec::with_capacity(body.len() + 58);
    out.extend_from_slice(f.start);
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(body);
    let crc = crc16(CRC_SEED, &out[16..]);
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(f.end);
    Ok(out)
}
