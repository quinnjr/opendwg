//! Corpus checks of the summary information, preview and application
//! information sections: every corpus file's sections decode, the
//! R2018 files' sections re-encode byte for byte, older versions' values
//! convert to the R2018 form and back, and the other auxiliary sections stay
//! in the extra sections.

mod common;

use opendwg::Handle;
use opendwg::container::{self, SectionName};
use opendwg::section::acds::{ACIS_SCHEMA, DataStorage};
use opendwg::section::{AppInfo, JulianDate, Preview, SummaryInfo};
use opendwg::{Drawing, Version};

#[test]
fn corpus_summary_info() {
    let mut dates = None;
    for (e, path) in common::fetched_files_or_skip() {
        let name = &e.name;
        let bytes = std::fs::read(&path).unwrap();
        let d = Drawing::from_bytes(&bytes).unwrap();
        let v = d.source_version().unwrap();
        assert!(
            d.diagnostics()
                .iter()
                .all(|x| !x.message.contains("section"))
        );
        if v < Version::R2004 {
            // R2000 files have no summary section.
            assert!(d.summary.is_none(), "{name}");
            continue;
        }
        let s = d.summary.as_ref().unwrap_or_else(|| panic!("{name}"));
        // Converted to the R2018 form and back: the same values.
        // The dates copy the header's (exactly, in every corpus file).
        let h = &d.header;
        assert_eq!(s.create, h.tdcreate, "{name}");
        assert_eq!(s.modified, h.tdupdate, "{name}");
        assert_eq!(s.total_edit_time, h.tdindwg, "{name}");
        let out = s.encode_r2018(&d.header).unwrap();
        let back = SummaryInfo::decode(&out, Version::R2018, 30, &mut vec![]).unwrap();
        assert_eq!(&back, s, "{name}");
        if v == Version::R2018 {
            let raw = container::read(&bytes).unwrap();
            assert_eq!(out, raw.sections[&SectionName::SummaryInfo], "{name}");
        }
        if name.starts_with("acadsharp-sample-") {
            // Every sample was last saved by "alber", and every version keeps
            // the drawing's creation date.
            assert_eq!(s.last_saved_by, "alber", "{name}");
            assert!(s.title.is_empty() && s.author.is_empty(), "{name}");
            assert!(s.custom.is_empty(), "{name}");
            match &dates {
                None => dates = Some(s.create),
                Some(c) => assert_eq!(c, &s.create, "{name}"),
            }
        }
    }
}

#[test]
fn corpus_preview() {
    for (e, path) in common::fetched_files_or_skip() {
        let name = &e.name;
        let bytes = std::fs::read(&path).unwrap();
        let d = Drawing::from_bytes(&bytes).unwrap();
        let raw = container::read(&bytes).unwrap();
        let p = d.preview.as_ref().unwrap_or_else(|| panic!("{name}"));
        assert_eq!(p.header.len(), 80, "{name}");
        // R2013+ files hold a PNG, older ones a BMP.
        if d.source_version().unwrap() >= Version::R2013 {
            assert!(p.png.as_ref().unwrap().starts_with(b"\x89PNG"), "{name}");
            assert!(p.bmp.is_none(), "{name}");
        } else {
            assert!(p.bmp.as_ref().is_some_and(|b| !b.is_empty()), "{name}");
            assert!(p.png.is_none(), "{name}");
        }
        assert!(p.wmf.is_none(), "{name}");
        // Written back at the same address, the bytes are the same (every
        // version stores the section alike).
        let out = p.encode(raw.preview_offset.unwrap()).unwrap();
        assert_eq!(out, raw.sections[&SectionName::Preview], "{name}");
        assert_eq!(&Preview::decode(&out, None).unwrap(), p, "{name}");
    }
}

#[test]
fn corpus_app_info() {
    for (e, path) in common::fetched_files_or_skip() {
        let name = &e.name;
        let bytes = std::fs::read(&path).unwrap();
        let d = Drawing::from_bytes(&bytes).unwrap();
        if d.source_version().unwrap() < Version::R2004 {
            assert!(d.app_info.is_none(), "{name}");
            continue;
        }
        let a = d.app_info.as_ref().unwrap_or_else(|| panic!("{name}"));
        assert_eq!((a.class_version, a.name.as_str()), (3, "AppInfoDataList"));
        assert_eq!(a.entries.len(), 2, "{name}");
        assert!(a.entries[0].text.starts_with("Autodesk DWG."), "{name}");
        let product = &a.entries[1].text;
        // AecObjects.dwg's escapes its quotes (`name =\"AutoCAD\"`).
        assert!(product.starts_with("<ProductInformation name ="), "{name}");
        if name.starts_with("acadsharp-sample-") {
            assert!(product.contains("name =\"AutoCAD\""), "{name}: {product}");
        }
        assert_eq!(a.version, None, "{name}");
        // Every corpus file, R2004 ones included, stores the R21 layout.
        let raw = container::read(&bytes).unwrap();
        let out = a.encode_r2018().unwrap();
        assert_eq!(out, raw.sections[&SectionName::AppInfo], "{name}");
        assert_eq!(&AppInfo::decode(&out, &mut vec![]).unwrap(), a);
    }
}

