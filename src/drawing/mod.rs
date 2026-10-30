//! A whole drawing: header, classes and objects, read from a DWG file or
//! created empty ([`Drawing::new`], see `drawing::new`).
//!
//! [`Drawing::from_bytes`] slices the file into sections
//! ([`crate::container::read`]), decodes the header, the classes and the
//! object map, then every object the map lists. Problems that do not stop the
//! read become [`Diagnostic`]s:
//!
//! - container warnings (a bad data page checksum, an unreadable optional
//!   section);
//! - a drawing code page the crate cannot decode (R2000/R2004 text is then
//!   read as Windows-1252);
//! - header and classes warnings (lossy text, bits left unread);
//! - object map entries outside the objects section or inside another
//!   object, and duplicate handles (the first usable entry wins), skipped;
//! - objects whose common data cannot be read (an `Error`; kept as stored,
//!   see [`crate::object::UnreadableObject`], and unconvertible), or whose
//!   size cannot be read (an `Error`, skipped);
//! - objects of a class type with no class (kept raw);
//! - per-object warnings from [`crate::object::decode_object`];
//! - R2000 block records, inserts and polylines whose entity chain is
//!   broken (see `link_entity_lists`);
//! - R2000 plot views are resolved by name when read (see
//!   `resolve_plot_view_names`) and again when saved, where a name no VIEW
//!   has is cleared with a warning (see [`Drawing::write_diagnostics`]);
//! - a summary information, preview, application information or (R2013+)
//!   data storage section that cannot be decoded (kept as read in the extra
//!   sections).

mod api;
mod new;
pub(crate) mod refs;
mod repair;
mod rewrite;
mod validate;
mod write;

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use crate::bits::BitReader;
use crate::container::{self, SectionName};
use crate::diag::{Diagnostic, Severity};
use crate::error::{Error, Result};
use crate::handle::Handle;
use crate::object::{
    AcisPayload, DecodeCtx, Entity, EntityList, MInsert, MLineLinetype, NonGraphical, ObjReader,
    Object, TableKind, decode_object, type_code,
};
use crate::section::{
    AppInfo, DataStorage, DxfClass, HeaderVars, Preview, SummaryInfo, classes, header, objmap,
};
use crate::strings;
use crate::version::Version;

/// A DWG drawing.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Drawing {
    pub(crate) source_version: Option<Version>,
    /// The header variables ($ACADVER, $HANDSEED, $CLAYER, ...).
    pub header: HeaderVars,
    /// The custom classes (the CLASSES section): the class-numbered object types.
    pub classes: Vec<DxfClass>,
    /// The objects. Code that changes them other than through the
    /// mutation methods must reset `ref_index` (see `drawing::refs`).
    pub(crate) objects: BTreeMap<Handle, Object>,
    /// The reverse reference index, built on first use and kept up to date
    /// by the mutation methods (see `drawing::refs`). Not serialized: it is
    /// rebuilt on first use.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub(crate) ref_index: refs::IndexCell,
    /// The summary information (`AcDb:SummaryInfo`, R2004+).
    pub summary: Option<SummaryInfo>,
    /// The preview images.
    pub preview: Option<Preview>,
    /// The application that wrote the file (`AcDb:AppInfo`, R2004+).
    pub app_info: Option<AppInfo>,
    /// The data storage section (`AcDb:AcDsPrototype_1b`, R2013+): the
    /// ACIS data of REGION, 3DSOLID and BODY entities, keyed by handle (see
    /// [`Drawing::acis_data`]), and AutoCAD's block thumbnails. The writer
    /// adds the data of entities converted from older versions and drops
    /// the ACIS records of entities no longer in the drawing (see
    /// `drawing::write`). A handle rewrite must remap its records too
    /// ([`DataStorage::remap_handles`]).
    pub data_storage: Option<DataStorage>,
    /// Sections the drawing does not decode, kept as read, and a summary,
    /// preview or app info section that could not be decoded.
    pub(crate) extra_sections: BTreeMap<SectionName, Vec<u8>>,
    pub(crate) diagnostics: Vec<Diagnostic>,
    /// What the writer keeps from the file read (see `drawing::write`).
    /// Not serialized: a deserialized drawing is written like one built in
    /// memory.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub(crate) read: write::ReadState,
}

// A drawing can be shared between threads (Global Constraints).
const _: () = {
    const fn send_sync<T: Send + Sync>() {}
    send_sync::<Drawing>();
};

fn warning(handle: Option<Handle>, message: String) -> Diagnostic {
    Diagnostic {
        severity: Severity::Warning,
        handle,
        message,
    }
}

impl Drawing {
    /// Read a drawing from the bytes of a DWG file.
    ///
    /// A broken container (file header, section maps), header section,
    /// classes section or object map (any of its pages) fails the read.
    /// Objects that fail to decode are kept raw with warnings, and other
    /// problems that do not stop the read become diagnostics (see the
    /// module doc).
    pub fn from_bytes(bytes: &[u8]) -> Result<Drawing> {
        let mut raw = container::read(bytes)?;
        let version = raw.version;
        let codepage = raw.codepage;
        let mut diagnostics: Vec<Diagnostic> =
            raw.warnings.drain(..).map(|w| warning(None, w)).collect();
        diagnostics.extend(codepage_diagnostic(version, codepage));
        let mut take = |name: SectionName| raw.sections.remove(&name).unwrap_or_default();
        let header_bytes = take(SectionName::Header);
        let classes_bytes = take(SectionName::Classes);
        let map_bytes = take(SectionName::Handles);
        let objects_bytes = take(SectionName::Objects);
        let mut read = write::ReadState {
            file_header: raw.file_header,
            section_meta: std::mem::take(&mut raw.section_meta),
            preview_address: raw.preview_offset,
            ..Default::default()
        };

        let decoded = header::decode_with_diagnostics(&header_bytes, version, codepage)?;
        diagnostics.extend(decoded.diagnostics);
        let (classes, class_diags) =
            classes::decode_with_diagnostics(&classes_bytes, version, codepage)?;
        diagnostics.extend(class_diags);
        let map = objmap::decode(&map_bytes)?;

        let layout = MapLayout::new(&map, &objects_bytes, raw.objects_base, version);
        let mleader_style_marker =
            find_mleader_style_marker(&map, &layout, &objects_bytes, version, codepage, &classes);
        let mut ctx = DecodeCtx::new(version, codepage, &classes)
            .with_mleader_style_marker(mleader_style_marker);
        ctx.diagnostics = diagnostics;
        let mut objects = read_objects(&map, &objects_bytes, raw.objects_base, &layout, &mut ctx);
        let mut diagnostics = ctx.diagnostics;
        let (mut summary, preview, app_info) = decode_info_sections(
            &mut raw.sections,
            version,
            codepage,
            raw.preview_offset,
            &mut diagnostics,
        );
        let data_storage = decode_data_storage(
            &mut raw.sections,
            version,
            &objects,
            &mut read,
            &mut diagnostics,
        );
        link_entity_lists(&mut objects, &mut diagnostics);
        let mut vars = decoded.vars;
        vars.measurement = read_measurement(&raw.sections, version, codepage, &mut diagnostics);
        if let Some(s) = &mut summary {
            s.set_read_header(&vars);
        }
        if version < Version::R2018 {
            convert_mtext_columns(&mut objects);
            resolve_mline_style_linetypes(&mut objects);
        }
        if version < Version::R2004 {
            fill_layout_viewports(&mut objects);
            resolve_plot_view_names(&mut objects);
        }
        read.capture(&map, &objects, &classes, version);
        Ok(Drawing {
            source_version: Some(version),
            header: vars,
            classes,
            objects,
            ref_index: refs::IndexCell::default(),
            summary,
            preview,
            app_info,
            data_storage,
            extra_sections: raw.sections,
            diagnostics,
            read,
        })
    }

    /// Read a drawing from a DWG file.
    pub fn open(path: impl AsRef<Path>) -> Result<Drawing> {
        let bytes = std::fs::read(path).map_err(Error::Io)?;
        Drawing::from_bytes(&bytes)
    }

    /// The version of the file the drawing was read from.
    pub fn source_version(&self) -> Option<Version> {
        self.source_version
    }

