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
//! cannot be made (an entry of that name that is not a dictionary, handles
//! used up) is a warning saying why, and left to validation, which reports
//! what is missing. The additions are built and checked by the code
//! [`Drawing::add_dim_style`] and [`Drawing::add_object`] use.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;

use super::Drawing;
use super::api::{invalid, next_handles};
use super::new::dim_style;
use crate::diag::{Diagnostic, Severity};
use crate::error::{Error, Result};
use crate::handle::Handle;
use crate::object::{Dictionary, DimStyle, NonGraphical, Object, TableKind};
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

/// The warning for the added dimension style `Standard`, DIMSTYLE `h`.
fn added_dim_style(h: Handle) -> Diagnostic {
    warning(format!(
        "the drawing has no dimension style \"Standard\" (AutoCAD requires it); the writer \
         adds one (DIMSTYLE {h}) from the header's dimension variables"
    ))
}

/// The warning for the added empty dictionary `name`, DICTIONARY `h`.
fn added_dictionary(name: &str, h: Handle) -> Diagnostic {
    warning(format!(
        "the named object dictionary has no {name} dictionary (AutoCAD requires it); the \
         writer adds an empty one (DICTIONARY {h})"
    ))
}

/// The warning for a required `what` the writer cannot add, and why.
fn not_added(what: &str, e: &Error) -> Diagnostic {
    warning(format!(
        "the drawing has no {what} (AutoCAD requires it), and the writer cannot add one: {e}"
    ))
}

/// What [`not_added`] calls the dictionary `name`.
fn dictionary_what(name: &str) -> String {
    format!("{name} dictionary in the named object dictionary")
}

