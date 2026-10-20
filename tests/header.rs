//! Header variables (`AcDb:Header`) against the sample corpus.

mod common;

use opendwg::Version;
use opendwg::container::{self, SectionName};
use opendwg::section::header::{self, HeaderVars};
use opendwg::strings;

/// Decode, requiring an exactly consumed header with no diagnostics.
fn decode_exact(bytes: &[u8], version: Version, codepage: u16, name: &str) -> HeaderVars {
    let d = header::decode_with_diagnostics(bytes, version, codepage)
        .unwrap_or_else(|err| panic!("{name}: {err}"));
    assert!(d.unread.is_exact(), "{name}: unread {:?}", d.unread);
    assert!(d.diagnostics.is_empty(), "{name}: {:?}", d.diagnostics);
    d.vars
}

/// The raw header section, version and code page of a corpus file.
fn header_section(path: &std::path::Path) -> (Vec<u8>, Version, u16) {
    let bytes = std::fs::read(path).expect("corpus file is readable");
    let raw = container::read(&bytes).expect("container reads");
    let h = raw
        .sections
        .get(&SectionName::Header)
        .expect("header section")
        .clone();
    (h, raw.version, raw.codepage)
}

#[test]
fn header_decodes_all_corpus_versions() {
    let files = common::fetched_files_or_skip();
    for (e, path) in files {
        let (bytes, version, codepage) = header_section(&path);
        let h = decode_exact(&bytes, version, codepage, &e.name);
        assert!(h.handseed.0 > 0, "{}: handseed", e.name);
        assert!(strings::is_supported(h.dwgcodepage), "{}: codepage", e.name);
        assert!(h.ltscale > 0.0, "{}: ltscale", e.name);
        // Version-dependent fields are present exactly from their version on.
        assert_eq!(
            h.sortents.is_some(),
            version >= Version::R2004,
            "{}",
            e.name
        );
        assert_eq!(
            h.cmaterial.is_some(),
            version >= Version::R2007,
            "{}",
            e.name
        );
        assert_eq!(h.dimmzs.is_some(), version >= Version::R2010, "{}", e.name);
        assert_eq!(
            h.requiredversions.is_some(),
            version >= Version::R2013,
            "{}",
            e.name
        );
        // Spot checks: control objects and the table entries are real handles.
        for (what, handle) in [
            ("block control", h.block_control),
            ("layer control", h.layer_control),
            ("clayer", h.clayer),
            ("model space", h.block_record_model_space),
            ("continuous", h.ltype_continuous),
            ("named objects", h.dict_named_objects),
        ] {
            assert!(handle.0 > 0, "{}: {what}", e.name);
            assert!(handle < h.handseed, "{}: {what}", e.name);
        }
        assert!(h.fingerprintguid.starts_with('{'), "{}", e.name);
        // ODS deviations (see the `section::header` module docs): MENUNAME in
        // every version, the first unknown BL and TV, and the R2004+ tail.
        assert_eq!(h.menuname, ".", "{}", e.name);
        assert_eq!(h.unknown.unknown_tv1, "m", "{}", e.name);
        assert_eq!(h.unknown.unknown_bl1, 0, "{}", e.name);
        let u = &h.unknown;
        if version >= Version::R2004 {
            assert_eq!(
                (u.unknown_bl11, u.unknown_bl12, u.unknown_b3),
                (Some(0), Some(0), Some(true)),
                "{}",
                e.name
            );
        } else {
            assert!(u.unknown_bl11.is_none(), "{}", e.name);
        }
    }
}

#[test]
fn header_r2018_roundtrip() {
    let files = common::fetched_files_for("AC1032");
    for (e, path) in files {
        let (bytes, version, codepage) = header_section(&path);
        let h = decode_exact(&bytes, version, codepage, &e.name);
        let out = header::encode_r2018(&h).unwrap();
        if out != bytes {
            let at = out
                .iter()
                .zip(&bytes)
                .position(|(a, b)| a != b)
                .unwrap_or(out.len().min(bytes.len()));
            panic!(
                "{}: encode differs at byte {at} (lengths {} vs {})",
                e.name,
                out.len(),
                bytes.len()
            );
        }
    }
}

/// Every field that both an R2000 and an R2018 header store must survive
/// R2000 -> R2018 -> decode. Fields R2000 does not store come back with their
/// R2018 defaults; fields only R2000 stores are dropped.
#[test]
fn header_cross_version_roundtrip() {
    let files = common::fetched_files_for("AC1015");
    for (e, path) in files {
        let (bytes, version, codepage) = header_section(&path);
        let old = decode_exact(&bytes, version, codepage, &e.name);
        let out = header::encode_r2018(&old).unwrap();
        let new = decode_exact(&out, Version::R2018, codepage, &e.name);
        let defaults = header::defaults_r2018();
        assert_eq!(new, old.to_r2018(), "{}", e.name);
        // Spot checks on fields both versions store.
        assert_eq!(new.handseed, old.handseed);
        assert_eq!(new.clayer, old.clayer);
        assert_eq!(new.tdcreate, old.tdcreate);
        assert_eq!(new.extmax, old.extmax);
        assert_eq!(new.dimclrd, old.dimclrd);
        assert_eq!(new.fingerprintguid, old.fingerprintguid);
        // Fields R2000 lacks take the R2018 defaults; R2000-only ones go.
        assert_eq!(new.dimfxl, defaults.dimfxl);
        assert_eq!(new.requiredversions, defaults.requiredversions);
        assert!(old.vpentity_control.is_some() && new.vpentity_control.is_none());
    }
}

/// The same conversion from every corpus version: decode, encode as R2018,
/// decode again gives the header as `to_r2018` describes it.
#[test]
fn header_every_version_converts_to_r2018() {
    for (e, path) in common::fetched_files_or_skip() {
        let (bytes, version, codepage) = header_section(&path);
        let old = decode_exact(&bytes, version, codepage, &e.name);
        let out = header::encode_r2018(&old).unwrap();
        let new = decode_exact(&out, Version::R2018, codepage, &e.name);
        assert_eq!(new, old.to_r2018(), "{}", e.name);
    }
}

#[test]
fn defaults_r2018_encode_and_decode() {
    let d = header::defaults_r2018();
    assert_eq!(d.insunits, 1);
    assert_eq!(d.measurement, Some(0));
    let bytes = header::encode_r2018(&d).unwrap();
    let back = decode_exact(&bytes, Version::R2018, d.dwgcodepage, "defaults");
    let mut want: HeaderVars = d.clone();
    // MEASUREMENT is stored in AcDb:Template, not in the header section.
    want.measurement = None;
    assert_eq!(back, want);
}
