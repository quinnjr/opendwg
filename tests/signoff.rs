//! The AutoCAD sign-off pack (`tests/common/pack.rs`, written by
//! `examples/make_signoff.rs`). Every drawing of the pack reads back with
//! no diagnostic and validates; the edits carry what they claim; the
//! generated SIGNOFF.md has every placeholder filled, a section per file,
//! and names only files the pack makes.

mod common;

use std::collections::BTreeSet;

use common::pack::{self, Kind, Pack};
use opendwg::Drawing;
use opendwg::object::{DimKind, Entity, NonGraphical, Object, TableStyle};

const TEMPLATE: &str = include_str!("../scripts/SIGNOFF-TEMPLATE.md");

/// A pack with few lines in `new_200k_lines.dwg` (the content is the same
/// otherwise; the example writes the full count).
fn small_pack() -> Pack {
    pack::build(100)
}

/// The names every pack has, and those it has when the corpus is fetched.
fn expected_names() -> BTreeSet<String> {
    let mut names: BTreeSet<String> = [
        "new_empty.dwg",
        "new_every_entity.dwg",
        "new_200k_lines.dwg",
        "edit_dimlfac.dwg",
        pack::MTEXT_FRAME_EDIT,
    ]
    .map(String::from)
    .into();
    for (n, _) in pack::EXTENDED {
        if common::corpus_file(n).is_some() {
            names.insert(format!("extended_{n}.dwg"));
        }
    }
    if common::corpus_file(pack::BODY_SOURCE).is_some() {
        names.insert(pack::BODY_EDIT.into());
    }
    let fetched = common::fetched_files();
    for (e, _) in &fetched {
        names.insert(format!("corpus_{}.dwg", e.name));
    }
    if common::corpus_file(pack::TABLE_STYLE_SOURCE).is_some() {
        names.insert("edit_table_style.dwg".into());
    }
    if common::corpus_file(pack::RENUMBER_SOURCE).is_some() {
        names.insert(pack::RENUMBERED.into());
    }
    names
}

#[test]
fn pack_has_every_file_and_each_reads_back() {
    let pack = small_pack();
    assert!(pack.errors.is_empty(), "{:?}", pack.errors);
    let names: BTreeSet<String> = pack.files.iter().map(|f| f.name.clone()).collect();
    assert_eq!(names, expected_names(), "skipped: {:?}", pack.skipped);
    assert_eq!(names.len(), pack.files.len(), "a name repeats");
    for f in &pack.files {
        pack::reads_back_clean(f).unwrap();
        assert!(!f.what.is_empty(), "{}", f.name);
        if let Kind::Corpus(p) | Kind::Edit(Some(p)) = &f.kind {
            assert!(p.is_file(), "{}: {}", f.name, p.display());
        }
    }
    // Every corpus file saves (R2000/R2004 after the removals), so none is
    // skipped but for an unfetched corpus.
    for (name, why) in &pack.skipped {
        assert!(why.contains("fetch-corpus"), "{name}: {why}");
    }
    let lines = pack
        .files
        .iter()
        .find(|f| f.name == "new_200k_lines.dwg")
        .unwrap();
    let d = Drawing::from_bytes(&lines.bytes).unwrap();
    assert_eq!(d.entities_in(d.model_space()).count(), 100);
}

/// The table style edit: the converted R2010 form has the new data text
/// height in its data cell style (id 3).
#[test]
fn table_style_edit_carries_the_height() {
    let pack = small_pack();
    let Some(f) = pack.files.iter().find(|f| f.name == "edit_table_style.dwg") else {
        common::skip(pack::TABLE_STYLE_SOURCE);
        return;
    };
    let d = Drawing::from_bytes(&f.bytes).unwrap();
    let styles: Vec<_> = d
        .objects()
        .filter_map(|(_, o)| match o {
            Object::Other {
                object: NonGraphical::TableStyle(TableStyle::Cells(s)),
                ..
            } => Some(s),
            _ => None,
        })
        .collect();
    assert_eq!(styles.len(), 1);
    let data = styles[0]
        .cell_styles
        .iter()
        .find(|(_, s)| s.id == 3)
        .expect("a data cell style");
    let height = data.1.style.data.as_ref().unwrap().content.text_height;
    assert_eq!(height, pack::TABLE_DATA_HEIGHT);
}