    /// Problems found while reading.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// The object `h`; `None` when there is none.
    pub fn get(&self, h: Handle) -> Option<&Object> {
        self.objects.get(&h)
    }

    /// The object `h`, to change in place.
    ///
    /// Nothing is checked: a change here can break the drawing's structure
    /// (a layer reference to a missing layer, an entity listed by a block it
    /// does not name as owner, a handle changed, ...). The add and remove
    /// methods keep the structure consistent; `validate()` reports what a
    /// change through this method broke. The object's references are
    /// recomputed for [`Drawing::referrers`] at its next use.
    pub fn get_mut(&mut self, h: Handle) -> Option<&mut Object> {
        // The circular insert check's cache: a block record's list, or an
        // entity of its list, may change.
        let block = match self.objects.get(&h) {
            Some(Object::Entity { .. }) => self.owner_of(h),
            Some(_) => Some(h),
            None => None,
        };
        let state = self.ref_index.get_mut();
        state.mark_dirty(h);
        if let Some(block) = block {
            state.forget_inserts(block);
        }
        // The name index: a table control's or dictionary's list, or a
        // table entry's name, may change.
        state.forget_names(h);
        self.objects.get_mut(&h)
    }

    /// The sections the drawing keeps undecoded (AuxHeader, RevHistory,
    /// a data storage section that could not be decoded, ...), as read.
    pub fn extra_sections(&self) -> impl Iterator<Item = (&SectionName, &[u8])> {
        self.extra_sections.iter().map(|(n, b)| (n, b.as_slice()))
    }

    /// Every object, by handle.
    pub fn objects(&self) -> impl Iterator<Item = (Handle, &Object)> {
        self.objects.iter().map(|(&h, o)| (h, o))
    }

    /// The ACIS (SAB) data of the REGION, 3DSOLID or BODY `h`: the data it
    /// carries (read before R2013; R2000 SAT is encrypted text, not
    /// returned), or its record in [`Drawing::data_storage`] (R2013+).
    pub fn acis_data(&self, h: Handle) -> Option<&[u8]> {
        let Some(Object::Entity {
            entity: Entity::Acis(a),
            ..
        }) = self.objects.get(&h)
        else {
            return None;
        };
        match &a.payload {
            AcisPayload::Inline { data, .. } => Some(data),
            AcisPayload::DataStorage { .. } => self.data_storage.as_ref()?.acis_data(h),
            AcisPayload::Empty | AcisPayload::SatBlocks { .. } => None,
        }
    }
}

/// Decode the data storage section of an R2013+ drawing, removing it from
/// `sections` and keeping its bytes for the writer. One that cannot be
/// decoded stays in `sections`, with a warning. Older files store ACIS data
/// in the entities; a data storage section of theirs is left as read.
fn decode_data_storage(
    sections: &mut BTreeMap<SectionName, Vec<u8>>,
    version: Version,
    objects: &BTreeMap<Handle, Object>,
    read: &mut write::ReadState,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<DataStorage> {
    if version < Version::R2013 {
        return None;
    }
    // The R2013+-form ACIS entities without a record when read: they hold
    // no data (an entity converted from an empty one), which the writer
    // accepts (see `drawing::write`).
    let without_record = |s: Option<&DataStorage>| -> HashSet<Handle> {
        let schema = s.and_then(|s| s.schema_index(crate::section::acds::ACIS_SCHEMA));
        let linked: HashSet<Handle> = s.map_or_else(HashSet::new, |s| {
            s.records
                .iter()
                .filter(|r| Some(r.schema) == schema)
                .map(|r| r.handle)
                .collect()
        });
        objects
            .iter()
            .filter(|(h, o)| {
                matches!(
                    o,
                    Object::Entity { entity: Entity::Acis(a), .. }
                        if matches!(a.payload, AcisPayload::DataStorage { .. })
                ) && !linked.contains(h)
            })
            .map(|(&h, _)| h)
            .collect()
    };
    let Some(bytes) = sections.get(&SectionName::AcDsPrototype) else {
        read.acis_without_record = without_record(None);
        return None;
    };
    match DataStorage::parse(bytes) {
        Ok(s) => {
            read.acis_without_record = without_record(Some(&s));
            let bytes = sections.remove(&SectionName::AcDsPrototype)?;
            read.data_storage = Some((s.fingerprint(), bytes));
            Some(s)
        }
        Err(e) => {
            read.undecoded_storage_acis = objects
                .iter()
                .filter(|(_, o)| {
                    matches!(
                        o,
                        Object::Entity {
                            entity: Entity::Acis(_),
                            ..
                        }
                    )
                })
                .map(|(&h, _)| h)
                .collect();
            diagnostics.push(warning(
                None,
                format!("the data storage section could not be decoded ({e}); kept as read"),
            ));
            None
        }
    }
}

/// The APPID named ACAD_MLEADERVER, which marks pre-R2010 multileader
/// styles in the new format (see [`DecodeCtx::mleader_style_marker`]); R2010+
/// files need none. The type of every object [`marker_candidates`] gives is
/// read and the APPIDs decoded; their problems are reported when the objects
/// are read.
fn find_mleader_style_marker(
    map: &[(Handle, u64)],
    layout: &MapLayout,
    objects: &[u8],
    version: Version,
    codepage: u16,
    classes: &[DxfClass],
) -> Option<Handle> {
    if version >= Version::R2010 {
        return None;
    }
    marker_candidates(map, layout).find_map(|(h, off)| {
        let at = &objects[off..];
        let (_, pre) = ObjReader::split(at, version, codepage).ok()?;
        if pre.type_code != type_code::APPID {
            return None;
        }
        let mut ctx = DecodeCtx::new(version, codepage, classes);
        match decode_object(at, h, &mut ctx).ok()? {
            Object::Other {
                object: NonGraphical::AppId(a),
                ..
            } if a.entry.name.eq_ignore_ascii_case("ACAD_MLEADERVER") => Some(h),
            _ => None,
        }
    })
}

/// The map entries [`find_mleader_style_marker`] reads, with their offsets:
/// those [`read_objects`] would decode by location (inside the objects
/// section, not inside another object), each offset once. Every object's
/// bytes are then read (its CRC computed) at most once, however many entries
/// point at or into it.
fn marker_candidates<'a>(
    map: &'a [(Handle, u64)],
    layout: &'a MapLayout,
) -> impl Iterator<Item = (Handle, usize)> + 'a {
    let mut seen = HashSet::new();
    map.iter()
        .zip(layout.offsets.iter().zip(&layout.inside))
        .filter_map(move |(&(h, _), (&off, inside))| {
            let off = off?;
            (inside.is_none() && seen.insert(off)).then_some((h, off))
        })
}

