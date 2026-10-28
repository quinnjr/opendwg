//! Required objects the writer adds when a drawing lacks them.
//!
//! The writer never refuses what it can repair. Files written by other
//! producers lack objects AutoCAD requires, which the writer can make from
//! the drawing's own values:
//!
//! - the dimension style `Standard` (BricsCAD, SolidWorks, Teigha and
//!   GDAL's R2000 files of the extended corpus have none): the header's
//!   dimension variables, as [`Drawing::new`] makes it, with the header's
//!   dimension text style, arrow blocks and linetypes when they name an
//!   object of their kind (else the text style `Standard` or `$TEXTSTYLE`,
//!   and the defaults);
//! - the named object dictionary's `ACAD_GROUP` and `ACAD_PLOTSETTINGS`
//!   dictionaries (civilcraft's own writer leaves `ACAD_PLOTSETTINGS` out):
//!   empty, as AutoCAD creates them, and named by `$DICT_PLOTSETTINGS` /
//!   `$DICT_ACAD_GROUP` (which, naming another dictionary the named object
//!   dictionary does not list, would disagree with it).
//!
//! The additions are not made to the drawing, nor to a copy of it: they are
//! a [`Repair`], the objects added and the copies of the objects and header
//! they change, which the writer lays over the drawing with its other
//! refreshed copies (see `drawing::write`), validates and writes; the
//! drawing itself is not changed. Each one is a warning, from
//! [`Drawing::validate`] and [`Drawing::write_diagnostics`]. A repair that
//! cannot be made (no DIMSTYLE table, an entry of that name that is not a
//! dictionary, handles used up) is left to validation, which reports what is
//! missing. The additions are made as [`Drawing::add_dim_style`] and
//! [`Drawing::add_object`] would make them, with the same checks.

use std::collections::BTreeMap;

use super::Drawing;
use super::api::{names_eq, next_handles};
use super::new::dim_style;
use crate::diag::{Diagnostic, Severity};
use crate::handle::Handle;
use crate::object::{Dictionary, DimStyle, NonGraphical, Object, ObjectCommon, TableKind};
use crate::section::HeaderVars;

/// The named object dictionary entries the writer creates empty.
const EMPTY_NAMED_DICTIONARIES: [&str; 2] = ["ACAD_GROUP", "ACAD_PLOTSETTINGS"];

fn warning(message: String) -> Diagnostic {
    Diagnostic {
        severity: Severity::Warning,
        handle: None,
        message,
    }
}

/// The required objects the writer adds to a drawing that lacks them (see
/// the module documentation).
#[derive(Debug)]
pub(super) struct Repair {
    /// The header as written: `$HANDSEED` past the added objects, and
    /// `$DICT_ACAD_GROUP` / `$DICT_PLOTSETTINGS` naming the added
    /// dictionaries.
    pub(super) header: HeaderVars,
    /// The added objects and the changed copies of the objects listing them
    /// (the DIMSTYLE control, the named object dictionary), by handle.
    pub(super) objects: BTreeMap<Handle, Object>,
    /// The added objects' handles, ascending, above every object's.
    pub(super) added: Vec<Handle>,
    /// A warning per addition.
    pub(super) warnings: Vec<Diagnostic>,
}

impl Repair {
    /// The object `h` as repaired: the changed copy, else the drawing's.
    fn get<'a>(&'a self, d: &'a Drawing, h: Handle) -> Option<&'a Object> {
        self.objects.get(&h).or_else(|| d.objects.get(&h))
    }

    /// The copy of the drawing's object `h` to change, made on first use.
    fn edit(&mut self, d: &Drawing, h: Handle) -> Option<&mut Object> {
        if !self.objects.contains_key(&h) {
            self.objects.insert(h, d.objects.get(&h)?.clone());
        }
        self.objects.get_mut(&h)
    }

    /// Add `obj` under handle `h`, allocated with seed `seed` (see
    /// [`next_handles`]): the seed moves past it.
    fn add(&mut self, h: Handle, seed: Handle, obj: Object) {
        self.objects.insert(h, obj);
        self.added.push(h);
        self.header.handseed = seed;
    }

