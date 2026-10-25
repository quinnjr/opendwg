//! The sign-off pack: the drawings opendwg writes that a person
//! checks in AutoCAD (`examples/make_signoff.rs`, `scripts/make-signoff.sh`)
//! and that the ODA File Converter oracle reads (`tests/oda.rs`).
//!
//! - every corpus file, read and written as R2018: as it is, or (R2000 and
//!   R2004) with the objects R2018 cannot hold removed through the API;
//! - new drawings: an empty one, one with an entity of every kind, one with
//!   200k lines;
//! - edits whose result is unverified: a converted pre-R2010 table style
//!   whose data text height is not 0.18, and a dimension under a style whose
//!   DIMLFAC is not 1, edited after a read.

use std::collections::BTreeMap;
use std::path::PathBuf;

use opendwg::object::{DimKind, Entity, EntityCommon, Line, NonGraphical, Object, TableStyle};
use opendwg::{Drawing, Handle, Severity, Version};

use super::every_entity::{dimension, with_every_entity};
use super::remove::remove_all;

/// The line count of `new_200k_lines.dwg`.
pub const LINES: usize = 200_000;

/// The corpus file the table style edit starts from: an R2007 drawing,
/// whose TABLESTYLE is in the pre-R2010 form the writer converts.
pub const TABLE_STYLE_SOURCE: &str = "acadsharp-sample-ac1021";

/// The data row text height the table style edit sets (AutoCAD's default
/// is 0.18, the value every corpus style has).
pub const TABLE_DATA_HEIGHT: f64 = 0.25;

/// The DIMLFAC of the dimension style the dimension edit uses.
pub const DIMLFAC: f64 = 2.0;

/// The kind of a pack file, which decides its checks.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// A corpus file written as R2018; the original is at the path.
    Corpus(PathBuf),
    /// A drawing made with `Drawing::new`.
    New,
    /// An edit of a corpus file (its path) or of a new drawing (`None`).
    Edit(Option<PathBuf>),
}

/// One drawing of the pack.
#[derive(Debug, Clone)]
pub struct PackFile {
    /// The file name, `*.dwg`.
    pub name: String,
    pub bytes: Vec<u8>,
    pub kind: Kind,
    /// What the drawing is, in plain words.
    pub what: String,
    /// What is expected to differ from the original, or to look odd, in
    /// plain words.
    pub expected: Vec<String>,
}

/// The pack, and the files that could not be made (name, reason).
#[derive(Debug, Default)]
pub struct Pack {
    pub files: Vec<PackFile>,
    pub skipped: Vec<(String, String)>,
    /// Problems that make the pack unusable (a written file that does not
    /// read back): generation fails on any.
    pub errors: Vec<String>,
}

/// The release name of a version magic.
pub fn release(magic: &str) -> &'static str {
    match magic {
        "AC1015" => "AutoCAD 2000",
        "AC1018" => "AutoCAD 2004",
        "AC1021" => "AutoCAD 2007",
        "AC1024" => "AutoCAD 2010",
        "AC1027" => "AutoCAD 2013",
        "AC1032" => "AutoCAD 2018",
        _ => "an unknown release",
    }
}

fn count_by_type(list: &[(Handle, String)]) -> String {
    let mut by_type: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, t) in list {
        *by_type.entry(t).or_default() += 1;
    }
    let v: Vec<String> = by_type.iter().map(|(t, n)| format!("{n} {t}")).collect();
    v.join(", ")
}

/// What a person sees missing when `list` is removed, in plain words.
fn visible_losses(list: &[(Handle, String)]) -> String {
    let mut seen: Vec<&str> = Vec::new();
    for (_, t) in list {
        let plain = match t.as_str() {
            "3DSOLID" => "3D solids",
            "REGION" => "regions",
            "MESH" => "mesh objects",
            "ACAD_TABLE" => "tables",
            "PDFUNDERLAY" => "PDF underlays",
            "MATERIAL" => "materials",
            "VISUALSTYLE" => "visual styles",
            "DIMASSOC" => "dimension associativity (dimensions no longer follow what they measure)",
            "ACDBSECTIONVIEWSTYLE" | "ACDBDETAILVIEWSTYLE" => "section and detail view styles",
            t if t.starts_with("BLOCK") => "dynamic block behaviour (grips and parameters)",
            _ => "other non-visible data",
        };
        if !seen.contains(&plain) {
            seen.push(plain);
        }
    }
    // The invisible rest last.
    seen.sort_by_key(|&p| p == "other non-visible data");
    seen.join(", ")
}

