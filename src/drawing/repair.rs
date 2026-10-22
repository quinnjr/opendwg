//! Required objects the writer adds when a drawing lacks them.
//!
//! The writer never refuses what it can repair. Files written by other
//! producers lack objects AutoCAD requires, which the writer can make from
//! the drawing's own values:
//!
//! - the dimension style `Standard` (BricsCAD, SolidWorks, Teigha and
//!   GDAL's R2000 files of the extended corpus have none): the header's
//!   dimension variables, as [`Drawing::new`] makes it, with the text style
//!   `Standard` (or `$TEXTSTYLE`);
//! - the named object dictionary's `ACAD_GROUP` and `ACAD_PLOTSETTINGS`
//!   dictionaries (civilcraft's own writer leaves `ACAD_PLOTSETTINGS` out):
//!   empty, as AutoCAD creates them, and named by `$DICT_PLOTSETTINGS` /
//!   `$DICT_ACAD_GROUP` when those name no dictionary.
//!
//! The additions are made on a copy, which the writer validates and writes
//! in place of the drawing: the drawing itself is not changed. Each one is a
//! warning, from [`Drawing::validate`] and [`Drawing::write_diagnostics`]. A
//! repair that cannot be made (no DIMSTYLE table, an entry of that name that
//! is not a dictionary, handles used up) is left to validation, which
//! reports what is missing.

use super::Drawing;
use super::api::names_eq;
use super::new::dim_style;
use crate::diag::{Diagnostic, Severity};
use crate::handle::Handle;
use crate::object::{Dictionary, NonGraphical, Object, TableKind};

/// The named object dictionary entries the writer creates empty.
const EMPTY_NAMED_DICTIONARIES: [&str; 2] = ["ACAD_GROUP", "ACAD_PLOTSETTINGS"];

fn warning(message: String) -> Diagnostic {
    Diagnostic {
        severity: Severity::Warning,
        handle: None,
        message,
    }
}

impl Drawing {
    /// The drawing with the required objects it lacks added (see the module
    /// documentation), and a warning for each; `None` when it lacks none the
    /// writer can add.
    pub(super) fn repaired(&self) -> Option<(Drawing, Vec<Diagnostic>)> {
        let standard = self.lacks_standard_dim_style();
        let dictionaries: Vec<&str> = EMPTY_NAMED_DICTIONARIES
            .into_iter()
            .filter(|n| self.lacks_named_dictionary(n))
            .collect();
        if !standard && dictionaries.is_empty() {
            return None;
        }
        let mut d = self.clone();
        let mut warnings = Vec::new();
        if standard {
            let text_style = d.current_text_style().unwrap_or_default();
            let style = dim_style("Standard", &d.header, text_style);
            if let Ok(h) = d.add_dim_style(style) {
                warnings.push(warning(format!(
                    "the drawing has no dimension style \"Standard\" (AutoCAD requires it); \
                     the writer adds one (DIMSTYLE {h}) from the header's dimension variables"
                )));
            }
        }
        for name in dictionaries {
            if let Some(h) = d.add_named_dictionary(name) {
                warnings.push(warning(format!(
                    "the named object dictionary has no {name} dictionary (AutoCAD requires \
                     it); the writer adds an empty one (DICTIONARY {h})"
                )));
            }
        }
        (!warnings.is_empty()).then_some((d, warnings))
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
    /// `name`.
    fn lacks_named_dictionary(&self, name: &str) -> bool {
        match self.objects.get(&self.header.dict_named_objects) {
            Some(Object::Other {
                object: NonGraphical::Dictionary(d),
                ..
            }) => !d.entries.iter().any(|(n, _)| n == name),
            Some(Object::Other {
                object: NonGraphical::DictionaryWithDefault(d),
                ..
            }) => !d.dictionary.entries.iter().any(|(n, _)| n == name),
            _ => false,
        }
    }

    /// Add the empty dictionary `name` to the named object dictionary, its
    /// entries kept in byte order when they were, and point the header
    /// variable for it at the new dictionary when that names none.
    fn add_named_dictionary(&mut self, name: &str) -> Option<Handle> {
        let nod = self.header.dict_named_objects;
        let empty = Dictionary {
            entries: Vec::new(),
            cloning: 1,
            hard_owner: 0,
        };
        let h = self
            .add_object(nod, name, NonGraphical::Dictionary(empty))
            .ok()?;
        if let Some(Object::Other {
            object: NonGraphical::Dictionary(d),
            ..
        }) = self.objects.get_mut(&nod)
        {
            let sorted = d.entries[..d.entries.len() - 1]
                .windows(2)
                .all(|w| w[0].0 <= w[1].0);
            if sorted {
                let entry = d.entries.pop().expect("the entry just added");
                let at = d.entries.partition_point(|(n, _)| n.as_str() < name);
                d.entries.insert(at, entry);
            }
        }
        let is_dictionary = |d: &Drawing, x: Handle| {
            matches!(
                d.objects.get(&x),
                Some(Object::Other {
                    object: NonGraphical::Dictionary(_) | NonGraphical::DictionaryWithDefault(_),
                    ..
                })
            )
        };
        let current = match name {
            "ACAD_GROUP" => self.header.dict_acad_group,
            _ => self.header.dict_plotsettings,
        };
        if !is_dictionary(self, current) {
            match name {
                "ACAD_GROUP" => self.header.dict_acad_group = h,
                _ => self.header.dict_plotsettings = h,
            }
        }
        Some(h)
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
        assert!(Drawing::new().repaired().is_none());
    }
}