/// The decoded sections leave the extra sections; the others stay there as
/// read.
#[test]
fn corpus_extra_sections_are_kept() {
    for (e, path) in common::fetched_files_or_skip() {
        let bytes = std::fs::read(&path).unwrap();
        let d = Drawing::from_bytes(&bytes).unwrap();
        let raw = container::read(&bytes).unwrap();
        let names: Vec<&SectionName> = d.extra_sections().map(|(n, _)| n).collect();
        for gone in [
            SectionName::SummaryInfo,
            SectionName::Preview,
            SectionName::AppInfo,
            SectionName::AcDsPrototype,
        ] {
            assert!(!names.contains(&&gone), "{}: {gone:?}", e.name);
        }
        for (n, b) in d.extra_sections() {
            assert_eq!(b, raw.sections[n].as_slice(), "{}: {n:?}", e.name);
        }
        for kept in [SectionName::AuxHeader, SectionName::Template] {
            assert!(names.contains(&&kept), "{}: {kept:?}", e.name);
        }
    }
}

const SAVED: JulianDate = JulianDate {
    day: 2_460_000,
    ms: 0,
};

/// Every R2013+ corpus file's data storage section parses: its schemas,
/// the ACIS records of the three solids and regions (sample files) and the
/// thumbnails; it encodes to a section that parses back the same; the
/// search segment AutoCAD wrote is the one derived from the records; and a
/// saved drawing carries the section over byte for byte.
#[test]
fn corpus_data_storage() {
    let mut seen = 0;
    for (e, path) in common::fetched_files_or_skip() {
        let name = &e.name;
        let bytes = std::fs::read(&path).unwrap();
        let d = Drawing::from_bytes(&bytes).unwrap();
        let raw = container::read(&bytes).unwrap();
        let Some(acds) = raw.sections.get(&SectionName::AcDsPrototype) else {
            assert!(d.data_storage.is_none(), "{name}");
            continue;
        };
        seen += 1;
        assert!(d.source_version().unwrap() >= Version::R2013, "{name}");
        let s = d.data_storage.as_ref().unwrap_or_else(|| panic!("{name}"));
        assert_eq!(s, &DataStorage::parse(acds).unwrap(), "{name}");
        let names: Vec<&str> = s.schemas.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names[..5],
            [
                "AcDb_Thumbnail_Schema",
                "AcDbDs::TreatedAsObjectDataSchema",
                "AcDbDs::LegacySchema",
                "AcDbDs::IndexedPropertySchema",
                "AcDbDs::HandleAttributeSchema"
            ],
            "{name}"
        );
        assert_eq!(s.unknown_properties.len(), 4 * (names.len() - 4), "{name}");
        if name.starts_with("acadsharp-sample-") {
            assert_eq!(names.len(), 6, "{name}");
            assert_eq!(s.schema_index(ACIS_SCHEMA), Some(5), "{name}");
            for h in [0xD65, 0xD69, 0xD6A] {
                let sab = s
                    .acis_data(Handle(h))
                    .unwrap_or_else(|| panic!("{name}: {h:X}"));
                assert!(sab.ends_with(b"\x0E\x03End\x0E\x02of\x0E\x03ASM\x0D\x04data"));
                assert_eq!(d.acis_data(Handle(h)), Some(sab), "{name}");
            }
            assert_eq!(s.records.len(), 8, "{name}");
        } else {
            assert_eq!(names.len(), 5, "{name}");
        }
        // Thumbnails: PNG images keyed by block records.
        assert!(
            s.records
                .iter()
                .filter(|r| r.schema == 0)
                .all(|r| r.data.starts_with(b"\x89PNG"))
        );
        assert_eq!(
            DataStorage::stored_search(acds).unwrap(),
            s.search(),
            "{name}"
        );
        let out = s.encode().unwrap();
        assert_eq!(&DataStorage::parse(&out).unwrap(), s, "{name}");
        assert_eq!(
            DataStorage::stored_search(&out).unwrap(),
            s.search(),
            "{name}"
        );
        // Unchanged, the section is written as read.
        if d.unconvertible_objects().is_empty() {
            let saved = container::read(&d.to_bytes_at(SAVED).unwrap()).unwrap();
            assert_eq!(&saved.sections[&SectionName::AcDsPrototype], acds, "{name}");
        }
    }
    if seen > 0 {
        assert_eq!(seen, 4);
    }
}

/// A data storage section that cannot be parsed stays in the extra
/// sections, with a warning, and is written as read.
#[test]
fn undecodable_data_storage_is_kept() {
    let Some(path) = common::corpus_file_or_skip("acadsharp-dynblock-lookup") else {
        return;
    };
    let mut raw = container::read(&std::fs::read(&path).unwrap()).unwrap();
    let acds = raw.sections.get_mut(&SectionName::AcDsPrototype).unwrap();
    acds[0] ^= 0xFF;
    let bad = acds.clone();
    let bytes = container::write_r2018(&raw).unwrap();
    let back = Drawing::from_bytes(&bytes).unwrap();
    assert!(back.data_storage.is_none());
    assert!(
        back.diagnostics()
            .iter()
            .any(|x| x.message.contains("data storage"))
    );
    let kept = back
        .extra_sections()
        .find(|(n, _)| **n == SectionName::AcDsPrototype)
        .unwrap()
        .1;
    assert_eq!(kept, bad.as_slice());
    let saved = container::read(&back.to_bytes_at(SAVED).unwrap()).unwrap();
    assert_eq!(saved.sections[&SectionName::AcDsPrototype], bad);
}