/// The ACIS entities (3D solids, regions, bodies) of `d`.
fn acis_count(d: &Drawing) -> usize {
    d.objects()
        .filter(|(_, o)| {
            matches!(
                o,
                Object::Entity {
                    entity: Entity::Acis(_),
                    ..
                }
            )
        })
        .count()
}

/// `d` written now, and the writer's warnings as plain notes.
fn write(d: &Drawing) -> Result<(Vec<u8>, Vec<String>), String> {
    let bytes = d.to_bytes().map_err(|e| e.to_string())?;
    let notes = d
        .write_diagnostics()
        .into_iter()
        .map(|x| format!("Writer note: {}", x.message))
        .collect();
    Ok((bytes, notes))
}

/// Every fetched corpus file written as R2018 (`corpus_<name>.dwg`); those
/// that cannot be written are listed as skipped.
pub fn corpus_files(pack: &mut Pack) {
    for (e, path) in super::fetched_files() {
        let name = format!("corpus_{}.dwg", e.name);
        let mut d = match Drawing::open(&path) {
            Ok(d) => d,
            Err(err) => {
                pack.skipped.push((name, format!("cannot be read: {err}")));
                continue;
            }
        };
        let source = d.source_version();
        let unconvertible = d.unconvertible_objects();
        let mut expected = Vec::new();
        if !unconvertible.is_empty() {
            expected.push(format!(
                "Missing on purpose, so not a fail: {}. opendwg cannot yet write these as \
                 AutoCAD 2018 and removed them before saving (technical list: {}).",
                visible_losses(&unconvertible),
                count_by_type(&unconvertible)
            ));
            remove_all(&mut d, &unconvertible, &e.name);
        }
        let solids = acis_count(&d);
        let mut what = format!(
            "The public sample `{}` ({}), opened and saved as AutoCAD 2018 by opendwg.",
            e.name,
            release(&e.version)
        );
        if solids > 0 && source.is_some_and(|v| v < Version::R2013) {
            what.push_str(&format!(
                " It has {solids} 3D solids or regions; opendwg moved their shape data into \
                 the storage AutoCAD 2018 uses for solids."
            ));
        }
        match write(&d) {
            Ok((bytes, notes)) => {
                expected.extend(notes);
                pack.files.push(PackFile {
                    name,
                    bytes,
                    kind: Kind::Corpus(path),
                    what,
                    expected,
                });
            }
            Err(err) => pack.skipped.push((name, format!("does not save: {err}"))),
        }
    }
    if super::fetched_files().is_empty() {
        pack.skipped.push((
            "corpus_*.dwg".into(),
            "the sample corpus is not fetched: run scripts/fetch-corpus.sh".into(),
        ));
    }
}

/// A new drawing with `n` lines in model space, one unit apart.
pub fn lines_drawing(n: usize) -> Drawing {
    let mut d = Drawing::new();
    let ms = d.model_space();
    let layer = d.layer_by_name("0").unwrap();
    for i in 0..n {
        let x = i as f64;
        let line = Entity::Line(Line {
            start: [x, 0.0, 0.0],
            end: [x, 1.0, 0.0],
            thickness: 0.0,
            extrusion: [0.0, 0.0, 1.0],
        });
        d.add_entity(ms, EntityCommon::new(layer), line).unwrap();
    }
    d
}

/// What AutoCAD shows for the identity variables (S2) of the written file
/// `bytes`: read back from the bytes, so the note names what the file holds
/// even when the writer fills or changes a GUID. A file that does not read
/// back is an error, not a checklist line.
pub fn guid_note(bytes: &[u8]) -> Result<String, String> {
    let w = Drawing::from_bytes(bytes).map_err(|e| format!("does not read back: {e}"))?;
    Ok(format!(
        "Typing `FINGERPRINTGUID` shows {} and typing `VERSIONGUID` shows {} (AutoCAD may \
         show them with or without the braces).",
        w.header.fingerprintguid, w.header.versionguid
    ))
}

