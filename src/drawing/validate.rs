//! [`Drawing::validate`]: the structural checks run before every save.
//!
//! AutoCAD is stricter than the ODA File Converter about a drawing's
//! cross-object structure, so the writer refuses a drawing with any
//! [`Severity::Error`] finding ([`Error::Invalid`]); warnings do not stop it.
//!
//! The drawing checked is the one that will be written: the writer's
//! refreshed copies (block insert lists, layout viewport lists, polyface
//! mesh counts, ACIS entities in the R2013+ form, the dictionary entries and
//! reactors naming an absorbed CELLSTYLEMAP) stand in for the objects they
//! replace, and the objects it leaves out (that CELLSTYLEMAP, the R2000
//! VP_ENT_HDR table) are gone, so nothing the writer fixes is reported (see
//! `drawing::write`). The header is checked in its R2018 form.
//!
//! Errors:
//!
//! - an object stored under a handle its common data does not give (two
//!   objects would share a handle), the null handle, or a handle not below
//!   the $HANDSEED written (the writer raises the seed past every handle, so
//!   only the largest possible handle);
//! - a hard owner or hard pointer reference (codes 3 and 5) to an object that
//!   does not exist (but an XRECORD's: its items are application data, and
//!   AutoCAD keeps stale ones), and an extension dictionary that does not
//!   exist; an
//!   entity's layer and linetype are hard pointers, and must be a layer and
//!   a linetype;
//! - an owner that does not list the object (an entity in
//!   its block record's entity list, its insert's attributes or its
//!   polyline's vertices, a SEQEND, BLOCK or ENDBLK; a table record among its
//!   control's entries; a dictionary entry; an extension dictionary by its
//!   object; any other owned object among its owner's references), but
//!   when the owner is a dictionary (its entries are soft owner references,
//!   and AutoCAD's own files have objects their owner dictionary does not
//!   list) or either is a raw object (a class the library does not model,
//!   whose ownership semantics are unknown: Civil 3D's AECC styles) and the
//!   object is not an entity;
//! - an object listed as owned (a hard owner reference, an entity list, a
//!   table control's entry, a hard-owner dictionary's entry, an extension
//!   dictionary) whose owner is another object, but when either is a raw
//!   object and the listed object is not an entity (an entity's owner and
//!   the block list it is in are always checked); a null owner of a
//!   modelled object exactly one object lists the writer sets instead, see
//!   `drawing::write`;
//! - a missing required table, record or dictionary: the nine table controls,
//!   layer `0`, linetypes `ByLayer`, `ByBlock` and `Continuous`, text style
//!   and dimension style `Standard`, application `ACAD`, block records
//!   `*Model_Space` and `*Paper_Space`, the named object dictionary with
//!   `ACAD_GROUP`, `ACAD_MLINESTYLE`, `ACAD_LAYOUT`, `ACAD_PLOTSETTINGS` and
//!   `ACAD_PLOTSTYLENAME`, and layouts for model space and the header's paper
//!   space in the layout dictionary (but the dimension style `Standard` and
//!   the `ACAD_GROUP` and `ACAD_PLOTSETTINGS` dictionaries, which the writer
//!   adds when they are missing, a warning: see `drawing::repair`);
//! - a header handle variable naming an object that does not exist or is
//!   not of its kind, and a null $CLAYER, $TEXTSTYLE, $DIMSTYLE or $CELTYPE;
//! - two records of one table with the same name, compared without case
//!   (but block records named `*...`: AutoCAD writes anonymous and extra
//!   layout blocks under repeated names, `*U`, `*D`, `*T`, `*Paper_Space`,
//!   and numbers them on load; every corpus file has them; and VPORTs named
//!   `*Active`: AutoCAD writes one per tiled model space viewport);
//! - classes: a class number below 500 or used twice, an object whose
//!   class says the other kind (an item class id other than entity, 0x1F2,
//!   says object, as the reader takes it; the writer writes it as 0x1F3),
//!   and an object of a class type with no class;
//! - cross-object counts: a polyface mesh's vertex and face counts against
//!   its vertices and faces, its faces' indices against its vertices, a
//!   polygon mesh's M x N against its vertices, a multiline's line count
//!   against its style's elements; an attribute, vertex or SEQEND of the
//!   wrong kind for its owner (the writer recounts a polyface mesh's counts
//!   and sets vertex flags, so only counts too large for their field are
//!   reported);
//! - an object whose `has_ds_binary_data` does not say whether it has a
//!   record in the data storage section as written (the writer derives the
//!   flag, so this only guards that derivation; not checked when the
//!   section is kept undecoded);
//!
//! Warnings: the exceptions above (as warnings), soft owner and soft
//! pointer references (codes 2 and 4) to
//! objects that do not exist, reactors that do not exist and owners (a soft
//! pointer) that do not exist (real AutoCAD files have them all: they load
//! and save), an XRECORD 480-481 item (a hard pointer stored as text) whose
//! non-empty text is not hexadecimal (kept as text, it names nothing), a
//! group member that does not name its group among its
//! reactors, and an entity naming a group among its reactors that the
//! group does not list (AutoCAD repairs both).
//!
//! A block record's insert list is a cache the writer refreshes: its
//! entries are soft pointers, checked as such and no further.
//!
//! Objects that cannot be written as R2018 (see
//! [`Drawing::unconvertible_objects`]) are not checked: the writer refuses
//! them before validating, and their structure is the source version's.
//! References to them are checked.
//!
//! The work is linear in the objects and their references: each object's
//! references come from the reverse reference index (`drawing::refs`), the
//! writer's copies are indexed once, and "does the owner list it" is one
//! index lookup.

use std::collections::{HashMap, HashSet};

use super::Drawing;
use super::api::names_eq;
use super::refs::{RefIndex, object_refs};
use super::write::{Changes, WriteWork};
use crate::diag::{Diagnostic, Severity};
#[cfg(doc)]
use crate::error::Error;
use crate::handle::Handle;
use crate::object::objects::{handle_text_is_reference, parse_handle_text};
use crate::object::tables::{HARD_OWNER, HARD_POINTER, SOFT_OWNER};
use crate::object::{
    ClassIndex, Entity, EntityList, LinetypeRef, MInsert, NonGraphical, Object, TableControl,
    TableKind, XRecordValue, written_type_code,
};
use crate::section::HeaderVars;
use crate::section::classes::FIRST_CLASS_NUMBER;
use crate::section::header::handle_vars;

/// The tables a drawing must have (VP_ENT_HDR is R2000's alone).
const TABLES: [TableKind; 9] = [
    TableKind::Block,
    TableKind::Layer,
    TableKind::Style,
    TableKind::Linetype,
    TableKind::View,
    TableKind::Ucs,
    TableKind::Vport,
    TableKind::AppId,
    TableKind::DimStyle,
];

/// The records each drawing must have, by table, with what to call them.
const RECORDS: [(TableKind, &str, &str); 9] = [
    (TableKind::Layer, "layer", "0"),
    (TableKind::Linetype, "linetype", "ByLayer"),
    (TableKind::Linetype, "linetype", "ByBlock"),
    (TableKind::Linetype, "linetype", "Continuous"),
    (TableKind::Style, "text style", "Standard"),
    (TableKind::DimStyle, "dimension style", "Standard"),
    (TableKind::AppId, "application", "ACAD"),
    (TableKind::Block, "block record", "*Model_Space"),
    (TableKind::Block, "block record", "*Paper_Space"),
];