/// What [`not_added`] calls the dimension style `Standard`.
const DIM_STYLE_WHAT: &str = "dimension style \"Standard\"";

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
    /// The copy of the drawing's object `h` to change, made on first use.
    fn edit(&mut self, d: &Drawing, h: Handle) -> Option<&mut Object> {
        match self.objects.entry(h) {
            Entry::Occupied(e) => Some(e.into_mut()),
            Entry::Vacant(e) => Some(e.insert(d.objects.get(&h)?.clone())),
        }
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
    fn alloc(&self, d: &Drawing) -> Result<(Handle, Handle)> {
        let last = self
            .added
            .last()
            .copied()
            .or_else(|| d.objects.last_key_value().map(|(&h, _)| h));
        let (handles, seed) = next_handles(self.header.handseed, last, 1)?;
        Ok((handles[0], seed))
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
            let w = match self.add_standard_dim_style(&mut r, style) {
                Ok(h) => added_dim_style(h),
                Err(e) => not_added(DIM_STYLE_WHAT, &e),
            };
            r.warnings.push(w);
        }
        for name in dictionaries {
            let w = match self.add_named_dictionary(&mut r, name) {
                Ok(h) => added_dictionary(name, h),
                Err(e) => not_added(&dictionary_what(name), &e),
            };
            r.warnings.push(w);
        }
        (!r.warnings.is_empty()).then_some(r)
    }

    /// Add `style` as the DIMSTYLE `Standard` to `r`, as
    /// [`Drawing::add_dim_style`] adds it, with its checks: owned by the
    /// DIMSTYLE control and among its entries.
    fn add_standard_dim_style(&self, r: &mut Repair, style: DimStyle) -> Result<Handle> {
        let kind = TableKind::DimStyle;
        let new = self.new_table_entry(kind, NonGraphical::DimStyle(Box::new(style)), || {
            r.alloc(self)
        })?;
        let h = new.obj.handle();
        if let Some(Object::Other {
            object: NonGraphical::TableControl(c),
            ..
        }) = r.edit(self, new.list)
        {
            c.entries.push(h);
        }
        r.add(h, new.seed, new.obj);
        Ok(h)
    }

    /// Add the empty dictionary `name` to the named object dictionary in
    /// `r`, as [`Drawing::add_object`] adds an entry, with its checks (owned
    /// by it, naming it among its reactors), and point the header variable
    /// for it at the new dictionary.
    ///
    /// Two of `add_object`'s rules differ here. A plain dictionary's
    /// entries are kept in byte order when they were (as AutoCAD keeps
    /// them), not appended. And
    /// the writer writes the drawing's classes, so an object that needs a
    /// class it lacks cannot be added; a DICTIONARY is of a fixed type and
    /// never does. The name is checked against the drawing's entries, not
    /// the repair's: the repair adds each name once.
    fn add_named_dictionary(&self, r: &mut Repair, name: &str) -> Result<Handle> {
        let nod = self.header.dict_named_objects;
        let empty = Dictionary {
            entries: Vec::new(),
            cloning: 1,
            hard_owner: 0,
        };
        let object = NonGraphical::Dictionary(empty);
        let new = self.new_dictionary_entry(nod, name, object, || r.alloc(self))?;
        if !new.classes.is_empty() {
            return Err(invalid(
                None,
                "the dictionary needs a class the drawing lacks",
            ));
        }
        let h = new.obj.handle();
        match r.edit(self, nod) {
            Some(Object::Other {
                object: NonGraphical::Dictionary(d),
                ..
            }) if d.entries.windows(2).all(|w| w[0].0 <= w[1].0) => {
                let at = d.entries.partition_point(|(n, _)| n.as_str() < name);
                d.entries.insert(at, (name.to_owned(), h));
            }
            Some(o) => {
                if let Some(d) = o.dictionary_mut() {
                    d.entries.push((name.to_owned(), h));
                }
            }
            None => {}
        }
        r.add(h, new.seed, new.obj);
        // The header variable names the dictionary the named object
        // dictionary lists, so the two never disagree.
        match name {
            "ACAD_GROUP" => r.header.dict_acad_group = h,
            _ => r.header.dict_plotsettings = h,
        }
        Ok(h)
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

    /// Whether the DIMSTYLE table exists and has no record `Standard`
    /// (compared without case, from the name index).
    fn lacks_standard_dim_style(&self) -> bool {
        self.table_control(TableKind::DimStyle).is_some()
            && self.table_named(TableKind::DimStyle, "Standard").is_empty()
    }

    /// Whether the named object dictionary is a dictionary without an entry
    /// `name`, compared without case (as AutoCAD looks keys up, and
    /// [`Drawing::add_object`] refuses a second entry of the name), from
    /// the name index.
    fn lacks_named_dictionary(&self, name: &str) -> bool {
        let nod = self.header.dict_named_objects;
        self.objects
            .get(&nod)
            .and_then(Object::dictionary)
            .is_some()
            && self.dictionary_named(nod, name).is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drawing::ObjectSet;
    use crate::object::ObjectCommon;

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
            warnings.push(match d.add_dim_style(style) {
                Ok(h) => added_dim_style(h),
                Err(e) => not_added(DIM_STYLE_WHAT, &e),
            });
        }
        for name in names {
            let nod = d.header.dict_named_objects;
            let empty = Dictionary {
                entries: Vec::new(),
                cloning: 1,
                hard_owner: 0,
            };
            let h = match d.add_object(nod, name, NonGraphical::Dictionary(empty)) {
                Ok(h) => h,
                Err(e) => {
                    warnings.push(not_added(&dictionary_what(name), &e));
                    continue;
                }
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
            warnings.push(added_dictionary(name, h));
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
        // The copy lacks only what could not be added, which its own
        // repair warns about again: the first of its write warnings.
        let own = copy.repair().map_or_else(Vec::new, |r| {
            assert!(r.added.is_empty(), "{what}");
            r.warnings
        });
        assert!(own.iter().all(|w| warnings.contains(w)), "{what}");
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
        diags.extend(copy.write_diagnostics().into_iter().skip(own.len()));
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

    /// `d` with a `$HANDSEED` below its largest handle (a damaged file):
    /// the additions take the handles past it.
    fn stale_seed(mut d: Drawing) -> Drawing {
        d.header.handseed = Handle(1);
        d
    }

    /// `d` with an object at the largest handle: no handle is left for the
    /// additions, which are not made.
    fn used_up(mut d: Drawing) -> Drawing {
        let common = ObjectCommon {
            handle: Handle(u64::MAX),
            ..ObjectCommon::new()
        };
        d.objects.insert(
            Handle(u64::MAX),
            Object::Other {
                common,
                object: NonGraphical::Placeholder,
            },
        );
        d.ref_index = Default::default();
        d
    }

    /// Drawings lacking each required object, all of them, or with a named
    /// object dictionary out of byte order or with a default, with a stale
    /// seed or with the handles used up, are written, validated and warned
    /// about as the repaired copy is; so is every corpus drawing that needs
    /// a repair, and those with unconvertible objects once they are
    /// removed.
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
        assert_eq!((changes.added.len(), changes.overlay.len()), (3, 5));
        // The refresh passes read the added objects with the drawing's.
        let base = changes.objects(&all);
        let seen: Vec<Handle> = base.each().map(|(h, _)| h).collect();
        assert_eq!(seen.len(), all.objects.len() + 3);
        assert!(seen.windows(2).all(|w| w[0] < w[1]));
        for h in &changes.added {
            assert!(seen.contains(h) && base.object(*h).is_some(), "{h}");
        }
        for (i, d) in [
            strip(true, &[], true, false),
            strip(false, &["ACAD_GROUP"], true, false),
            strip(false, &["ACAD_PLOTSETTINGS"], true, false),
            strip(true, &both, true, false),
            strip(true, &both, false, false),
            strip(false, &both, true, true),
            Drawing::new(),
            stale_seed(strip(true, &both, true, false)),
            used_up(strip(true, &both, true, false)),
            used_up(strip(false, &["ACAD_GROUP"], true, false)),
        ]
        .iter()
        .enumerate()
        {
            assert_repair_as_copy(d, &format!("case {i}"));
        }
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus/files");
        let Ok(files) = std::fs::read_dir(&dir) else {
            assert!(
                std::env::var("OPENDWG_REQUIRE_CORPUS").as_deref() != Ok("1"),
                "OPENDWG_REQUIRE_CORPUS=1 but there is no corpus"
            );
            eprintln!("SKIPPED: no corpus; run scripts/fetch-corpus.sh");
            return;
        };
        let (mut repaired, mut unconvertible) = (0, 0);
        for f in files.flatten() {
            let path = f.path();
            if path.extension().is_none_or(|x| x != "dwg") {
                continue;
            }
            let Ok(mut d) = Drawing::open(&path) else {
                continue;
            };
            if d.repair().is_none() {
                continue;
            }
            let what = path.display().to_string();
            // With unconvertible objects the write is refused, as it is for
            // the copy; validation and the warnings still compare.
            assert_repair_as_copy(&d, &what);
            repaired += 1;
            let gone: Vec<Handle> = d
                .unconvertible_objects()
                .into_iter()
                .map(|(h, _)| h)
                .collect();
            if !gone.is_empty() {
                d.remove_all(&gone)
                    .unwrap_or_else(|e| panic!("{what}: removing the unconvertible objects: {e}"));
                assert!(d.unconvertible_objects().is_empty(), "{what}");
                assert_repair_as_copy(&d, &format!("{what}, unconvertible objects removed"));
                unconvertible += 1;
            }
        }
        // The corpus has 15 drawings that need a repair, 5 of them with
        // objects the writer cannot convert.
        assert!(
            repaired >= 15 && unconvertible >= 5,
            "{repaired} repaired, {unconvertible} unconvertible"
        );
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

    /// A required object the writer cannot add is a warning saying why,
    /// besides validation's error for what is missing.
    #[test]
    fn repair_that_cannot_be_made_says_why() {
        let mut d = Drawing::new();
        let nod = d.header.dict_named_objects;
        if let Some(Object::Other {
            object: NonGraphical::Dictionary(n),
            ..
        }) = d.objects.get_mut(&nod)
        {
            n.entries.retain(|(name, _)| name != "ACAD_GROUP");
        }
        let common = ObjectCommon {
            handle: Handle(u64::MAX),
            ..ObjectCommon::new()
        };
        d.objects.insert(
            Handle(u64::MAX),
            Object::Other {
                common,
                object: NonGraphical::Placeholder,
            },
        );
        d.ref_index = Default::default();
        let r = d.repair().unwrap();
        assert!(r.added.is_empty());
        let [w] = &r.warnings[..] else {
            panic!("{:?}", r.warnings);
        };
        assert!(
            w.message.contains("ACAD_GROUP") && w.message.contains("handles are used up"),
            "{w:?}"
        );
        assert!(warned(&d, "cannot add one"));
    }

    /// A drawing that lacks nothing is not copied.
    #[test]
    fn complete_drawing_needs_no_repair() {
        assert!(Drawing::new().repair().is_none());
    }
}
