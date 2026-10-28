//! Writing a drawing as R2018 (AC1032): [`Drawing::to_bytes`],
//! [`Drawing::to_bytes_at`] and [`Drawing::save`].
//!
//! The steps, in order:
//!
//! 1. Objects that cannot be written as R2018 fail the write with
//!    [`Error::UnconvertibleObjects`] (see [`Drawing::unconvertible_objects`]).
//! 2. Required objects the drawing lacks and the writer can make (the
//!    dimension style `Standard`, the `ACAD_GROUP` and `ACAD_PLOTSETTINGS`
//!    dictionaries) are added, with a warning each (see `drawing::repair`):
//!    the new objects and the copies of the objects and header listing them
//!    are laid over the drawing, as the refreshed copies below are.
//!
//!    The caches and derived values the drawing keeps as read are refreshed
//!    on copies (the drawing itself is not changed), and objects R2018 does
//!    not have are left out (both listed below).
//! 3. [`Drawing::validate`] checks the drawing as it will be written: the
//!    copies of step 2 in place of the objects they replace, without the
//!    objects step 2 leaves out (see `drawing::validate`). Any error
//!    diagnostic fails the write with [`Error::Invalid`]. Validation comes
//!    after step 2 so that it never reports what the writer fixes.
//!
//! Step 2 in detail:
//!
//! - refreshed values:
//!   - a block record's insert list, a cache of the block references to it:
//!     INSERTs, MINSERTs and ACAD_TABLEs (their own `*T` block). In every
//!     corpus file AutoCAD lists exactly these, never a dimension (whose
//!     anonymous block it also points to) or a block used in a table cell,
//!     and keeps entries of erased inserts. While a block's references are
//!     those read, its list is kept as read; otherwise the references gone
//!     leave it and the new ones follow in handle order;
//!   - a layout's viewport list, for a layout its block names back: when
//!     the layout was moved to another block since read, that block's
//!     viewports; when the set of VIEWPORT entities in its block differs
//!     from the one read, entries owned elsewhere stay, gone viewports of
//!     the block leave, new ones follow in block order. Other layouts keep
//!     their list, without entries naming no object (a warning);
//!   - a polyface mesh's vertex and face counts, purely derived (ODS
//!     20.4.33): recounted from its vertices and faces when they disagree
//!     (a count too large for its field is left, and validation reports
//!     it). A polygon mesh's M x N cannot be derived from its vertex count
//!     (M and N cannot be told apart): it is written as it is, and
//!     validation reports a mismatch;
//!   - a block record's `has_attdefs`: whether it holds an ATTDEF, kept as
//!     read while that is as read (AutoCAD leaves it set on model space);
//!   - a vertex's flags: the bits of its kind (3D polyline vertex 32,
//!     polygon mesh vertex 64, polyface mesh vertex 64 and 128) set and the
//!     other kinds' clear, when they are not;
//!   - an ACIS entity read from R2004-R2010 takes the R2013+ form
//!     ([`crate::object::AcisEntity::to_r2013`]): its SAB moves to a data
//!     storage record keyed by its handle, and it gets a GUID derived from
//!     $FINGERPRINTGUID and its handle ([`AcisGuid::derived`]).
//!   - an object listed as owned (an extension dictionary, an entry of a
//!     hard-owner dictionary) that stores a null owner gets its lister as
//!     owner, with a warning (AutoCAD 2002's R14 round-trip data);
//!   - an entity exactly one block record lists, whose owner names another
//!     object, gets that block as owner (and entity mode), with a warning;
//!   - a VISUALSTYLE stored in a layout older than R2018's (read before
//!     R2013 without the marker of one AutoCAD saved in its older layout,
//!     which is copied): replaced by an R2018 VISUALSTYLE with AutoCAD's
//!     standard values for its name (compared without case), or
//!     2dWireframe's values under a custom name, keeping its handle and
//!     common data, with a warning each;
//!   - `has_ds_binary_data` in each object's common data, derived: set
//!     exactly on the objects with a record in the data storage section as
//!     written (a converted ACIS entity gets it, an emptied one loses it).
//!     Objects whose flag agrees are written as read; with the section kept
//!     undecoded every flag is.
//! - objects R2018 does not have, left out: a pre-R2010 CELLSTYLEMAP
//!   whose content the converted TABLESTYLE absorbed (with its dictionary
//!   entry and the reactors naming it, as AutoCAD saves R2010+), and the
//!   R2000 VP_ENT_HDR table (control and entries), which R2004+ files do not
//!   have.
//!
//! Then:
//!
//! 4. Every object is encoded ([`crate::object::encode_object`]) with the
//!    handle it was read with (handles are never renumbered).
//! 5. The header ($HANDSEED past every handle, $TDUPDATE the save time), the
//!    classes, the object map, the summary information
//!    (dates following the header), the preview (encoded for its address in
//!    the R2018 file), opendwg's application information, the data storage
//!    and the other sections (table below) are encoded, and the file is
//!    assembled by [`crate::container::write_r2018`].
//!
//! # Class instance counts
//!
//! A class's instance count (DXF 91) follows its objects: the objects whose
//! type code names it, and for a fixed type the class with its DXF name
//! (AutoCAD counts LAYOUT and ACDBPLACEHOLDER objects in those classes).
//! AutoCAD's counts also include instances not stored as objects (embedded
//! table contents, dynamic block grips, ...), so for a drawing read from
//! R2004+ the stored count moves by the change in the class's objects since
//! read; an unchanged drawing keeps AutoCAD's counts. Before R2004 no count
//! is stored, and the objects are counted. A class left without instances
//! keeps its entry, as AutoCAD keeps classes with a count of 0.
//!
//! # Object order
//!
//! AutoCAD does not write objects in handle order: in every corpus file the
//! object map is sorted by handle but the objects section follows AutoCAD's
//! own traversal (the root dictionary, block contents, tables, ...). The
//! writer keeps the order the objects were read in, whatever the source
//! version, and writes objects added since after them in handle order. The
//! object map is built from the offsets actually written.
//!
//! # Other sections
//!
//! What each section is written as, by source version ("2010+" means read
//! from R2010, R2013 or R2018); AutoCAD 2018's files in the corpus are the
//! guide; the decisions are unconfirmed against AutoCAD until the sign-off
//! pack (`tests/common/pack.rs`) is checked in it.
//!
//! | Section | Written as |
//! |---|---|
//! | AuxHeader | Always written (AutoCAD always writes it): the source's, converted to the R2018 layout (the three maintenance version fields AutoCAD widens from `RS` to `RL` in R2018; ODS puts the extra bytes at the end, the corpus shows them there), with version 33 (AC1032), $TDCREATE, $TDUPDATE and $HANDSEED updated. Without one, or with an unknown layout (a warning), the ODS 27 fields with one save (see `aux_default`). |
//! | ObjFreeSpace | Always written. 2010+: copied (the 89-byte form), with $TDUPDATE updated and the object count raised to the objects written when that is larger. Before: regenerated in that form (ODS 21 with 64-bit values, as the corpus has them). The count is "approximate" (ODS) and AutoCAD's exceeds the objects stored (949 for 842 in the R2018 sample); a copied count left above the objects after removals is not lowered. Unconfirmed against AutoCAD: whether it accepts both. |
//! | Template | Always written (ODS: mandatory from R2004). R2007+: copied (UTF-16 description). Before: converted (description re-encoded). Without one: an empty description. $MEASUREMENT is the header's when set, else the source's, else 0. A description too long for its 16-bit length is left empty, with a warning. |
//! | RevHistory | Always written: copied (the same 16 bytes in every R2004+ corpus file), else ODS's R21 form (0, 0, 1, 0). |
//! | AppInfoHistory | Copied (the same form from R2004 to R2018 in the corpus); its content is unknown, so without one none is written. |
//! | AcDsPrototype | Decoded as [`Drawing::data_storage`] (R2013+; the same format in R2013 and R2018). Written as read while unchanged; otherwise encoded ([`DataStorage::encode`], a fresh layout, revision raised), with the records of ACIS entities converted from older versions added and the ACIS records of objects no longer written dropped. A drawing without one gets ODS's default content when it has converted ACIS entities, else none. An R2013+-form ACIS entity without its record fails the write with [`Error::Invalid`], unless it holds no data (converted from no data, or without a record when read); an entity set to no data loses its record. One that could not be decoded (R2013+) is copied unchecked (with a warning once ACIS entities read were removed), and then converted ACIS entities fail the write with [`Error::Invalid`]. A pre-R2013 source's own: left out with a warning (its form is unknown). |
//! | VBAProject | Copied (the R2004+ section format does not vary). |
//! | FileDepList, Security, unnamed others | R2018: copied (with their section metadata). Before: left out with a warning (their form may differ). |
//! | Signature | Left out with a warning: a signature cannot survive rewriting the file (AutoCAD drops it on save too). |
//! | SummaryInfo | Encoded from [`Drawing::summary`]; without one (R2000, or one that could not be decoded, a warning), an empty one (AutoCAD always writes it). |
//! | Preview | Encoded from [`Drawing::preview`]; without one, an empty preview (no images). One kept undecoded is relocated to its new address when its entry table can be read, else left out with a warning: its entries hold absolute addresses, never written stale. |
//! | AppInfo | Always [`AppInfo::opendwg`]. |
//!
//! The warnings are listed by [`Drawing::write_diagnostics`].
//!
//! # Determinism
//!
//! [`Drawing::to_bytes_at`] takes the save time ($TDUPDATE) and gives the
//! same bytes for the same drawing and time. [`Drawing::to_bytes`] and
//! [`Drawing::save`] use the system clock (UTC: the crate has no time zone
//! data).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::Write as _;
use std::path::Path;

use super::{Drawing, ObjectSet, own_layout_viewports};
use crate::container::{
    R2004_PLAIN_HEADER_LEN, RawSections, SectionMeta, SectionName, r2018_preview_address,
    write_r2018,
};
use crate::diag::{Diagnostic, Severity};
use crate::error::{Error, Result};
use crate::handle::Handle;
use crate::object::{
    AcisGuid, AcisPayload, ClassIndex, Entity, EntityList, MInsert, NonGraphical, Object,
    TableKind, encode_object_in, written_type_code,
};
use crate::section::acds::ACIS_SCHEMA;
use crate::section::objmap;
use crate::section::{
    AppInfo, DataStorage, DxfClass, HeaderVars, JulianDate, Preview, classes, header,
};
use crate::strings;
use crate::version::Version;

/// What the writer keeps from the file a drawing was read from.
#[derive(Debug, Clone, Default)]
pub(crate) struct ReadState {
    /// The plain file header bytes (R2004 family).
    pub file_header: Option<[u8; R2004_PLAIN_HEADER_LEN]>,
    /// Section map metadata (R2004 family).
    pub section_meta: BTreeMap<SectionName, SectionMeta>,
    /// The file address of the preview section.
    pub preview_address: Option<u64>,
    /// The objects in the order of the objects section.
    pub order: Vec<Handle>,
    /// The block viewports of each layout its block names back, when read
    /// (see [`own_layout_viewports`]).
    pub layout_viewports: HashMap<Handle, Vec<Handle>>,
    /// Each layout's block record when read.
    pub layout_blocks: HashMap<Handle, Handle>,
    /// Each block record's block references when read (see
    /// [`block_references`]).
    pub block_users: HashMap<Handle, Vec<Handle>>,
    /// The objects of each class (by class number) when read (see
    /// [`class_counts`]), when the source stores instance counts (R2004+).
    pub class_counts: Option<HashMap<u16, u32>>,
    /// The data storage section read: a fingerprint of its decoded value
    /// ([`DataStorage::fingerprint`]) and its bytes.
    pub data_storage: Option<(u64, Vec<u8>)>,
    /// The ACIS entities when read, when the drawing's (R2013+) data
    /// storage section could not be decoded and is kept as read.
    pub undecoded_storage_acis: Vec<Handle>,
    /// The R2013+-form ACIS entities that had no data storage record when
    /// read (R2013+ sources): entities without data, accepted as such.
    pub acis_without_record: HashSet<Handle>,
    /// Each block record when read: whether it held an ATTDEF, and its
    /// `has_attdefs` as stored (see [`Drawing::refresh_attdef_flags`]).
    pub attdefs: HashMap<Handle, (bool, bool)>,
}

impl ReadState {
    /// Record the object order (`map` entries by location, the objects read)
    /// and the lists the writer compares with at save.
    pub(crate) fn capture(
        &mut self,
        map: &[(Handle, u64)],
        objects: &BTreeMap<Handle, Object>,
        classes: &[DxfClass],
        version: Version,
    ) {
        self.block_users = block_references(objects);
        self.attdefs = objects
            .iter()
            .filter_map(|(&h, o)| match o {
                Object::Other {
                    object: NonGraphical::BlockRecord(b),
                    ..
                } => Some((h, (holds_attdef(objects, &b.entities), b.has_attdefs))),
                _ => None,
            })
            .collect();
        let classes = ClassIndex::new(classes);
        self.class_counts = (version >= Version::R2004).then(|| class_counts(objects, &classes));
        self.layout_viewports = own_layout_viewports(objects).0.into_iter().collect();
        self.layout_blocks = objects
            .iter()
            .filter_map(|(&h, o)| match o {
                Object::Other {
                    object: NonGraphical::Layout(l),
                    ..
                } => Some((h, l.block_record)),
                _ => None,
            })
            .collect();
        let mut by_location: Vec<(u64, Handle)> = map
            .iter()
            .filter(|(h, _)| objects.contains_key(h))
            .map(|&(h, l)| (l, h))
            .collect();
        by_location.sort_unstable();
        let mut seen = HashSet::new();
        self.order = by_location
            .into_iter()
            .map(|(_, h)| h)
            .filter(|&h| seen.insert(h))
            .collect();
    }
}

/// The block record `obj` is a block reference to: an INSERT's or MINSERT's
/// block, or an ACAD_TABLE's own (anonymous `*T`) block, its first
/// type-specific reference. AutoCAD lists these in a block record's insert
/// list; dimensions and the blocks of table cells are not listed (every
/// corpus file).
fn block_reference(obj: &Object) -> Option<Handle> {
    match obj {
        Object::Entity {
            entity: Entity::Insert(i) | Entity::MInsert(MInsert { insert: i, .. }),
            ..
        } => Some(i.block),
        Object::Entity {
            common,
            entity: Entity::Unknown(r),
        } if r.class_name.as_deref() == Some("ACAD_TABLE") => {
            r.handles.first()?.resolve(common.obj.handle)
        }
        _ => None,
    }
}

/// The block references of every block record, in handle order.
fn block_references(objects: &impl ObjectSet) -> HashMap<Handle, Vec<Handle>> {
    let mut users: HashMap<Handle, Vec<Handle>> = HashMap::new();
    for (h, obj) in objects.each() {
        if let Some(b) = block_reference(obj) {
            users.entry(b).or_default().push(h);
        }
    }
    users
}