    /// The next handle and the seed after it, as `Drawing::alloc_handle`
    /// gives it in the drawing with the additions so far.
    fn alloc(&self, d: &Drawing) -> Option<(Handle, Handle)> {
        let last = self
            .added
            .last()
            .copied()
            .or_else(|| d.objects.last_key_value().map(|(&h, _)| h));
        let (handles, seed) = next_handles(self.header.handseed, last, 1).ok()?;
        Some((handles[0], seed))
    }
}

impl Drawing {
    /// The required objects the drawing lacks that the writer adds (see the
    /// module documentation), with a warning for each; `None` when it lacks
    /// none the writer can add.
    pub(super) fn repair(&self) -> Option<Repair> {
        let standard = self.lacks_standard_dim_style();
        let dictionaries: Vec<&str> = EMPTY_NAMED_DICTIONARIES
            .into_iter()
            .filter(|n| self.lacks_named_dictionary(n))
            .collect();
        if !standard && dictionaries.is_empty() {
            return None;
        }
        let mut r = Repair {
            header: self.header.clone(),
            objects: BTreeMap::new(),
            added: Vec::new(),
            warnings: Vec::new(),
        };
        if standard {
            let text_style = self.current_text_style().unwrap_or_default();
            let mut style = dim_style("Standard", &self.header, text_style);
            self.header_dim_handles(&mut style);
            if let Some(h) = self.add_standard_dim_style(&mut r, style) {
                r.warnings.push(warning(format!(
                    "the drawing has no dimension style \"Standard\" (AutoCAD requires it); \
                     the writer adds one (DIMSTYLE {h}) from the header's dimension variables"
                )));
            }
        }
        for name in dictionaries {
            if let Some(h) = self.add_named_dictionary(&mut r, name) {
                r.warnings.push(warning(format!(
                    "the named object dictionary has no {name} dictionary (AutoCAD requires \
                     it); the writer adds an empty one (DICTIONARY {h})"
                )));
            }
        }
        (!r.warnings.is_empty()).then_some(r)
    }

    /// Add `style` as the DIMSTYLE `Standard` to `r`, as
    /// [`Drawing::add_dim_style`] adds it: owned by the DIMSTYLE control and
    /// among its entries; `None` (nothing added) where that refuses it.
    fn add_standard_dim_style(&self, r: &mut Repair, style: DimStyle) -> Option<Handle> {
        let (control, _) = self.table_control(TableKind::DimStyle)?;
        if !self.table_named(TableKind::DimStyle, "Standard").is_empty() {
            return None;
        }
        let (h, seed) = r.alloc(self)?;
        let obj = Object::Other {
            common: ObjectCommon {
                handle: h,
                owner: control,
                ..ObjectCommon::new()
            },
            object: NonGraphical::DimStyle(Box::new(style)),
        };
        self.check_new(&obj, &[h]).ok()?;
        if let Some(Object::Other {
            object: NonGraphical::TableControl(c),
            ..
        }) = r.edit(self, control)
        {
            c.entries.push(h);
        }
        r.add(h, seed, obj);
        Some(h)
    }

    /// Add the empty dictionary `name` to the named object dictionary in
    /// `r`, as [`Drawing::add_object`] adds an entry (owned by it, naming it
    /// among its reactors), its entries kept in byte order when they were,
    /// and point the header variable for it at the new dictionary; `None`
    /// (nothing added) where `add_object` refuses it.
    fn add_named_dictionary(&self, r: &mut Repair, name: &str) -> Option<Handle> {
        let nod = r.header.dict_named_objects;
        let entries = match r.get(self, nod)? {
            Object::Other {
                object: NonGraphical::Dictionary(d),
                ..
            } => &d.entries,
            Object::Other {
                object: NonGraphical::DictionaryWithDefault(d),
                ..
            } => &d.dictionary.entries,
            _ => return None,
        };
        if entries.iter().any(|(n, _)| names_eq(n, name)) {
            return None;
        }
        let (h, seed) = r.alloc(self)?;
        let empty = Dictionary {
            entries: Vec::new(),
            cloning: 1,
            hard_owner: 0,
        };
        let obj = Object::Other {
            common: ObjectCommon {
                handle: h,
                owner: nod,
                reactors: vec![nod],
                ..ObjectCommon::new()
            },
            object: NonGraphical::Dictionary(empty),
        };
        self.check_new(&obj, &[h]).ok()?;
        match r.edit(self, nod) {
            Some(Object::Other {
                object: NonGraphical::Dictionary(d),
                ..
            }) => {
                let sorted = d.entries.windows(2).all(|w| w[0].0 <= w[1].0);
                let at = if sorted {
                    d.entries.partition_point(|(n, _)| n.as_str() < name)
                } else {
                    d.entries.len()
                };
                d.entries.insert(at, (name.to_owned(), h));
            }
            Some(Object::Other {
                object: NonGraphical::DictionaryWithDefault(d),
                ..
            }) => d.dictionary.entries.push((name.to_owned(), h)),
            _ => {}
        }
        r.add(h, seed, obj);
        // The header variable names the dictionary the named object
        // dictionary lists, so the two never disagree.
        match name {
            "ACAD_GROUP" => r.header.dict_acad_group = h,
            _ => r.header.dict_plotsettings = h,
        }
        Some(h)
    }