/// The named object dictionary's required entries.
const NAMED_OBJECTS: [&str; 5] = [
    "ACAD_GROUP",
    "ACAD_MLINESTYLE",
    "ACAD_LAYOUT",
    "ACAD_PLOTSETTINGS",
    "ACAD_PLOTSTYLENAME",
];

/// Table record names compare without case, as in AutoCAD.
fn fold(name: &str) -> String {
    name.chars().flat_map(char::to_lowercase).collect()
}

fn table_name(kind: TableKind) -> &'static str {
    match kind {
        TableKind::Block => "BLOCK_RECORD",
        TableKind::Layer => "LAYER",
        TableKind::Style => "STYLE",
        TableKind::Linetype => "LTYPE",
        TableKind::View => "VIEW",
        TableKind::Ucs => "UCS",
        TableKind::Vport => "VPORT",
        TableKind::AppId => "APPID",
        TableKind::DimStyle => "DIMSTYLE",
        TableKind::VpEntHdr => "VP_ENT_HDR",
    }
}

fn code_name(code: u8) -> &'static str {
    match code {
        HARD_OWNER => "hard owner",
        HARD_POINTER => "hard pointer",
        SOFT_OWNER => "soft owner",
        _ => "soft pointer",
    }
}

/// What kind of object a header handle variable must name, by its name in
/// [`handle_vars`]; `None` for those of no checked kind.
fn header_var_kind(name: &str) -> Option<&'static str> {
    Some(match name {
        "$CLAYER" => "LAYER",
        "$TEXTSTYLE" | "$DIMTXSTY" => "STYLE",
        "$CELTYPE" | "$DIMLTYPE" | "$DIMLTEX1" | "$DIMLTEX2" => "LTYPE",
        "LTYPE (BYLAYER)" | "LTYPE (BYBLOCK)" | "LTYPE (CONTINUOUS)" => "LTYPE",
        "$DIMSTYLE" => "DIMSTYLE",
        "$CMLSTYLE" => "MLINESTYLE",
        "$DIMLDRBLK" | "$DIMBLK" | "$DIMBLK1" | "$DIMBLK2" => "BLOCK_HEADER",
        "BLOCK_RECORD (*PAPER_SPACE)" | "BLOCK_RECORD (*MODEL_SPACE)" => "BLOCK_HEADER",
        "$UCSNAME" | "$PUCSNAME" => "UCS",
        n if n.starts_with("DICTIONARY (") => "DICTIONARY",
        n if n.ends_with(" CONTROL OBJECT") => {
            let table = n.trim_end_matches(" CONTROL OBJECT");
            return TABLES
                .into_iter()
                .find(|&k| table_control_name(k) == table)
                .map(control_type_name);
        }
        _ => return None,
    })
}

/// The table's name in [`handle_vars`]'s control object names.
fn table_control_name(kind: TableKind) -> &'static str {
    match kind {
        TableKind::Block => "BLOCK",
        TableKind::Linetype => "LINETYPE",
        k => table_name(k),
    }
}

fn control_type_name(kind: TableKind) -> &'static str {
    match kind {
        TableKind::Block => "BLOCK_CONTROL",
        TableKind::Layer => "LAYER_CONTROL",
        TableKind::Style => "STYLE_CONTROL",
        TableKind::Linetype => "LTYPE_CONTROL",
        TableKind::View => "VIEW_CONTROL",
        TableKind::Ucs => "UCS_CONTROL",
        TableKind::Vport => "VPORT_CONTROL",
        TableKind::AppId => "APPID_CONTROL",
        TableKind::DimStyle => "DIMSTYLE_CONTROL",
        TableKind::VpEntHdr => "VP_ENT_HDR_CONTROL",
    }
}

/// Whether `obj` is of the kind `kind` names (a type name; any dictionary
/// for `DICTIONARY`).
fn is_kind(obj: &Object, kind: &str) -> bool {
    match kind {
        "DICTIONARY" => is_dictionary(obj),
        k => obj.type_name() == k,
    }
}

fn is_dictionary(obj: &Object) -> bool {
    matches!(
        obj,
        Object::Other {
            object: NonGraphical::Dictionary(_) | NonGraphical::DictionaryWithDefault(_),
            ..
        }
    )
}

/// A dictionary's entries (with or without default), and whether it owns
/// them.
pub(super) fn dictionary_entries(obj: &Object) -> Option<(&[(String, Handle)], bool)> {
    match obj {
        Object::Other {
            object: NonGraphical::Dictionary(d),
            ..
        } => Some((&d.entries, d.is_hard_owner())),
        Object::Other {
            object: NonGraphical::DictionaryWithDefault(d),
            ..
        } => Some((&d.dictionary.entries, d.dictionary.is_hard_owner())),
        _ => None,
    }
}

impl Drawing {
    /// Check the drawing's structure as it will be written (see the module
    /// documentation of `drawing::validate` for the checks). Findings are
    /// sorted by handle (those of no object first), then message, each
    /// once. [`Drawing::to_bytes`] refuses a drawing with
    /// [`Severity::Error`] findings.
    pub fn validate(&self) -> Vec<Diagnostic> {
        // Required objects the writer adds (see `drawing::repair`) are
        // warnings; the drawing is checked with them.
        let changes = self.refreshed(&mut WriteWork::default());
        let mut found = self.validate_written(&changes, &mut 0);
        if !changes.repairs.is_empty() {
            found.extend(changes.repairs.iter().cloned());
            found.sort_by(|a, b| (a.handle, &a.message).cmp(&(b.handle, &b.message)));
            found.dedup();
        }
        found
    }

    /// [`Drawing::validate`] with the writer's `changes`, counting the
    /// references and list entries examined in `work`.
    pub(super) fn validate_written(&self, changes: &Changes, work: &mut usize) -> Vec<Diagnostic> {
        let mut state = self.ref_index.lock();
        let index = state.ensure(&self.objects);
        let replaced = changes
            .replaced
            .iter()
            .map(|(&h, o)| {
                let refs = object_refs(o).0;
                let targets = refs.iter().map(|&(t, _)| t).collect();
                let reactors = o.object_common().reactors.iter().copied().collect();
                (
                    h,
                    CopyRefs {
                        refs,
                        targets,
                        reactors,
                    },
                )
            })
            .collect();
        let unconvertible: HashSet<Handle> = changes
            .handles(self)
            .filter(|&h| changes.get(self, h).is_some_and(|o| !o.is_convertible()))
            .collect();
        let mut v = Validator {
            d: self,
            header: changes.header(self),
            changes,
            index,
            replaced,
            model_space: self.model_space(),
            unconvertible: &unconvertible,
            out: Vec::new(),
            work: 0,
        };
        v.handles();
        v.classes();
        v.references();
        v.owners();
        v.entities();
        v.groups();
        v.tables();
        v.xref_blocks();
        v.named_objects();
        v.header();
        v.ds_flags();
        *work += v.work;
        let mut out = v.out;
        out.sort_by(|a, b| (a.handle, &a.message).cmp(&(b.handle, &b.message)));
        out.dedup();
        out
    }
}