/// For a pre-R2010 table style converted to the R2010 form, the
/// CELLSTYLEMAP its extension dictionary `xdict` lists under
/// `ACAD_ROUNDTRIP_2008_TABLESTYLE_CELLSTYLEMAP`, as (dictionary, map):
/// its content is the converted style's cell styles. Only a raw
/// CELLSTYLEMAP read before R2010 without an extension dictionary of its
/// own qualifies.
fn absorbed_cell_style_map(
    objects: &impl ObjectSet,
    xdict: Option<Handle>,
    style: &crate::object::TableStyle,
) -> Option<(Handle, Handle)> {
    use crate::object::TableStyle;
    if !matches!(style, TableStyle::Legacy(_)) || !style.is_convertible() {
        return None;
    }
    let dict = xdict?;
    let Some(Object::Other {
        object: NonGraphical::Dictionary(d),
        ..
    }) = objects.object(dict)
    else {
        return None;
    };
    let &(_, map) = d
        .entries
        .iter()
        .find(|(name, _)| name == "ACAD_ROUNDTRIP_2008_TABLESTYLE_CELLSTYLEMAP")?;
    let obj = objects.object(map)?;
    let raw = obj.raw()?;
    (raw.class_name.as_deref() == Some("CELLSTYLEMAP")
        && raw.source_version < Version::R2010
        && obj.object_common().xdict.is_none()
        && obj.object_common().owner == dict)
        .then_some((dict, map))
}

/// The class (by number) an object counts toward: the class its type code
/// names, or for a fixed type the class with its DXF name (AutoCAD counts
/// its LAYOUT and ACDBPLACEHOLDER objects, stored with fixed codes, in those
/// classes: the corpus's counts match).
fn class_of(obj: &Object, classes: &ClassIndex) -> Option<u16> {
    match written_type_code(obj, classes) {
        Ok(code) if code >= classes::FIRST_CLASS_NUMBER => Some(code),
        _ => classes.by_name(&obj.type_name()).map(|c| c.class_number),
    }
}

/// The number of objects of each class (see [`class_of`]).
pub(super) fn class_counts(
    objects: &BTreeMap<Handle, Object>,
    classes: &ClassIndex,
) -> HashMap<u16, u32> {
    let mut counts = HashMap::new();
    for obj in objects.values() {
        if let Some(c) = class_of(obj, classes) {
            *counts.entry(c).or_default() += 1;
        }
    }
    counts
}

fn warning(handle: Option<Handle>, message: impl Into<String>) -> Diagnostic {
    Diagnostic {
        severity: Severity::Warning,
        handle,
        message: message.into(),
    }
}

fn error(handle: Option<Handle>, message: impl Into<String>) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        handle,
        message: message.into(),
    }
}

/// The Julian day number of 1970-01-01.
const UNIX_EPOCH_JULIAN_DAY: i64 = 2_440_588;

/// The current time (UTC) as a Julian date.
pub(crate) fn julian_now() -> JulianDate {
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = since.as_secs();
    let day = UNIX_EPOCH_JULIAN_DAY.saturating_add((secs / 86_400) as i64);
    JulianDate {
        day: i32::try_from(day).unwrap_or(i32::MAX),
        ms: ((secs % 86_400) * 1000 + u64::from(since.subsec_millis())) as i32,
    }
}

/// The work of one write, for work-count tests: objects encoded, class
/// lookups, and block entity list entries walked for layout viewports.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct WriteWork {
    encoded: usize,
    class_probes: usize,
    viewport_steps: usize,
}

/// What [`Drawing::sections`] lays out.
struct Sections {
    /// The header as written.
    header: HeaderVars,
    /// The summary information.
    summary: Vec<u8>,
    /// The preview address.
    preview_address: u64,
    /// The sections other than the objects, the object map, the header,
    /// the classes and the data storage.
    sections: BTreeMap<SectionName, Vec<u8>>,
    /// The warnings about sections left out.
    warnings: Vec<Diagnostic>,
}

/// What [`Drawing::drops`] finds.
#[derive(Debug, Default)]
struct Drops {
    /// Objects left out.
    dropped: BTreeSet<Handle>,
    /// (dictionary, CELLSTYLEMAP) for each absorbed CELLSTYLEMAP.
    owners: Vec<(Handle, Handle)>,
    /// VP_ENT_HDR table members that cannot be left out.
    blocked: BTreeSet<Handle>,
}

/// Objects the writer replaces, adds or leaves out.
#[derive(Debug, Default)]
pub(super) struct Changes {
    /// The objects as written where they are not the drawing's: refreshed
    /// copies of the drawing's, and the objects the writer adds, by handle.
    pub(super) overlay: BTreeMap<Handle, Object>,
    /// Objects not written.
    pub(super) dropped: BTreeSet<Handle>,
    /// The SAB data of ACIS entities converted from older versions, for
    /// their AcDs records.
    acis_data: Vec<(Handle, Vec<u8>)>,
    /// ACIS entities converted without data: they have no record.
    acis_empty: HashSet<Handle>,
    /// The handles with a record in the data storage section as written,
    /// from which `has_ds_binary_data` is derived; `None` when the section
    /// is kept undecoded (the flags are then kept as they are).
    pub(super) ds_records: Option<HashSet<Handle>>,
    /// Warnings about what refreshing changed (see
    /// [`Drawing::write_diagnostics`]).
    warnings: Vec<Diagnostic>,
    /// The objects the writer adds (see `drawing::repair`), whose objects
    /// are in `overlay`: their handles, ascending, above every object's.
    pub(super) added: Vec<Handle>,
    /// The header as written, when the additions change it.
    header: Option<Box<HeaderVars>>,
    /// A warning per addition (see [`Drawing::validate`]).
    pub(super) repairs: Vec<Diagnostic>,
}

impl Changes {
    /// The header as written (but `$HANDSEED` and `$TDUPDATE`, which the
    /// writer sets).
    pub(super) fn header<'a>(&'a self, d: &'a Drawing) -> &'a HeaderVars {
        self.header.as_deref().unwrap_or(&d.header)
    }

    /// Every object's handle, the drawing's and then the added ones, in
    /// handle order (dropped ones included).
    pub(super) fn handles<'a>(&'a self, d: &'a Drawing) -> impl Iterator<Item = Handle> + 'a {
        d.objects.keys().copied().chain(self.added.iter().copied())
    }

    /// The objects the refresh passes start from: the drawing's as stored
    /// and the ones the writer adds (as `overlay` has them), dropped ones
    /// included. A pass reads them, then edits what it changes
    /// ([`Changes::edit`]).
    pub(super) fn objects<'a>(&'a self, d: &'a Drawing) -> Base<'a> {
        Base { d, changes: self }
    }

    /// The object as it will be written (`None` when dropped).
    pub(super) fn get<'a>(&'a self, d: &'a Drawing, h: Handle) -> Option<&'a Object> {
        if self.dropped.contains(&h) {
            return None;
        }
        self.overlay.get(&h).or_else(|| d.objects.get(&h))
    }

    /// The copy of object `h` to change, made on first use.
    fn edit(&mut self, d: &Drawing, h: Handle) -> Option<&mut Object> {
        match self.overlay.entry(h) {
            std::collections::btree_map::Entry::Occupied(e) => Some(e.into_mut()),
            std::collections::btree_map::Entry::Vacant(e) => {
                Some(e.insert(d.objects.get(&h)?.clone()))
            }
        }
    }
}

/// See [`Changes::objects`].
pub(super) struct Base<'a> {
    d: &'a Drawing,
    changes: &'a Changes,
}

impl ObjectSet for Base<'_> {
    fn object(&self, h: Handle) -> Option<&Object> {
        match self.d.objects.get(&h) {
            Some(o) => Some(o),
            None if self.changes.added.binary_search(&h).is_ok() => self.changes.overlay.get(&h),
            None => None,
        }
    }

    fn each(&self) -> impl Iterator<Item = (Handle, &Object)> {
        let added = self.changes.added.iter();
        self.d
            .objects
            .each()
            .chain(added.filter_map(|&h| Some((h, self.changes.overlay.get(&h)?))))
    }
}

impl Drawing {
    /// The objects that cannot be written as R2018, with their type names, in
    /// handle order: objects whose common data could not be read, kept as
    /// stored ([`NonGraphical::Unreadable`], listed by its class name or type
    /// code, `UNREADABLE` when it has neither), raw
    /// objects read from R2000 or R2004 (and raw objects of a type the
    /// library models read before R2018), R2000 lists whose chain could not
    /// be followed, R2000 ACIS entities' SAT data (see
    /// [`crate::object::AcisEntity`]), and the other typed values R2018 has
    /// no form for (see [`Object`]). A plot view named after no VIEW is not
    /// listed: saving clears it, with a warning ([`Drawing::write_diagnostics`]).
    /// A VISUALSTYLE in an older layout is not listed either: the writer
    /// replaces it (see the module documentation).
    /// Objects the writer leaves out (a CELLSTYLEMAP absorbed by its
    /// converted table style, the R2000 VP_ENT_HDR table) are not listed,
    /// except a VP_ENT_HDR or its control that has an extension dictionary or
    /// is another object's reactor: leaving it out would break those links.
    pub fn unconvertible_objects(&self) -> Vec<(Handle, String)> {
        let drops = self.drops(&self.objects);
        self.objects
            .iter()
            .filter(|(h, o)| {
                (!o.is_convertible()
                    && !drops.dropped.contains(h)
                    && !Self::replaced_visual_style(o))
                    || drops.blocked.contains(h)
            })
            .map(|(&h, o)| (h, o.type_name().into_owned()))
            .collect()
    }

    /// Warnings about what writing the drawing as R2018 adds (required
    /// objects the drawing lacks, see `drawing::repair`) and leaves out
    /// (layout viewport entries naming no object, sections the writer cannot
    /// carry over; see the module documentation of `drawing::write`). They
    /// do not stop the write.
    pub fn write_diagnostics(&self) -> Vec<Diagnostic> {
        // A dry run of the write: the same steps, without the checks that
        // refuse it and without encoding the objects.
        match self.write(JulianDate::default(), &mut WriteWork::default(), true) {
            Ok((_, diags)) => diags,
            Err(e) => vec![warning(None, format!("the file cannot be laid out: {e}"))],
        }
    }