    /// Give `style` the header's dimension handles that name an object of
    /// the right kind: `$DIMTXSTY` (a text style), the arrow blocks
    /// (`$DIMLDRBLK`, `$DIMBLK`, `$DIMBLK1`, `$DIMBLK2`: block records) and
    /// the linetypes (`$DIMLTYPE`, `$DIMLTEX1`, `$DIMLTEX2`). Others keep
    /// the defaults.
    fn header_dim_handles(&self, style: &mut DimStyle) {
        let is = |h: Handle, f: fn(&NonGraphical) -> bool| {
            h.0 != 0
                && matches!(self.objects.get(&h), Some(Object::Other { object, .. }) if f(object))
        };
        let text = |o: &NonGraphical| matches!(o, NonGraphical::TextStyle(_));
        let block = |o: &NonGraphical| matches!(o, NonGraphical::BlockRecord(_));
        let linetype = |o: &NonGraphical| matches!(o, NonGraphical::Linetype(_));
        let h = &self.header;
        if is(h.dimtxsty, text) {
            style.dimtxsty = h.dimtxsty;
        }
        for (from, to) in [
            (h.dimldrblk, &mut style.dimldrblk),
            (h.dimblk, &mut style.dimblk),
            (h.dimblk1, &mut style.dimblk1),
            (h.dimblk2, &mut style.dimblk2),
        ] {
            if is(from, block) {
                *to = from;
            }
        }
        for (from, to) in [
            (h.dimltype, &mut style.dimltype),
            (h.dimltex1, &mut style.dimltex1),
            (h.dimltex2, &mut style.dimltex2),
        ] {
            if let Some(from) = from.filter(|&x| is(x, linetype)) {
                *to = from;
            }
        }
    }

    /// Whether the DIMSTYLE table exists and has no record `Standard`.
    fn lacks_standard_dim_style(&self) -> bool {
        self.table_control(TableKind::DimStyle).is_some()
            && !self
                .table_handles(TableKind::DimStyle)
                .into_iter()
                .any(|h| match self.objects.get(&h) {
                    Some(Object::Other {
                        object: NonGraphical::DimStyle(s),
                        ..
                    }) => names_eq(&s.entry.name, "Standard"),
                    _ => false,
                })
    }