/// $MEASUREMENT, which the `AcDb:Template` section stores (ODS 22; the
/// header section does not); `None` without one, and with a warning when
/// it cannot be read (the section is kept as read).
fn read_measurement(
    sections: &BTreeMap<SectionName, Vec<u8>>,
    version: Version,
    codepage: u16,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<i16> {
    let b = sections.get(&SectionName::Template)?;
    let m = write::parse_template(b, version >= Version::R2007, codepage)
        .and_then(|(_, m)| i16::try_from(m).ok());
    if m.is_none() {
        diagnostics.push(warning(
            None,
            "the Template section cannot be read; $MEASUREMENT is unknown".into(),
        ));
    }
    m
}

/// Decode the summary information, preview and application information
/// sections, removing each one decoded from `sections`. A section that
/// cannot be decoded stays in `sections`, with a warning.
fn decode_info_sections(
    sections: &mut BTreeMap<SectionName, Vec<u8>>,
    version: Version,
    codepage: u16,
    preview_address: Option<u64>,
    diagnostics: &mut Vec<Diagnostic>,
) -> (Option<SummaryInfo>, Option<Preview>, Option<AppInfo>) {
    fn take<T>(
        sections: &mut BTreeMap<SectionName, Vec<u8>>,
        name: SectionName,
        diagnostics: &mut Vec<Diagnostic>,
        decode: impl FnOnce(&[u8], &mut Vec<Diagnostic>) -> Result<T>,
    ) -> Option<T> {
        let bytes = sections.get(&name)?;
        match decode(bytes, diagnostics) {
            Ok(v) => {
                sections.remove(&name);
                Some(v)
            }
            Err(e) => {
                diagnostics.push(warning(
                    None,
                    format!("{name:?} section cannot be decoded ({e}); kept as read"),
                ));
                None
            }
        }
    }
    let summary = take(sections, SectionName::SummaryInfo, diagnostics, |b, d| {
        SummaryInfo::decode(b, version, codepage, d)
    });
    let preview = take(sections, SectionName::Preview, diagnostics, |b, _| {
        Preview::decode(b, preview_address)
    });
    let app_info = take(sections, SectionName::AppInfo, diagnostics, AppInfo::decode);
    (summary, preview, app_info)
}

/// A warning when the drawing code page cannot be decoded. From R2007 text
/// is UTF-16, so the code page decodes nothing and no warning is given.
fn codepage_diagnostic(version: Version, codepage: u16) -> Option<Diagnostic> {
    (version < Version::R2007 && !strings::is_supported(codepage)).then(|| {
        warning(
            None,
            format!("drawing code page {codepage} is not supported; text is read as Windows-1252"),
        )
    })
}

/// Move the columns of pre-R2018 MTEXTs from the ACAD application's
/// extended data into their R2018 context, as AutoCAD 2018 converts them
/// (see `object::entities::text::take_column_xdata`).
fn convert_mtext_columns(objects: &mut BTreeMap<Handle, Object>) {
    let Some(acad) = objects.iter().find_map(|(&h, o)| match o {
        Object::Other {
            object: NonGraphical::AppId(a),
            ..
        } if a.entry.name.eq_ignore_ascii_case("ACAD") => Some(h),
        _ => None,
    }) else {
        return;
    };
    for obj in objects.values_mut() {
        if let Object::Entity {
            common,
            entity: Entity::MText(m),
        } = obj
        {
            crate::object::entities::text::take_column_xdata(&mut common.obj.xdata, acad, m);
        }
    }
}

/// Replace the pre-R2018 linetype indexes of multiline style elements with
/// the linetype handles R2018 stores: 32767 is BYLAYER and 32766 BYBLOCK (as
/// ACadSharp (MIT) `CadMLineStyleTemplate` reads them; the corpus has 32767
/// where its R2018 sample has BYLAYER), taken from the LTYPE control's extra
/// references (`[BYBLOCK, BYLAYER]`, see [`crate::object::TableControl`]).
/// Other indexes are left as they are (see [`MLineLinetype::Index`]), and
/// such a style cannot be written.
fn resolve_mline_style_linetypes(objects: &mut BTreeMap<Handle, Object>) {
    let Some((by_block, by_layer)) = objects.values().find_map(|o| match o {
        Object::Other {
            object: NonGraphical::TableControl(c),
            ..
        } if c.kind == TableKind::Linetype => match c.extra.as_slice() {
            [block, layer] => Some((*block, *layer)),
            _ => None,
        },
        _ => None,
    }) else {
        return;
    };
    for obj in objects.values_mut() {
        let Object::Other {
            object: NonGraphical::MLineStyle(m),
            ..
        } = obj
        else {
            continue;
        };
        for e in &mut m.elements {
            let resolved = match e.linetype {
                MLineLinetype::Index(32767) => by_layer,
                MLineLinetype::Index(32766) => by_block,
                _ => continue,
            };
            e.linetype = MLineLinetype::Handle(resolved);
        }
    }
}

/// Objects by handle: a drawing's, or those plus the objects the writer adds
/// (see `Changes::stored`), for the passes that read either.
pub(crate) trait ObjectSet {
    /// Object `h`.
    fn object(&self, h: Handle) -> Option<&Object>;
    /// Every object, in handle order.
    fn each(&self) -> impl Iterator<Item = (Handle, &Object)>;
}

impl ObjectSet for BTreeMap<Handle, Object> {
    fn object(&self, h: Handle) -> Option<&Object> {
        self.get(&h)
    }

    fn each(&self) -> impl Iterator<Item = (Handle, &Object)> {
        self.iter().map(|(&h, o)| (h, o))
    }
}

/// The handle of each VIEW, by lowercase name.
pub(crate) fn views_by_name(objects: &impl ObjectSet) -> std::collections::HashMap<String, Handle> {
    objects
        .each()
        .filter_map(|(h, o)| match o {
            Object::Other {
                object: NonGraphical::View(v),
                ..
            } => Some((v.entry.name.to_lowercase(), h)),
            _ => None,
        })
        .collect()
}

/// R2000 plot settings name their plot view; R2004+ store its handle
/// ([`crate::object::PlotSettings::plot_view`]). Give each layout and plot
/// settings object with a name and no handle the handle of the VIEW of that
/// name (compared without case, as AutoCAD looks names up). A name no view
/// has is left; the writer resolves it again when saving (a view added or
/// renamed since may have it) and clears it if it still does not resolve.
fn resolve_plot_view_names(objects: &mut BTreeMap<Handle, Object>) {
    let views = views_by_name(objects);
    if views.is_empty() {
        return;
    }
    for obj in objects.values_mut() {
        let plot = match obj {
            Object::Other {
                object: NonGraphical::Layout(l),
                ..
            } => &mut l.plot,
            Object::Other {
                object: NonGraphical::PlotSettings(p),
                ..
            } => &mut **p,
            _ => continue,
        };
        if plot.plot_view.0 == 0 && !plot.plot_view_name.is_empty() {
            if let Some(&h) = views.get(&plot.plot_view_name.to_lowercase()) {
                plot.plot_view = h;
            }
        }
    }
}

/// The VIEWPORT entities of block record `block`, in its entity order, or
/// `None` when it is not a block record with a built entity list. A
/// layout's viewport list holds these (checked on the corpus).
pub(crate) fn block_viewports(objects: &impl ObjectSet, block: Handle) -> Option<Vec<Handle>> {
    let Some(Object::Other {
        object: NonGraphical::BlockRecord(b),
        ..
    }) = objects.object(block)
    else {
        return None;
    };
    let EntityList::Owned(list) = &b.entities else {
        return None;
    };
    Some(
        list.iter()
            .copied()
            .filter(|h| {
                matches!(
                    objects.object(*h),
                    Some(Object::Entity {
                        entity: Entity::Viewport(_),
                        ..
                    })
                )
            })
            .collect(),
    )
}

/// Each layout whose block record names it back (the block's `layout`),
/// with that block's VIEWPORT entities ([`block_viewports`]), and the number
/// of block entity list entries walked. A block names one layout, so each
/// block's list is walked at most once however many layouts claim it (a
/// hostile file can make many share one large block): the work and the
/// lists are linear in the objects. Layouts their block does not name back
/// are left out.
pub(crate) fn own_layout_viewports(
    objects: &impl ObjectSet,
) -> (Vec<(Handle, Vec<Handle>)>, usize) {
    let mut steps = 0;
    let mut out = Vec::new();
    for (h, o) in objects.each() {
        let Object::Other {
            object: NonGraphical::Layout(l),
            ..
        } = o
        else {
            continue;
        };
        let Some(Object::Other {
            object: NonGraphical::BlockRecord(b),
            ..
        }) = objects.object(l.block_record)
        else {
            continue;
        };
        if b.layout != h {
            continue;
        }
        if let EntityList::Owned(list) = &b.entities {
            steps += list.len();
        }
        if let Some(v) = block_viewports(objects, l.block_record) {
            out.push((h, v));
        }
    }
    (out, steps)
}

/// R2000 layouts store no viewport list; fill it in from the layout's block
/// record, as AutoCAD writes it in later versions (for layouts their block
/// names back, see [`own_layout_viewports`]). Must run after the entity
/// lists are built.
fn fill_layout_viewports(objects: &mut BTreeMap<Handle, Object>) {
    let (lists, _) = own_layout_viewports(objects);
    for (h, list) in lists {
        if let Some(Object::Other {
            object: NonGraphical::Layout(l),
            ..
        }) = objects.get_mut(&h)
        {
            if l.viewports.is_empty() {
                l.viewports = list;
            }
        }
    }
}

/// Which kind of owner an R2000 entity chain belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChainOwner {
    /// A block record's entities.
    Block,
    /// An INSERT's or MINSERT's attributes.
    Insert,
    /// A 2D, 3D, polyface mesh or polygon mesh POLYLINE's vertices.
    Polyline,
}