    /// The drawing as an R2018 DWG file, saved now (see
    /// [`Drawing::to_bytes_at`]). Sections the write leaves out are not
    /// reported here: [`Drawing::to_bytes_with_diagnostics`] returns them
    /// with the file, and [`Drawing::write_diagnostics`] lists them.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        self.to_bytes_at(julian_now())
    }

    /// [`Drawing::to_bytes`], with the warnings about what the write leaves
    /// out ([`Drawing::write_diagnostics`]): an application can show them
    /// after saving. They never stop the write.
    pub fn to_bytes_with_diagnostics(&self) -> Result<(Vec<u8>, Vec<Diagnostic>)> {
        self.write(julian_now(), &mut WriteWork::default(), false)
    }

    /// The drawing as an R2018 DWG file whose $TDUPDATE is `saved`. The same
    /// drawing and time always give the same bytes.
    ///
    /// Fails with [`Error::UnconvertibleObjects`] when objects cannot be
    /// written as R2018 (see [`Drawing::unconvertible_objects`]), and with
    /// [`Error::Invalid`] when [`Drawing::validate`] finds errors in the
    /// drawing as it will be written or an ACIS entity in the R2013+ form
    /// has no data storage record (see the module documentation). The
    /// drawing is not changed. Sections left
    /// out (with a warning) are listed by [`Drawing::write_diagnostics`].
    pub fn to_bytes_at(&self, saved: JulianDate) -> Result<Vec<u8>> {
        self.write(saved, &mut WriteWork::default(), false)
            .map(|(bytes, _)| bytes)
    }

    /// [`Drawing::to_bytes_at`], counting its work in `work`: steps 1 to 3
    /// of the module documentation, then [`Drawing::encode_file`]; with the
    /// warnings about what the write adds and leaves out. A `dry_run` (for
    /// [`Drawing::write_diagnostics`]) refuses nothing and returns no bytes,
    /// only the warnings, which are thus those of a real write.
    fn write(
        &self,
        saved: JulianDate,
        work: &mut WriteWork,
        dry_run: bool,
    ) -> Result<(Vec<u8>, Vec<Diagnostic>)> {
        if !dry_run {
            let unconvertible = self.unconvertible_objects();
            if !unconvertible.is_empty() {
                return Err(Error::UnconvertibleObjects(unconvertible));
            }
        }
        let mut changes = self.refreshed(work);
        let mut diags = std::mem::take(&mut changes.repairs);
        diags.append(&mut changes.warnings);
        if dry_run {
            let written = self.objects.len() + changes.added.len() - changes.dropped.len();
            // A layout that fails is one more warning: those found so far
            // are kept.
            match self.sections(saved, written, &changes) {
                Ok(s) => diags.extend(s.warnings),
                Err(e) => diags.push(warning(None, format!("the file cannot be laid out: {e}"))),
            }
            return Ok((Vec::new(), diags));
        }
        let invalid: Vec<Diagnostic> = self
            .validate_written(&changes, &mut 0)
            .into_iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        if !invalid.is_empty() {
            return Err(Error::Invalid(invalid));
        }
        let (bytes, warnings) = self.encode_file(saved, work, changes)?;
        diags.extend(warnings);
        Ok((bytes, diags))
    }

    /// The layout of a file of `objects` objects but the objects, the object
    /// map, the header, the classes and the data storage (see [`Sections`]).
    fn sections(&self, saved: JulianDate, objects: usize, changes: &Changes) -> Result<Sections> {
        let mut header = changes.header(self).clone();
        header.handseed = self.next_handseed(changes);
        header.tdupdate = saved;
        let summary = self.summary.clone().unwrap_or_default();
        let summary = summary.encode_r2018(&header)?;
        let preview_address = r2018_preview_address(Some(summary.len()))?;
        let (sections, warnings) = self.other_sections(&header, objects, preview_address);
        Ok(Sections {
            header,
            summary,
            preview_address,
            sections,
            warnings,
        })
    }

    /// The file, with the objects as `changes` has them (steps 4 and 5 of the
    /// module documentation), and the warnings about sections left out.
    fn encode_file(
        &self,
        saved: JulianDate,
        work: &mut WriteWork,
        mut changes: Changes,
    ) -> Result<(Vec<u8>, Vec<Diagnostic>)> {
        // Objects: "AcDb:AcDbObjects" starts with the RL 0x0DCA (ODS 20.1;
        // every R2004+ corpus file), locations count from the section start.
        let mut objects = 0x0DCAu32.to_le_bytes().to_vec();
        let mut map = Vec::with_capacity(self.objects.len());
        let mut counts: HashMap<u16, u32> = HashMap::new();
        let index = ClassIndex::new(&self.classes);
        for h in self.write_order(&changes) {
            let Some(obj) = changes.get(self, h) else {
                continue;
            };
            work.encoded += 1;
            let bytes = encode_object_in(obj, &index).map_err(|e| match e {
                Error::Encode(m) => Error::Encode(format!("object {h}: {m}")),
                e => e,
            })?;
            map.push((h, objects.len() as u64));
            objects.extend_from_slice(&bytes);
            if let Some(c) = class_of(obj, &index) {
                *counts.entry(c).or_default() += 1;
            }
        }
        work.class_probes += index.probes.get();

        let Sections {
            header,
            summary,
            preview_address,
            mut sections,
            warnings: diags,
        } = self.sections(saved, map.len(), &changes)?;
        let mut classes = self.classes.clone();
        for c in &mut classes {
            let now = counts.get(&c.class_number).copied().unwrap_or(0);
            c.instance_count = match &self.read.class_counts {
                // AutoCAD's counts include instances not stored as objects
                // (embedded table contents, dynamic block grips, ...): the
                // stored count moves by the change in objects since read.
                Some(read) => {
                    let then = read.get(&c.class_number).copied().unwrap_or(0);
                    (i64::from(c.instance_count) + i64::from(now) - i64::from(then))
                        .clamp(0, i64::from(u32::MAX)) as u32
                }
                None => now,
            };
        }

        if let Some(acds) = self.data_storage_section(&mut changes)? {
            sections.insert(SectionName::AcDsPrototype, acds);
        }
        sections.insert(SectionName::Header, header::encode_r2018(&header)?);
        sections.insert(SectionName::Classes, classes::encode_r2018(&classes)?);
        sections.insert(SectionName::Handles, objmap::encode(&map)?);
        sections.insert(SectionName::Objects, objects);
        sections.insert(SectionName::SummaryInfo, summary);
        sections.insert(SectionName::AppInfo, AppInfo::opendwg().encode_r2018()?);
        let r2018 = self.source_version == Some(Version::R2018);
        let raw = RawSections {
            version: Version::R2018,
            // AutoCAD 2018 writes 0 (every AC1032 corpus file).
            maintenance: 0,
            codepage: header.dwgcodepage,
            preview_offset: sections
                .contains_key(&SectionName::Preview)
                .then_some(preview_address),
            sections,
            objects_base: 0,
            section_meta: if r2018 {
                self.read.section_meta.clone()
            } else {
                BTreeMap::new()
            },
            file_header: if r2018 { self.read.file_header } else { None },
            warnings: Vec::new(),
        };
        Ok((write_r2018(&raw)?, diags))
    }

    /// Write the drawing as an R2018 DWG file at `path`, saved now (see
    /// [`Drawing::to_bytes_at`]; sections left out are listed by
    /// [`Drawing::write_diagnostics`]).
    ///
    /// The file is written to a temporary file in the same directory, flushed
    /// to disk, then renamed over the target, so the target holds either its
    /// old content or the whole new file; on Unix the directory is then
    /// flushed too, so the rename survives a crash. An existing target keeps
    /// its permissions. When `path` is a symbolic link, the file it points to
    /// is replaced and the link is kept; a link whose target does not exist
    /// fails with [`Error::Io`] (nothing is written).
    ///
    /// When flushing the directory fails after the rename succeeded, the
    /// file has already been replaced by the new content, but the error is
    /// returned ([`Error::Io`]): the rename may not survive a crash.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        write_file(path.as_ref(), &self.to_bytes()?)
    }

    /// [`Drawing::save`], returning the warnings about what the write left
    /// out ([`Drawing::write_diagnostics`]).
    pub fn save_with_diagnostics(&self, path: impl AsRef<Path>) -> Result<Vec<Diagnostic>> {
        let (bytes, diagnostics) = self.to_bytes_with_diagnostics()?;
        write_file(path.as_ref(), &bytes)?;
        Ok(diagnostics)
    }

    /// $HANDSEED as written: above every handle (the added objects'
    /// included), unless the largest handle is the largest possible one
    /// (which validation refuses).
    pub(super) fn next_handseed(&self, changes: &Changes) -> Handle {
        let last = changes.handles(self).last();
        let seed = super::api::normalised_seed(changes.header(self).handseed, last);
        Handle(seed.0.max(1))
    }

    /// The objects in write order: as read, then the others (the added
    /// ones last) by handle.
    fn write_order(&self, changes: &Changes) -> Vec<Handle> {
        let added: HashSet<Handle> = changes.added.iter().copied().collect();
        let mut seen = HashSet::with_capacity(self.objects.len() + added.len());
        let mut order: Vec<Handle> = self
            .read
            .order
            .iter()
            .copied()
            .filter(|h| (self.objects.contains_key(h) || added.contains(h)) && seen.insert(*h))
            .collect();
        order.extend(changes.handles(self).filter(|h| !seen.contains(h)));
        order
    }

    /// The objects left out (step 2 of the module documentation), the
    /// dictionary listing each absorbed CELLSTYLEMAP, and the members of the
    /// VP_ENT_HDR table that cannot be left out: those with an extension
    /// dictionary or named among another object's reactors.
    fn drops(&self, objects: &impl ObjectSet) -> Drops {
        let mut drops = Drops::default();
        let mut vp_table = Vec::new();
        for (h, obj) in objects.each() {
            match obj {
                Object::Other {
                    object: NonGraphical::VpEntHdr(_),
                    ..
                } => vp_table.push(h),
                Object::Other {
                    object: NonGraphical::TableControl(c),
                    ..
                } if c.kind == TableKind::VpEntHdr => vp_table.push(h),
                Object::Other {
                    common,
                    object: NonGraphical::TableStyle(t),
                } => {
                    if let Some(entry) = absorbed_cell_style_map(objects, common.xdict, t) {
                        drops.dropped.insert(entry.1);
                        drops.owners.push(entry);
                    }
                }
                _ => {}
            }
        }
        if !vp_table.is_empty() {
            let members: HashSet<Handle> = vp_table.iter().copied().collect();
            let reactor_of_others: HashSet<Handle> = objects
                .each()
                .filter(|(h, _)| !members.contains(h))
                .flat_map(|(_, o)| o.object_common().reactors.iter().copied())
                .filter(|r| members.contains(r))
                .collect();
            for h in vp_table {
                let has_xdict = objects
                    .object(h)
                    .is_some_and(|o| o.object_common().xdict.is_some());
                if has_xdict || reactor_of_others.contains(&h) {
                    drops.blocked.insert(h);
                } else {
                    drops.dropped.insert(h);
                }
            }
        }
        drops
    }

    /// The refreshed copies of the objects whose caches or derived values
    /// changed, and the objects left out (step 2 of the module
    /// documentation).
    pub(super) fn refreshed(&self, work: &mut WriteWork) -> Changes {
        let mut changes = Changes::default();
        // The required objects the writer adds come first: every pass after
        // reads them with the drawing's (`Changes::objects`) and edits the
        // overlay's copy (`Changes::edit`).
        if let Some(repair) = self.repair() {
            changes.overlay = repair.objects;
            changes.added = repair.added;
            changes.header = Some(Box::new(repair.header));
            changes.repairs = repair.warnings;
        }
        let Drops {
            dropped, owners, ..
        } = self.drops(&changes.objects(self));
        // An absorbed CELLSTYLEMAP leaves its dictionary and every reactor
        // list naming it.
        for &(dict, map) in &owners {
            if let Some(Object::Other {
                object: NonGraphical::Dictionary(d),
                ..
            }) = changes.edit(self, dict)
            {
                d.entries.retain(|&(_, h)| h != map);
            }
        }
        if !owners.is_empty() {
            let gone: HashSet<Handle> = owners.iter().map(|&(_, m)| m).collect();
            let holders: Vec<Handle> = changes
                .objects(self)
                .each()
                .filter(|(h, o)| {
                    !gone.contains(h) && o.object_common().reactors.iter().any(|r| gone.contains(r))
                })
                .map(|(h, _)| h)
                .collect();
            for h in holders {
                if let Some(obj) = changes.edit(self, h) {
                    obj.object_common_mut()
                        .reactors
                        .retain(|r| !gone.contains(r));
                }
            }
        }
        changes.dropped = dropped;
        self.convert_acis(&mut changes);
        self.refresh_ds_flags(&mut changes);
        self.refresh_meshes(&mut changes);
        self.refresh_vertex_flags(&mut changes);
        work.viewport_steps += self.refresh_layout_viewports(&mut changes);
        self.refresh_block_inserts(&mut changes);
        self.refresh_attdef_flags(&mut changes);
        self.refresh_plot_views(&mut changes);
        self.refresh_null_owners(&mut changes);
        self.refresh_entity_owners(&mut changes);
        self.refresh_visual_styles(&mut changes);
        changes
    }

    /// Whether `obj` is a VISUALSTYLE the writer replaces (step 2): a raw one
    /// in a layout older than R2018's (read before R2013, without the marker
    /// of one AutoCAD saved in its older layout, see
    /// `object::saved_by_object_version`), which cannot be copied.
    fn replaced_visual_style(obj: &Object) -> bool {
        matches!(obj, Object::Other {
            object: NonGraphical::Unknown(r),
            ..
        } if r.class_name.as_deref() == Some("VISUALSTYLE"))
            && !obj.is_convertible()
    }

    /// Replace each VISUALSTYLE [`Drawing::replaced_visual_style`] names by
    /// an R2018 one, with a warning: AutoCAD's standard style of its name
    /// (compared without case), or `2dWireframe`'s values under its own
    /// name. Its handle and common data (owner, reactors, extension
    /// dictionary, extended data) stay, so the views, viewports and VPORTs
    /// naming it keep a valid reference. The name is the object's own text,
    /// else its owner dictionary's entry for it.
    fn refresh_visual_styles(&self, changes: &mut Changes) {
        let codepage = self.header.dwgcodepage;
        let base = changes.objects(self);
        let mut found = Vec::new();
        for (h, obj) in base.each() {
            if !Self::replaced_visual_style(obj) {
                continue;
            }
            let Object::Other {
                common,
                object: NonGraphical::Unknown(raw),
            } = obj
            else {
                continue;
            };
            let listed = || match base.object(common.owner) {
                Some(Object::Other {
                    object: NonGraphical::Dictionary(d),
                    ..
                }) => d
                    .entries
                    .iter()
                    .find(|&&(_, e)| e == h)
                    .map(|(n, _)| n.clone()),
                _ => None,
            };
            let name = raw
                .first_text(codepage)
                .filter(|n| !n.is_empty())
                .or_else(listed)
                .unwrap_or_else(|| "2dWireframe".into());
            let style = super::new::standard_visual_style(&name);
            let standard = super::new::is_standard_visual_style(&name);
            let message = format!(
                "VISUALSTYLE {h} \"{name}\" is stored in the layout of {:?}, which R2018 cannot \
                 hold; it is written as {}",
                raw.source_version,
                if standard {
                    "AutoCAD's standard style of that name"
                } else {
                    "a style with 2dWireframe's settings under its own name"
                }
            );
            found.push((h, style, message));
        }
        for (h, style, message) in found {
            // The common data as refreshed so far (a null owner set, the
            // data storage flag derived, ...), not as stored.
            if let Some(Object::Other { object, .. }) = changes.edit(self, h) {
                *object = NonGraphical::VisualStyle(Box::new(style));
            }
            changes.warnings.push(warning(Some(h), message));
        }
    }

    /// The entities exactly one block record lists whose owner (stored, or
    /// given by the entity mode) is another object, with that block and a
    /// warning. The list is what AutoCAD draws the entity in; the owner
    /// only repeats it. An entity listed by two blocks, or by none, is left
    /// to `validate()`, which refuses it.
    fn misowned_entities(&self, objects: &impl ObjectSet) -> Vec<(Handle, Handle, Diagnostic)> {
        let mut listers: HashMap<Handle, (Handle, usize)> = HashMap::new();
        for (b, obj) in objects.each() {
            if let Object::Other {
                object: NonGraphical::BlockRecord(rec),
                ..
            } = obj
            {
                if let EntityList::Owned(list) = &rec.entities {
                    for &e in list {
                        let entry = listers.entry(e).or_insert((b, 0));
                        entry.1 += 1;
                    }
                }
            }
        }
        let mut out: Vec<(Handle, Handle, Diagnostic)> = listers
            .into_iter()
            .filter(|&(e, (b, n))| {
                n == 1
                    && matches!(objects.object(e), Some(Object::Entity { .. }))
                    && self.owner_of(e) != Some(b)
            })
            .map(|(e, (b, _))| {
                let name = |h: Handle| {
                    objects
                        .object(h)
                        .map_or_else(|| h.to_string(), |o| format!("{} {h}", o.type_name()))
                };
                let owner = self.owner_of(e).unwrap_or_default();
                let message = format!(
                    "{}: its owner is {owner}, but {} lists it among its entities; it is \
                     written with that block as owner",
                    name(e),
                    name(b)
                );
                (e, b, warning(Some(e), message))
            })
            .collect();
        out.sort_by_key(|&(e, _, _)| e);
        out
    }

    /// Give the entities [`Drawing::misowned_entities`] finds the block that
    /// lists them as owner (step 2), as `add_entity` places an entity: model
    /// space and the header's paper space by entity mode, other blocks as
    /// stored owners.
    fn refresh_entity_owners(&self, changes: &mut Changes) {
        let model = self.model_space();
        let paper = self.header.block_record_paper_space;
        for (e, block, w) in self.misowned_entities(&changes.objects(self)) {
            if let Some(Object::Entity { common, .. }) = changes.edit(self, e) {
                let (entmode, owner) = if block == model {
                    (2, Handle(0))
                } else if block == paper {
                    (1, Handle(0))
                } else {
                    (0, block)
                };
                common.entmode = entmode;
                common.obj.owner = owner;
                common.obj.owner_relative = false;
                changes.warnings.push(w);
            }
        }
    }

    /// The objects listed as owned that store a null owner, with their
    /// lister and a warning: an extension dictionary, an entry of a
    /// hard-owner dictionary (with or without default). AutoCAD 2002 and
    /// 2004 wrote a DIMSTYLE's R14 round-trip extension dictionary and its
    /// XRECORD so (`arx-02`, `arx-03` of the extended corpus). An object
    /// two objects list is left to `validate()`, which refuses it: which
    /// one owns it is not known.
    fn null_owners(objects: &impl ObjectSet) -> Vec<(Handle, Handle, Diagnostic)> {
        let null = |h: Handle| matches!(objects.object(h), Some(Object::Other { common, .. }) if common.owner.0 == 0);
        let name = |h: Handle| {
            objects
                .object(h)
                .map_or_else(|| h.to_string(), |o| format!("{} {h}", o.type_name()))
        };
        // For each child, its listers and how the first lists it, in the
        // order first found.
        let mut found: Vec<(Handle, Handle, &str)> = Vec::new();
        let mut listers: HashMap<Handle, BTreeSet<Handle>> = HashMap::new();
        for (h, obj) in objects.each() {
            let mut listed = Vec::new();
            if let Some(x) = obj.object_common().xdict {
                listed.push((x, "its extension dictionary"));
            }
            if let Some(d) = obj.dictionary().filter(|d| d.is_hard_owner()) {
                listed.extend(d.entries.iter().map(|&(_, e)| (e, "an entry")));
            }
            for (child, how) in listed {
                if child != h && null(child) {
                    let by = listers.entry(child).or_default();
                    if by.is_empty() {
                        found.push((child, h, how));
                    }
                    by.insert(h);
                }
            }
        }
        found
            .into_iter()
            .filter(|(child, _, _)| listers[child].len() == 1)
            .map(|(child, h, how)| {
                let message = format!(
                    "{}: its owner is null, but {} lists it as {how}; it is written with \
                     that owner",
                    name(child),
                    name(h)
                );
                (child, h, warning(Some(child), message))
            })
            .collect()
    }

    /// Set the null owners [`Drawing::null_owners`] finds (step 2), but not
    /// to a lister the writer leaves out.
    fn refresh_null_owners(&self, changes: &mut Changes) {
        for (child, owner, w) in Self::null_owners(&changes.objects(self)) {
            if changes.dropped.contains(&owner) {
                continue;
            }
            if let Some(obj) = changes.edit(self, child) {
                obj.object_common_mut().owner = owner;
                changes.warnings.push(w);
            }
        }
    }

    /// The layouts and plot settings that name a plot view (R2000) and have
    /// no handle for it, with the handle of the VIEW of that name (compared
    /// without case), or the warning when no view has it.
    fn plot_view_names(
        objects: &impl ObjectSet,
    ) -> Vec<(Handle, std::result::Result<Handle, Diagnostic>)> {
        let mut views = None;
        let mut out = Vec::new();
        for (h, obj) in objects.each() {
            let (plot, what) = match obj {
                Object::Other {
                    object: NonGraphical::Layout(l),
                    ..
                } => (&l.plot, format!("layout \"{}\"", l.name)),
                Object::Other {
                    object: NonGraphical::PlotSettings(p),
                    ..
                } => (&**p, format!("plot settings \"{}\"", p.page_setup_name)),
                _ => continue,
            };
            if plot.plot_view.0 != 0 || plot.plot_view_name.is_empty() {
                continue;
            }
            let views = views.get_or_insert_with(|| super::views_by_name(objects));
            out.push((
                h,
                views
                    .get(&plot.plot_view_name.to_lowercase())
                    .copied()
                    .ok_or_else(|| {
                        warning(
                            Some(h),
                            format!(
                                "{what} names the plot view \"{}\", which no VIEW has; the plot view is cleared",
                                plot.plot_view_name
                            ),
                        )
                    }),
            ));
        }
        out
    }

    /// A plot view given by name (R2000) is resolved to its VIEW's handle
    /// here, so a view added or renamed since reading is found. A name no
    /// view has is cleared (a null handle), with a warning: the layout is
    /// still written, as the Model and paper layouts cannot be removed.
    fn refresh_plot_views(&self, changes: &mut Changes) {
        for (h, found) in Self::plot_view_names(&changes.objects(self)) {
            let view = match found {
                Ok(view) => view,
                Err(w) => {
                    changes.warnings.push(w);
                    Handle(0)
                }
            };
            let plot = match changes.edit(self, h) {
                Some(Object::Other {
                    object: NonGraphical::Layout(l),
                    ..
                }) => &mut l.plot,
                Some(Object::Other {
                    object: NonGraphical::PlotSettings(p),
                    ..
                }) => &mut **p,
                _ => continue,
            };
            plot.plot_view = view;
            plot.plot_view_name.clear();
        }
    }

    /// A block record's `has_attdefs` (step 2), derived from its entities:
    /// whether it holds an ATTDEF. While that is as read, the flag is
    /// written as read (AutoCAD leaves it set on model space after its
    /// ATTDEFs are gone, in every corpus sample); otherwise, and for a block
    /// added since, it is the derived value.
    fn refresh_attdef_flags(&self, changes: &mut Changes) {
        let base = changes.objects(self);
        let mut stale = Vec::new();
        for (h, obj) in base.each() {
            let Object::Other {
                object: NonGraphical::BlockRecord(b),
                ..
            } = obj
            else {
                continue;
            };
            if matches!(b.entities, EntityList::Linked { .. }) {
                continue;
            }
            let holds = holds_attdef(&base, &b.entities);
            let want = match self.read.attdefs.get(&h) {
                Some(&(held, stored)) if held == holds => stored,
                _ => holds,
            };
            if want != b.has_attdefs {
                stale.push((h, want));
            }
        }
        for (h, want) in stale {
            if let Some(Object::Other {
                object: NonGraphical::BlockRecord(b),
                ..
            }) = changes.edit(self, h)
            {
                b.has_attdefs = want;
            }
        }
    }

    /// ACIS entities read before R2013 (step 2): the R2013+ form, with a
    /// GUID derived from $FINGERPRINTGUID and the handle; their SAB goes to
    /// `changes.acis_data`.
    fn convert_acis(&self, changes: &mut Changes) {
        let mut converted = Vec::new();
        for (h, obj) in changes.objects(self).each() {
            let Object::Entity {
                entity: Entity::Acis(a),
                ..
            } = obj
            else {
                continue;
            };
            if matches!(a.payload, AcisPayload::DataStorage { .. }) {
                continue;
            }
            let guid = AcisGuid::derived(&self.header.fingerprintguid, h);
            // An entity that cannot be converted was refused before.
            if let Some(c) = a.to_r2013(guid) {
                converted.push((h, c));
            }
        }
        for (h, (converted, data)) in converted {
            if let Some(Object::Entity {
                entity: Entity::Acis(x),
                ..
            }) = changes.edit(self, h)
            {
                **x = converted;
            }
            match data {
                Some(data) => changes.acis_data.push((h, data)),
                None => {
                    changes.acis_empty.insert(h);
                }
            }
        }
    }

    /// Whether the drawing keeps an R2013+ data storage section it could not
    /// decode (copied unchecked).
    fn storage_undecoded(&self) -> bool {
        self.data_storage.is_none()
            && self.source_version >= Some(Version::R2013)
            && self
                .extra_sections
                .contains_key(&SectionName::AcDsPrototype)
    }

    /// The handles with a record in the data storage section as
    /// [`Drawing::data_storage_section`] writes it: the records read (the
    /// ACIS ones of objects written and not emptied) and those of converted
    /// entities. `None` when the section is kept undecoded.
    fn written_ds_records(&self, changes: &Changes) -> Option<HashSet<Handle>> {
        if self.storage_undecoded() {
            return None;
        }
        let mut out: HashSet<Handle> = changes.acis_data.iter().map(|&(h, _)| h).collect();
        if let Some(s) = &self.data_storage {
            let acis = s.schema_index(ACIS_SCHEMA);
            out.extend(
                s.records
                    .iter()
                    .filter(|r| {
                        Some(r.schema) != acis
                            || (changes.get(self, r.handle).is_some()
                                && !changes.acis_empty.contains(&r.handle))
                    })
                    .map(|r| r.handle),
            );
        }
        Some(out)
    }

    /// `has_ds_binary_data` (step 2), derived: set exactly on the objects
    /// with a data storage record as written (AutoCAD sets it on every R2013+
    /// object with one: ACIS entities, layouts and views with thumbnails in
    /// the corpus). So an ACIS entity converted from an older version gets
    /// it, and one emptied loses it. Objects whose flag already agrees are
    /// written as read; with the section kept undecoded every flag is.
    fn refresh_ds_flags(&self, changes: &mut Changes) {
        let Some(records) = self.written_ds_records(changes) else {
            return;
        };
        let handles: Vec<Handle> = changes.handles(self).collect();
        for h in handles {
            let want = records.contains(&h);
            let stale = changes
                .get(self, h)
                .is_some_and(|o| o.object_common().has_ds_binary_data != want);
            if stale {
                if let Some(obj) = changes.edit(self, h) {
                    obj.object_common_mut().has_ds_binary_data = want;
                }
            }
        }
        changes.ds_records = Some(records);
    }

    /// The data storage section to write (step 5): the one read, with the
    /// records of converted ACIS entities added and the ACIS records of
    /// objects not written dropped (records keyed by other objects stay);
    /// for a drawing without one, ODS's default content with the converted
    /// entities' records, or none when there are none. Unchanged, it is
    /// written as read; otherwise encoded with its revision raised, as
    /// AutoCAD does on every save.
    ///
    /// An ACIS entity emptied (its payload set to no data) loses its record.
    /// Every ACIS entity written in the R2013+ form must have its record,
    /// unless it holds no data: converted from no data in this save, or
    /// without a record when read (an R2013+ entity without data, such as
    /// one this writer converted from no data). One whose record was there
    /// when read and is gone, or one added or changed to the R2013+ form
    /// without a record, fails the write with [`Error::Invalid`], a
    /// diagnostic per entity. A data storage section that could not be
    /// decoded is copied unchecked.
    fn data_storage_section(&self, changes: &mut Changes) -> Result<Option<Vec<u8>>> {
        let undecoded = self.storage_undecoded();
        let mut storage = match &self.data_storage {
            Some(s) => Some(s.clone()),
            None if changes.acis_data.is_empty() => None,
            None if undecoded => {
                return Err(Error::Invalid(vec![error(
                    None,
                    "ACIS entities converted from an older version need records in the data \
                     storage section, which could not be decoded",
                )]));
            }
            None => Some(DataStorage::with_default_schemas()),
        };
        if let Some(storage) = &mut storage {
            storage.set_acis_data_all(std::mem::take(&mut changes.acis_data));
            // Records of objects not written leave, and those of entities
            // emptied (converted from no data in this save).
            storage.retain_acis(|h| {
                changes.get(self, h).is_some() && !changes.acis_empty.contains(&h)
            });
        }
        if !undecoded {
            let linked: HashSet<Handle> = storage.as_ref().map_or_else(HashSet::new, |s| {
                let schema = s.schema_index(ACIS_SCHEMA);
                s.records
                    .iter()
                    .filter(|r| Some(r.schema) == schema)
                    .map(|r| r.handle)
                    .collect()
            });
            let missing: Vec<Diagnostic> = changes
                .handles(self)
                .filter(|&h| {
                    matches!(
                        changes.get(self, h),
                        Some(Object::Entity { entity: Entity::Acis(a), .. })
                            if matches!(a.payload, AcisPayload::DataStorage { .. })
                    ) && !changes.acis_empty.contains(&h)
                        && !self.read.acis_without_record.contains(&h)
                        && !linked.contains(&h)
                })
                .map(|h| {
                    error(
                        Some(h),
                        "the ACIS entity's data is in the data storage section, which has no \
                         record for it",
                    )
                })
                .collect();
            if !missing.is_empty() {
                return Err(Error::Invalid(missing));
            }
        }
        let Some(mut storage) = storage else {
            return Ok(None);
        };
        if let Some((read, bytes)) = &self.read.data_storage {
            if storage.fingerprint() == *read {
                return Ok(Some(bytes.clone()));
            }
            storage.revision = storage.revision.wrapping_add(1);
        }
        storage.encode().map(Some)
    }

    /// Polyface mesh counts (step 2): purely derived (ODS 20.4.33), so
    /// recounted whenever they disagree with the mesh's vertices and faces
    /// (consistent counts are kept, and the mesh written as read). Counts
    /// too large for their field are left for validation to report.
    fn refresh_meshes(&self, changes: &mut Changes) {
        let base = changes.objects(self);
        let mut stale = Vec::new();
        for (h, obj) in base.each() {
            let Object::Entity {
                entity: Entity::PolyfaceMesh(p),
                ..
            } = obj
            else {
                continue;
            };
            let EntityList::Owned(list) = &p.vertices else {
                continue;
            };
            let count = |pick: fn(&Entity) -> bool| {
                list.iter()
                    .filter(|&&v| {
                        matches!(base.object(v), Some(Object::Entity { entity, .. }) if pick(entity))
                    })
                    .count()
            };
            let vertices = count(|e| matches!(e, Entity::VertexPface(_)));
            let faces = count(|e| matches!(e, Entity::VertexPfaceFace(_)));
            let (Ok(vertices), Ok(faces)) = (i16::try_from(vertices), i16::try_from(faces)) else {
                continue;
            };
            if (p.vertex_count, p.face_count) != (vertices, faces) {
                stale.push((h, vertices, faces));
            }
        }
        for (h, vertices, faces) in stale {
            if let Some(Object::Entity {
                entity: Entity::PolyfaceMesh(p),
                ..
            }) = changes.edit(self, h)
            {
                p.vertex_count = vertices;
                p.face_count = faces;
            }
        }
    }

    /// Vertex flags (step 2): a 3D polyline vertex has DXF 70 bit 32, a
    /// polygon mesh vertex bit 64, a polyface mesh vertex bits 64 and 128,
    /// and none has the bits of the other kinds (see [`vertex_flags`]).
    /// Flags already so are kept, and the vertex written as read.
    fn refresh_vertex_flags(&self, changes: &mut Changes) {
        let stale: Vec<(Handle, u8)> = changes
            .objects(self)
            .each()
            .filter_map(|(h, obj)| match obj {
                Object::Entity { entity, .. } => match vertex_flags(entity)? {
                    (flags, fixed) if flags != fixed => Some((h, fixed)),
                    _ => None,
                },
                Object::Other { .. } => None,
            })
            .collect();
        for (h, fixed) in stale {
            if let Some(Object::Entity {
                entity: Entity::Vertex3d(v) | Entity::VertexMesh(v) | Entity::VertexPface(v),
                ..
            }) = changes.edit(self, h)
            {
                v.flags = fixed;
            }
        }
    }

    /// Layout viewport lists (step 2). For the layouts their block names
    /// back (see [`own_layout_viewports`]): a layout moved to another block
    /// since read lists that block's viewports; otherwise, when the block's
    /// viewports changed, entries owned elsewhere stay, gone viewports of
    /// the block leave and new ones follow in block order. The other
    /// layouts keep their list, but entries naming no object are dropped,
    /// with a warning. Returns the block entity list entries walked.
    fn refresh_layout_viewports(&self, changes: &mut Changes) -> usize {
        let base = changes.objects(self);
        let (lists, steps) = own_layout_viewports(&base);
        let named_back: HashSet<Handle> = lists.iter().map(|(h, _)| *h).collect();
        let mut stale = Vec::new();
        for (h, now) in lists {
            let Some(Object::Other {
                object: NonGraphical::Layout(l),
                ..
            }) = base.object(h)
            else {
                continue;
            };
            let moved = self
                .read
                .layout_blocks
                .get(&h)
                .is_some_and(|&b| b != l.block_record);
            let same = !moved
                && self
                    .read
                    .layout_viewports
                    .get(&h)
                    .is_some_and(|read| sets_equal(read, &now));
            if same {
                continue;
            }
            let list = if moved {
                now
            } else {
                let in_block: HashSet<Handle> = now.iter().copied().collect();
                let mut list: Vec<Handle> = l
                    .viewports
                    .iter()
                    .copied()
                    .filter(|v| {
                        in_block.contains(v)
                            || matches!(base.object(*v), Some(Object::Entity { .. }))
                                && self.owner_of(*v) != Some(l.block_record)
                    })
                    .collect();
                let listed: HashSet<Handle> = list.iter().copied().collect();
                list.extend(now.iter().filter(|v| !listed.contains(v)));
                list
            };
            if list != l.viewports {
                stale.push((h, list));
            }
        }
        let dangling = Self::dangling_layout_viewports(&base, &named_back, |v| {
            changes.dropped.contains(&v) || base.object(v).is_none()
        });
        for (h, list) in stale {
            if let Some(Object::Other {
                object: NonGraphical::Layout(l),
                ..
            }) = changes.edit(self, h)
            {
                l.viewports = list;
            }
        }
        for (h, missing) in dangling {
            changes
                .warnings
                .push(dangling_viewports_warning(h, &missing));
            let gone: HashSet<Handle> = missing.into_iter().collect();
            if let Some(Object::Other {
                object: NonGraphical::Layout(l),
                ..
            }) = changes.edit(self, h)
            {
                l.viewports.retain(|v| !gone.contains(v));
            }
        }
        steps
    }

    /// The layouts their block does not name back (not in `named_back`)
    /// whose viewport list has entries `gone` says are not written, with
    /// those entries.
    fn dangling_layout_viewports(
        objects: &impl ObjectSet,
        named_back: &HashSet<Handle>,
        gone: impl Fn(Handle) -> bool,
    ) -> Vec<(Handle, Vec<Handle>)> {
        let mut out = Vec::new();
        for (h, obj) in objects.each() {
            let Object::Other {
                object: NonGraphical::Layout(l),
                ..
            } = obj
            else {
                continue;
            };
            if named_back.contains(&h) {
                continue;
            }
            let missing: Vec<Handle> = l.viewports.iter().copied().filter(|&v| gone(v)).collect();
            if !missing.is_empty() {
                out.push((h, missing));
            }
        }
        out
    }

    /// Block record insert lists (step 2): when a block's references (see
    /// [`block_reference`]) differ from those read, the ones gone leave the
    /// list and the new ones follow in handle order; entries AutoCAD keeps
    /// for erased objects stay.
    fn refresh_block_inserts(&self, changes: &mut Changes) {
        let base = changes.objects(self);
        let users = block_references(&base);
        let mut stale = Vec::new();
        for (h, obj) in base.each() {
            let Object::Other {
                object: NonGraphical::BlockRecord(b),
                ..
            } = obj
            else {
                continue;
            };
            let now = users.get(&h).map_or(&[][..], Vec::as_slice);
            let then = self.read.block_users.get(&h).map_or(&[][..], Vec::as_slice);
            if sets_equal(now, then) {
                continue;
            }
            let now_set: HashSet<Handle> = now.iter().copied().collect();
            let gone: HashSet<Handle> = then
                .iter()
                .filter(|u| !now_set.contains(u))
                .copied()
                .collect();
            let mut list: Vec<Handle> = b
                .inserts
                .iter()
                .copied()
                .filter(|i| !gone.contains(i))
                .collect();
            let listed: HashSet<Handle> = list.iter().copied().collect();
            list.extend(now.iter().filter(|u| !listed.contains(u)));
            if list != b.inserts {
                stale.push((h, list));
            }
        }
        for (h, list) in stale {
            if let Some(Object::Other {
                object: NonGraphical::BlockRecord(b),
                ..
            }) = changes.edit(self, h)
            {
                b.inserts = list;
            }
        }
    }

    /// The sections other than the header, classes, object map, objects,
    /// summary information and application information (the table of the
    /// module documentation), and the warnings about those left out.
    /// `objects` is the number of objects written, `preview_address` the
    /// address the preview lands at.
    fn other_sections(
        &self,
        header: &HeaderVars,
        objects: usize,
        preview_address: u64,
    ) -> (BTreeMap<SectionName, Vec<u8>>, Vec<Diagnostic>) {
        let source = self.source_version;
        let at_least = |v: Version| source.is_some_and(|s| s >= v);
        let r2018 = source == Some(Version::R2018);
        let mut out = BTreeMap::new();
        let mut diags = Vec::new();
        let extra = |name: &SectionName| self.extra_sections.get(name).map(Vec::as_slice);

        let (aux, warn) = aux_header(extra(&SectionName::AuxHeader), header);
        out.insert(SectionName::AuxHeader, aux);
        diags.extend(warn);
        out.insert(
            SectionName::ObjFreeSpace,
            obj_free_space(
                extra(&SectionName::ObjFreeSpace).filter(|_| at_least(Version::R2010)),
                header,
                objects,
            ),
        );
        let (template, warn) = template(extra(&SectionName::Template), source, header);
        out.insert(SectionName::Template, template);
        diags.extend(warn);
        let rev_history = extra(&SectionName::RevHistory).map_or_else(
            || {
                [0u32, 0, 1, 0]
                    .iter()
                    .flat_map(|v| v.to_le_bytes())
                    .collect()
            },
            <[u8]>::to_vec,
        );
        out.insert(SectionName::RevHistory, rev_history);
        if self.summary.is_none() && self.extra_sections.contains_key(&SectionName::SummaryInfo) {
            diags.push(warning(
                None,
                "the summary information could not be decoded; an empty one is written",
            ));
        }
        let preview = match (&self.preview, extra(&SectionName::Preview)) {
            (Some(p), _) => p
                .encode(preview_address)
                .map_err(|e| format!("the preview cannot be written ({e}); it is left out")),
            (None, Some(raw)) => relocate_preview(raw, self.read.preview_address, preview_address)
                .ok_or_else(|| {
                    "the undecoded preview's entry table cannot be read, so its addresses \
                         cannot be moved; it is left out"
                        .to_owned()
                }),
            (None, None) => Preview::default()
                .encode(preview_address)
                .map_err(|e| e.to_string()),
        };
        match preview {
            Ok(b) => {
                out.insert(SectionName::Preview, b);
            }
            Err(why) => diags.push(warning(None, why)),
        }

        for (name, bytes) in &self.extra_sections {
            use SectionName::*;
            let keep = match name {
                // Written above, or by `to_bytes_at`.
                Header | Classes | Handles | Objects | AuxHeader | ObjFreeSpace | Template
                | RevHistory | SummaryInfo | Preview | AppInfo => continue,
                // The empty section the writer adds itself.
                Other(n) if n.is_empty() => continue,
                AppInfoHistory | VbaProject => Ok(()),
                AcDsPrototype if at_least(Version::R2013) => {
                    let removed = self.read.undecoded_storage_acis.iter().any(|h| {
                        !matches!(
                            self.objects.get(h),
                            Some(Object::Entity {
                                entity: Entity::Acis(_),
                                ..
                            })
                        )
                    });
                    if removed {
                        diags.push(warning(
                            None,
                            "the data storage section could not be decoded and is written as \
                             read: it may hold records of ACIS entities removed since",
                        ));
                    }
                    Ok(())
                }
                AcDsPrototype => Err("pre-R2013 data storage has no known R2018 form"),
                Signature => Err("a digital signature cannot survive rewriting the file"),
                FileDepList | Security | Other(_) if r2018 => Ok(()),
                FileDepList | Security | Other(_) => {
                    Err("its form in the source version is not known to match R2018")
                }
            };
            match keep {
                Ok(()) => {
                    out.insert(name.clone(), bytes.clone());
                }
                Err(why) => diags.push(warning(
                    None,
                    format!("section {} is left out: {why}", name.r2004_name()),
                )),
            }
        }
        (out, diags)
    }
}