    /// Whether the named object dictionary is a dictionary without an entry
    /// `name`, compared without case (as AutoCAD looks keys up, and
    /// [`Drawing::add_object`] refuses a second entry of the name).
    fn lacks_named_dictionary(&self, name: &str) -> bool {
        match self.objects.get(&self.header.dict_named_objects) {
            Some(Object::Other {
                object: NonGraphical::Dictionary(d),
                ..
            }) => !d.entries.iter().any(|(n, _)| names_eq(n, name)),
            Some(Object::Other {
                object: NonGraphical::DictionaryWithDefault(d),
                ..
            }) => !d.dictionary.entries.iter().any(|(n, _)| names_eq(n, name)),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn errors(d: &Drawing) -> Vec<Diagnostic> {
        d.validate()
            .into_iter()
            .filter(|x| x.severity == Severity::Error)
            .collect()
    }

    /// The drawing repaired as the writer did before the additions were an
    /// overlay: a copy, with the objects added through the mutation API.
    fn repaired_copy(d: &Drawing) -> Option<(Drawing, Vec<Diagnostic>)> {
        let standard = d.lacks_standard_dim_style();
        let names: Vec<&str> = EMPTY_NAMED_DICTIONARIES
            .into_iter()
            .filter(|n| d.lacks_named_dictionary(n))
            .collect();
        if !standard && names.is_empty() {
            return None;
        }
        let mut d = d.clone();
        let mut warnings = Vec::new();
        if standard {
            let text_style = d.current_text_style().unwrap_or_default();
            let mut style = dim_style("Standard", &d.header, text_style);
            d.header_dim_handles(&mut style);
            if let Ok(h) = d.add_dim_style(style) {
                warnings.push(warning(format!(
                    "the drawing has no dimension style \"Standard\" (AutoCAD requires it); \
                     the writer adds one (DIMSTYLE {h}) from the header's dimension variables"
                )));
            }
        }
        for name in names {
            let nod = d.header.dict_named_objects;
            let empty = Dictionary {
                entries: Vec::new(),
                cloning: 1,
                hard_owner: 0,
            };
            let Ok(h) = d.add_object(nod, name, NonGraphical::Dictionary(empty)) else {
                continue;
            };
            if let Some(Object::Other {
                object: NonGraphical::Dictionary(n),
                ..
            }) = d.objects.get_mut(&nod)
            {
                let sorted = n.entries[..n.entries.len() - 1]
                    .windows(2)
                    .all(|w| w[0].0 <= w[1].0);
                if sorted {
                    let entry = n.entries.pop().unwrap();
                    let at = n.entries.partition_point(|(x, _)| x.as_str() < name);
                    n.entries.insert(at, entry);
                }
            }
            match name {
                "ACAD_GROUP" => d.header.dict_acad_group = h,
                _ => d.header.dict_plotsettings = h,
            }
            warnings.push(warning(format!(
                "the named object dictionary has no {name} dictionary (AutoCAD requires \
                 it); the writer adds an empty one (DICTIONARY {h})"
            )));
        }
        (!warnings.is_empty()).then_some((d, warnings))
    }

    /// The overlay gives what the repaired copy gives: the same file,
    /// validation findings and write warnings.
    fn assert_repair_as_copy(d: &Drawing, what: &str) {
        let Some((copy, warnings)) = repaired_copy(d) else {
            assert!(d.repair().is_none(), "{what}");
            return;
        };
        assert!(copy.repair().is_none(), "{what}");
        let saved = crate::section::JulianDate {
            day: 2_461_000,
            ms: 1234,
        };
        let mut found = warnings.clone();
        found.extend(copy.validate());
        found.sort_by(|a, b| (a.handle, &a.message).cmp(&(b.handle, &b.message)));
        found.dedup();
        assert_eq!(d.validate(), found, "{what}");
        let mut diags = warnings.clone();
        diags.extend(copy.write_diagnostics());
        assert_eq!(d.write_diagnostics(), diags, "{what}");
        match (d.to_bytes_at(saved), copy.to_bytes_at(saved)) {
            (Ok(a), Ok(b)) => assert!(a == b, "{what}: the files differ"),
            (a, b) => assert_eq!(
                a.err().map(|e| e.to_string()),
                b.err().map(|e| e.to_string()),
                "{what}"
            ),
        }
    }

    /// Drawings lacking each required object, all of them, or with a named
    /// object dictionary out of byte order or with a default, are written,
    /// validated and warned about as the repaired copy is; so is every
    /// corpus drawing that needs a repair.
    #[test]
    fn repair_overlay_matches_the_repaired_copy() {
        let strip = |dims: bool, names: &[&str], sorted: bool, default: bool| {
            let mut d = Drawing::new();
            if dims {
                for (h, _) in d
                    .dim_styles()
                    .map(|(h, s)| (h, s.clone()))
                    .collect::<Vec<_>>()
                {
                    if let Some(Object::Other {
                        object: NonGraphical::DimStyle(s),
                        ..
                    }) = d.objects.get_mut(&h)
                    {
                        s.entry.name = format!("X{h}");
                    }
                }
            }
            let nod = d.header.dict_named_objects;
            if let Some(Object::Other { object, .. }) = d.objects.get_mut(&nod) {
                if let NonGraphical::Dictionary(n) = object {
                    n.entries.retain(|(x, _)| !names.contains(&x.as_str()));
                    if !sorted {
                        n.entries.reverse();
                    }
                }
                if default {
                    let NonGraphical::Dictionary(n) = object.clone() else {
                        panic!()
                    };
                    *object =
                        NonGraphical::DictionaryWithDefault(crate::object::DictionaryWithDefault {
                            dictionary: n,
                            default_entry: Handle(0),
                        });
                }
            }
            d.ref_index = Default::default();
            d
        };
        let both = ["ACAD_GROUP", "ACAD_PLOTSETTINGS"];
        // No copy of the drawing: the three objects added, and copies of
        // the DIMSTYLE control and the named object dictionary.
        let all = strip(true, &both, true, false);
        let changes = all.refreshed(&mut super::super::write::WriteWork::default());
        assert_eq!((changes.added.len(), changes.replaced.len()), (3, 5));
        for (i, d) in [
            strip(true, &[], true, false),
            strip(false, &["ACAD_GROUP"], true, false),
            strip(false, &["ACAD_PLOTSETTINGS"], true, false),
            strip(true, &both, true, false),
            strip(true, &both, false, false),
            strip(false, &both, true, true),
            Drawing::new(),
        ]
        .iter()
        .enumerate()
        {
            assert_repair_as_copy(d, &format!("case {i}"));
        }
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus/files");
        let Ok(files) = std::fs::read_dir(&dir) else {
            return;
        };
        let mut repaired = 0;
        for f in files.flatten() {
            let path = f.path();
            if path.extension().is_none_or(|x| x != "dwg") {
                continue;
            }
            let Ok(d) = Drawing::open(&path) else {
                continue;
            };
            if d.repair().is_some() && d.unconvertible_objects().is_empty() {
                assert_repair_as_copy(&d, &path.display().to_string());
                repaired += 1;
            }
        }
        eprintln!("{repaired} corpus drawings repaired");
    }

    fn warned(d: &Drawing, what: &str) -> bool {
        d.validate()
            .iter()
            .any(|x| x.severity == Severity::Warning && x.message.contains(what))
    }

    /// A drawing without the dimension style `Standard` (BricsCAD's files:
    /// `brx-01` of the extended corpus) validates with a warning, and is
    /// saved with one added; the drawing itself is not changed.
    #[test]
    fn missing_standard_dim_style_is_added_at_save() {
        let mut d = Drawing::new();
        let standard = d
            .dim_styles()
            .find(|(_, s)| s.entry.name == "Standard")
            .map(|(h, _)| h)
            .unwrap();
        if let Some(Object::Other {
            object: NonGraphical::DimStyle(s),
            ..
        }) = d.objects.get_mut(&standard)
        {
            s.entry.name = "ISO-25".into();
        }
        let before = d.objects.len();
        assert!(errors(&d).is_empty(), "{:?}", errors(&d));
        assert!(warned(&d, "dimension style \"Standard\""));
        let (bytes, diags) = d.to_bytes_with_diagnostics().unwrap();
        assert!(diags.iter().any(|x| x.message.contains("\"Standard\"")));
        assert_eq!(d.objects.len(), before);
        let back = Drawing::from_bytes(&bytes).unwrap();
        assert_eq!(back.objects.len(), before + 1);
        assert!(back.dim_styles().any(|(_, s)| s.entry.name == "Standard"));
        assert!(back.dim_styles().any(|(_, s)| s.entry.name == "ISO-25"));
        assert!(back.validate().is_empty(), "{:?}", back.validate());
    }

    /// The added `Standard` dimension style takes the header's dimension
    /// text style, arrow block and linetype when they name objects of their
    /// kind, and the defaults for those that name none.
    #[test]
    fn added_standard_dim_style_takes_the_header_handles() {
        let mut d = Drawing::new();
        let standard = d
            .dim_styles()
            .find(|(_, s)| s.entry.name == "Standard")
            .map(|(h, _)| h)
            .unwrap();
        if let Some(Object::Other {
            object: NonGraphical::DimStyle(s),
            ..
        }) = d.objects.get_mut(&standard)
        {
            s.entry.name = "ISO-25".into();
        }
        let arrow = d.add_block("_ARROW").unwrap();
        let mut style = d.text_styles().next().unwrap().1.clone();
        style.entry.name = "DimText".into();
        let text = d.add_text_style(style).unwrap();
        d.header.dimblk = arrow;
        d.header.dimtxsty = text;
        d.header.dimblk1 = Handle(0); // none: the default stays
        let back = Drawing::from_bytes(&d.to_bytes().unwrap()).unwrap();
        let (_, s) = back
            .dim_styles()
            .find(|(_, s)| s.entry.name == "Standard")
            .unwrap();
        assert_eq!((s.dimblk, s.dimtxsty, s.dimblk1), (arrow, text, Handle(0)));
    }

    /// `$DICT_PLOTSETTINGS` naming a dictionary the named object dictionary
    /// does not list under `ACAD_PLOTSETTINGS` is pointed at the one added,
    /// so the two agree.
    #[test]
    fn plot_settings_variable_follows_the_added_dictionary() {
        let mut d = Drawing::new();
        let nod = d.header.dict_named_objects;
        if let Some(Object::Other {
            object: NonGraphical::Dictionary(n),
            ..
        }) = d.objects.get_mut(&nod)
        {
            n.entries.retain(|(name, _)| name != "ACAD_PLOTSETTINGS");
        }
        let other = d.header.dict_plotsettings;
        d.ref_index = Default::default();
        let back = Drawing::from_bytes(&d.to_bytes().unwrap()).unwrap();
        let Some(Object::Other {
            object: NonGraphical::Dictionary(n),
            ..
        }) = back.objects.get(&nod)
        else {
            panic!("the named object dictionary");
        };
        let (_, listed) = n
            .entries
            .iter()
            .find(|(n, _)| n == "ACAD_PLOTSETTINGS")
            .unwrap();
        assert_ne!(*listed, other);
        assert_eq!(back.header.dict_plotsettings, *listed);
    }

    /// A named object dictionary key differing only in case
    /// (`acad_plotsettings`) is the required entry, as AutoCAD looks keys
    /// up: nothing is added, and validation finds nothing missing.
    #[test]
    fn named_dictionary_keys_compare_without_case() {
        let mut d = Drawing::new();
        let nod = d.header.dict_named_objects;
        if let Some(Object::Other {
            object: NonGraphical::Dictionary(n),
            ..
        }) = d.objects.get_mut(&nod)
        {
            for (name, _) in &mut n.entries {
                if name == "ACAD_PLOTSETTINGS" {
                    *name = "acad_plotsettings".into();
                }
            }
        }
        d.ref_index = Default::default();
        assert!(d.repair().is_none());
        assert!(d.validate().is_empty(), "{:?}", d.validate());
        let (bytes, diags) = d.to_bytes_with_diagnostics().unwrap();
        assert!(diags.is_empty(), "{diags:?}");
        let back = Drawing::from_bytes(&bytes).unwrap();
        assert_eq!(back.objects.len(), d.objects.len());
    }

    /// A named object dictionary without `ACAD_PLOTSETTINGS` (civilcraft's
    /// writer: `cc-12`) gets an empty one at save, which `$DICT_PLOTSETTINGS`
    /// names, among the entries in byte order.
    #[test]
    fn missing_plot_settings_dictionary_is_added_at_save() {
        let mut d = Drawing::new();
        let nod = d.header.dict_named_objects;
        let old = d.header.dict_plotsettings;
        if let Some(Object::Other {
            object: NonGraphical::Dictionary(n),
            ..
        }) = d.objects.get_mut(&nod)
        {
            n.entries.retain(|(name, _)| name != "ACAD_PLOTSETTINGS");
        }
        d.objects.remove(&old);
        d.header.dict_plotsettings = Handle(0);
        d.ref_index = Default::default();
        assert!(errors(&d).is_empty(), "{:?}", errors(&d));
        assert!(warned(&d, "ACAD_PLOTSETTINGS"));
        let back = Drawing::from_bytes(&d.to_bytes().unwrap()).unwrap();
        assert!(back.validate().is_empty(), "{:?}", back.validate());
        let Some(Object::Other {
            object: NonGraphical::Dictionary(n),
            ..
        }) = back.objects.get(&nod)
        else {
            panic!("the named object dictionary");
        };
        let names: Vec<&str> = n.entries.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.is_sorted(), "{names:?}");
        let (_, added) = n
            .entries
            .iter()
            .find(|(n, _)| n == "ACAD_PLOTSETTINGS")
            .unwrap();
        assert_eq!(back.header.dict_plotsettings, *added);
        assert_eq!(d.header.dict_plotsettings, Handle(0));
    }

    /// A drawing that lacks nothing is not copied.
    #[test]
    fn complete_drawing_needs_no_repair() {
        assert!(Drawing::new().repair().is_none());
    }
}