/// The new drawings: `new_empty.dwg`, `new_every_entity.dwg` and
/// `new_200k_lines.dwg` (with `lines` lines).
pub fn new_files(pack: &mut Pack, lines: usize) {
    let mut push = |name: &str, d: &Drawing, what: String, mut expected: Vec<String>| match write(d)
    {
        Ok((bytes, notes)) => {
            match guid_note(&bytes) {
                Ok(note) => expected.push(note),
                Err(err) => pack.errors.push(format!("{name}: {err}")),
            }
            expected.extend(notes);
            pack.files.push(PackFile {
                name: name.into(),
                bytes,
                kind: Kind::New,
                what,
                expected,
            });
        }
        Err(err) => pack
            .skipped
            .push((name.into(), format!("does not save: {err}"))),
    };
    push(
        "new_empty.dwg",
        &Drawing::new(),
        "A new, empty drawing made by opendwg (no AutoCAD template involved).".into(),
        Vec::new(),
    );
    let (d, _) = with_every_entity(Drawing::new());
    push(
        "new_every_entity.dwg",
        &d,
        "A new drawing with one of every kind of object opendwg can create: lines, \
         circles, arcs, text, multiline text, blocks with attributes, polylines, splines, \
         every dimension kind, a leader and a multileader, hatches, an image, a wipeout, \
         an OLE frame and a layout viewport."
            .into(),
        vec![
            "The image refers to `logo.png`, which does not exist: a \"file not found\" \
             image frame is expected."
                .into(),
            "The OLE frame holds dummy data: it may show as an empty box.".into(),
        ],
    );
    push(
        "new_200k_lines.dwg",
        &lines_drawing(lines),
        format!(
            "A new drawing with {lines} short vertical lines side by side (a large-drawing \
             test)."
        ),
        Vec::new(),
    );
}

/// The edits: `edit_table_style.dwg` (from [`TABLE_STYLE_SOURCE`]) and
/// `edit_dimlfac.dwg`.
pub fn edit_files(pack: &mut Pack) {
    match table_style_edit() {
        Ok(Some(f)) => pack.files.push(f),
        Ok(None) => pack.skipped.push((
            "edit_table_style.dwg".into(),
            format!("the corpus lacks {TABLE_STYLE_SOURCE}: run scripts/fetch-corpus.sh"),
        )),
        Err(e) => pack.skipped.push(("edit_table_style.dwg".into(), e)),
    }
    match dimlfac_edit() {
        Ok(f) => pack.files.push(f),
        Err(e) => pack.skipped.push(("edit_dimlfac.dwg".into(), e)),
    }
    match renumbered_edit() {
        Ok(Some(f)) => pack.files.push(f),
        Ok(None) => pack.skipped.push((
            RENUMBERED.into(),
            format!("the corpus lacks {RENUMBER_SOURCE}: run scripts/fetch-corpus.sh"),
        )),
        Err(e) => pack.skipped.push((RENUMBERED.into(), e)),
    }
}

/// The corpus file the renumbering edit starts from: an R2010 drawing with
/// solids, whose ACIS data the writer moves into the data storage section.
pub const RENUMBER_SOURCE: &str = "acadsharp-sample-ac1024";

/// The file name of the renumbering edit.
pub const RENUMBERED: &str = "edit_renumbered_ac1024.dwg";

/// How far the renumbering edit moves every handle.
pub const RENUMBER_SHIFT: u64 = 0x1000;

/// [`RENUMBER_SOURCE`] with every handle moved up by [`RENUMBER_SHIFT`]
/// (`Drawing::rewrite_handles`): its solids' data storage records must
/// follow their entities.
fn renumbered_edit() -> Result<Option<PackFile>, String> {
    let Some(path) = super::corpus_file(RENUMBER_SOURCE) else {
        return Ok(None);
    };
    let mut d = Drawing::open(&path).map_err(|e| e.to_string())?;
    d.rewrite_handles(|h| Handle(h.0 + RENUMBER_SHIFT))
        .map_err(|e| e.to_string())?;
    let (bytes, notes) = write(&d)?;
    let mut expected = vec![format!(
        "Looks exactly like `corpus_{RENUMBER_SOURCE}.dwg`, solids included (S3): only \
         the internal object numbers (handles) differ. `LIST` on any object shows a handle \
         {RENUMBER_SHIFT:X} (hexadecimal) higher than the same object in that file."
    )];
    expected.extend(notes);
    Ok(Some(PackFile {
        name: RENUMBERED.into(),
        bytes,
        kind: Kind::Edit(Some(path)),
        what: format!(
            "The public sample `{RENUMBER_SOURCE}` (AutoCAD 2010) with every internal object \
             number (handle) moved up by {RENUMBER_SHIFT:X} hexadecimal by opendwg before \
             saving. Its 3D solids' shape data must follow their new numbers."
        ),
        expected,
    }))
}