fn dangling_viewports_warning(layout: Handle, missing: &[Handle]) -> Diagnostic {
    let names: Vec<String> = missing.iter().map(Handle::to_string).collect();
    warning(
        Some(layout),
        format!(
            "the layout's viewport list names objects that do not exist ({}); they are left out",
            names.join(", ")
        ),
    )
}

/// Vertex flags (DXF 70): 3D polyline vertex, polygon mesh vertex,
/// polyface mesh vertex.
const VERTEX_3D: u8 = 32;
const VERTEX_MESH: u8 = 64;
const VERTEX_PFACE: u8 = 128;

/// A 3D polyline, polygon mesh or polyface mesh vertex's flags and the
/// flags its kind needs: its own other bits, its kind's bits set and the
/// other kinds' clear. A polyface face stores no flags.
fn vertex_flags(entity: &Entity) -> Option<(u8, u8)> {
    let (flags, set, clear) = match entity {
        Entity::Vertex3d(v) => (v.flags, VERTEX_3D, VERTEX_MESH | VERTEX_PFACE),
        Entity::VertexMesh(v) => (v.flags, VERTEX_MESH, VERTEX_3D | VERTEX_PFACE),
        Entity::VertexPface(v) => (v.flags, VERTEX_MESH | VERTEX_PFACE, VERTEX_3D),
        _ => return None,
    };
    Some((flags, (flags | set) & !clear))
}