/// The references of one of the writer's copies, as a list and a set of
/// targets, and its reactors as a set.
struct CopyRefs {
    refs: Vec<(Handle, u8)>,
    targets: HashSet<Handle>,
    reactors: HashSet<Handle>,
}

/// The drawing as it will be written, and the findings.
struct Validator<'a> {
    d: &'a Drawing,
    /// The header as written (see [`Changes::header`]).
    header: &'a HeaderVars,
    changes: &'a Changes,
    index: &'a RefIndex,
    /// The references of the writer's copies (the index has the objects'
    /// as they are), as a list and a set of targets.
    replaced: HashMap<Handle, CopyRefs>,
    model_space: Handle,
    /// The objects that cannot be written as R2018 (raw objects of R2000 and
    /// R2004, ...; see [`Drawing::unconvertible_objects`]): the writer
    /// refuses them before validating, and their structure is the source
    /// version's (an R2004 3DSOLID's history is owned through an XRECORD of
    /// its extension dictionary, for one), so they are not checked;
    /// references to them are.
    unconvertible: &'a HashSet<Handle>,
    out: Vec<Diagnostic>,
    work: usize,
}

impl<'a> Validator<'a> {
    fn error(&mut self, handle: Option<Handle>, message: String) {
        self.out.push(Diagnostic {
            severity: Severity::Error,
            handle,
            message,
        });
    }

    fn warning(&mut self, handle: Option<Handle>, message: String) {
        self.out.push(Diagnostic {
            severity: Severity::Warning,
            handle,
            message,
        });
    }