/// [`TABLE_STYLE_SOURCE`] with its "Standard" table style's data row text
/// height set to [`TABLE_DATA_HEIGHT`].
fn table_style_edit() -> Result<Option<PackFile>, String> {
    let Some(path) = super::corpus_file(TABLE_STYLE_SOURCE) else {
        return Ok(None);
    };
    let mut d = Drawing::open(&path).map_err(|e| e.to_string())?;
    let h = d
        .objects()
        .find(|(_, o)| o.type_name() == "TABLESTYLE")
        .map(|(h, _)| h)
        .ok_or("no TABLESTYLE")?;
    let Some(Object::Other {
        object: NonGraphical::TableStyle(TableStyle::Legacy(style)),
        ..
    }) = d.get_mut(h)
    else {
        return Err(format!("{h} is not a pre-R2010 table style"));
    };
    // Data, title and header rows.
    style.rows[0].text_height = TABLE_DATA_HEIGHT;
    let [data, title, header] = [0, 1, 2].map(|i| style.rows[i].text_height);
    let margins = (style.horizontal_margin, style.vertical_margin);
    let (bytes, notes) = write(&d)?;
    let mut expected = vec![format!(
        "Table style \"Standard\": text height {data} for Data, {header} for Header, \
         {title} for Title; cell margins {} horizontal, {} vertical.",
        margins.0, margins.1
    )];
    expected.extend(notes);
    Ok(Some(PackFile {
        name: "edit_table_style.dwg".into(),
        bytes,
        kind: Kind::Edit(Some(path)),
        what: format!(
            "The public sample `{TABLE_STYLE_SOURCE}` (AutoCAD 2007) with its table style \
             \"Standard\" changed by opendwg: Data text height {TABLE_DATA_HEIGHT} instead of \
             0.18. opendwg converts this older kind of table style to the AutoCAD 2010+ kind \
             when it saves."
        ),
        expected,
    }))
}

/// A new drawing with a dimension style "DIMLFAC2" (DIMLFAC
/// [`DIMLFAC`]) and two linear dimensions, one in that style and one in
/// "Standard"; saved, read back, and both dimensions stretched from 5 to 6
/// units long.
fn dimlfac_edit() -> Result<PackFile, String> {
    let mut d = Drawing::new();
    let ms = d.model_space();
    let l0 = d.layer_by_name("0").ok_or("no layer 0")?;
    let standard = d.header.dimstyle;
    let mut style = d
        .dim_styles()
        .find(|&(h, _)| h == standard)
        .map(|(_, s)| s.clone())
        .ok_or("no Standard dimension style")?;
    style.entry.name = "DIMLFAC2".into();
    style.dimlfac = DIMLFAC;
    let scaled = d.add_dim_style(style).map_err(|e| e.to_string())?;
    let linear = |y: f64, len: f64| DimKind::Linear {
        ext1: [0.0, y, 0.0],
        ext2: [len, y, 0.0],
        def_point: [len, y + 1.0, 0.0],
        ext_line_rotation: 0.0,
        rotation: 0.0,
    };
    let mut dims = Vec::new();
    for (y, s) in [(0.0, scaled), (3.0, standard)] {
        let mut e = dimension(s, linear(y, 5.0));
        if let Entity::Dimension(dim) = &mut e {
            dim.common.text_midpoint = [2.5, y + 1.2];
        }
        dims.push(
            d.add_entity(ms, EntityCommon::new(l0), e)
                .map_err(|e| e.to_string())?,
        );
    }
    let first = d.to_bytes().map_err(|e| e.to_string())?;
    let mut d = Drawing::from_bytes(&first).map_err(|e| e.to_string())?;
    for (&h, y) in dims.iter().zip([0.0, 3.0]) {
        let Some(Object::Entity {
            entity: Entity::Dimension(dim),
            ..
        }) = d.get_mut(h)
        else {
            return Err(format!("{h} is not a dimension"));
        };
        dim.kind = linear(y, 6.0);
        dim.common.text_midpoint = [3.0, y + 1.2];
    }
    let (bytes, notes) = write(&d)?;
    let mut expected = vec![
        "Both dimensions measure 6 units. The one in style DIMLFAC2 should read 12 \
         (6 x 2), the one in Standard 6."
            .into(),
        "The dimensions were made without their picture block (AutoCAD keeps one per \
         dimension): they may only appear after a regeneration."
            .into(),
    ];
    expected.extend(notes);
    Ok(PackFile {
        name: "edit_dimlfac.dwg".into(),
        bytes,
        kind: Kind::Edit(None),
        what: format!(
            "A new drawing with a dimension style \"DIMLFAC2\" whose linear scale factor \
             (DIMLFAC) is {DIMLFAC}, and two horizontal dimensions: the bottom one in \
             DIMLFAC2, the top one in Standard. Both were saved, read back and edited by \
             opendwg (made one unit longer)."
        ),
        expected,
    })
}