/// Whether the entity list `list` holds an ATTDEF.
fn holds_attdef(objects: &impl ObjectSet, list: &EntityList) -> bool {
    let EntityList::Owned(list) = list else {
        return false;
    };
    list.iter().any(|&e| {
        matches!(
            objects.object(e),
            Some(Object::Entity {
                entity: Entity::AttDef(_),
                ..
            })
        )
    })
}

fn sets_equal(a: &[Handle], b: &[Handle]) -> bool {
    a.iter().collect::<HashSet<_>>() == b.iter().collect::<HashSet<_>>()
}

/// Flush directory `dir`, so a rename in it is durable (Unix; elsewhere a
/// directory cannot be opened to flush, and nothing is done).
fn sync_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::fs::File::open(dir)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        Ok(())
    }
}

/// Write `bytes` to `path` as [`Drawing::save`] describes: through a
/// temporary file renamed over the target.
fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let is_link = std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
    let target = if is_link {
        std::fs::canonicalize(path).map_err(Error::Io)?
    } else {
        path.to_path_buf()
    };
    let dir = match target.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let name = target
        .file_name()
        .ok_or_else(|| {
            Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "the path names no file",
            ))
        })?
        .to_string_lossy();
    let permissions = std::fs::metadata(&target).ok().map(|m| m.permissions());
    let (tmp, mut file) = create_temp(dir, &name)?;
    let written = file
        .write_all(bytes)
        .and_then(|()| match &permissions {
            Some(p) => file.set_permissions(p.clone()),
            None => Ok(()),
        })
        .and_then(|()| file.sync_all())
        .and_then(|()| {
            drop(file);
            std::fs::rename(&tmp, &target)
        });
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(Error::Io(e));
    }
    sync_dir(dir).map_err(Error::Io)
}

/// Create a new temporary file in `dir` for `name`.
fn create_temp(dir: &Path, name: &str) -> Result<(std::path::PathBuf, std::fs::File)> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut last = None;
    for _ in 0..16 {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = dir.join(format!(".{name}.{}.{n}.opendwg-tmp", std::process::id()));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
        {
            Ok(f) => return Ok((tmp, f)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last = Some(e),
            Err(e) => return Err(Error::Io(e)),
        }
    }
    Err(Error::Io(last.unwrap_or_else(|| {
        std::io::Error::other("no temporary file name is free")
    })))
}

// ---- AcDb:AuxHeader (ODS 27) ----

/// The R2018 AuxHeader's size and the pre-R2018 one's.
const AUX_LEN: usize = 129;
const AUX_OLD_LEN: usize = 123;
const AUX_MAGIC: [u8; 3] = [0xFF, 0x77, 0x01];
/// R2018 offsets: the DWG version (`RS`, 33 for AC1032), $TDCREATE,
/// $TDUPDATE and $HANDSEED.
const AUX_VERSION: usize = 3;
const AUX_TDCREATE: usize = 69;
const AUX_TDUPDATE: usize = 77;
const AUX_HANDSEED: usize = 85;
/// The pre-R2018 offsets after which R2018 has two more bytes: the three
/// maintenance version fields grow from `RS` to `RL` (the corpus's R2018
/// files against the older ones).
const AUX_WIDENED: [usize; 3] = [7, 27, 31];
/// The R2018 AuxHeader of a drawing written for the first time: the
/// fields of ODS 27 in order, the three maintenance versions widened to `RL`
/// as AutoCAD 2018 writes them, with one save. Where ODS gives a value or a
/// rule it is followed; where it names a field without a value (the
/// version string words, the two unknown pairs) the value is that of
/// AutoCAD's R2018 corpus files (`acadsharp-dynblock-lookup`). The dates and
/// handle seed are the header's (filled in by [`aux_header`]).
/// `tests/from_scratch.rs` (`new_aux_header_fields`) compares it with the
/// corpus field by field and lists the differences.
fn aux_default() -> Vec<u8> {
    /// ODS 27: AC1032.
    const DWG_VERSION: i16 = 33;
    /// The maintenance version this crate writes (as in the file header).
    const MAINTENANCE: i32 = 0;
    /// ODS: "starts at 1".
    const SAVES: i32 = 1;
    /// ODS: the saves up to 0x7FFF, and those above it.
    const SAVES_PART_1: i16 = 1;
    const SAVES_PART_2: i16 = 0;
    /// ODS names these "DWG version string" and "Maintenance version"
    /// without values; AutoCAD's R2018 files have 22 and 46 in both pairs.
    const VERSION_STRING: i16 = 22;
    const VERSION_STRING_MAINTENANCE: i32 = 46;
    /// ODS gives 0x0005, 0x0893 twice; AutoCAD's R2018 files have 4,
    /// 0x0565, then 0, 0.
    const UNKNOWN_PAIRS: [i16; 4] = [4, 0x0565, 0, 0];
    let mut b = AUX_MAGIC.to_vec();
    let rs = |b: &mut Vec<u8>, v: i16| b.extend_from_slice(&v.to_le_bytes());
    let rl = |b: &mut Vec<u8>, v: i32| b.extend_from_slice(&v.to_le_bytes());
    rs(&mut b, DWG_VERSION);
    rl(&mut b, MAINTENANCE);
    rl(&mut b, SAVES);
    rl(&mut b, -1);
    rs(&mut b, SAVES_PART_1);
    rs(&mut b, SAVES_PART_2);
    rl(&mut b, 0);
    for _ in 0..2 {
        rs(&mut b, VERSION_STRING);
        rl(&mut b, VERSION_STRING_MAINTENANCE);
    }
    for v in UNKNOWN_PAIRS {
        rs(&mut b, v);
    }
    rs(&mut b, 0);
    rs(&mut b, 1);
    for _ in 0..5 {
        rl(&mut b, 0);
    }
    // $TDCREATE, $TDUPDATE, $HANDSEED (filled in by `aux_header`), the
    // educational plot stamp (0).
    b.resize(b.len() + 8 + 8 + 4, 0);
    rl(&mut b, 0);
    rs(&mut b, 0);
    rs(&mut b, SAVES_PART_1 - SAVES_PART_2);
    for _ in 0..3 {
        rl(&mut b, 0);
    }
    rl(&mut b, SAVES);
    for _ in 0..4 {
        rl(&mut b, 0);
    }
    debug_assert_eq!(b.len(), AUX_LEN);
    b
}

fn put_date(b: &mut [u8], at: usize, d: JulianDate) {
    b[at..at + 4].copy_from_slice(&d.day.to_le_bytes());
    b[at + 4..at + 8].copy_from_slice(&d.ms.to_le_bytes());
}

/// The R2018 AuxHeader from the source's (see the module documentation).
fn aux_header(source: Option<&[u8]>, header: &HeaderVars) -> (Vec<u8>, Option<Diagnostic>) {
    let mut warn = None;
    let mut aux = match source {
        Some(b) if b.len() == AUX_LEN && b[..3] == AUX_MAGIC => b.to_vec(),
        Some(b) if b.len() == AUX_OLD_LEN && b[..3] == AUX_MAGIC => {
            let mut v = b.to_vec();
            for &at in AUX_WIDENED.iter().rev() {
                v.splice(at..at, [0, 0]);
            }
            v
        }
        other => {
            if other.is_some() {
                warn = Some(warning(
                    None,
                    "the AuxHeader section has an unknown layout; the ODS field default is \
                     written",
                ));
            }
            aux_default()
        }
    };
    aux[AUX_VERSION..AUX_VERSION + 2].copy_from_slice(&33u16.to_le_bytes());
    put_date(&mut aux, AUX_TDCREATE, header.tdcreate);
    put_date(&mut aux, AUX_TDUPDATE, header.tdupdate);
    let seed = i32::try_from(header.handseed.0)
        .ok()
        .filter(|&s| s < i32::MAX)
        .unwrap_or(-1);
    aux[AUX_HANDSEED..AUX_HANDSEED + 4].copy_from_slice(&seed.to_le_bytes());
    (aux, warn)
}

// ---- AcDb:ObjFreeSpace (ODS 21) ----

const FREE_SPACE_LEN: usize = 89;
const FREE_SPACE_COUNT: usize = 8;
const FREE_SPACE_TDUPDATE: usize = 16;