    /// The object `h` as written.
    fn get(&self, h: Handle) -> Option<&'a Object> {
        self.changes.get(self.d, h)
    }

    /// Every object written, by handle, but those that cannot be written
    /// as R2018 (see [`Validator::unconvertible`]).
    fn objects(&self) -> impl Iterator<Item = (Handle, &'a Object)> + 'a {
        let (d, changes, skip) = (self.d, self.changes, self.unconvertible);
        changes
            .handles(d)
            .filter(move |h| !skip.contains(h))
            .filter_map(move |h| changes.get(d, h).map(|o| (h, o)))
    }

    /// The classes: numbers from 500 and each used once, and each object of
    /// a class of the kind its class says (an item class id other than
    /// entity's says object). The reader refuses a class number below 500, drops
    /// a repeated one and decodes an object by its class's kind.
    fn classes(&mut self) {
        let mut seen = HashSet::new();
        for c in &self.d.classes {
            self.work += 1;
            let what = format!("class {} ({})", c.class_number, c.dxf_name);
            if c.class_number < FIRST_CLASS_NUMBER {
                self.error(None, format!("{what}: class number below 500"));
            }
            if !seen.insert(c.class_number) {
                self.error(None, format!("{what}: class number used twice"));
            }
        }
        let index = ClassIndex::new(&self.d.classes);
        for (h, obj) in self.objects() {
            self.work += 1;
            let code = match written_type_code(obj, &index) {
                Ok(code) => code,
                Err(e) => {
                    let message = format!("{}: {e}", self.name(h));
                    self.error(Some(h), message);
                    continue;
                }
            };
            let Some(class) = (code >= FIRST_CLASS_NUMBER)
                .then(|| index.by_number(code))
                .flatten()
            else {
                continue;
            };
            let entity = matches!(obj, Object::Entity { .. });
            if class.is_entity() != entity {
                let message = format!(
                    "{}: its class {} says {}, so it would be read back as one",
                    self.name(h),
                    class.class_number,
                    if class.is_entity() {
                        "entity"
                    } else {
                        "object"
                    }
                );
                self.error(Some(h), message);
            }
        }
    }

    /// Each object's `has_ds_binary_data` against the data storage records
    /// as written (see `drawing::write`).
    fn ds_flags(&mut self) {
        let Some(records) = &self.changes.ds_records else {
            return;
        };
        let wrong: Vec<(Handle, bool)> = self
            .objects()
            .map(|(h, o)| (h, o.object_common().has_ds_binary_data))
            .filter(|&(h, flag)| flag != records.contains(&h))
            .collect();
        for (h, flag) in wrong {
            self.work += 1;
            let message = format!(
                "{}: has_ds_binary_data is {flag}, but the object {} a data storage record",
                self.name(h),
                if flag { "has no" } else { "has" }
            );
            self.error(Some(h), message);
        }
    }

    /// `h` named by its type: `LINE 2A`.
    fn name(&self, h: Handle) -> String {
        match self.get(h) {
            Some(o) => format!("{} {h}", o.type_name()),
            None => format!("object {h}"),
        }
    }

    /// The references of `h` (see `drawing::refs::object_refs`).
    fn refs(&self, h: Handle) -> &[(Handle, u8)] {
        match self.replaced.get(&h) {
            Some(c) => &c.refs,
            None => self.index.refs_of(h),
        }
    }

    /// Whether `from` refers to `to`.
    fn refers(&self, from: Handle, to: Handle) -> bool {
        match self.replaced.get(&from) {
            Some(c) => c.targets.contains(&to),
            None => self.index.refers(from, to),
        }
    }

    /// Whether `holder` names `r` among its reactors (one lookup).
    fn has_reactor(&self, holder: Handle, r: Handle) -> bool {
        match self.replaced.get(&holder) {
            Some(c) => c.reactors.contains(&r),
            None => self.index.holds_reactor(holder, r),
        }
    }

    /// The owner of `obj`: the stored one, or for an entity that stores none
    /// the block record its entity mode names (null for mode 3).
    fn owner(&self, obj: &Object) -> Handle {
        match obj {
            Object::Entity { common, .. } => match common.entmode {
                0 => common.obj.owner,
                1 => self.header.block_record_paper_space,
                2 => self.model_space,
                _ => Handle(0),
            },
            Object::Other { common, .. } => common.owner,
        }
    }

    /// Objects stored under the handle they give, below the seed.
    fn handles(&mut self) {
        let seed = self.d.next_handseed(self.changes);
        for (h, obj) in self.objects() {
            self.work += 1;
            let own = obj.handle();
            if h.0 == 0 {
                let message = format!("{}: an object has the null handle 0", self.name(h));
                self.error(Some(h), message);
            }
            if own != h {
                let why = if self.get(own).is_some() {
                    format!(
                        "{} has that handle too, so two objects would share it",
                        self.name(own)
                    )
                } else {
                    "it would be written under that handle".to_owned()
                };
                let message = format!(
                    "{}: its common data gives the handle {own} ({why})",
                    self.name(h)
                );
                self.error(Some(h), message);
            }
            if h >= seed {
                let message = format!(
                    "{}: its handle is not below $HANDSEED ({seed}); no handle seed can be \
                     above the largest possible handle",
                    self.name(h)
                );
                self.error(Some(h), message);
            }
        }
    }

    /// References to objects that do not exist; hard owner references to
    /// objects owned by another; reactors and extension dictionaries.
    fn references(&mut self) {
        for (h, obj) in self.objects() {
            // An entity's layer and linetype are checked in `entities`.
            let (layer, linetype) = match obj {
                Object::Entity { common, .. } => (
                    Some(common.layer),
                    match common.linetype {
                        LinetypeRef::Handle(l) => Some(l),
                        _ => None,
                    },
                ),
                Object::Other { .. } => (None, None),
            };
            let refs: Vec<(Handle, u8)> = self.refs(h).to_vec();
            for (t, code) in refs {
                self.work += 1;
                if self.get(t).is_some() {
                    if code == HARD_OWNER {
                        self.owned_by(t, h, "a hard owner reference");
                    }
                    continue;
                }
                if code == HARD_POINTER && (Some(t) == layer || Some(t) == linetype) {
                    continue;
                }
                let message = format!(
                    "{}: its {} reference to {t} names an object that does not exist",
                    self.name(h),
                    code_name(code)
                );
                // An XRECORD's hard references are its items (application
                // data): AutoCAD keeps stale ones (`adsk-cab-04`).
                let xrecord = matches!(
                    obj,
                    Object::Other {
                        object: NonGraphical::XRecord(_),
                        ..
                    }
                );
                match code {
                    HARD_OWNER | HARD_POINTER if !xrecord => self.error(Some(h), message),
                    _ => self.warning(Some(h), message),
                }
            }
            let common = obj.object_common();
            for &r in &common.reactors {
                self.work += 1;
                if self.get(r).is_none() {
                    let message = format!(
                        "{}: its reactor {r} names an object that does not exist",
                        self.name(h)
                    );
                    self.warning(Some(h), message);
                }
            }
            if let Object::Other {
                object: NonGraphical::XRecord(r),
                ..
            } = obj
            {
                for (code, value) in &r.items {
                    self.work += 1;
                    if let XRecordValue::HandleText(s) = value {
                        if handle_text_is_reference(*code)
                            && !s.is_empty()
                            && parse_handle_text(s).is_none()
                        {
                            let message = format!(
                                "{}: its {code} item {s:?} is not a handle (hexadecimal text); \
                                 it is kept as text and names no object",
                                self.name(h)
                            );
                            self.warning(Some(h), message);
                        }
                    }
                }
            }
            if let Some(x) = common.xdict {
                self.work += 1;
                if self.get(x).is_none() {
                    let message = format!(
                        "{}: its extension dictionary {x} does not exist",
                        self.name(h)
                    );
                    self.error(Some(h), message);
                } else {
                    self.owned_by(x, h, "its extension dictionary");
                }
            }
        }
    }

    /// Whether `h` is a raw object (a class the library does not model):
    /// which of its references own what is not known, so its ownership
    /// findings are warnings. Civil 3D's AECC styles name, as hard owner
    /// references, styles another style owns (`camber-*`, `c3dtk-01`).
    fn unmodelled(&self, h: Handle) -> bool {
        self.get(h).is_some_and(|o| o.raw().is_some())
    }

    /// Whether an ownership finding between `child` and `owner` is only a
    /// warning: one of them is raw (see [`Self::unmodelled`]) and the child
    /// is not an entity. An entity's owner and the block list it is in are
    /// always checked (the writer repairs an entity exactly one block
    /// lists; see `Drawing::misowned_entities`), raw or not: the evidence
    /// for the relaxation is non-graphical Civil 3D styles only.
    fn lenient(&self, child: Handle, owner: Handle) -> bool {
        let entity = matches!(self.get(child), Some(Object::Entity { .. }));
        !entity && (self.unmodelled(child) || self.unmodelled(owner))
    }

    /// `child` (which exists) is listed by `owner` as `how`: its owner must
    /// be `owner` (a warning only as [`Self::lenient`] says).
    fn owned_by(&mut self, child: Handle, owner: Handle, how: &str) {
        let Some(obj) = self.get(child) else {
            return;
        };
        if self.unconvertible.contains(&child) {
            return;
        }
        let actual = self.owner(obj);
        if actual != owner {
            let message = format!(
                "{}: owned by {} ({how}), but its owner is {actual}",
                self.name(child),
                self.name(owner)
            );
            if self.lenient(child, owner) {
                self.warning(Some(child), message);
            } else {
                self.error(Some(child), message);
            }
        }
    }

    /// Every owner exists and lists what it owns; the entries of table
    /// controls, hard-owner dictionaries and MINSERT attributes (soft
    /// references) are owned by their lister.
    fn owners(&mut self) {
        for (h, obj) in self.objects() {
            let owner = self.owner(obj);
            if owner.0 != 0 {
                self.work += 1;
                match self.get(owner) {
                    None => {
                        // The owner is a soft pointer: AutoCAD's own files
                        // keep objects whose owner was erased.
                        let message = format!("{}: its owner {owner} does not exist", self.name(h));
                        self.warning(Some(h), message);
                    }
                    Some(o) => {
                        let listed = o.object_common().xdict == Some(h)
                            || self.refers(owner, h)
                            || self.index.is_incomplete(owner);
                        if !listed {
                            let message = format!(
                                "{}: its owner {} does not list it",
                                self.name(h),
                                self.name(owner)
                            );
                            // A dictionary's entries are soft owner
                            // references: AutoCAD's own files have objects
                            // their owner dictionary does not list
                            // (`adsk-py-01`, `asp-05`, `asp-10`). A raw
                            // object's ownership is not modelled.
                            if is_dictionary(o) || self.lenient(h, owner) {
                                self.warning(Some(h), message);
                            } else {
                                self.error(Some(h), message);
                            }
                        }
                    }
                }
            }
            let (listed, how): (Vec<Handle>, &str) = match obj {
                Object::Other {
                    object: NonGraphical::TableControl(c),
                    ..
                } => (c.entries.clone(), "a table entry"),
                Object::Entity {
                    entity: Entity::MInsert(MInsert { insert, .. }),
                    ..
                } => match &insert.attribs {
                    EntityList::Owned(v) => (v.clone(), "an attribute"),
                    EntityList::Linked { .. } => (Vec::new(), ""),
                },
                o => match dictionary_entries(o) {
                    Some((entries, true)) => (
                        entries.iter().map(|&(_, e)| e).collect(),
                        "a hard-owner dictionary entry",
                    ),
                    _ => (Vec::new(), ""),
                },
            };
            for e in listed {
                self.work += 1;
                if e.0 != 0 {
                    self.owned_by(e, h, how);
                }
            }
        }
    }

    /// Per entity: layer and linetype, children of the right kind, mesh and
    /// multiline counts, vertex flags.
    fn entities(&mut self) {
        for (h, obj) in self.objects() {
            let Object::Entity { common, entity } = obj else {
                continue;
            };
            self.work += 1;
            let name = self.name(h);
            match self.get(common.layer) {
                None => self.error(
                    Some(h),
                    format!("{name}: its layer {} does not exist", common.layer),
                ),
                Some(l)
                    if !matches!(
                        l,
                        Object::Other {
                            object: NonGraphical::Layer(_),
                            ..
                        }
                    ) =>
                {
                    let message = format!(
                        "{name}: its layer {} is not a layer",
                        self.name(common.layer)
                    );
                    self.error(Some(h), message);
                }
                Some(_) => {}
            }
            if let LinetypeRef::Handle(lt) = common.linetype {
                match self.get(lt) {
                    None => {
                        self.error(Some(h), format!("{name}: its linetype {lt} does not exist"))
                    }
                    Some(l)
                        if !matches!(
                            l,
                            Object::Other {
                                object: NonGraphical::Linetype(_),
                                ..
                            }
                        ) =>
                    {
                        let message =
                            format!("{name}: its linetype {} is not a linetype", self.name(lt));
                        self.error(Some(h), message);
                    }
                    Some(_) => {}
                }
            }
            self.children(h, entity);
            if let Entity::MLine(m) = entity {
                if let Some(Object::Other {
                    object: NonGraphical::MLineStyle(s),
                    ..
                }) = self.get(m.style)
                {
                    if usize::from(m.lines_in_style) != s.elements.len() {
                        let message = format!(
                            "{name}: it has {} lines, but its style {} has {} elements",
                            m.lines_in_style,
                            self.name(m.style),
                            s.elements.len()
                        );
                        self.error(Some(h), message);
                    }
                }
            }
        }
    }

    /// An insert's attributes and a polyline's vertices are of its kind,
    /// SEQENDs are SEQENDs, and mesh counts and face indices match.
    fn children(&mut self, h: Handle, entity: &Entity) {
        let name = self.name(h);
        let (list, seqend) = match entity {
            Entity::Insert(i) | Entity::MInsert(MInsert { insert: i, .. }) => {
                (&i.attribs, i.seqend)
            }
            e => match (e.polyline_vertices(), e.polyline_seqend()) {
                (Some(list), seqend) => (list, seqend.filter(|s| s.0 != 0)),
                _ => return,
            },
        };
        let EntityList::Owned(list) = list else {
            return;
        };
        let fits = |child: &Entity| match entity {
            Entity::Insert(_) | Entity::MInsert(_) => matches!(child, Entity::Attrib(_)),
            Entity::Polyline2d(_) => matches!(child, Entity::Vertex2d(_)),
            Entity::Polyline3d(_) => matches!(child, Entity::Vertex3d(_)),
            Entity::PolygonMesh(_) => matches!(child, Entity::VertexMesh(_)),
            Entity::PolyfaceMesh(_) => {
                matches!(child, Entity::VertexPface(_) | Entity::VertexPfaceFace(_))
            }
            _ => false,
        };
        let (mut vertices, mut faces) = (0usize, Vec::new());
        for &c in list {
            self.work += 1;
            let Some(Object::Entity { entity: child, .. }) = self.get(c) else {
                if self.get(c).is_some() {
                    let message = format!("{name}: its child {} is not an entity", self.name(c));
                    self.error(Some(h), message);
                }
                continue;
            };
            if !fits(child) {
                let message = format!(
                    "{name}: {} cannot be one of its attributes or vertices",
                    self.name(c)
                );
                self.error(Some(h), message);
            }
            match child {
                Entity::VertexPface(_) => vertices += 1,
                Entity::VertexPfaceFace(f) => faces.push((c, f.indices)),
                _ => {}
            }
        }
        if let Some(s) = seqend {
            if let Some(o) = self.get(s) {
                if !matches!(
                    o,
                    Object::Entity {
                        entity: Entity::SeqEnd,
                        ..
                    }
                ) {
                    let message = format!("{name}: its SEQEND {} is not a SEQEND", self.name(s));
                    self.error(Some(h), message);
                }
            }
        }
        match entity {
            Entity::PolyfaceMesh(_) => {
                // The writer recounts the counts; only a count too large
                // for its field is left.
                let max = i16::MAX as usize;
                if vertices > max || faces.len() > max {
                    self.error(
                        Some(h),
                        format!(
                            "{name}: it has {vertices} vertices and {} faces, more than a \
                             polyface mesh's counts can hold ({max} each)",
                            faces.len()
                        ),
                    );
                }
                for (f, indices) in faces {
                    if let Some(&i) = indices
                        .iter()
                        .find(|&&i| usize::from(i.unsigned_abs()) > vertices)
                    {
                        let message = format!(
                            "{}: it uses vertex {i}, but its polyface mesh {name} has {vertices}",
                            self.name(f)
                        );
                        self.error(Some(f), message);
                    }
                }
            }
            Entity::PolygonMesh(p) => {
                let size = i64::from(p.m_count) * i64::from(p.n_count);
                if size != list.len() as i64 {
                    self.error(
                        Some(h),
                        format!(
                            "{name}: it is {} x {} but has {} vertices",
                            p.m_count,
                            p.n_count,
                            list.len()
                        ),
                    );
                }
            }
            _ => {}
        }
    }

    /// GROUP members name the group among their reactors, and entities
    /// naming a group among their reactors are its members (warnings: both
    /// are reactor links, which AutoCAD repairs).
    fn groups(&mut self) {
        for (g, obj) in self.objects() {
            let Object::Other {
                object: NonGraphical::Group(group),
                ..
            } = obj
            else {
                continue;
            };
            let gname = self.name(g);
            let members: HashSet<Handle> = group.entities.iter().copied().collect();
            for &e in &group.entities {
                self.work += 1;
                if self.get(e).is_some() && !self.has_reactor(e, g) {
                    let message = format!(
                        "{}: it is a member of {gname} but does not name the group among its \
                         reactors",
                        self.name(e)
                    );
                    self.warning(Some(e), message);
                }
            }
            let holders: Vec<Handle> = self.index.reactor_holders(g).collect();
            for e in holders {
                self.work += 1;
                let names_it =
                    matches!(self.get(e), Some(Object::Entity { .. })) && self.has_reactor(e, g);
                if names_it && !members.contains(&e) {
                    let message = format!(
                        "{}: it names {gname} among its reactors, but the group does not list it",
                        self.name(e)
                    );
                    self.warning(Some(e), message);
                }
            }
        }
    }

    /// The table control of `kind` named by the header, if it is one.
    fn control(&self, kind: TableKind) -> Option<(Handle, &'a TableControl)> {
        let h = self.header;
        let control = match kind {
            TableKind::Block => h.block_control,
            TableKind::Layer => h.layer_control,
            TableKind::Style => h.style_control,
            TableKind::Linetype => h.linetype_control,
            TableKind::View => h.view_control,
            TableKind::Ucs => h.ucs_control,
            TableKind::Vport => h.vport_control,
            TableKind::AppId => h.appid_control,
            TableKind::DimStyle => h.dimstyle_control,
            TableKind::VpEntHdr => h.vpentity_control.unwrap_or_default(),
        };
        match self.get(control) {
            Some(Object::Other {
                object: NonGraphical::TableControl(c),
                ..
            }) if c.kind == kind => Some((control, c)),
            _ => None,
        }
    }

    /// An xref or overlay block record owns no entity: they are in the
    /// referenced file, and R2018 stores no entity list for it.
    fn xref_blocks(&mut self) {
        for (h, obj) in self.objects() {
            if let Object::Other {
                object: NonGraphical::BlockRecord(b),
                ..
            } = obj
            {
                self.work += 1;
                let owns = matches!(&b.entities, EntityList::Owned(v) if !v.is_empty());
                if (b.is_xref || b.xref_overlaid) && owns {
                    let message = format!(
                        "{}: an xref or overlay block cannot own entities (they are in the \
                         referenced file)",
                        self.name(h)
                    );
                    self.error(Some(h), message);
                }
            }
        }
    }

    /// The nine tables, their required records, and unique record names.
    fn tables(&mut self) {
        let mut names: HashMap<TableKind, HashMap<String, Vec<Handle>>> = HashMap::new();
        for kind in TABLES {
            let Some((_, c)) = self.control(kind) else {
                self.error(
                    None,
                    format!(
                        "the drawing has no {} table: the header's {} CONTROL OBJECT is not a {}",
                        table_name(kind),
                        table_control_name(kind),
                        control_type_name(kind)
                    ),
                );
                continue;
            };
            let extra: &[Handle] = match kind {
                TableKind::Block | TableKind::Linetype => &c.extra,
                _ => &[],
            };
            let mut seen = HashSet::new();
            let by_name = names.entry(kind).or_default();
            for &e in extra.iter().chain(&c.entries) {
                self.work += 1;
                if e.0 == 0 || !seen.insert(e) {
                    continue;
                }
                let Some(Object::Other { object, .. }) = self.get(e) else {
                    continue;
                };
                if let Some(entry) = object.table_entry() {
                    if !entry.name.is_empty() {
                        by_name.entry(fold(&entry.name)).or_default().push(e);
                    }
                }
            }
        }
        for (kind, what, name) in RECORDS {
            if names
                .get(&kind)
                .is_some_and(|m| !m.contains_key(&fold(name)))
            {
                self.error(
                    None,
                    format!(
                        "the drawing has no {what} \"{name}\" (the {} table must have it)",
                        table_name(kind)
                    ),
                );
            }
        }
        for (kind, by_name) in names {
            for (name, records) in by_name {
                // AutoCAD writes anonymous and extra layout blocks under
                // repeated names (`*U`, `*D`, `*T`, `*Paper_Space`; every
                // corpus file) and numbers them on load.
                // AutoCAD writes one `*Active` VPORT per tiled viewport of
                // the model space configuration (`rc-02`: four).
                if records.len() < 2
                    || kind == TableKind::Block && name.starts_with('*')
                    || kind == TableKind::Vport && name == "*active"
                {
                    continue;
                }
                for (i, &r) in records.iter().enumerate() {
                    let other = records[usize::from(i == 0)];
                    let message = format!(
                        "{}: it has the same name as {} (table record names must differ, \
                         without regard to case)",
                        self.name(r),
                        self.name(other)
                    );
                    self.error(Some(r), message);
                }
            }
        }
    }

    /// The named object dictionary, its required entries, and the model
    /// and paper space layouts.
    fn named_objects(&mut self) {
        let nod = self.header.dict_named_objects;
        let Some((entries, _)) = self.get(nod).and_then(dictionary_entries) else {
            self.error(
                None,
                format!("the named object dictionary {nod} (header) is not a dictionary"),
            );
            return;
        };
        let name = self.name(nod);
        let mut layouts = None;
        for required in NAMED_OBJECTS {
            // Keys compare without case, as AutoCAD looks them up.
            let entry = entries
                .iter()
                .find(|(n, _)| names_eq(n, required))
                .map(|&(_, h)| h);
            match entry.and_then(|h| self.get(h).map(|o| (h, o))) {
                Some((h, o)) if is_dictionary(o) => {
                    if required == "ACAD_LAYOUT" {
                        layouts = Some(h);
                    }
                }
                _ => self.error(
                    Some(nod),
                    format!("{name}: the named object dictionary has no {required} dictionary"),
                ),
            }
        }
        let Some(layouts) = layouts else {
            return;
        };
        let Some((entries, _)) = self.get(layouts).and_then(dictionary_entries) else {
            return;
        };
        let blocks: HashSet<Handle> = entries
            .iter()
            .filter_map(|&(_, h)| match self.get(h) {
                Some(Object::Other {
                    object: NonGraphical::Layout(l),
                    ..
                }) => Some(l.block_record),
                _ => None,
            })
            .collect();
        self.work += entries.len();
        let lname = self.name(layouts);
        for (space, block) in [
            ("model space", self.model_space),
            ("paper space", self.header.block_record_paper_space),
        ] {
            if !blocks.contains(&block) {
                self.error(
                    Some(layouts),
                    format!("{lname}: the layout dictionary has no layout for {space} ({block})"),
                );
            }
        }
    }

    /// Header handle variables (as written in R2018) name objects of their
    /// kind.
    fn header(&mut self) {
        let header = self.header;
        for (var, h, kind) in [
            ("$CLAYER", header.clayer, "LAYER"),
            ("$TEXTSTYLE", header.textstyle, "STYLE"),
            ("$DIMSTYLE", header.dimstyle, "DIMSTYLE"),
            ("$CELTYPE", header.celtype, "LTYPE"),
        ] {
            if h.0 == 0 {
                self.error(
                    None,
                    format!("header variable {var} is null; it must name a {kind}"),
                );
            }
        }
        for (var, h) in handle_vars(&header.to_r2018()) {
            self.work += 1;
            match self.get(h) {
                None => self.error(
                    None,
                    format!("header variable {var} names {h}, which does not exist"),
                ),
                Some(o) => {
                    if let Some(kind) = header_var_kind(var) {
                        if !is_kind(o, kind) {
                            let message = format!(
                                "header variable {var} names {}, not a {kind}",
                                self.name(h)
                            );
                            self.error(None, message);
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::object::{
        AppId, BlockRecord, Dictionary, EntityCommon, Group, Line, ObjectCommon, TableEntry,
    };
    use crate::section::HeaderVars;

    fn other(h: u64, owner: u64, object: NonGraphical) -> (Handle, Object) {
        let common = ObjectCommon {
            handle: Handle(h),
            owner: Handle(owner),
            ..ObjectCommon::new()
        };
        (Handle(h), Object::Other { common, object })
    }

    fn line(h: u64, owner: u64, reactors: &[u64]) -> (Handle, Object) {
        let mut common = EntityCommon::new(Handle(0x2));
        common.obj.handle = Handle(h);
        common.obj.owner = Handle(owner);
        common.obj.reactors = reactors.iter().map(|&r| Handle(r)).collect();
        let entity = Entity::Line(Line {
            start: [0.0; 3],
            end: [1.0, 0.0, 0.0],
            thickness: 0.0,
            extrusion: [0.0, 0.0, 1.0],
        });
        (Handle(h), Object::Entity { common, entity })
    }

    fn block(h: u64, entities: Vec<Handle>) -> (Handle, Object) {
        other(
            h,
            0,
            NonGraphical::BlockRecord(BlockRecord {
                entry: TableEntry::default(),
                anonymous: false,
                has_attdefs: false,
                is_xref: false,
                xref_overlaid: false,
                xref_unloaded: false,
                base_point: [0.0; 3],
                xref_path: String::new(),
                description: String::new(),
                preview: vec![],
                insert_units: 0,
                explodable: true,
                block_scaling: 0,
                block_entity: Handle(0),
                entities: EntityList::Owned(entities),
                end_block: Handle(0),
                inserts: vec![],
                layout: Handle(0),
            }),
        )
    }

    fn dictionary(h: u64, owner: u64, hard_owner: u8, entries: &[u64]) -> (Handle, Object) {
        other(
            h,
            owner,
            NonGraphical::Dictionary(Dictionary {
                entries: entries
                    .iter()
                    .map(|&e| (format!("E{e}"), Handle(e)))
                    .collect(),
                hard_owner,
                ..Dictionary::default()
            }),
        )
    }

    /// A drawing of `objects` (not a valid one: no tables, no named object
    /// dictionary, ...).
    fn drawing(objects: impl IntoIterator<Item = (Handle, Object)>) -> Drawing {
        Drawing {
            source_version: None,
            header: HeaderVars::default(),
            classes: Vec::new(),
            objects: objects.into_iter().collect(),
            ref_index: Default::default(),
            summary: None,
            preview: None,
            app_info: None,
            data_storage: None,
            extra_sections: BTreeMap::new(),
            diagnostics: Vec::new(),
            read: Default::default(),
        }
    }

    /// The findings on `h` of severity `s`.
    fn on(diags: &[Diagnostic], h: u64, s: Severity) -> Vec<String> {
        diags
            .iter()
            .filter(|d| d.handle == Some(Handle(h)) && d.severity == s)
            .map(|d| d.message.clone())
            .collect()
    }

    /// The validator's work: a block of `n` lines, each in a group and a
    /// hard-owner dictionary (as entries), and one more line in `n` groups
    /// (each naming it, and it naming each among its reactors).
    fn work(n: u64) -> usize {
        let mut lines: Vec<Handle> = (0..n).map(|i| Handle(0x100 + i)).collect();
        let shared = 0x100 + n;
        let groups: Vec<u64> = (0..n).map(|i| 0x10_0000 + i).collect();
        let mut objects = vec![
            block(0x10, lines.clone()),
            other(
                0x11,
                0,
                NonGraphical::Group(Group {
                    entities: lines.clone(),
                    ..Group::default()
                }),
            ),
            dictionary(0x12, 0, 1, &[]),
        ];
        if let (
            _,
            Object::Other {
                object: NonGraphical::Dictionary(d),
                ..
            },
        ) = &mut objects[2]
        {
            d.entries = lines.iter().map(|h| (h.to_string(), *h)).collect();
        }
        objects.extend((0..n).map(|i| line(0x100 + i, 0x10, &[0x11])));
        objects.push(line(shared, 0x10, &groups));
        objects.extend(groups.iter().map(|&g| {
            other(
                g,
                0,
                NonGraphical::Group(Group {
                    entities: vec![Handle(shared)],
                    ..Group::default()
                }),
            )
        }));
        lines.push(Handle(shared));
        if let (
            _,
            Object::Other {
                object: NonGraphical::BlockRecord(b),
                ..
            },
        ) = &mut objects[0]
        {
            b.entities = EntityList::Owned(lines);
        }
        let d = drawing(objects);
        let mut work = 0;
        let changes = d.refreshed(&mut WriteWork::default());
        let diags = d.validate_written(&changes, &mut work);
        assert!(
            !diags.iter().any(|x| x.message.contains("GROUP")),
            "{:?}",
            &diags[..3]
        );
        // Each line is owned by the block, not by the dictionary listing it.
        assert_eq!(
            diags
                .iter()
                .filter(|x| x.message.contains("hard-owner dictionary entry"))
                .count() as u64,
            n
        );
        work
    }

    /// `has_ds_binary_data` follows the data storage records: the writer
    /// derives it, so a drawing whose flags disagree validates clean, and
    /// the guard reports a set of written objects that still disagrees.
    #[test]
    fn ds_flag_follows_the_records() {
        use crate::section::DataStorage;
        let (h, mut obj) = line(0x30, 0, &[]);
        if let Object::Entity { common, .. } = &mut obj {
            common.obj.has_ds_binary_data = true;
        }
        let mut d = drawing([(h, obj.clone()), other(0x31, 0, NonGraphical::Placeholder)]);
        d.source_version = Some(crate::version::Version::R2018);
        let mut storage = DataStorage::with_default_schemas();
        storage.set_acis_data(Handle(0x31), vec![1]);
        d.data_storage = Some(storage);
        let flagged = |d: &Drawing| -> Vec<Diagnostic> {
            d.validate()
                .into_iter()
                .filter(|x| x.message.contains("has_ds_binary_data"))
                .collect()
        };
        assert!(flagged(&d).is_empty(), "{:?}", flagged(&d));
        let changes = d.refreshed(&mut WriteWork::default());
        let flag = |h: u64| {
            changes
                .get(&d, Handle(h))
                .unwrap()
                .object_common()
                .has_ds_binary_data
        };
        assert!(!flag(0x30));
        assert!(flag(0x31));
        // The guard itself: written copies that disagree are errors.
        let mut stale = d.refreshed(&mut WriteWork::default());
        stale.replaced.insert(h, obj);
        let found: Vec<_> = d
            .validate_written(&stale, &mut 0)
            .into_iter()
            .filter(|x| x.message.contains("has_ds_binary_data"))
            .collect();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].handle, Some(h));
        assert_eq!(found[0].severity, Severity::Error);
    }

    /// The work is linear in the objects and their references.
    #[test]
    fn validation_work_is_linear() {
        let small = work(1_000);
        let large = work(10_000);
        assert!(small >= 5_000, "{small}");
        assert!(large <= 10 * small + 100, "{small} {large}");
    }

    /// Handles: the null handle, one a common data does not give, and the
    /// largest possible handle (no $HANDSEED can be above it).
    #[test]
    fn handle_checks() {
        let (_, mut moved) = line(0x30, 0, &[]);
        if let Object::Entity { common, .. } = &mut moved {
            common.obj.handle = Handle(0x31);
        }
        let d = drawing([
            other(0, 0, NonGraphical::Placeholder),
            (Handle(0x30), moved),
            other(u64::MAX, 0, NonGraphical::Placeholder),
        ]);
        let diags = d.validate();
        assert!(
            on(&diags, 0, Severity::Error)
                .iter()
                .any(|m| m.contains("null handle"))
        );
        assert!(
            on(&diags, 0x30, Severity::Error)
                .iter()
                .any(|m| m.contains("gives the handle 31"))
        );
        assert!(
            on(&diags, u64::MAX, Severity::Error)
                .iter()
                .any(|m| m.contains("$HANDSEED"))
        );
    }

    /// Table entries and hard-owner dictionary entries are owned by their
    /// lister; a soft-owner dictionary's entries need not be. An owner that
    /// does not exist is a warning (the owner is a soft pointer); one that
    /// does not list the object an error, but a dictionary (its entries are
    /// soft owner references; AutoCAD's own files have unlisted objects).
    #[test]
    fn listed_entries_are_owned_by_their_lister() {
        let app = |h: u64, owner: u64| {
            other(
                h,
                owner,
                NonGraphical::AppId(AppId {
                    entry: TableEntry::default(),
                    unknown: 0,
                }),
            )
        };
        let d = drawing([
            other(
                0x10,
                0,
                NonGraphical::TableControl(TableControl {
                    kind: TableKind::AppId,
                    entries: vec![Handle(0x11)],
                    extra: vec![],
                }),
            ),
            app(0x11, 0x20),
            dictionary(0x20, 0, 1, &[0x21]),
            dictionary(0x21, 0x22, 0, &[]),
            dictionary(0x22, 0, 0, &[0x23]),
            dictionary(0x23, 0x24, 0, &[]),
            dictionary(0x24, 0, 0, &[0x23]),
            dictionary(0x25, 0x99, 0, &[]),
            dictionary(0x26, 0x11, 0, &[]),
        ]);
        let diags = d.validate();
        assert_eq!(
            on(&diags, 0x11, Severity::Error),
            ["APPID 11: owned by APPID_CONTROL 10 (a table entry), but its owner is 20"]
        );
        assert_eq!(
            on(&diags, 0x11, Severity::Warning),
            ["APPID 11: its owner DICTIONARY 20 does not list it"]
        );
        assert_eq!(
            on(&diags, 0x21, Severity::Error),
            [
                "DICTIONARY 21: owned by DICTIONARY 20 (a hard-owner dictionary entry), but \
                 its owner is 22"
            ]
        );
        assert_eq!(
            on(&diags, 0x21, Severity::Warning),
            ["DICTIONARY 21: its owner DICTIONARY 22 does not list it"]
        );
        // Listed by a soft-owner dictionary that is not its owner: fine.
        assert!(on(&diags, 0x23, Severity::Error).is_empty(), "{diags:#?}");
        assert!(on(&diags, 0x25, Severity::Error).is_empty());
        assert_eq!(
            on(&diags, 0x25, Severity::Warning),
            ["DICTIONARY 25: its owner 99 does not exist"]
        );
        // An owner that is not a dictionary must list what it owns.
        assert_eq!(
            on(&diags, 0x26, Severity::Error),
            ["DICTIONARY 26: its owner APPID 11 does not list it"]
        );
    }

    fn raw(h: u64, owner: u64, refs: &[(u8, u64)]) -> (Handle, Object) {
        use crate::handle::HandleRef;
        use crate::object::RawObject;
        other(
            h,
            owner,
            NonGraphical::Unknown(RawObject {
                type_code: 500,
                class_name: Some("AECC_LABEL_TEXT_STYLE".into()),
                source_version: crate::version::Version::R2018,
                data: Vec::new(),
                data_bits: 0,
                strings: None,
                handles: refs
                    .iter()
                    .map(|&(code, value)| HandleRef { code, value })
                    .collect(),
                tail_handles_raw: Vec::new(),
                tail_handle_bits: 0,
            }),
        )
    }

    /// The ownership of raw objects (classes the library does not model) is
    /// not known: Civil 3D's AECC styles name, as hard owner references,
    /// styles another style owns and does not list (`camber-01`). Both
    /// findings are warnings; a hard owner reference to nothing is still an
    /// error.
    #[test]
    fn raw_object_ownership_is_a_warning() {
        let d = drawing([
            raw(0x30, 0, &[(HARD_OWNER, 0x32)]),
            raw(0x31, 0, &[]),
            raw(0x32, 0x31, &[]),
            raw(0x33, 0, &[(HARD_OWNER, 0x99)]),
        ]);
        // The class is missing too: only the reference findings matter.
        let diags: Vec<Diagnostic> = d
            .validate()
            .into_iter()
            .filter(|x| !x.message.contains("no class"))
            .collect();
        assert!(on(&diags, 0x32, Severity::Error).is_empty(), "{diags:#?}");
        assert_eq!(on(&diags, 0x32, Severity::Warning).len(), 2, "{diags:#?}");
        assert_eq!(on(&diags, 0x33, Severity::Error).len(), 1, "{diags:#?}");
    }

    /// An XRECORD's hard references are application data: one naming
    /// nothing is a warning (AutoCAD keeps stale ones: `adsk-cab-04`).
    #[test]
    fn dangling_xrecord_item_is_a_warning() {
        use crate::object::{XRecord, XRecordValue};
        let d = drawing([other(
            0x40,
            0,
            NonGraphical::XRecord(XRecord {
                items: vec![(340, XRecordValue::ObjectId(Handle(0x99)))],
                ..XRecord::default()
            }),
        )]);
        let diags = d.validate();
        assert!(on(&diags, 0x40, Severity::Error).is_empty(), "{diags:#?}");
        assert_eq!(
            on(&diags, 0x40, Severity::Warning),
            ["XRECORD 40: its hard pointer reference to 99 names an object that does not exist"]
        );
    }

    /// AutoCAD writes one VPORT `*Active` per tiled model space viewport
    /// (`rc-02`: four); other repeated VPORT names are still errors.
    #[test]
    fn repeated_active_vports_are_allowed() {
        let named = |name: &str| {
            let mut d = Drawing::new();
            let control = d.header.vport_control;
            let (&active, vport) = d
                .objects
                .iter()
                .find(|(_, o)| o.type_name() == "VPORT")
                .unwrap();
            let mut twin = vport.clone();
            let h = Handle(d.header.handseed.0);
            d.header.handseed = Handle(h.0 + 1);
            twin.object_common_mut().handle = h;
            for v in [&mut twin, d.objects.get_mut(&active).unwrap()] {
                if let Object::Other {
                    object: NonGraphical::Vport(p),
                    ..
                } = v
                {
                    p.entry.name = name.into();
                }
            }
            d.objects.insert(h, twin);
            if let Some(Object::Other {
                object: NonGraphical::TableControl(c),
                ..
            }) = d.objects.get_mut(&control)
            {
                c.entries.push(h);
            }
            d.validate()
        };
        let diags = named("*Active");
        assert!(diags.is_empty(), "{diags:#?}");
        let diags = named("Mine");
        assert_eq!(
            diags
                .iter()
                .filter(|x| x.severity == Severity::Error && x.message.contains("same name"))
                .count(),
            2,
            "{diags:#?}"
        );
    }

    /// A group member without the group among its reactors, and an entity
    /// with the group as reactor that it does not list, are warnings.
    #[test]
    fn group_reactor_checks() {
        let d = drawing([
            block(0x10, vec![Handle(0x30), Handle(0x31)]),
            other(
                0x11,
                0,
                NonGraphical::Group(Group {
                    entities: vec![Handle(0x30)],
                    ..Group::default()
                }),
            ),
            line(0x30, 0x10, &[]),
            line(0x31, 0x10, &[0x11]),
        ]);
        // The lines' layer is missing too: only the group findings matter.
        let diags: Vec<Diagnostic> = d
            .validate()
            .into_iter()
            .filter(|x| x.message.contains("GROUP"))
            .collect();
        assert_eq!(
            on(&diags, 0x30, Severity::Warning),
            ["LINE 30: it is a member of GROUP 11 but does not name the group among its reactors"]
        );
        assert_eq!(
            on(&diags, 0x31, Severity::Warning),
            ["LINE 31: it names GROUP 11 among its reactors, but the group does not list it"]
        );
        assert!(diags.iter().all(|x| x.severity == Severity::Warning));
    }
}