/// The R2000 first/last pair of a block record's entities, an insert's
/// attributes or a polyline's vertices, if not yet a list.
fn linked_pair(obj: &Object) -> Option<(ChainOwner, Handle, Handle)> {
    let (kind, list) = match obj {
        Object::Other {
            object: NonGraphical::BlockRecord(b),
            ..
        } => (ChainOwner::Block, &b.entities),
        Object::Entity {
            entity: Entity::Insert(i) | Entity::MInsert(MInsert { insert: i, .. }),
            ..
        } => (ChainOwner::Insert, &i.attribs),
        Object::Entity { entity, .. } => (ChainOwner::Polyline, entity.polyline_vertices()?),
        _ => return None,
    };
    match *list {
        EntityList::Linked { first, last } => Some((kind, first, last)),
        EntityList::Owned(_) => None,
    }
}

/// Replace the first/last entity pairs of R2000 block records, inserts and
/// polylines ([`EntityList::Linked`]) with the full lists R2004+ stores, so
/// they can be written as R2018. Must run after every object is read. Returns the number
/// of chain steps taken (see [`ChainWalker`]).
///
/// A list follows each entity's next link from the first entity to the
/// last. An R2000 entity whose `nolinks` bit is set stores no links: its
/// neighbours are the handles one below and one above (ODS 20.4.1). When a
/// chain is broken (a missing or non-entity object, a null link, a loop, an
/// entity already in another list, or the walk budget used up) the owner
/// keeps its pair, a warning is added, and writing it fails.
fn link_entity_lists(
    objects: &mut BTreeMap<Handle, Object>,
    diagnostics: &mut Vec<Diagnostic>,
) -> usize {
    let linked: Vec<(Handle, ChainOwner, Handle, Handle)> = objects
        .iter()
        .filter_map(|(&h, o)| linked_pair(o).map(|(kind, first, last)| (h, kind, first, last)))
        .collect();
    // A block's entities are one kind of list; the entities a SEQEND ends
    // (attributes and vertices) are the other: one walker each.
    let mut blocks = ChainWalker::new(objects.len());
    let mut sequences = ChainWalker::new(objects.len());
    for (owner, kind, first, last) in linked {
        let (walker, what) = match kind {
            ChainOwner::Block => (&mut blocks, "block record entity"),
            ChainOwner::Insert => (&mut sequences, "insert attribute"),
            ChainOwner::Polyline => (&mut sequences, "polyline vertex"),
        };
        let list = match walker.walk(objects, first, last) {
            Ok(list) => list,
            Err(why) => {
                diagnostics.push(warning(
                    Some(owner),
                    format!(
                        "{what} chain from {first} to {last} is broken {why}; the list is not \
                         built and the object cannot be written"
                    ),
                ));
                continue;
            }
        };
        match objects.get_mut(&owner) {
            Some(Object::Other {
                object: NonGraphical::BlockRecord(b),
                ..
            }) => b.entities = EntityList::Owned(list),
            Some(Object::Entity {
                entity: Entity::Insert(i) | Entity::MInsert(MInsert { insert: i, .. }),
                ..
            }) => i.attribs = EntityList::Owned(list),
            Some(Object::Entity { entity, .. }) => {
                if let Some(vertices) = entity.polyline_vertices_mut() {
                    *vertices = EntityList::Owned(list);
                }
            }
            _ => {}
        }
    }
    blocks.steps + sequences.steps
}

/// Walks R2000 entity chains of one kind (block entities, or the attributes
/// and vertices a SEQEND ends).
/// An entity in a built list is never walked again. A failed walk releases
/// its entities, so it cannot spoil a valid chain that shares them; to keep
/// the work linear for hostile files that make many walks fail over the same
/// entities, the walks of one walker take at most `4 * objects + 64` steps
/// (valid chains are disjoint and take at most one step per object). With
/// the two walkers [`link_entity_lists`] uses, that is at most
/// `8 * objects + 128` steps in all.
struct ChainWalker {
    visited: HashSet<Handle>,
    steps: usize,
    budget: usize,
}

impl ChainWalker {
    fn new(objects: usize) -> Self {
        ChainWalker {
            visited: HashSet::new(),
            steps: 0,
            budget: objects.saturating_mul(4).saturating_add(64),
        }
    }

    /// The entities from `first` to `last` along their next links, or why
    /// the chain breaks. Both null means an empty list.
    fn walk(
        &mut self,
        objects: &BTreeMap<Handle, Object>,
        first: Handle,
        last: Handle,
    ) -> std::result::Result<Vec<Handle>, String> {
        if first.0 == 0 && last.0 == 0 {
            return Ok(Vec::new());
        }
        let mut list = Vec::new();
        let result = self.follow(objects, first, last, &mut list);
        if result.is_err() {
            for h in &list {
                self.visited.remove(h);
            }
        }
        result.map(|()| list)
    }

    fn follow(
        &mut self,
        objects: &BTreeMap<Handle, Object>,
        first: Handle,
        last: Handle,
        list: &mut Vec<Handle>,
    ) -> std::result::Result<(), String> {
        let mut cur = first;
        loop {
            if self.steps >= self.budget {
                return Err(format!("at {cur}: the walk budget is used up"));
            }
            self.steps += 1;
            let Some(Object::Entity { common, .. }) = objects.get(&cur) else {
                return Err(format!("at {cur}"));
            };
            if !self.visited.insert(cur) {
                return Err(format!("at {cur}"));
            }
            list.push(cur);
            if cur == last {
                return Ok(());
            }
            let next = match common.prev_next {
                Some((_, next)) => next,
                None => match cur.0.checked_add(1) {
                    Some(n) => Handle(n),
                    None => return Err(format!("at {cur}")),
                },
            };
            if next.0 == 0 {
                return Err(format!("at {cur}"));
            }
            cur = next;
        }
    }
}

/// The end (exclusive) of the object at `off`: its size fields, `MS` bytes
/// and CRC, or `None` when its size fields cannot be read or it runs past
/// `objects`.
fn object_end(objects: &[u8], off: usize, version: Version) -> Option<usize> {
    let mut r = BitReader::new(objects.get(off..)?);
    let size = r.read_ms().ok()?;
    if version >= Version::R2010 {
        r.read_umc().ok()?;
    }
    let end = off
        .checked_add((r.bit_pos() / 8) as usize)?
        .checked_add(usize::try_from(size).ok()?)?
        .checked_add(2)?;
    (end <= objects.len()).then_some(end)
}

/// For each entry of `offsets`, the offset of an earlier object (in offset
/// order) it lies inside, or `None`. `object_end` gives the end of the
/// object at an offset; it is called only for entries not inside another
/// object, once each, so entries pointing into one large object cost no
/// more than sorting them.
fn containing_objects(
    offsets: &[Option<usize>],
    mut object_end: impl FnMut(usize) -> Option<usize>,
) -> Vec<Option<usize>> {
    let mut order: Vec<(usize, usize)> = offsets
        .iter()
        .enumerate()
        .filter_map(|(i, o)| o.map(|o| (o, i)))
        .collect();
    order.sort_unstable();
    let mut inside = vec![None; offsets.len()];
    let (mut reach, mut reach_from) = (0usize, 0usize);
    for (off, i) in order {
        if off < reach {
            inside[i] = Some(reach_from);
        } else if let Some(end) = object_end(off) {
            (reach, reach_from) = (end, off);
        }
    }
    inside
}

/// The object at `off`, whose common data could not be read (`error`), kept
/// as stored ([`crate::object::UnreadableObject`]); `None` when its size
/// fields cannot be read or it runs past `objects` (there are no bits to
/// keep).
fn unreadable(
    objects: &[u8],
    off: usize,
    h: Handle,
    ctx: &DecodeCtx,
    error: &Error,
) -> Option<Object> {
    let end = object_end(objects, off, ctx.version)?;
    let bytes = objects.get(off..end)?.to_vec();
    let type_code = ObjReader::split(&bytes, ctx.version, ctx.codepage)
        .ok()
        .map(|(_, pre)| pre.type_code);
    let class_name = type_code
        .filter(|&c| c >= crate::section::classes::FIRST_CLASS_NUMBER)
        .and_then(|c| ctx.classes.iter().find(|k| k.class_number == c))
        .map(|k| k.dxf_name.clone());
    Some(Object::Other {
        common: crate::object::ObjectCommon {
            handle: h,
            ..crate::object::ObjectCommon::new()
        },
        object: NonGraphical::Unreadable(Box::new(crate::object::UnreadableObject {
            type_code,
            class_name,
            source_version: ctx.version,
            bytes,
            error: error.to_string(),
        })),
    })
}