/// The R2010+ ObjFreeSpace: the source's (`source`, R2010+ only) with
/// $TDUPDATE and the object count updated, or a new one.
fn obj_free_space(source: Option<&[u8]>, header: &HeaderVars, objects: usize) -> Vec<u8> {
    let mut b = match source {
        Some(s) if s.len() == FREE_SPACE_LEN => s.to_vec(),
        _ => {
            // ODS 21.1 with 64-bit values, as the corpus's R2010+ files have
            // it: 0, the object count, $TDUPDATE, then four pairs.
            let mut b = vec![0u8; 24];
            b.push(4);
            for v in [0x32u64, 0, 0x64, 0, 0x200, 0, 0xFFFF_FFFF, 0] {
                b.extend_from_slice(&v.to_le_bytes());
            }
            b
        }
    };
    let read = u64::from_le_bytes(
        b[FREE_SPACE_COUNT..FREE_SPACE_COUNT + 8]
            .try_into()
            .unwrap_or_default(),
    );
    let count = read.max(objects as u64);
    b[FREE_SPACE_COUNT..FREE_SPACE_COUNT + 8].copy_from_slice(&count.to_le_bytes());
    put_date(&mut b, FREE_SPACE_TDUPDATE, header.tdupdate);
    b
}

// ---- AcDb:Template (ODS 22) ----

/// The description and $MEASUREMENT of a Template section: R2007+ stores
/// the description as UTF-16 code units (the count includes a closing NUL,
/// as the corpus has `1, 0`), earlier versions as code page bytes.
pub(super) fn parse_template(b: &[u8], wide: bool, codepage: u16) -> Option<(String, u16)> {
    let n = usize::try_from(i16::from_le_bytes(b.get(..2)?.try_into().ok()?)).ok()?;
    let len = if wide { n.checked_mul(2)? } else { n };
    let end = 2usize.checked_add(len)?;
    let text = b.get(2..end)?;
    let [lo, hi] = b.get(end..)?.try_into().ok()?;
    let description = if wide {
        let units: Vec<u16> = text
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&u| u != 0)
            .collect();
        String::from_utf16(&units).ok()?
    } else {
        strings::decode(codepage, text)
    };
    Some((description, u16::from_le_bytes([lo, hi])))
}

/// The R2018 Template (see the module documentation).
fn template(
    source: Option<&[u8]>,
    version: Option<Version>,
    header: &HeaderVars,
) -> (Vec<u8>, Option<Diagnostic>) {
    let wide = version.is_some_and(|v| v >= Version::R2007);
    let measurement = header.measurement.and_then(|m| u16::try_from(m).ok());
    let parsed = source.map(|b| parse_template(b, wide, header.dwgcodepage));
    let (description, read_measurement, warn) = match (source, parsed) {
        (Some(b), Some(Some(_))) if wide && measurement.is_none() => return (b.to_vec(), None),
        (_, Some(Some((d, m)))) => (d, m, None),
        (_, Some(None)) => (
            String::new(),
            0,
            Some(warning(
                None,
                "the Template section cannot be read; an empty one is written",
            )),
        ),
        _ => (String::new(), 0, None),
    };
    let mut warn = warn;
    let mut units: Vec<u16> = description.encode_utf16().chain([0]).collect();
    if i16::try_from(units.len()).is_err() {
        units = vec![0];
        warn = Some(warning(
            None,
            "the Template description is too long for its 16-bit length; it is left empty",
        ));
    }
    let mut b = Vec::with_capacity(4 + 2 * units.len());
    b.extend_from_slice(&(units.len() as i16).to_le_bytes());
    for u in units {
        b.extend_from_slice(&u.to_le_bytes());
    }
    b.extend_from_slice(&measurement.unwrap_or(read_measurement).to_le_bytes());
    (b, warn)
}

// ---- AcDb:Preview (ODS 14) ----

