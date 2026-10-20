//! DWG file format versions.

use crate::error::{Error, Result};

/// Supported DWG versions, ordered oldest to newest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Version {
    /// AutoCAD 2000 (AC1015).
    R2000,
    /// AutoCAD 2004 (AC1018).
    R2004,
    /// AutoCAD 2007 (AC1021).
    R2007,
    /// AutoCAD 2010 (AC1024).
    R2010,
    /// AutoCAD 2013 (AC1027).
    R2013,
    /// AutoCAD 2018 (AC1032).
    R2018,
}

impl Version {
    /// Parse the 6-byte file magic (`AC1015` ... `AC1032`).
    pub fn from_magic(magic: &[u8; 6]) -> Result<Version> {
        Ok(match magic {
            b"AC1015" => Version::R2000,
            b"AC1018" => Version::R2004,
            b"AC1021" => Version::R2007,
            b"AC1024" => Version::R2010,
            b"AC1027" => Version::R2013,
            b"AC1032" => Version::R2018,
            other => {
                return Err(Error::UnsupportedVersion(
                    String::from_utf8_lossy(other).into_owned(),
                ));
            }
        })
    }

    /// The 6-byte file magic for this version.
    pub fn magic(self) -> &'static [u8; 6] {
        match self {
            Version::R2000 => b"AC1015",
            Version::R2004 => b"AC1018",
            Version::R2007 => b"AC1021",
            Version::R2010 => b"AC1024",
            Version::R2013 => b"AC1027",
            Version::R2018 => b"AC1032",
        }
    }
}

/// The release and its file magic: `R2018 (AC1032)`.
impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?} ({})", String::from_utf8_lossy(self.magic()))
    }
}

#[cfg(test)]
mod tests {
    use super::Version::*;
    use super::*;

    #[test]
    fn version_magic_roundtrip() {
        for v in [R2000, R2004, R2007, R2010, R2013, R2018] {
            assert_eq!(Version::from_magic(v.magic()).unwrap(), v);
        }
    }

    #[test]
    fn version_rejects_r14() {
        assert!(
            matches!(Version::from_magic(b"AC1014"), Err(Error::UnsupportedVersion(s)) if s == "AC1014")
        );
    }

    #[test]
    fn version_display() {
        assert_eq!(R2018.to_string(), "R2018 (AC1032)");
        assert_eq!(R2000.to_string(), "R2000 (AC1015)");
    }

    #[test]
    fn version_ordering() {
        assert!(R2000 < R2018);
    }
}