/// Where the object map's entries point, computed once for both passes over
/// the objects ([`find_mleader_style_marker`] and [`read_objects`]): for
/// each entry, its offset in the objects section (`None` outside it) and
/// the offset of an earlier object it lies inside (see
/// [`containing_objects`]).
struct MapLayout {
    offsets: Vec<Option<usize>>,
    inside: Vec<Option<usize>>,
}

impl MapLayout {
    /// `base` is the location of `objects[0]` in the map's terms.
    fn new(map: &[(Handle, u64)], objects: &[u8], base: u64, version: Version) -> MapLayout {
        let offsets: Vec<Option<usize>> = map
            .iter()
            .map(|&(_, loc)| {
                loc.checked_sub(base)
                    .and_then(|o| usize::try_from(o).ok())
                    .filter(|&o| o < objects.len())
            })
            .collect();
        let inside = containing_objects(&offsets, |off| object_end(objects, off, version));
        MapLayout { offsets, inside }
    }
}

/// Decode the objects `map` lists, laid out as `layout` says. `base` is the
/// location of `objects[0]` in
/// the map's terms. Entries that cannot be used are skipped with a
/// diagnostic whose message contains "skipped":
///
/// - a location outside the objects section;
/// - a location inside another entry's object (objects never overlap). In
///   location order the first entry is kept. This bounds the work: every
///   decoded object covers its own bytes, so a map whose entries all point
///   into one large object costs no more than that object;
/// - a handle already decoded (the first usable entry wins);
/// - an object whose size fields cannot be read (an `Error`).
///
/// An object whose common data cannot be read is kept as stored
/// ([`crate::object::UnreadableObject`], an `Error` diagnostic): it cannot
/// be written, and the caller decides (remove it, or not save).
fn read_objects(
    map: &[(Handle, u64)],
    objects: &[u8],
    base: u64,
    layout: &MapLayout,
    ctx: &mut DecodeCtx,
) -> BTreeMap<Handle, Object> {
    let MapLayout { offsets, inside } = layout;
    let mut out = BTreeMap::new();
    for (i, &(h, loc)) in map.iter().enumerate() {
        let Some(off) = offsets[i] else {
            ctx.diagnostics.push(warning(
                Some(h),
                format!("object location {loc} is outside the objects section; skipped"),
            ));
            continue;
        };
        if let Some(outer) = inside[i] {
            ctx.diagnostics.push(warning(
                Some(h),
                format!(
                    "object location {loc} lies inside the object at location {}; skipped",
                    base.saturating_add(outer as u64)
                ),
            ));
            continue;
        }
        if out.contains_key(&h) {
            ctx.diagnostics.push(warning(
                Some(h),
                format!("duplicate object map entry at {loc} skipped; the first one is kept"),
            ));
            continue;
        }
        match decode_object(&objects[off..], h, ctx) {
            Ok(obj) => {
                out.insert(h, obj);
            }
            Err(e) => match unreadable(objects, off, h, ctx, &e) {
                Some(obj) => {
                    ctx.diagnostics.push(Diagnostic {
                        severity: Severity::Error,
                        handle: Some(h),
                        message: format!(
                            "object unreadable, kept as stored; it cannot be written: {e}"
                        ),
                    });
                    out.insert(h, obj);
                }
                None => ctx.diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    handle: Some(h),
                    message: format!("object unreadable, skipped (its size is unreadable): {e}"),
                }),
            },
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::{ObjWriter, ObjectCommon, encode_object_common, type_code};

    /// $MEASUREMENT comes from the Template section; one that cannot be
    /// read gives a warning and no value.
    #[test]
    fn measurement_from_the_template_section() {
        let mut d = Vec::new();
        let mut sections = BTreeMap::new();
        assert_eq!(
            read_measurement(&sections, Version::R2018, 30, &mut d),
            None
        );
        assert!(d.is_empty());
        // R2007+: a UTF-16 description "" (count 1, a NUL), then the value.
        sections.insert(SectionName::Template, vec![1, 0, 0, 0, 1, 0]);
        assert_eq!(
            read_measurement(&sections, Version::R2018, 30, &mut d),
            Some(1)
        );
        assert!(d.is_empty());
        sections.insert(SectionName::Template, vec![9, 0, 0]);
        assert_eq!(
            read_measurement(&sections, Version::R2018, 30, &mut d),
            None
        );
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].severity, Severity::Warning);
    }

    fn other(object: NonGraphical) -> Object {
        Object::Other {
            common: ObjectCommon {
                handle: Handle(0),
                owner: Handle(0),
                owner_relative: false,
                reactors: vec![],
                xdict: None,
                xdata: vec![],
                has_ds_binary_data: false,
            },
            object,
        }
    }

    /// Pre-R2018 multiline style linetype indexes: 32767 becomes the
    /// BYLAYER linetype and 32766 BYBLOCK, the LTYPE control's second and
    /// first extra references; other indexes stay, and so does every index
    /// without a linetype control.
    #[test]
    fn mline_style_linetype_indexes_are_resolved() {
        use crate::bits::Color;
        use crate::object::{MLineStyle, MLineStyleElement, TableControl};
        let style = |indexes: &[i16]| {
            other(NonGraphical::MLineStyle(MLineStyle {
                name: "S".into(),
                description: String::new(),
                flags: 0,
                fill_color: Color::ByLayer,
                start_angle: 0.0,
                end_angle: 0.0,
                elements: indexes
                    .iter()
                    .map(|&i| MLineStyleElement {
                        offset: 0.0,
                        color: Color::ByLayer,
                        linetype: MLineLinetype::Index(i),
                    })
                    .collect(),
            }))
        };
        let linetypes = |o: &Object| match o {
            Object::Other {
                object: NonGraphical::MLineStyle(m),
                ..
            } => m.elements.iter().map(|e| e.linetype).collect::<Vec<_>>(),
            _ => unreachable!(),
        };
        let control = other(NonGraphical::TableControl(TableControl {
            kind: TableKind::Linetype,
            entries: vec![Handle(0x16)],
            extra: vec![Handle(0x14), Handle(0x15)],
        }));
        let mut objects = BTreeMap::from([
            (Handle(0x5), control),
            (Handle(0x18), style(&[32767, 32766, 3])),
        ]);
        resolve_mline_style_linetypes(&mut objects);
        assert_eq!(
            linetypes(&objects[&Handle(0x18)]),
            [
                MLineLinetype::Handle(Handle(0x15)),
                MLineLinetype::Handle(Handle(0x14)),
                MLineLinetype::Index(3)
            ]
        );
        let mut objects = BTreeMap::from([(Handle(0x18), style(&[32767]))]);
        resolve_mline_style_linetypes(&mut objects);
        assert_eq!(
            linetypes(&objects[&Handle(0x18)]),
            [MLineLinetype::Index(32767)]
        );
    }

    /// An R2000 layout's plot view named (no handle) gets the handle of
    /// the view of that name, compared without case; a name no view has is
    /// left (the writer resolves it again when saving).
    #[test]
    fn r2000_plot_view_names_are_resolved() {
        use crate::object::tables::{Lighting, ViewMode};
        use crate::object::{Layout, TableEntry, View};
        let view = |name: &str| {
            other(NonGraphical::View(Box::new(View {
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
            })))
        };
        let layout = |name: &str| {
            let mut l = Layout::default();
            l.plot.plot_view_name = name.into();
            other(NonGraphical::Layout(Box::new(l)))
        };
        let mut objects = BTreeMap::from([
            (Handle(0x30), view("Front")),
            (Handle(0x40), layout("FRONT")),
            (Handle(0x41), layout("Missing")),
            (Handle(0x42), layout("")),
        ]);
        resolve_plot_view_names(&mut objects);
        let plot_view = |h: u64| match &objects[&Handle(h)] {
            Object::Other {
                object: NonGraphical::Layout(l),
                ..
            } => l.plot.plot_view,
            _ => unreachable!(),
        };
        assert_eq!(plot_view(0x40), Handle(0x30));
        assert_eq!(plot_view(0x41), Handle(0));
        assert_eq!(plot_view(0x42), Handle(0));
    }

    #[test]
    fn drawing_is_send_sync() {
        fn a<T: Send + Sync>() {}
        a::<Drawing>();
        a::<Object>();
    }

    fn dictionary(handle: u64) -> Vec<u8> {
        let mut w = ObjWriter::new(Handle(handle));
        let c = ObjectCommon {
            handle: Handle(handle),
            owner: Handle(0xC),
            owner_relative: false,
            reactors: vec![],
            xdict: None,
            xdata: vec![],
            has_ds_binary_data: false,
        };
        encode_object_common(&mut w, &c).unwrap();
        // An empty dictionary: no entries, cloning flag 1, not hard owner.
        w.data.write_bl(0);
        w.data.write_bs(1);
        w.data.write_rc(0);
        w.finish(type_code::DICTIONARY).unwrap()
    }

    fn ctx(classes: &[DxfClass]) -> DecodeCtx<'_> {
        DecodeCtx::new(Version::R2018, 30, classes)
    }

    #[test]
    fn duplicate_and_out_of_range_entries_are_skipped() {
        let a = dictionary(0x10);
        let b = dictionary(0x11);
        let c2 = dictionary(0x10);
        let mut objects = a.clone();
        objects.extend(&b);
        objects.extend(&c2);
        let map = [
            (Handle(0x10), 0),
            (Handle(0x11), a.len() as u64),
            (Handle(0x10), (a.len() + b.len()) as u64), // duplicate: the first wins
            (Handle(0x12), objects.len() as u64),
            (Handle(0x13), u64::MAX),
        ];
        let mut c = ctx(&[]);
        let out = read_all(&map, &objects, 0, &mut c);
        assert_eq!(
            out.keys().copied().collect::<Vec<_>>(),
            [Handle(0x10), Handle(0x11)]
        );
        let skipped: Vec<_> = c.diagnostics.iter().map(|d| d.handle).collect();
        assert_eq!(
            skipped,
            [Some(Handle(0x10)), Some(Handle(0x12)), Some(Handle(0x13))]
        );
        assert!(c.diagnostics.iter().all(|d| d.message.contains("skipped")));
        assert!(
            c.diagnostics
                .iter()
                .all(|d| d.severity == Severity::Warning)
        );
    }

    #[test]
    fn first_usable_duplicate_wins() {
        // The first entry for 0x10 is unreadable; the second is used.
        let mut objects = vec![0xFF, 0xFF, 0xFF, 0xFF];
        let good = dictionary(0x10);
        objects.extend(&good);
        let map = [(Handle(0x10), 0), (Handle(0x10), 4)];
        let mut c = ctx(&[]);
        let out = read_all(&map, &objects, 0, &mut c);
        assert_eq!(out.len(), 1);
        assert_eq!(c.diagnostics.len(), 1);
        assert_eq!(c.diagnostics[0].severity, Severity::Error);
    }

    /// [`read_objects`] with the map's layout computed.
    fn read_all(
        map: &[(Handle, u64)],
        objects: &[u8],
        base: u64,
        ctx: &mut DecodeCtx,
    ) -> BTreeMap<Handle, Object> {
        let layout = MapLayout::new(map, objects, base, ctx.version);
        read_objects(map, objects, base, &layout, ctx)
    }

    /// Before R2010 the ACAD_MLEADERVER search reads objects too: it skips
    /// the entries pointing at or into another object, as the object pass
    /// does, so a map of many entries into one large object reads that
    /// object once (not once per entry, a CRC over it each time).
    #[test]
    fn marker_search_skips_entries_inside_another_object() {
        use crate::bits::BitWriter;
        use crate::handle::HandleRef;
        use crate::object::stream::tests::assemble;
        let mut d = BitWriter::new();
        d.write_handle_ref(HandleRef {
            code: 0,
            value: 0x10,
        })
        .unwrap();
        d.write_bl(0); // reactors
        d.write_bytes(&vec![0x55; 100_000]);
        let mut hs = BitWriter::new();
        hs.write_handle_ref(HandleRef { code: 4, value: 0 })
            .unwrap();
        let objects = assemble(Version::R2004, type_code::VBA_PROJECT, &d, None, &hs);
        let mut map = vec![(Handle(0x10), 0)];
        for i in 0..50_000u64 {
            // At the object itself, or inside it.
            map.push((Handle(0x100 + i), i % 2 * (i % 90_000)));
        }
        map.push((Handle(0x9_0000), u64::MAX)); // outside the section
        let layout = MapLayout::new(&map, &objects, 0, Version::R2004);
        assert_eq!(
            marker_candidates(&map, &layout).collect::<Vec<_>>(),
            [(Handle(0x10), 0)]
        );
        assert_eq!(
            find_mleader_style_marker(&map, &layout, &objects, Version::R2004, 30, &[]),
            None
        );
    }

    #[test]
    fn entries_inside_another_object_are_skipped() {
        // One large object; many entries pointing at it or into it. Only
        // the first is decoded, so the work stays linear.
        let mut w = ObjWriter::new(Handle(0x10));
        let common = ObjectCommon {
            handle: Handle(0x10),
            owner: Handle(0xC),
            owner_relative: false,
            reactors: vec![],
            xdict: None,
            xdata: vec![],
            has_ds_binary_data: false,
        };
        encode_object_common(&mut w, &common).unwrap();
        w.data.write_bytes(&vec![0x55; 100_000]);
        let objects = w.finish(type_code::VBA_PROJECT).unwrap();
        let mut map = vec![(Handle(0x10), 0)];
        for i in 0..50_000u64 {
            map.push((Handle(0x100 + i), i % 2 * (i % 90_000)));
        }
        // Work count: only the one real object is measured...
        let offsets: Vec<Option<usize>> = map.iter().map(|&(_, l)| Some(l as usize)).collect();
        let mut measured = 0;
        let inside = containing_objects(&offsets, |off| {
            measured += 1;
            object_end(&objects, off, Version::R2018)
        });
        assert_eq!(measured, 1);
        assert_eq!(inside.iter().filter(|i| i.is_some()).count(), 50_000);
        // ...and only it is decoded: every other entry is skipped unread.
        let mut c = ctx(&[]);
        let out = read_all(&map, &objects, 0, &mut c);
        assert_eq!(out.len(), 1);
        assert_eq!(c.diagnostics.len(), 50_000);
        assert!(c.diagnostics.iter().all(|d| {
            d.message
                .contains("inside the object at location 0; skipped")
        }));
    }

    #[test]
    fn unreadable_object_is_an_error_and_skipped() {
        let mut objects = dictionary(0x10);
        objects.truncate(objects.len() - 3);
        let mut c = ctx(&[]);
        let out = read_all(&[(Handle(0x10), 0)], &objects, 0, &mut c);
        assert!(out.is_empty());
        assert_eq!(c.diagnostics.len(), 1);
        assert_eq!(c.diagnostics[0].severity, Severity::Error);
        assert!(c.diagnostics[0].message.contains("skipped"));
    }

    /// An object whose size is readable but whose common data is not (here
    /// no handle stream at all) is kept as stored, unconvertible, with an
    /// error diagnostic; it refers to nothing, and can be removed.
    #[test]
    fn unreadable_common_data_is_kept_as_stored() {
        let mut w = ObjWriter::new(Handle(0x11));
        w.data.write_bl(0);
        let bad = w.finish(type_code::DICTIONARY).unwrap();
        let mut objects = dictionary(0x10);
        let at = objects.len();
        objects.extend(&bad);
        let mut c = ctx(&[]);
        let out = read_all(
            &[(Handle(0x10), 0), (Handle(0x11), at as u64)],
            &objects,
            0,
            &mut c,
        );
        assert_eq!(c.diagnostics.len(), 1, "{:?}", c.diagnostics);
        assert_eq!(c.diagnostics[0].severity, Severity::Error);
        assert!(c.diagnostics[0].message.contains("kept as stored"));
        let Some(
            obj @ Object::Other {
                object: NonGraphical::Unreadable(u),
                ..
            },
        ) = out.get(&Handle(0x11))
        else {
            panic!("{out:?}")
        };
        assert_eq!(u.bytes, bad);
        assert_eq!(u.type_code, Some(type_code::DICTIONARY));
        assert_eq!(obj.handle(), Handle(0x11));
        assert_eq!(obj.type_name(), "DICTIONARY");
        assert!(!obj.is_convertible());
        assert!(matches!(
            crate::object::encode_object(obj, &[]),
            Err(Error::UnconvertibleObjects(_))
        ));
        assert_eq!(refs::object_refs(obj), (vec![], true));
        // In a drawing: listed as unconvertible, and removable.
        let mut d = Drawing::new();
        let h = d.alloc_handle().unwrap();
        let mut obj = obj.clone();
        obj.object_common_mut().handle = h;
        d.objects.insert(h, obj);
        assert_eq!(d.unconvertible_objects(), [(h, "DICTIONARY".to_string())]);
        assert!(matches!(d.to_bytes(), Err(Error::UnconvertibleObjects(_))));
        d.remove(h).unwrap();
        assert!(d.to_bytes().is_ok());
    }

    #[test]
    fn base_offset_is_subtracted() {
        let objects = dictionary(0x10);
        let mut c = ctx(&[]);
        let out = read_all(
            &[(Handle(0x10), 100), (Handle(0x11), 99), (Handle(0x12), 103)],
            &objects,
            100,
            &mut c,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(c.diagnostics.len(), 2);
        // Both locations are in the object map's terms.
        assert_eq!(
            c.diagnostics[1].message,
            "object location 103 lies inside the object at location 100; skipped"
        );
    }

    fn common(h: u64) -> ObjectCommon {
        ObjectCommon {
            handle: Handle(h),
            owner: Handle(0x40),
            owner_relative: false,
            reactors: vec![],
            xdict: None,
            xdata: vec![],
            has_ds_binary_data: false,
        }
    }

    /// An R2000 entity with stored links, or none (`nolinks`).
    fn entity(h: u64, links: Option<(u64, u64)>) -> Object {
        use crate::bits::Color;
        use crate::object::{EntityCommon, LinetypeRef, MaterialRef, PlotStyleRef, RawObject};
        Object::Entity {
            common: EntityCommon {
                obj: common(h),
                entmode: 0,
                layer: Handle(0x10),
                linetype: LinetypeRef::ByLayer,
                plotstyle: PlotStyleRef::ByLayer,
                material: MaterialRef::ByLayer,
                color: Color::ByLayer,
                transparency: None,
                read_color: Default::default(),
                color_book: None,
                ltype_scale: 1.0,
                lineweight: 29,
                invisible: false,
                shadow_flags: 0,
                full_visual_style: None,
                face_visual_style: None,
                edge_visual_style: None,
                proxy_graphics: None,
                prev_next: links.map(|(p, n)| (Handle(p), Handle(n))),
            },
            entity: crate::object::Entity::Unknown(RawObject {
                type_code: type_code::LINE,
                class_name: None,
                source_version: Version::R2000,
                data: vec![],
                data_bits: 0,
                strings: None,
                handles: vec![],
                tail_handles_raw: vec![],
                tail_handle_bits: 0,
            }),
        }
    }

    fn linked_block(h: u64, first: u64, last: u64) -> Object {
        use crate::object::{BlockRecord, TableEntry};
        Object::Other {
            common: common(h),
            object: NonGraphical::BlockRecord(BlockRecord {
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
                block_entity: Handle(0x41),
                entities: EntityList::Linked {
                    first: Handle(first),
                    last: Handle(last),
                },
                end_block: Handle(0x42),
                inserts: vec![],
                layout: Handle(0),
            }),
        }
    }

    fn block_entities(objects: &BTreeMap<Handle, Object>, h: u64) -> EntityList {
        match &objects[&Handle(h)] {
            Object::Other {
                object: NonGraphical::BlockRecord(b),
                ..
            } => b.entities.clone(),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn r2000_entity_chains_become_lists() {
        let mut objects: BTreeMap<Handle, Object> = [
            // Block 0x40: 0x50 -> 0x51 (implied +1) -> 0x60 (stored) -> 0x61.
            linked_block(0x40, 0x50, 0x61),
            entity(0x50, None),
            entity(0x51, Some((0x50, 0x60))),
            entity(0x60, Some((0x51, 0x61))),
            entity(0x61, Some((0x60, 0))),
            // Block 0x43: empty.
            linked_block(0x43, 0, 0),
            // Block 0x44: one entity.
            linked_block(0x44, 0x70, 0x70),
            entity(0x70, Some((0, 0))),
        ]
        .into_iter()
        .map(|o| (o.handle(), o))
        .collect();
        let mut diags = vec![];
        link_entity_lists(&mut objects, &mut diags);
        assert!(diags.is_empty(), "{diags:?}");
        let owned = |v: &[u64]| EntityList::Owned(v.iter().map(|&h| Handle(h)).collect());
        assert_eq!(
            block_entities(&objects, 0x40),
            owned(&[0x50, 0x51, 0x60, 0x61])
        );
        assert_eq!(block_entities(&objects, 0x43), owned(&[]));
        assert_eq!(block_entities(&objects, 0x44), owned(&[0x70]));
    }

    #[test]
    fn shared_entity_chains_are_linear() {
        // 2000 blocks all claim the same 2000-entity chain whose last entity
        // is never reached. Each failed walk releases its entities, so the
        // walks share a budget: the work stays linear in the object count.
        let n = 2000u64;
        let mut objects: BTreeMap<Handle, Object> = (0..n)
            .map(|i| entity(0x1000 + i, None))
            .chain((0..n).map(|i| linked_block(0x10_0000 + i, 0x1000, 0x9_9999)))
            .map(|o| (o.handle(), o))
            .collect();
        let mut diags = vec![];
        let steps = link_entity_lists(&mut objects, &mut diags);
        // Exactly the block walker's budget (4 x 4000 objects + 64): eight
        // walks run the whole chain (2000 entities and the missing handle
        // after them, 2001 steps each), the ninth stops after the 56 steps
        // left, and every other walk stops before a step. The attribute
        // walker takes none.
        let budget = ChainWalker::new(objects.len()).budget;
        assert_eq!(budget as u64, 4 * 2 * n + 64);
        assert_eq!(steps, budget);
        assert_eq!(diags.len() as u64, n);
        let end = format!("broken at {:X}", 0x1000 + n);
        for d in &diags[..8] {
            assert!(d.message.contains(&end), "{}", d.message);
        }
        assert!(
            diags[8]
                .message
                .contains(&format!("at {:X}: the walk budget", 0x1000 + 56)),
            "{}",
            diags[8].message
        );
        assert!(
            diags.last().unwrap().message.contains("budget is used up"),
            "{}",
            diags.last().unwrap().message
        );
    }

    #[test]
    fn failed_chain_does_not_spoil_a_valid_one() {
        // Block 0x40 claims 0x50 to the missing 0x99 and walks through 0x51
        // and 0x52, which are block 0x41's whole valid chain.
        let mut objects: BTreeMap<Handle, Object> = [
            linked_block(0x40, 0x50, 0x99),
            linked_block(0x41, 0x51, 0x52),
            entity(0x50, None),
            entity(0x51, None),
            entity(0x52, Some((0x51, 0x53))),
        ]
        .into_iter()
        .map(|o| (o.handle(), o))
        .collect();
        let mut diags = vec![];
        link_entity_lists(&mut objects, &mut diags);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].handle, Some(Handle(0x40)));
        assert!(diags[0].message.contains("broken at 53"));
        assert!(matches!(
            block_entities(&objects, 0x40),
            EntityList::Linked { .. }
        ));
        assert_eq!(
            block_entities(&objects, 0x41),
            EntityList::Owned(vec![Handle(0x51), Handle(0x52)])
        );
    }

    /// An R2000 INSERT with linked attributes.
    fn linked_insert(h: u64, first: u64, last: u64, seqend: u64) -> Object {
        use crate::object::Insert;
        let Object::Entity { common, .. } = entity(h, Some((0, 0))) else {
            unreachable!()
        };
        Object::Entity {
            common,
            entity: Entity::Insert(Insert {
                insertion: [0.0; 3],
                scale: [1.0; 3],
                rotation: 0.0,
                extrusion: [0.0, 0.0, 1.0],
                block: Handle(0x30),
                attribs: EntityList::Linked {
                    first: Handle(first),
                    last: Handle(last),
                },
                seqend: Some(Handle(seqend)),
            }),
        }
    }

    fn insert_attribs(objects: &BTreeMap<Handle, Object>, h: u64) -> EntityList {
        match &objects[&Handle(h)] {
            Object::Entity {
                entity: Entity::Insert(i),
                ..
            } => i.attribs.clone(),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn r2000_insert_attrib_chains_become_lists() {
        // Insert 0x60: attributes 0x61 -> 0x63 (stored, skipping the SEQEND
        // 0x62) -> 0x64 (implied). Insert 0x70: its last attribute is
        // missing, so it stays linked and cannot be written.
        let mut objects: BTreeMap<Handle, Object> = [
            linked_insert(0x60, 0x61, 0x64, 0x62),
            entity(0x61, Some((0x60, 0x63))),
            entity(0x62, Some((0x64, 0))),
            entity(0x63, None),
            entity(0x64, Some((0x63, 0x62))),
            linked_insert(0x70, 0x71, 0x79, 0x72),
            entity(0x71, Some((0x70, 0x72))),
        ]
        .into_iter()
        .map(|o| (o.handle(), o))
        .collect();
        let mut diags = vec![];
        link_entity_lists(&mut objects, &mut diags);
        assert_eq!(
            insert_attribs(&objects, 0x60),
            EntityList::Owned(vec![Handle(0x61), Handle(0x63), Handle(0x64)])
        );
        assert!(crate::object::encode_object(&objects[&Handle(0x60)], &[]).is_ok());
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].handle, Some(Handle(0x70)));
        assert!(diags[0].message.contains("insert attribute chain"));
        assert!(matches!(
            insert_attribs(&objects, 0x70),
            EntityList::Linked { .. }
        ));
        assert!(matches!(
            crate::object::encode_object(&objects[&Handle(0x70)], &[]),
            Err(Error::UnconvertibleObjects(_))
        ));
    }

    /// An R2000 3D polyline with linked vertices.
    fn linked_polyline(h: u64, first: u64, last: u64, seqend: u64) -> Object {
        use crate::object::Polyline3d;
        let Object::Entity { common, .. } = entity(h, Some((0, 0))) else {
            unreachable!()
        };
        Object::Entity {
            common,
            entity: Entity::Polyline3d(Polyline3d {
                spline_flags: 0,
                closed_flags: 0,
                vertices: EntityList::Linked {
                    first: Handle(first),
                    last: Handle(last),
                },
                seqend: Handle(seqend),
            }),
        }
    }

    fn polyline_vertices(objects: &BTreeMap<Handle, Object>, h: u64) -> EntityList {
        match &objects[&Handle(h)] {
            Object::Entity { entity, .. } => entity.polyline_vertices().unwrap().clone(),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn r2000_polyline_vertex_chains_become_lists() {
        // Polyline 0x80: vertices 0x81 -> 0x82 (implied, no links) -> 0x83,
        // SEQEND 0x84. Insert 0x90 claims 0x83 as its attribute too: the
        // attribute and vertex walks share one walker, so the later list
        // (by handle) fails. Polyline 0xA0's chain is broken.
        let mut objects: BTreeMap<Handle, Object> = [
            linked_polyline(0x80, 0x81, 0x83, 0x84),
            entity(0x81, Some((0, 0x82))),
            entity(0x82, None),
            entity(0x83, Some((0x82, 0))),
            linked_insert(0x90, 0x83, 0x83, 0x91),
            linked_polyline(0xA0, 0xA1, 0xA3, 0xA4),
            entity(0xA1, Some((0, 0))),
        ]
        .into_iter()
        .map(|o| (o.handle(), o))
        .collect();
        let mut diags = vec![];
        link_entity_lists(&mut objects, &mut diags);
        assert_eq!(
            polyline_vertices(&objects, 0x80),
            EntityList::Owned(vec![Handle(0x81), Handle(0x82), Handle(0x83)])
        );
        assert!(crate::object::encode_object(&objects[&Handle(0x80)], &[]).is_ok());
        assert!(matches!(
            insert_attribs(&objects, 0x90),
            EntityList::Linked { .. }
        ));
        assert!(matches!(
            polyline_vertices(&objects, 0xA0),
            EntityList::Linked { .. }
        ));
        assert!(matches!(
            crate::object::encode_object(&objects[&Handle(0xA0)], &[]),
            Err(Error::UnconvertibleObjects(_))
        ));
        let msgs: Vec<_> = diags
            .iter()
            .map(|d| (d.handle, d.message.as_str()))
            .collect();
        assert_eq!(msgs.len(), 2, "{msgs:?}");
        assert_eq!(msgs[0].0, Some(Handle(0x90)));
        assert!(msgs[0].1.contains("insert attribute chain"));
        assert_eq!(msgs[1].0, Some(Handle(0xA0)));
        assert!(msgs[1].1.contains("polyline vertex chain"));
    }

    #[test]
    fn broken_entity_chains_stay_linked() {
        let cases: [(&str, Vec<Object>, u64); 4] = [
            // The next link is null before the last entity.
            ("null", vec![entity(0x50, Some((0, 0)))], 0x50),
            // The implied next entity is missing.
            ("missing", vec![entity(0x50, None)], 0x51),
            // The next object is not an entity.
            (
                "not an entity",
                vec![entity(0x50, Some((0, 0x41))), linked_block(0x41, 1, 1)],
                0x41,
            ),
            // A loop that never reaches the last entity.
            (
                "loop",
                vec![
                    entity(0x50, Some((0, 0x51))),
                    entity(0x51, Some((0x50, 0x50))),
                ],
                0x50,
            ),
        ];
        for (what, entities, broken_at) in cases {
            let mut objects: BTreeMap<Handle, Object> = entities
                .into_iter()
                .chain([linked_block(0x40, 0x50, 0x99)])
                .map(|o| (o.handle(), o))
                .collect();
            let mut diags = vec![];
            link_entity_lists(&mut objects, &mut diags);
            assert!(
                matches!(block_entities(&objects, 0x40), EntityList::Linked { .. }),
                "{what}"
            );
            let d = diags
                .iter()
                .find(|d| d.handle == Some(Handle(0x40)))
                .unwrap_or_else(|| panic!("{what}: {diags:?}"));
            assert_eq!(d.severity, Severity::Warning);
            assert!(
                d.message.contains(&format!("broken at {broken_at:X}")),
                "{what}: {}",
                d.message
            );
            // Writing it fails rather than writing a wrong list.
            let obj = &objects[&Handle(0x40)];
            assert!(matches!(
                crate::object::encode_object(obj, &[]),
                Err(Error::UnconvertibleObjects(_))
            ));
        }
    }

    #[test]
    fn unsupported_codepage_warns_before_r2007() {
        // 11 is DOS 437, which encoding_rs lacks; 30 is ANSI_1252.
        let d = codepage_diagnostic(Version::R2000, 11).unwrap();
        assert_eq!(d.severity, Severity::Warning);
        assert!(d.message.contains("code page 11"));
        assert!(codepage_diagnostic(Version::R2004, 11).is_some());
        assert!(codepage_diagnostic(Version::R2000, 30).is_none());
        assert!(codepage_diagnostic(Version::R2007, 11).is_none());
    }

    #[test]
    fn from_bytes_rejects_garbage() {
        assert!(Drawing::from_bytes(b"").is_err());
        assert!(Drawing::from_bytes(b"AC1015").is_err());
        assert!(matches!(
            Drawing::from_bytes(b"AC1014\0\0\0\0"),
            Err(Error::UnsupportedVersion(_))
        ));
        // An ASCII DXF file named .dwg: not a DWG, refused cleanly.
        let mut dxf =
            b"  0\r\nSECTION\r\n  2\r\nHEADER\r\n  9\r\n$ACADVER\r\n  1\r\nAC1015\r\n".to_vec();
        dxf.extend(b"  0\r\nENDSEC\r\n  0\r\nEOF\r\n".repeat(64));
        assert!(matches!(
            Drawing::from_bytes(&dxf),
            Err(Error::UnsupportedVersion(v)) if v.starts_with("  0")
        ));
    }
}