/// The DIMLFAC edit: two linear dimensions 6 units long, the bottom one in
/// a style with DIMLFAC 2, each storing the unscaled measurement 6.
#[test]
fn dimlfac_edit_stores_unscaled_measurements() {
    let pack = small_pack();
    let f = pack
        .files
        .iter()
        .find(|f| f.name == "edit_dimlfac.dwg")
        .unwrap();
    let d = Drawing::from_bytes(&f.bytes).unwrap();
    let mut seen = Vec::new();
    for (_, o) in d.objects() {
        let Object::Entity {
            entity: Entity::Dimension(dim),
            ..
        } = o
        else {
            continue;
        };
        let DimKind::Linear { ext1, ext2, .. } = dim.kind else {
            panic!("{:?}", dim.kind)
        };
        assert_eq!(ext2[0] - ext1[0], 6.0);
        assert_eq!(dim.common.measurement, 6.0);
        let style = d
            .dim_styles()
            .find(|&(h, _)| h == dim.common.dimstyle)
            .unwrap()
            .1;
        seen.push((ext1[1], style.entry.name.clone(), style.dimlfac));
    }
    seen.sort_by(|a, b| a.0.total_cmp(&b.0));
    assert_eq!(
        seen,
        [
            (0.0, "DIMLFAC2".to_string(), pack::DIMLFAC),
            (3.0, "Standard".to_string(), 1.0)
        ]
    );
}

/// The `*.dwg` names written in backquotes in `text`.
fn quoted_dwg_names(text: &str) -> BTreeSet<String> {
    text.split('`')
        .skip(1)
        .step_by(2)
        .filter(|s| s.len() > 4 && s.ends_with(".dwg") && !s.contains('*') && !s.contains('/'))
        .map(String::from)
        .collect()
}

#[test]
fn signoff_markdown_is_complete() {
    let pack = small_pack();
    let md = pack::signoff_markdown(&pack, TEMPLATE, "abc1234", "2026-10-20");
    assert!(!md.contains("{{"), "a placeholder is left");
    assert!(md.contains("commit `abc1234`") && md.contains("2026-10-20"));
    for (check, _) in pack::CHECKS {
        assert!(md.contains(check), "{check}");
    }
    for (i, f) in pack.files.iter().enumerate() {
        let heading = format!("### {}. `{}`", i + 1, f.name);
        let at = md.find(&heading).unwrap_or_else(|| panic!("{heading}"));
        // Its result table: one row per check.
        let section = &md[at..];
        let section = &section[..section[4..].find("\n### ").map_or(section.len(), |n| n + 4)];
        for (check, _) in pack::CHECKS {
            assert!(
                section.contains(&format!("| {check} |")),
                "{}: {check}",
                f.name
            );
        }
        let has_original = matches!(f.kind, Kind::Corpus(_) | Kind::Edit(Some(_)));
        assert_eq!(section.contains("originals/"), has_original, "{}", f.name);
    }
    // The specific checks name only files a full pack makes.
    let mut all = expected_names();
    for e in common::load_manifest().file {
        all.insert(format!("corpus_{}.dwg", e.name));
    }
    all.insert("edit_table_style.dwg".into());
    all.insert(pack::RENUMBERED.into());
    all.insert(pack::BODY_EDIT.into());
    for (n, _) in pack::EXTENDED {
        all.insert(format!("extended_{n}.dwg"));
    }
    let named = quoted_dwg_names(TEMPLATE);
    assert!(!named.is_empty());
    let unknown: Vec<_> = named.difference(&all).collect();
    assert!(unknown.is_empty(), "the template names {unknown:?}");
}