/// A preview kept undecoded, its entries' addresses moved from `from` to
/// `to`; `None` when its entry table cannot be read, an entry lies outside
/// it, or the old address is not known.
fn relocate_preview(raw: &[u8], from: Option<u64>, to: u64) -> Option<Vec<u8>> {
    let from = from?;
    // Start sentinel (16), overall size (4), entry count (1), entries of 9.
    let n = usize::from(*raw.get(20)?);
    let table_end = 21 + 9 * n;
    let data_end = raw.len().checked_sub(16)?;
    if table_end > data_end {
        return None;
    }
    let mut out = raw.to_vec();
    for i in 0..n {
        let at = 21 + 9 * i + 1;
        let start = u64::from(u32::from_le_bytes(raw[at..at + 4].try_into().ok()?));
        let size = u64::from(u32::from_le_bytes(raw[at + 4..at + 8].try_into().ok()?));
        let offset = start.checked_sub(from)?;
        if offset < table_end as u64 || offset.checked_add(size)? > data_end as u64 {
            return None;
        }
        let moved = u32::try_from(to.checked_add(offset)?).ok()?;
        out[at..at + 4].copy_from_slice(&moved.to_le_bytes());
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::section::header::defaults_r2018;

    /// An empty drawing read from `version` with these extra sections.
    fn drawing(version: Version, extra: &[(SectionName, Vec<u8>)]) -> Drawing {
        Drawing {
            source_version: Some(version),
            header: defaults_r2018(),
            classes: Vec::new(),
            objects: BTreeMap::new(),
            ref_index: Default::default(),
            summary: None,
            preview: None,
            app_info: None,
            data_storage: None,
            extra_sections: extra.iter().cloned().collect(),
            diagnostics: Vec::new(),
            read: ReadState::default(),
        }
    }

    fn header() -> HeaderVars {
        let mut h = defaults_r2018();
        h.tdcreate = JulianDate {
            day: 2_450_000,
            ms: 1,
        };
        h.tdupdate = JulianDate {
            day: 2_460_000,
            ms: 2,
        };
        h.handseed = Handle(0x1234);
        h
    }

    fn le32(b: &[u8], at: usize) -> i32 {
        i32::from_le_bytes(b[at..at + 4].try_into().unwrap())
    }

    /// An R2004-R2013 AuxHeader is widened to the R2018 layout (the corpus's
    /// AC1018 bytes against its AC1032 ones), with the dates, seed and version
    /// set.
    #[test]
    fn aux_header_is_widened_and_updated() {
        let old: Vec<u8> = [
            "ff 77 01 21 00 49 01 a2 00 00 00 ff ff ff ff 36",
            "00 20 00 00 00 00 00 16 00 2e 00 16 00 2e 00 04",
            "00 65 05 00 00 00 00 00 00 01 00 00 00 00 00 00",
            "00 00 00 00 00 00 00 00 01 01 01 01 00 07 00 f0",
            "87 25 00 58 6b 3a 02 54 8d 25 00 c0 68 07 02 1f",
            "10 00 00 00 00 00 00 00 00 52 00 00 00 00 00 00",
            "00 00 00 00 00 00 00 a2 00 00 00 00 00 00 00 00",
            "00 00 00 00 00 00 00 00 00 00 00",
        ]
        .join(" ")
        .split(' ')
        .map(|x| u8::from_str_radix(x, 16).unwrap())
        .collect();
        assert_eq!(old.len(), AUX_OLD_LEN);
        let (aux, warn) = aux_header(Some(&old), &header());
        assert!(warn.is_none());
        assert_eq!(aux.len(), AUX_LEN);
        // The R2018 sample's layout: the widened fields and what follows.
        assert_eq!(aux[5..9], [0x49, 0x01, 0, 0]);
        assert_eq!(aux[9..13], [0xa2, 0, 0, 0]);
        assert_eq!(
            aux[25..37],
            [0x16, 0, 0x2e, 0, 0, 0, 0x16, 0, 0x2e, 0, 0, 0]
        );
        assert_eq!(aux[109], 0xa2);
        assert_eq!((le32(&aux, 69), le32(&aux, 73)), (2_450_000, 1));
        assert_eq!((le32(&aux, 77), le32(&aux, 81)), (2_460_000, 2));
        assert_eq!(le32(&aux, 85), 0x1234);
        // An R2018 one keeps its layout; a seed past 31 bits is -1 (ODS).
        let mut h = header();
        h.handseed = Handle(0x1_0000_0000);
        let (again, _) = aux_header(Some(&aux), &h);
        assert_eq!(again[..85], aux[..85]);
        assert_eq!(le32(&again, 85), -1);
        // None, or an unknown layout (with a warning): the default.
        let (fresh, warn) = aux_header(None, &header());
        assert!(warn.is_none());
        assert_eq!(fresh.len(), AUX_LEN);
        assert_eq!(fresh[..3], AUX_MAGIC);
        assert_eq!((fresh[9], fresh[109]), (1, 1));
        assert_eq!((le32(&fresh, 69), le32(&fresh, 81)), (2_450_000, 2));
        assert_eq!(le32(&fresh, 85), 0x1234);
        let (_, warn) = aux_header(Some(&[1, 2, 3]), &header());
        assert_eq!(warn.unwrap().severity, Severity::Warning);
    }

    #[test]
    fn obj_free_space_forms() {
        let fresh = obj_free_space(None, &header(), 7);
        assert_eq!(fresh.len(), FREE_SPACE_LEN);
        assert_eq!(fresh[8], 7);
        assert_eq!((le32(&fresh, 16), le32(&fresh, 20)), (2_460_000, 2));
        assert_eq!(fresh[24], 4);
        assert_eq!(fresh[25], 0x32);
        assert_eq!(fresh[73..77], [0xFF; 4]);
        // A copy keeps its larger count; a smaller one is raised.
        let mut read = fresh.clone();
        read[8] = 100;
        read[0] = 0x4E;
        let copy = obj_free_space(Some(&read), &header(), 7);
        assert_eq!((copy[0], copy[8]), (0x4E, 100));
        assert_eq!(
            obj_free_space(Some(&read), &header(), 300)[8..10],
            [0x2C, 1]
        );
    }

    #[test]
    fn template_forms() {
        let mut h = header();
        h.measurement = None;
        // R2007+: copied as read.
        let wide = [1, 0, 0, 0, 1, 0];
        assert_eq!(template(Some(&wide), Some(Version::R2018), &h).0, wide);
        // Before R2007: re-encoded, the description in UTF-16.
        let narrow = [2, 0, b'A', b'B', 1, 0];
        let (t, warn) = template(Some(&narrow), Some(Version::R2004), &h);
        assert!(warn.is_none());
        assert_eq!(t, [3, 0, b'A', 0, b'B', 0, 0, 0, 1, 0]);
        assert_eq!(parse_template(&t, true, 30), Some(("AB".into(), 1)));
        // The header's $MEASUREMENT wins; without a section, an empty one.
        h.measurement = Some(1);
        assert_eq!(
            template(Some(&[1, 0, 0, 0, 0, 0]), Some(Version::R2018), &h).0,
            [1, 0, 0, 0, 1, 0]
        );
        h.measurement = None;
        assert_eq!(template(None, None, &h).0, [1, 0, 0, 0, 0, 0]);
        // Unreadable: an empty one, with a warning.
        let (t, warn) = template(Some(&[9, 0]), Some(Version::R2000), &h);
        assert_eq!(t, [1, 0, 0, 0, 0, 0]);
        assert!(warn.is_some());
    }

    /// A preview with one entry at `base + 30` of 4 bytes.
    fn raw_preview(base: u32) -> Vec<u8> {
        let mut p = vec![0u8; 16];
        p.extend_from_slice(&14u32.to_le_bytes());
        p.push(1);
        p.push(7); // a code the decoder does not know
        p.extend_from_slice(&(base + 30).to_le_bytes());
        p.extend_from_slice(&4u32.to_le_bytes());
        p.extend_from_slice(&[1, 2, 3, 4]);
        p.extend_from_slice(&[0u8; 16]);
        p
    }

    #[test]
    fn undecoded_preview_is_relocated() {
        let p = raw_preview(0x1000);
        let moved = relocate_preview(&p, Some(0x1000), 0x200).unwrap();
        assert_eq!(moved, raw_preview(0x200));
        // Unknown old address, an entry outside the section, a short table.
        assert!(relocate_preview(&p, None, 0x200).is_none());
        assert!(relocate_preview(&p, Some(0x1001), 0x200).is_none());
        assert!(relocate_preview(&p[..20], Some(0x1000), 0x200).is_none());
    }

    /// The section decisions of the module documentation's table.
    #[test]
    fn other_sections_by_source_version() {
        use SectionName::*;
        let extra = [
            (AppInfoHistory, vec![1]),
            (VbaProject, vec![2]),
            (AcDsPrototype, vec![3]),
            (FileDepList, vec![4]),
            (Security, vec![5]),
            (Signature, vec![6]),
            (Other("AcDb:Custom".into()), vec![7]),
            (Other(String::new()), vec![]),
            (RevHistory, vec![8; 16]),
        ];
        let names = |s: &BTreeMap<SectionName, Vec<u8>>| s.keys().cloned().collect::<Vec<_>>();
        let h = header();

        let d = drawing(Version::R2018, &extra);
        let (out, diags) = d.other_sections(&h, 0, 0x1A0);
        assert_eq!(
            names(&out),
            [
                AuxHeader,
                Preview,
                AppInfoHistory,
                FileDepList,
                RevHistory,
                Security,
                ObjFreeSpace,
                Template,
                AcDsPrototype,
                VbaProject,
                Other("AcDb:Custom".into()),
            ]
        );
        assert_eq!(out[&RevHistory], [8; 16]);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("AcDb:Signature"));

        let d = drawing(Version::R2004, &extra);
        let (out, diags) = d.other_sections(&h, 0, 0x1A0);
        assert_eq!(
            names(&out),
            [
                AuxHeader,
                Preview,
                AppInfoHistory,
                RevHistory,
                ObjFreeSpace,
                Template,
                VbaProject,
            ]
        );
        let left_out: Vec<&str> = diags
            .iter()
            .map(|d| {
                assert_eq!(d.severity, Severity::Warning);
                d.message.split(' ').nth(1).unwrap()
            })
            .collect();
        assert_eq!(
            left_out,
            [
                "AcDb:FileDepList",
                "AcDb:Security",
                "AcDb:AcDsPrototype_1b",
                "AcDb:Signature",
                "AcDb:Custom"
            ]
        );
        assert_eq!(d.write_diagnostics(), diags);

        // An R2000 drawing: the sections AutoCAD always writes are made.
        let d = drawing(Version::R2000, &[]);
        let (out, diags) = d.other_sections(&h, 3, 0x1A0);
        assert!(diags.is_empty());
        assert_eq!(
            names(&out),
            [AuxHeader, Preview, RevHistory, ObjFreeSpace, Template]
        );
        assert_eq!(
            out[&RevHistory],
            [0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(out[&ObjFreeSpace][8], 3);
    }

    /// An undecoded preview is moved to its new address, or left out with a
    /// warning; an undecoded summary is replaced by an empty one with a
    /// warning.
    #[test]
    fn undecoded_info_sections() {
        let h = header();
        let mut d = drawing(
            Version::R2018,
            &[(SectionName::Preview, raw_preview(0x1000))],
        );
        d.read.preview_address = Some(0x1000);
        let (out, diags) = d.other_sections(&h, 0, 0x1A0);
        assert_eq!(out[&SectionName::Preview], raw_preview(0x1A0));
        assert!(diags.is_empty());
        assert!(d.write_diagnostics().is_empty());

        d.read.preview_address = None;
        let (out, diags) = d.other_sections(&h, 0, 0x1A0);
        assert!(!out.contains_key(&SectionName::Preview));
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("preview"));
        assert_eq!(d.write_diagnostics(), diags);

        let d = drawing(Version::R2018, &[(SectionName::SummaryInfo, vec![1, 2])]);
        let diags = d.write_diagnostics();
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("summary"));
    }

    use crate::object::{
        BlockRecord, EntityCommon, Layout, Line, ObjectCommon, TableControl, TableEntry, VpEntHdr,
    };

    fn other(h: u64, object: NonGraphical) -> (Handle, Object) {
        let common = ObjectCommon {
            handle: Handle(h),
            ..ObjectCommon::new()
        };
        (Handle(h), Object::Other { common, object })
    }

    fn block(h: u64, entities: Vec<Handle>, layout: Handle) -> (Handle, Object) {
        other(
            h,
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
                layout,
            }),
        )
    }

    fn line(h: u64, owner: u64) -> (Handle, Object) {
        let mut common = EntityCommon::new(Handle(0));
        common.obj.handle = Handle(h);
        common.obj.owner = Handle(owner);
        common.entmode = 0;
        let entity = Entity::Line(Line {
            start: [0.0; 3],
            end: [1.0, 0.0, 0.0],
            thickness: 0.0,
            extrusion: [0.0, 0.0, 1.0],
        });
        (Handle(h), Object::Entity { common, entity })
    }

    /// A layout's block (0x10, naming the layout 0x20 back) of `n` lines,
    /// written.
    fn line_work(n: u64) -> WriteWork {
        let mut d = drawing(Version::R2018, &[]);
        let lines: Vec<Handle> = (0..n).map(|i| Handle(0x100 + i)).collect();
        d.objects.extend([block(0x10, lines, Handle(0x20))]);
        d.objects.extend([other(
            0x20,
            NonGraphical::Layout(Box::new(Layout {
                block_record: Handle(0x10),
                ..Layout::default()
            })),
        )]);
        d.objects.extend((0..n).map(|i| line(0x100 + i, 0x10)));
        let mut work = WriteWork::default();
        write_unvalidated(&d, &mut work);
        work
    }

    const SAVED: JulianDate = JulianDate {
        day: 2_461_300,
        ms: 0,
    };

    /// The file for a test drawing that lacks a valid drawing's structure
    /// (tables, layer 0, ...): written as [`Drawing::write`] does, without
    /// validating it.
    fn write_unvalidated(d: &Drawing, work: &mut WriteWork) -> Vec<u8> {
        assert!(d.unconvertible_objects().is_empty());
        let changes = d.refreshed(work);
        d.encode_file(SAVED, work, changes).unwrap().0
    }

    /// The deterministic companion of `large_drawing_roundtrip_is_linear`
    /// (tests/roundtrip.rs): the writer's work grows
    /// linearly with the objects.
    #[test]
    fn write_work_is_linear() {
        let small = line_work(1_000);
        let large = line_work(10_000);
        assert_eq!(small.encoded, 1_002);
        assert_eq!(large.encoded, 10_002);
        // The layout's block is walked once.
        assert_eq!(small.viewport_steps, 1_000);
        assert_eq!(large.viewport_steps, 10_000);
        assert!(
            large.class_probes <= 10 * small.class_probes + 10,
            "{small:?} {large:?}"
        );
        assert!(
            large.viewport_steps <= 10 * small.viewport_steps + 10,
            "{small:?} {large:?}"
        );
    }

    fn viewport(h: u64, owner: u64) -> (Handle, Object) {
        let (h, mut obj) = line(h, owner);
        if let Object::Entity { entity, .. } = &mut obj {
            *entity = Entity::Viewport(Box::new(crate::object::entities::misc::tests::viewport()));
        }
        (h, obj)
    }

    fn layout(h: u64, block: u64, viewports: &[u64]) -> (Handle, Object) {
        other(
            h,
            NonGraphical::Layout(Box::new(Layout {
                block_record: Handle(block),
                viewports: viewports.iter().map(|&v| Handle(v)).collect(),
                ..Layout::default()
            })),
        )
    }

    fn written_viewports(changes: &Changes, d: &Drawing, h: u64) -> Vec<u64> {
        match changes.get(d, Handle(h)) {
            Some(Object::Other {
                object: NonGraphical::Layout(l),
                ..
            }) => l.viewports.iter().map(|v| v.0).collect(),
            o => panic!("{o:?}"),
        }
    }

    /// A layout its block does not name back keeps its list, but entries
    /// that name no object are dropped, with a warning.
    #[test]
    fn unlinked_layout_drops_missing_viewports() {
        let mut d = drawing(Version::R2018, &[]);
        d.objects.extend([
            block(0x10, vec![Handle(0x100)], Handle(0)),
            layout(0x20, 0x10, &[0x100, 0x999, 0x101]),
            viewport(0x100, 0x10),
            viewport(0x101, 0x11),
        ]);
        let changes = d.refreshed(&mut WriteWork::default());
        assert_eq!(written_viewports(&changes, &d, 0x20), [0x100, 0x101]);
        assert_eq!(changes.warnings.len(), 1);
        assert_eq!(changes.warnings[0].handle, Some(Handle(0x20)));
        assert!(changes.warnings[0].message.contains("999"));
        let diags = d.write_diagnostics();
        assert!(
            diags.iter().any(|x| x.handle == Some(Handle(0x20))),
            "{diags:?}"
        );
        // With every entry present, nothing changes.
        d.objects.insert(Handle(0x999), viewport(0x999, 0x12).1);
        let changes = d.refreshed(&mut WriteWork::default());
        assert!(changes.warnings.is_empty());
        assert!(!changes.overlay.contains_key(&Handle(0x20)));
    }

    /// A layout moved to another block (which names it back) since read
    /// lists that block's viewports, not the old block's.
    #[test]
    fn moved_layout_takes_new_block_viewports() {
        let mut d = drawing(Version::R2018, &[]);
        d.objects.extend([
            block(0x10, vec![Handle(0x100)], Handle(0x20)),
            block(0x11, vec![Handle(0x200), Handle(0x201)], Handle(0)),
            layout(0x20, 0x10, &[0x100]),
            viewport(0x100, 0x10),
            viewport(0x200, 0x11),
            viewport(0x201, 0x11),
        ]);
        d.read.capture(&[], &d.objects.clone(), &[], Version::R2018);
        let changes = d.refreshed(&mut WriteWork::default());
        assert!(changes.overlay.is_empty());
        for (h, l) in [(0x10, Handle(0)), (0x11, Handle(0x20))] {
            if let Some(Object::Other {
                object: NonGraphical::BlockRecord(b),
                ..
            }) = d.objects.get_mut(&Handle(h))
            {
                b.layout = l;
            }
        }
        if let Some(Object::Other {
            object: NonGraphical::Layout(l),
            ..
        }) = d.objects.get_mut(&Handle(0x20))
        {
            l.block_record = Handle(0x11);
        }
        let changes = d.refreshed(&mut WriteWork::default());
        assert_eq!(written_viewports(&changes, &d, 0x20), [0x200, 0x201]);
        assert!(changes.warnings.is_empty());
    }

    /// Many layouts claiming one large block: only the layout the block
    /// names back gets its viewports, so the block is walked once.
    #[test]
    fn shared_layout_block_is_walked_once() {
        let n = 2_000u64;
        let layout = |h: u64| {
            other(
                h,
                NonGraphical::Layout(Box::new(Layout {
                    block_record: Handle(0x10),
                    ..Layout::default()
                })),
            )
        };
        let mut objects: BTreeMap<Handle, Object> = (0..n).map(|i| layout(0x1_0000 + i)).collect();
        let lines: Vec<Handle> = (0..n).map(|i| Handle(0x100 + i)).collect();
        objects.extend([block(0x10, lines, Handle(0x1_0000))]);
        objects.extend((0..n).map(|i| line(0x100 + i, 0x10)));
        let (lists, steps) = own_layout_viewports(&objects);
        assert_eq!(steps as u64, n);
        assert_eq!(lists.len(), 1);
        assert_eq!(lists[0].0, Handle(0x1_0000));
        let mut read = ReadState::default();
        read.capture(&[], &objects, &[], Version::R2018);
        assert_eq!(read.layout_viewports.len(), 1);
        let mut d = drawing(Version::R2018, &[]);
        d.objects = objects;
        let mut work = WriteWork::default();
        write_unvalidated(&d, &mut work);
        assert_eq!(work.viewport_steps as u64, n);
    }

    /// Type codes and class counts take a constant number of class lookups
    /// per object, however many classes there are.
    #[test]
    fn class_lookups_are_constant_per_object() {
        use crate::object::RawObject;
        use crate::section::classes::ITEM_CLASS_OBJECT;
        let n = 3_000u16;
        let classes: Vec<DxfClass> = (0..n)
            .map(|i| DxfClass {
                class_number: 500 + i,
                proxy_flags: 0,
                app_name: "A".into(),
                cpp_class_name: "C".into(),
                dxf_name: format!("THING{i}"),
                was_zombie: false,
                item_class_id: ITEM_CLASS_OBJECT,
                instance_count: 0,
                dwg_version: 0,
                maintenance_version: 0,
                unknown: [0; 2],
            })
            .collect();
        let mut d = drawing(Version::R2018, &[]);
        d.objects = (0..n)
            .map(|i| {
                other(
                    0x100 + u64::from(i),
                    NonGraphical::Unknown(RawObject {
                        type_code: 500 + i,
                        class_name: Some(format!("THING{i}")),
                        source_version: Version::R2018,
                        data: vec![],
                        data_bits: 0,
                        strings: Some((vec![], 0)),
                        handles: vec![],
                        tail_handles_raw: vec![],
                        tail_handle_bits: 0,
                    }),
                )
            })
            .collect();
        // Fixed-type objects too: no class has their name.
        let lines: Vec<Handle> = (0..u64::from(n)).map(|i| Handle(0x10_000 + i)).collect();
        d.objects.extend([block(0x10, lines, Handle(0))]);
        d.objects
            .extend((0..u64::from(n)).map(|i| line(0x10_000 + i, 0x10)));
        d.classes = classes;
        let mut work = WriteWork::default();
        write_unvalidated(&d, &mut work);
        let n = usize::from(n);
        assert_eq!(work.encoded, 2 * n + 1);
        // A class object: one lookup for its type code when encoding, one
        // when counting. A fixed-type object: one name lookup when counting.
        assert_eq!(work.class_probes, 2 * n + n + 1);
    }

    /// The VP_ENT_HDR table is left out, unless a member has an extension
    /// dictionary or is another object's reactor: it is then reported
    /// unconvertible rather than dropped with links to it.
    #[test]
    fn vp_ent_hdr_table_drop_is_guarded() {
        let table = || {
            let mut d = drawing(Version::R2000, &[]);
            d.objects.extend([
                other(
                    0x20,
                    NonGraphical::TableControl(TableControl {
                        kind: TableKind::VpEntHdr,
                        entries: vec![Handle(0x21)],
                        extra: vec![],
                    }),
                ),
                other(
                    0x21,
                    NonGraphical::VpEntHdr(VpEntHdr {
                        entry: TableEntry::default(),
                        flag: false,
                        viewport: Handle(0),
                        prev: Handle(0),
                    }),
                ),
            ]);
            d
        };
        let d = table();
        assert!(d.unconvertible_objects().is_empty());
        assert_eq!(
            d.drops(&d.objects).dropped,
            BTreeSet::from([Handle(0x20), Handle(0x21)])
        );
        let back = Drawing::from_bytes(&write_unvalidated(&d, &mut WriteWork::default())).unwrap();
        assert!(back.objects.is_empty());

        let mut d = table();
        if let Some(Object::Other { common, .. }) = d.objects.get_mut(&Handle(0x21)) {
            common.xdict = Some(Handle(0x30));
        }
        assert_eq!(
            d.unconvertible_objects(),
            [(Handle(0x21), "VP_ENT_HDR".to_owned())]
        );
        assert!(matches!(
            d.to_bytes_at(SAVED),
            Err(Error::UnconvertibleObjects(_))
        ));

        let mut d = table();
        let (h, mut o) = other(0x40, NonGraphical::Placeholder);
        if let Object::Other { common, .. } = &mut o {
            common.reactors.push(Handle(0x20));
        }
        d.objects.insert(h, o);
        assert_eq!(
            d.unconvertible_objects(),
            [(Handle(0x20), "VP_ENT_HDR_CONTROL".to_owned())]
        );
    }

    #[test]
    fn overlong_template_description_warns() {
        let mut h = header();
        h.measurement = None;
        // 32767 bytes, the most a pre-R2007 count holds: with R2018's
        // closing NUL that is one unit too many.
        let mut narrow = Vec::new();
        let text = vec![b'x'; 32_767];
        narrow.extend_from_slice(&32_767i16.to_le_bytes());
        narrow.extend_from_slice(&text);
        narrow.extend_from_slice(&[0, 0]);
        let (t, warn) = template(Some(&narrow), Some(Version::R2004), &h);
        assert_eq!(t, [1, 0, 0, 0, 0, 0]);
        assert!(warn.unwrap().message.contains("too long"));
    }

    /// A proxy entity read from R2000 or R2004 is convertible (its data is
    /// in the object format its own version fields name): added to a new
    /// R2018 drawing, it saves and reads back with its payload and object
    /// ids unchanged (no corpus file has proxy entities). This shows the
    /// library's own consistency only: whether AutoCAD accepts such a proxy
    /// is unconfirmed against AutoCAD.
    #[test]
    fn proxy_entity_from_old_file_roundtrips() {
        use crate::bits::BitWriter;
        use crate::handle::HandleRef;
        use crate::object::stream::tests::assemble;
        use crate::object::{EntityCommon, ObjReader, ProxyEntity, type_code};

        for v in [Version::R2000, Version::R2004] {
            let mut d = Drawing::new_at(JulianDate {
                day: 2_461_300,
                ms: 0,
            });
            let l0 = d.layer_by_name("0").unwrap();
            let continuous = d.header.ltype_continuous;
            let mut data = BitWriter::new();
            data.write_bl(499);
            data.write_bl(27 | 105 << 16);
            data.write_b(false);
            // The proxy's own data: an old-style text field and odd bits.
            data.write_tv_raw(b"custom").unwrap();
            data.write_bs(-3);
            data.write_b(true);
            let ids = [
                HandleRef {
                    code: 4,
                    value: l0.0,
                },
                HandleRef {
                    code: 5,
                    value: continuous.0,
                },
                HandleRef { code: 4, value: 0 },
            ];
            let mut handles = BitWriter::new();
            for id in ids {
                handles.write_handle_ref(id).unwrap();
            }
            let bytes = assemble(v, type_code::ACAD_PROXY_ENTITY, &data, None, &handles);
            let (mut r, _) = ObjReader::split(&bytes, v, 30).unwrap();
            let proxy = ProxyEntity::decode(&mut r).unwrap();
            assert_eq!(proxy.object_ids, ids, "{v:?}");
            let ms = d.model_space();
            let h = d
                .add_entity(
                    ms,
                    EntityCommon::new(l0),
                    Entity::ProxyEntity(Box::new(proxy.clone())),
                )
                .unwrap();
            assert!(d.unconvertible_objects().is_empty(), "{v:?}");
            let back = Drawing::from_bytes(&d.to_bytes().unwrap()).unwrap();
            match back.get(h) {
                Some(Object::Entity {
                    entity: Entity::ProxyEntity(p),
                    ..
                }) => assert_eq!(**p, proxy, "{v:?}"),
                other => panic!("{v:?}: {other:?}"),
            }
        }
    }

    /// Hermetic (no corpus): a custom-class object kept as R2004 bits, in a
    /// new drawing's named objects dictionary, blocks saving with
    /// [`Error::UnconvertibleObjects`] naming it by its class's DXF name;
    /// once `remove` takes it (and its dictionary entry), the drawing saves
    /// and reads back valid.
    #[test]
    fn hermetic_custom_object_blocks_save_until_removed() {
        use crate::object::{Dictionary, ObjectCommon, RawObject};
        use crate::section::classes::ITEM_CLASS_OBJECT;

        let mut d = Drawing::new_at(JulianDate {
            day: 2_461_300,
            ms: 0,
        });
        let number = d.classes.iter().map(|c| c.class_number).max().unwrap() + 1;
        d.classes.push(DxfClass {
            class_number: number,
            proxy_flags: 0,
            app_name: "AcmeApp".into(),
            cpp_class_name: "AcmeThing".into(),
            dxf_name: "ACME_THING".into(),
            was_zombie: false,
            item_class_id: ITEM_CLASS_OBJECT,
            instance_count: 0,
            dwg_version: 0,
            maintenance_version: 0,
            unknown: [0; 2],
        });
        let nod = d.named_objects();
        let h = d.alloc_handle().unwrap();
        let common = ObjectCommon {
            handle: h,
            owner: nod,
            reactors: vec![nod],
            ..ObjectCommon::new()
        };
        let raw = RawObject {
            type_code: number,
            class_name: Some("ACME_THING".into()),
            source_version: Version::R2004,
            data: vec![0xA5, 0x80],
            data_bits: 9,
            strings: None,
            handles: vec![],
            tail_handles_raw: vec![],
            tail_handle_bits: 0,
        };
        d.objects.insert(
            h,
            Object::Other {
                common,
                object: NonGraphical::Unknown(raw),
            },
        );
        let Some(Object::Other {
            object: NonGraphical::Dictionary(Dictionary { entries, .. }),
            ..
        }) = d.objects.get_mut(&nod)
        else {
            panic!("no named objects dictionary")
        };
        entries.push(("ACME_THING".into(), h));
        *d.ref_index.get_mut() = Default::default();

        let expect = vec![(h, "ACME_THING".to_owned())];
        assert_eq!(d.unconvertible_objects(), expect);
        match d.to_bytes() {
            Err(Error::UnconvertibleObjects(list)) => assert_eq!(list, expect),
            other => panic!("{:?}", other.map(|b| b.len())),
        }
        d.remove(h).unwrap();
        let Some(Object::Other {
            object: NonGraphical::Dictionary(dict),
            ..
        }) = d.get(nod)
        else {
            panic!()
        };
        assert!(dict.entries.iter().all(|&(_, e)| e != h));
        let back = Drawing::from_bytes(&d.to_bytes().unwrap()).unwrap();
        assert!(back.diagnostics().is_empty(), "{:?}", back.diagnostics());
        assert!(back.get(h).is_none());
        let errors: Vec<_> = back
            .validate()
            .into_iter()
            .filter(|x| x.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "{errors:?}");
    }

    /// A VIEW named `name`.
    fn view_named(name: &str) -> NonGraphical {
        use crate::object::View;
        use crate::object::tables::{Lighting, ViewMode};
        NonGraphical::View(Box::new(View {
            entry: TableEntry {
                name: name.into(),
                ..TableEntry::default()
            },
            height: 1.0,
            width: 1.0,
            center: [0.0; 2],
            target: [0.0; 3],
            direction: [0.0, 0.0, 1.0],
            twist_angle: 0.0,
            lens_length: 50.0,
            front_clip: 0.0,
            back_clip: 0.0,
            view_mode: ViewMode::default(),
            render_mode: 0,
            lighting: Lighting::default(),
            paper_space: false,
            ucs: None,
            camera_plottable: false,
            background: Handle(0),
            visual_style: Handle(0),
            sun: Handle(0),
            base_ucs: Handle(0),
            named_ucs: Handle(0),
            live_section: Handle(0),
        }))
    }

    /// The model space layout of a new drawing, named after plot view `name`
    /// as an R2000 file has it.
    fn layout_naming_view(d: &mut Drawing, name: &str) -> Handle {
        let h = d.layouts()[0].0;
        let Some(Object::Other {
            object: NonGraphical::Layout(l),
            ..
        }) = d.get_mut(h)
        else {
            panic!()
        };
        l.plot.plot_view_name = name.into();
        h
    }

    fn written_plot_view(bytes: &[u8], layout: Handle) -> Handle {
        let back = Drawing::from_bytes(bytes).unwrap();
        let errors: Vec<_> = back
            .validate()
            .into_iter()
            .filter(|x| x.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "{errors:?}");
        match back.get(layout) {
            Some(Object::Other {
                object: NonGraphical::Layout(l),
                ..
            }) => l.plot.plot_view,
            o => panic!("{o:?}"),
        }
    }

    /// A plot view name no VIEW had when the file was read is resolved when
    /// saving: a view added since is found (compared without case).
    #[test]
    fn plot_view_name_resolves_at_save() {
        let mut d = Drawing::new();
        let layout = layout_naming_view(&mut d, "Front");
        let view = d
            .add_table_entry(TableKind::View, view_named("FRONT"))
            .unwrap();
        assert!(d.unconvertible_objects().is_empty());
        let (bytes, diags) = d.to_bytes_with_diagnostics().unwrap();
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(written_plot_view(&bytes, layout), view);
    }

    /// A name no view has is cleared (a null handle) with a warning naming
    /// the layout and the view; the layout is still written.
    #[test]
    fn unresolved_plot_view_name_is_cleared_with_a_warning() {
        let mut d = Drawing::new();
        let layout = layout_naming_view(&mut d, "Nowhere");
        let name = d.layouts()[0].1.name.clone();
        assert!(d.unconvertible_objects().is_empty());
        let (bytes, diags) = d.to_bytes_with_diagnostics().unwrap();
        let w: Vec<_> = diags.iter().filter(|x| x.handle == Some(layout)).collect();
        assert_eq!(w.len(), 1, "{diags:?}");
        assert_eq!(w[0].severity, Severity::Warning);
        assert!(w[0].message.contains("Nowhere") && w[0].message.contains(&name));
        assert_eq!(d.write_diagnostics(), diags);
        assert_eq!(written_plot_view(&bytes, layout), Handle(0));
    }

    /// An extension dictionary and a hard-owner dictionary's entry (with or
    /// without default) that store a null owner (AutoCAD 2002's R14
    /// round-trip dictionary of a DIMSTYLE and its XRECORD: `arx-02` of the
    /// extended corpus) are written with their lister as owner, with a
    /// warning each.
    #[test]
    fn null_owners_of_listed_objects_are_set() {
        for with_default in [false, true] {
            let mut d = Drawing::new();
            let layers = d.header.layer_control;
            let xdict = d.get(layers).unwrap().object_common().xdict.unwrap();
            let Some(Object::Other {
                object: NonGraphical::Dictionary(x),
                ..
            }) = d.get(xdict)
            else {
                panic!("an extension dictionary");
            };
            assert!(x.is_hard_owner());
            let (_, states) = x.entries[0].clone();
            if with_default {
                let Some(Object::Other { object, .. }) = d.get_mut(xdict) else {
                    panic!()
                };
                let NonGraphical::Dictionary(x) = object.clone() else {
                    panic!()
                };
                *object =
                    NonGraphical::DictionaryWithDefault(crate::object::DictionaryWithDefault {
                        dictionary: x,
                        default_entry: states,
                    });
            }
            for h in [xdict, states] {
                d.get_mut(h).unwrap().object_common_mut().owner = Handle(0);
            }
            let diags = d.validate();
            assert!(
                diags.iter().all(|x| x.severity != Severity::Error),
                "{diags:#?}"
            );
            let (bytes, warnings) = d.to_bytes_with_diagnostics().unwrap();
            let named: Vec<Option<Handle>> = warnings
                .iter()
                .filter(|w| w.message.contains("null"))
                .map(|w| w.handle)
                .collect();
            assert_eq!(named, [Some(xdict), Some(states)], "{warnings:#?}");
            let back = Drawing::from_bytes(&bytes).unwrap();
            assert_eq!(back.get(xdict).unwrap().object_common().owner, layers);
            assert_eq!(back.get(states).unwrap().object_common().owner, xdict);
            assert!(back.validate().is_empty(), "{:#?}", back.validate());
            assert_eq!(d.get(xdict).unwrap().object_common().owner, Handle(0));
        }
    }

    /// An object with a null owner that two objects list (here an
    /// extension dictionary's entry, also another object's extension
    /// dictionary) is not given either as owner: `validate()` refuses it,
    /// and neither the write nor `write_diagnostics` reports a repair.
    #[test]
    fn null_owner_listed_twice_is_not_repaired() {
        let mut d = Drawing::new();
        let layers = d.header.layer_control;
        let xdict = d.get(layers).unwrap().object_common().xdict.unwrap();
        let Some(Object::Other {
            object: NonGraphical::Dictionary(x),
            ..
        }) = d.get(xdict)
        else {
            panic!("an extension dictionary");
        };
        let (_, states) = x.entries[0].clone();
        d.get_mut(states).unwrap().object_common_mut().owner = Handle(0);
        let linetypes = d.header.linetype_control;
        d.get_mut(linetypes).unwrap().object_common_mut().xdict = Some(states);
        let repaired = |diags: &[Diagnostic]| {
            diags
                .iter()
                .any(|w| w.handle == Some(states) && w.message.contains("owner is null"))
        };
        assert!(!repaired(&d.write_diagnostics()));
        let errors: Vec<_> = d
            .validate()
            .into_iter()
            .filter(|x| x.severity == Severity::Error && x.handle == Some(states))
            .collect();
        assert!(!errors.is_empty());
        assert!(matches!(d.to_bytes(), Err(Error::Invalid(_))));
    }

    /// When the file cannot be laid out (a summary title too long to
    /// encode), `write_diagnostics` still lists the warnings found before,
    /// with one more for the layout.
    #[test]
    fn layout_failure_keeps_earlier_warnings() {
        let mut d = Drawing::new();
        let layers = d.header.layer_control;
        let xdict = d.get(layers).unwrap().object_common().xdict.unwrap();
        d.get_mut(xdict).unwrap().object_common_mut().owner = Handle(0);
        let mut summary = d.summary.clone().unwrap_or_default();
        summary.title = "x".repeat(70_000);
        d.summary = Some(summary);
        let diags = d.write_diagnostics();
        assert!(
            diags.iter().any(|x| x.message.contains("null")),
            "{diags:#?}"
        );
        assert!(
            diags
                .iter()
                .any(|x| x.message.contains("cannot be laid out"))
        );
        assert!(d.to_bytes().is_err());
    }

    /// A VISUALSTYLE in an R2007 layout (raw, without the "saved by object
    /// version" marker) is not listed as unconvertible: the writer replaces
    /// it by AutoCAD's standard style of its name (without case), or by
    /// 2dWireframe's values under a custom name, keeping its handle and
    /// common data, with a warning each. The drawing keeps the raw object.
    #[test]
    fn old_visual_styles_are_replaced_at_save() {
        use crate::bits::BitWriter;
        use crate::object::RawObject;
        let mut d = Drawing::new();
        let styles: Vec<Handle> = d
            .objects
            .iter()
            .filter(|(_, o)| o.type_name() == "VISUALSTYLE")
            .map(|(&h, _)| h)
            .take(2)
            .collect();
        for (&h, name) in styles.iter().zip(["realistic", "My Style"]) {
            let mut sw = BitWriter::new();
            sw.write_tu(name).unwrap();
            let bits = sw.bit_len();
            let raw = RawObject {
                type_code: 500,
                class_name: Some("VISUALSTYLE".into()),
                source_version: Version::R2007,
                data: vec![0x55],
                data_bits: 7,
                strings: Some((sw.into_bytes(), bits)),
                handles: Vec::new(),
                tail_handles_raw: Vec::new(),
                tail_handle_bits: 0,
            };
            if let Some(Object::Other { object, .. }) = d.objects.get_mut(&h) {
                *object = NonGraphical::Unknown(raw);
            }
        }
        d.ref_index = Default::default();
        assert!(d.unconvertible_objects().is_empty());
        let before: Vec<Object> = styles.iter().map(|h| d.objects[h].clone()).collect();
        let (bytes, warnings) = d.to_bytes_with_diagnostics().unwrap();
        let named: Vec<Option<Handle>> = warnings.iter().map(|w| w.handle).collect();
        assert_eq!(named, styles.iter().map(|&h| Some(h)).collect::<Vec<_>>());
        assert!(warnings[0].message.contains("standard style"));
        assert!(warnings[1].message.contains("2dWireframe"));
        let back = Drawing::from_bytes(&bytes).unwrap();
        assert!(back.diagnostics().is_empty(), "{:?}", back.diagnostics());
        assert!(back.validate().is_empty(), "{:?}", back.validate());
        let wireframe = super::super::new::standard_visual_style("2dWireframe");
        let realistic = super::super::new::standard_visual_style("Realistic");
        for ((h, was), (name, like)) in styles
            .iter()
            .zip(&before)
            .zip([("realistic", realistic), ("My Style", wireframe)])
        {
            let Some(Object::Other {
                common,
                object: NonGraphical::VisualStyle(v),
            }) = back.get(*h)
            else {
                panic!("{h} not an R2018 visual style");
            };
            assert_eq!(common, was.object_common());
            assert_eq!(v.name, name);
            assert_eq!(
                (v.style_type, &v.properties),
                (like.style_type, &like.properties)
            );
        }
        // The drawing itself keeps the raw objects.
        assert!(styles.iter().all(|h| d.objects[h].raw().is_some()));
    }

    /// A replaced VISUALSTYLE keeps the refreshes made to its common data
    /// before (here its null owner, set to the hard-owner dictionary that
    /// lists it), not the common data as stored.
    #[test]
    fn replaced_visual_style_keeps_earlier_refreshes() {
        use crate::object::RawObject;
        let mut d = Drawing::new();
        let (dict, style) = d
            .objects
            .iter()
            .find_map(|(&h, o)| match o {
                Object::Other {
                    object: NonGraphical::Dictionary(x),
                    ..
                } => x
                    .entries
                    .iter()
                    .find(|&&(_, e)| d.objects[&e].type_name() == "VISUALSTYLE")
                    .map(|&(_, e)| (h, e)),
                _ => None,
            })
            .unwrap();
        if let Some(Object::Other {
            object: NonGraphical::Dictionary(x),
            ..
        }) = d.objects.get_mut(&dict)
        {
            x.hard_owner = 1;
        }
        let Some(Object::Other { common, object }) = d.objects.get_mut(&style) else {
            panic!()
        };
        common.owner = Handle(0);
        *object = NonGraphical::Unknown(RawObject {
            type_code: 500,
            class_name: Some("VISUALSTYLE".into()),
            source_version: Version::R2007,
            data: vec![0x55],
            data_bits: 7,
            strings: None,
            handles: Vec::new(),
            tail_handles_raw: Vec::new(),
            tail_handle_bits: 0,
        });
        d.ref_index = Default::default();
        let (bytes, warnings) = d.to_bytes_with_diagnostics().unwrap();
        assert!(
            warnings.iter().any(|w| w.message.contains("owner is null")),
            "{warnings:?}"
        );
        let back = Drawing::from_bytes(&bytes).unwrap();
        let Some(Object::Other {
            common,
            object: NonGraphical::VisualStyle(_),
        }) = back.get(style)
        else {
            panic!("{style} not an R2018 visual style");
        };
        assert_eq!(common.owner, dict);
        assert!(back.validate().is_empty(), "{:?}", back.validate());
    }

    #[test]
    fn julian_now_is_a_plausible_date() {
        let now = julian_now();
        // After 2026-01-01 (Julian day 2461042), within a day's milliseconds.
        assert!(now.day >= 2_461_042, "{now:?}");
        assert!((0..86_400_000).contains(&now.ms));
    }
}
