//! Dictionaries (ODS 20.4.44, 20.4.45) and dictionary variables (20.4.74).

use crate::error::Result;
use crate::handle::Handle;
use crate::object::entities::dim::write_count;
use crate::object::stream::{ObjReader, ObjWriter};
use crate::object::tables::{HARD_POINTER, SOFT_OWNER, handle_count, put};

/// A dictionary (ODS 20.4.44): named references to objects.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Dictionary {
    /// The entries, in file order. AutoCAD may keep an entry with an empty
    /// name or a null handle; such entries are kept too.
    pub entries: Vec<(String, Handle)>,
    /// DXF 281: the duplicate record cloning flag.
    pub cloning: i16,
    /// DXF 280, the hard owner flag: non-zero when the dictionary owns its
    /// entries (ACadSharp (MIT) reads any value over 0 as set; the corpus
    /// has 0 and 1). Kept as stored. Either way the entries are stored as
    /// soft owners, as ODS says (every corpus dictionary has them so).
    pub hard_owner: u8,
}

impl Dictionary {
    /// Whether the dictionary owns its entries ([`Dictionary::hard_owner`]
    /// is not 0).
    pub fn is_hard_owner(&self) -> bool {
        self.hard_owner != 0
    }

    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let n = r.data.read_bl()?;
        let n = handle_count(r, n, "dictionary entry")?;
        let cloning = r.data.read_bs()?;
        let hard_owner = r.data.read_rc()?;
        let mut entries = Vec::with_capacity(n);
        for _ in 0..n {
            let name = r.text()?;
            entries.push((name, r.href_abs()?));
        }
        Ok(Dictionary {
            entries,
            cloning,
            hard_owner,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        write_count(w, self.entries.len(), "dictionary entries")?;
        w.data.write_bs(self.cloning);
        w.data.write_rc(self.hard_owner);
        for (name, h) in &self.entries {
            w.text(name)?;
            put(w, SOFT_OWNER, *h)?;
        }
        Ok(())
    }

    /// The object of the first entry named `name`.
    pub fn get(&self, name: &str) -> Option<Handle> {
        self.entries
            .iter()
            .find(|(n, _)| n == name)
            .map(|&(_, h)| h)
    }
}

/// A dictionary with a default entry (ODS 20.4.45, ACDBDICTIONARYWDFLT).
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct DictionaryWithDefault {
    pub dictionary: Dictionary,
    /// The default entry (hard pointer).
    pub default_entry: Handle,
}

impl DictionaryWithDefault {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let dictionary = Dictionary::decode(r)?;
        Ok(DictionaryWithDefault {
            dictionary,
            default_entry: r.href_abs()?,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        self.dictionary.encode(w)?;
        put(w, HARD_POINTER, self.default_entry)
    }
}

/// A dictionary variable (ODS 20.4.74, DICTIONARYVAR), such as the entries
/// of the `AcDbVariableDictionary`.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DictionaryVar {
    /// ODS "Intval" (DXF 280, the object schema number).
    pub schema: u8,
    /// The value. ODS gives it as a `BS`; it is a text field.
    pub value: String,
}

impl DictionaryVar {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        Ok(DictionaryVar {
            schema: r.data.read_rc()?,
            value: r.text()?,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        w.data.write_rc(self.schema);
        w.text(&self.value)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::bits::BitWriter;
    use crate::object::objects::test_util::H;

    use crate::handle::Handle;
    use crate::object::entities::tests::{reader, roundtrip};

    use crate::object::stream::tests::assemble;

    use crate::object::type_code;
    use crate::version::Version;

    pub(crate) fn dictionary() -> Dictionary {
        Dictionary {
            entries: vec![
                ("ACAD_GROUP".into(), Handle(0xD)),
                (String::new(), Handle(0)),
                ("日本😀".into(), Handle(0x1234)),
            ],
            cloning: 3,
            hard_owner: 1,
        }
    }

    #[test]
    fn roundtrip_dictionaries() {
        roundtrip(
            &dictionary(),
            Dictionary::encode,
            Dictionary::decode,
            type_code::DICTIONARY,
        );
        let d = DictionaryWithDefault {
            dictionary: dictionary(),
            default_entry: Handle(0xF),
        };
        roundtrip(
            &d,
            DictionaryWithDefault::encode,
            DictionaryWithDefault::decode,
            500,
        );
        let v = DictionaryVar {
            schema: 2,
            value: "Imperial24 ß".into(),
        };
        roundtrip(&v, DictionaryVar::encode, DictionaryVar::decode, 500);
        assert_eq!(dictionary().get("日本😀"), Some(Handle(0x1234)));
        assert_eq!(dictionary().get("missing"), None);
    }

    /// The dictionary entry count is checked against the handle stream; a
    /// hard owner flag other than 0 or 1 is kept as stored.
    #[test]
    fn dictionary_errors() {
        let mut d = BitWriter::new();
        d.write_bl(i32::MAX);
        d.write_bs(1);
        d.write_rc(0);
        let bytes = assemble(Version::R2018, 42, &d, None, &BitWriter::new());
        assert!(Dictionary::decode(&mut reader(&bytes)).is_err());
        let mut d = BitWriter::new();
        d.write_bl(0);
        d.write_bs(1);
        d.write_rc(2);
        let bytes = assemble(Version::R2018, 42, &d, None, &BitWriter::new());
        let x = Dictionary::decode(&mut reader(&bytes)).unwrap();
        assert_eq!(x.hard_owner, 2);
        assert!(x.is_hard_owner());
        let mut w = ObjWriter::new(H);
        x.encode(&mut w).unwrap();
        let out = w.finish(42).unwrap();
        assert_eq!(Dictionary::decode(&mut reader(&out)).unwrap(), x);
    }
}