/// The renumbering edit: every object of the unshifted round trip is there
/// under its handle plus the shift, and each solid's ACIS data (from the
/// data storage section) is the unshifted one's, byte for byte.
#[test]
fn renumbered_solids_keep_their_data() {
    use opendwg::Handle;
    let pack = small_pack();
    let find = |name: &str| pack.files.iter().find(|f| f.name == name);
    let (Some(shifted), Some(plain)) = (
        find(pack::RENUMBERED),
        find(&format!("corpus_{}.dwg", pack::RENUMBER_SOURCE)),
    ) else {
        common::skip(pack::RENUMBER_SOURCE);
        return;
    };
    let shifted = Drawing::from_bytes(&shifted.bytes).unwrap();
    let plain = Drawing::from_bytes(&plain.bytes).unwrap();
    assert_eq!(shifted.objects().count(), plain.objects().count());
    let mut solids = 0;
    for (h, o) in plain.objects() {
        let moved = Handle(h.0 + pack::RENUMBER_SHIFT);
        let other = shifted
            .get(moved)
            .unwrap_or_else(|| panic!("{h} not at {moved}"));
        assert_eq!(other.type_name(), o.type_name(), "{h}");
        if let Object::Entity {
            entity: Entity::Acis(_),
            ..
        } = o
        {
            let data = plain.acis_data(h).expect("ACIS data");
            assert!(!data.is_empty());
            assert_eq!(shifted.acis_data(moved), Some(data), "{h}");
            solids += 1;
        }
    }
    assert_eq!(solids, 3);
}

/// The GUIDs the checklist names for each new drawing are the ones stored in
/// the written file (S2 compares them with what AutoCAD shows).
#[test]
fn guid_notes_name_the_written_guids() {
    let pack = small_pack();
    let new: Vec<_> = pack
        .files
        .iter()
        .filter(|f| matches!(f.kind, Kind::New))
        .collect();
    assert_eq!(new.len(), 3);
    for f in new {
        let written = Drawing::from_bytes(&f.bytes).unwrap();
        let (fingerprint, version) = (
            written.header.fingerprintguid.to_string(),
            written.header.versionguid.to_string(),
        );
        assert!(fingerprint.len() > 30 && version.len() > 30, "{}", f.name);
        let note = f
            .expected
            .iter()
            .find(|n| n.contains("FINGERPRINTGUID"))
            .unwrap_or_else(|| panic!("{}: no GUID note", f.name));
        assert!(
            note.contains(&format!("shows {fingerprint} and"))
                && note.contains(&format!("`VERSIONGUID` shows {version} ")),
            "{}: {note}",
            f.name
        );
    }
}

/// A written file that does not read back fails pack generation rather
/// than giving a checklist line.
#[test]
fn unreadable_file_has_no_guid_note() {
    let err = pack::guid_note(b"AC1032 not a drawing").unwrap_err();
    assert!(err.contains("does not read back"), "{err}");
}

/// The BODY edit: the converted solid reads back as a BODY with its ACIS
/// data in the data storage section and no history handle.
#[test]
fn body_edit_has_no_history() {
    use opendwg::object::{AcisKind, AcisPayload};
    let pack = small_pack();
    let Some(f) = pack.files.iter().find(|f| f.name == pack::BODY_EDIT) else {
        common::skip(pack::BODY_SOURCE);
        return;
    };
    let d = Drawing::from_bytes(&f.bytes).unwrap();
    let bodies: Vec<_> = d
        .objects()
        .filter_map(|(h, o)| match o {
            Object::Entity {
                entity: Entity::Acis(a),
                ..
            } if a.kind == AcisKind::Body => Some((h, a)),
            _ => None,
        })
        .collect();
    assert_eq!(bodies.len(), 1);
    let (h, a) = bodies[0];
    assert_eq!(a.history, None);
    assert!(matches!(a.payload, AcisPayload::DataStorage { .. }));
    assert!(d.acis_data(h).is_some_and(|x| !x.is_empty()));
}

/// The MTEXT frame edit: one MTEXT with flags 0x10 and the default
/// background data, one with neither.
#[test]
fn mtext_frame_edit_keeps_the_frame() {
    let pack = small_pack();
    let f = pack
        .files
        .iter()
        .find(|f| f.name == pack::MTEXT_FRAME_EDIT)
        .unwrap();
    let d = Drawing::from_bytes(&f.bytes).unwrap();
    let mut flags: Vec<(i32, bool)> = d
        .objects()
        .filter_map(|(_, o)| match o {
            Object::Entity {
                entity: Entity::MText(m),
                ..
            } => Some((m.background_flags, m.background.is_some())),
            _ => None,
        })
        .collect();
    flags.sort();
    assert_eq!(flags, [(0, false), (0x10, true)]);
}