/// The whole pack, with `lines` lines in `new_200k_lines.dwg`.
pub fn build(lines: usize) -> Pack {
    let mut pack = Pack::default();
    corpus_files(&mut pack);
    new_files(&mut pack, lines);
    edit_files(&mut pack);
    pack
}

/// Whether `f` reads back with no diagnostic and validates with no error.
pub fn reads_back_clean(f: &PackFile) -> Result<(), String> {
    let d = Drawing::from_bytes(&f.bytes).map_err(|e| format!("{}: {e}", f.name))?;
    if !d.diagnostics().is_empty() {
        return Err(format!(
            "{}: {:?}",
            f.name,
            &d.diagnostics()[..d.diagnostics().len().min(5)]
        ));
    }
    let errors: Vec<_> = d
        .validate()
        .into_iter()
        .filter(|x| x.severity == Severity::Error)
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!("{}: {errors:?}", f.name))
    }
}

// ---- SIGNOFF.md ----

/// The checks every file gets, in order: (short name, what to do and look
/// for). The third applies to corpus files and edits of them only.
pub const CHECKS: [(&str, &str); 4] = [
    (
        "Opens cleanly",
        "Open the file. Pass if it opens with no \"drawing file needs recovery\", \
         \"drawing is not valid\" or error message (the \"non-Autodesk DWG\" notice is \
         expected and does not count).",
    ),
    (
        "AUDIT: 0 errors",
        "Type `AUDIT`, press Enter, answer `N` (do not fix). Pass if the last line says \
         `Total errors found 0 fixed 0`. On a fail, write down the numbers and copy the \
         command-line text into the notes.",
    ),
    (
        "Looks like the original",
        "Open the original from the `originals` folder next to it and compare: same \
         objects, same places, same colours and text. Check the Model tab and every \
         layout tab. Differences listed under \"Expected\" are not a fail.",
    ),
    (
        "Saves and reopens",
        "Type `QSAVE` (keep the AutoCAD 2018 format if asked), close the file, open it \
         again. Pass if it saves and reopens with no message.",
    ),
];

/// The SIGNOFF.md checklist: `template` with `{{COMMIT}}`, `{{DATE}}`,
/// `{{FILE_TABLE}}`, `{{FILE_CHECKS}}` and `{{SKIPPED}}` filled in.
pub fn signoff_markdown(pack: &Pack, template: &str, commit: &str, date: &str) -> String {
    let mut table = String::from("| # | File | What it is |\n|---|---|---|\n");
    for (i, f) in pack.files.iter().enumerate() {
        table.push_str(&format!("| {} | `{}` | {} |\n", i + 1, f.name, f.what));
    }
    let mut checks = String::new();
    for (i, f) in pack.files.iter().enumerate() {
        checks.push_str(&format!("### {}. `{}`\n\n{}\n\n", i + 1, f.name, f.what));
        let original = match &f.kind {
            Kind::Corpus(p) | Kind::Edit(Some(p)) => p.file_name().map(|n| n.to_string_lossy()),
            _ => None,
        };
        if let Some(o) = &original {
            checks.push_str(&format!("Original to compare with: `originals/{o}`\n\n"));
        }
        if !f.expected.is_empty() {
            checks.push_str("Expected:\n\n");
            for x in &f.expected {
                checks.push_str(&format!("- {x}\n"));
            }
            checks.push('\n');
        }
        checks.push_str("| Check | Result (Pass / Fail / N/A) | Notes |\n|---|---|---|\n");
        for (j, (check, _)) in CHECKS.iter().enumerate() {
            let result = if j == 2 && original.is_none() {
                "N/A (no original)"
            } else {
                " "
            };
            checks.push_str(&format!("| {} | {result} | |\n", check));
        }
        checks.push('\n');
    }
    let skipped = if pack.skipped.is_empty() {
        "None: every file was made.".to_owned()
    } else {
        let v: Vec<String> = pack
            .skipped
            .iter()
            .map(|(n, why)| format!("- `{n}`: {why}"))
            .collect();
        v.join("\n")
    };
    let mut how = String::new();
    for (i, (check, what)) in CHECKS.iter().enumerate() {
        how.push_str(&format!("{}. **{check}.** {what}\n", i + 1));
    }
    template
        .replace("{{COMMIT}}", commit)
        .replace("{{DATE}}", date)
        .replace("{{CHECKS}}", how.trim_end())
        .replace("{{FILE_TABLE}}", table.trim_end())
        .replace("{{FILE_CHECKS}}", checks.trim_end())
        .replace("{{SKIPPED}}", &skipped)
}
