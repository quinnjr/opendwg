//! Rewriting the handles an object stores, for a handle renumbering (see
//! `Drawing::rewrite_handles`).
//!
//! [`map_object_handles`] passes every non-null handle an object stores
//! through a mapping: its own handle, its common data (owner, reactors,
//! extension dictionary, extended data, an entity's layer, styles and R2000
//! list links) and every reference of its type-specific data, typed or kept
//! as stored ([`HandleRef`] lists: a raw object's references, a proxy's
//! object ids, an XRECORD's, a spatial filter's or index's), and an
//! XRECORD's handle items (330-369 and 390-399 handles, 480-481 hard
//! pointers written as hexadecimal text, rewritten in the same form).
//! Typed handles are absolute, and the encoders choose their codes (an
//! owner stored as an offset is recomputed from the new handles); a stored
//! reference is rewritten by [`rewrite_ref`]. The null handle is never
//! passed and stays null.
//!
//! What it cannot reach, noted in [`Mapper::notes`] when the rewrite moves
//! something:
//!
//! - handles inside a raw object's data stream bits. A type the library
//!   does not decode may keep references there; of the types it decodes,
//!   an XRECORD (its items) and a SORTENTSTABLE (its sort handles) do, and
//!   one of them kept raw (its typed decode failed) is noted;
//! - a stored offset that does not resolve from the object's old handle:
//!   once the object moves it would name an unrelated object, so it
//!   becomes a null soft pointer, noted.
//!
//! Handles written as text other than XRECORD 480-481 items (320-329,
//! "not translated during INSERT and XREF operations" in DXF) are kept.

use super::{
    CellStyle, Dictionary, DimStyle, Entity, EntityCommon, EntityList, Insert, LeaderRoot,
    LinetypeRef, MLeaderContent, MLineLinetype, MText, MaterialRef, NonGraphical, Object,
    ObjectCommon, PlotSettings, PlotStyleRef, TableEntry, TableStyle, Text, XDataItem,
    XRecordValue,
};
use crate::handle::{Handle, HandleRef};
use crate::object::objects::{format_handle_text, handle_text_is_reference, parse_handle_text};
use crate::object::tables::SOFT_POINTER;

/// A handle mapping applied to the non-null handles only.
pub(crate) struct Mapper<'a> {
    f: &'a mut dyn FnMut(Handle) -> Handle,
    /// What could not be rewritten faithfully (see the module
    /// documentation), as messages about the object being mapped.
    pub(crate) notes: Vec<String>,
}

impl<'a> Mapper<'a> {
    pub(crate) fn new(f: &'a mut dyn FnMut(Handle) -> Handle) -> Self {
        Mapper {
            f,
            notes: Vec::new(),
        }
    }

    /// `h` mapped; the null handle stays null.
    pub(crate) fn map(&mut self, h: Handle) -> Handle {
        if h.0 == 0 { h } else { (self.f)(h) }
    }

    pub(crate) fn h(&mut self, h: &mut Handle) {
        *h = self.map(*h);
    }

    fn opt(&mut self, h: &mut Option<Handle>) {
        if let Some(h) = h {
            self.h(h);
        }
    }

    fn all<'h>(&mut self, hs: impl IntoIterator<Item = &'h mut Handle>) {
        for h in hs {
            self.h(h);
        }
    }

    /// References kept as stored by object `base` (its handle before the
    /// rewrite), see [`rewrite_ref`].
    fn refs(&mut self, refs: &mut [HandleRef], base: Handle) {
        let new_base = self.map(base);
        for r in refs {
            let old = *r;
            *r = rewrite_ref(old, base, new_base, self);
            if clears_offset(old, base, new_base) {
                self.notes.push(format!(
                    "a reference stored as an offset (code {:X}, value {:X}) names no handle \
                     from {base}; cleared, as the object moved",
                    old.code, old.value
                ));
            }
        }
    }
}

/// Whether [`rewrite_ref`] clears `r`, a reference of the object whose
/// handle was `base` and becomes `new_base`: an offset (codes 6, 8, 0xA,
/// 0xC) that names no handle from `base` (it runs below 0 or past the
/// largest handle), once the object moves.
pub(crate) fn clears_offset(r: HandleRef, base: Handle, new_base: Handle) -> bool {
    matches!(r.code, 6 | 8 | 0xA | 0xC) && new_base != base && r.resolve(base).is_none()
}

/// A stored reference of the object whose handle was `base` and becomes
/// `new_base`, rewritten:
///
/// - codes 0 to 5 hold an absolute handle (ODS 2.13 defines 2-5; codes 0
///   and 1 are read as absolute too, see `stream::resolve`): the handle is
///   mapped and the code kept;
/// - codes 6, 8, 0xA and 0xC are offsets from `base`: kept while neither
///   `base` nor the target changes, otherwise written as the absolute
///   target with code 4. ODS: an offset handle "always represent\[s\] a soft
///   pointer reference", so code 4 (soft pointer) keeps its meaning;
/// - an offset that does not resolve is kept while `base` stays, and
///   otherwise becomes a null soft pointer (it would name an unrelated
///   object from the new handle);
/// - other codes are kept.
pub(crate) fn rewrite_ref(
    r: HandleRef,
    base: Handle,
    new_base: Handle,
    m: &mut Mapper,
) -> HandleRef {
    match r.code {
        0..=5 => HandleRef {
            code: r.code,
            value: m.map(Handle(r.value)).0,
        },
        6 | 8 | 0xA | 0xC => match r.resolve(base) {
            Some(target) => {
                let mapped = m.map(target);
                if new_base == base && mapped == target {
                    r
                } else {
                    HandleRef {
                        code: SOFT_POINTER,
                        value: mapped.0,
                    }
                }
            }
            None if new_base == base => r,
            None => HandleRef {
                code: SOFT_POINTER,
                value: 0,
            },
        },
        _ => r,
    }
}

/// Map every handle `obj` stores (see the module documentation).
pub(crate) fn map_object_handles(obj: &mut Object, m: &mut Mapper) {
    let own = obj.handle();
    if obj.raw().is_some() {
        let name = obj.type_name();
        if (name == "XRECORD" || name == "SORTENTSTABLE") && m.map(own) != own {
            m.notes.push(format!(
                "a {name} kept as bits may hold handles in its data, which cannot be \
                 rewritten; they keep their old values"
            ));
        }
    }
    match obj {
        Object::Entity { common, entity } => {
            entity_common(common, m);
            map_entity(entity, own, m);
        }
        Object::Other { common, object } => {
            object_common(common, m);
            map_non_graphical(object, own, m);
        }
    }
}

fn object_common(c: &mut ObjectCommon, m: &mut Mapper) {
    m.h(&mut c.handle);
    m.h(&mut c.owner);
    m.all(&mut c.reactors);
    m.opt(&mut c.xdict);
    for app in &mut c.xdata {
        m.h(&mut app.app);
        for item in &mut app.items {
            match item {
                XDataItem::LayerRef(h) | XDataItem::Handle(h) => m.h(h),
                XDataItem::String(_)
                | XDataItem::ControlBrace(_)
                | XDataItem::Binary(_)
                | XDataItem::Point(_)
                | XDataItem::WorldPos(_)
                | XDataItem::WorldDisp(_)
                | XDataItem::WorldDir(_)
                | XDataItem::Real(_)
                | XDataItem::Distance(_)
                | XDataItem::Scale(_)
                | XDataItem::Short(_)
                | XDataItem::Long(_) => {}
            }
        }
    }
}

fn entity_common(c: &mut EntityCommon, m: &mut Mapper) {
    object_common(&mut c.obj, m);
    m.h(&mut c.layer);
    match &mut c.linetype {
        LinetypeRef::Handle(h) => m.h(h),
        LinetypeRef::ByLayer | LinetypeRef::ByBlock | LinetypeRef::Continuous => {}
    }
    match &mut c.plotstyle {
        PlotStyleRef::Handle(h) => m.h(h),
        PlotStyleRef::ByLayer | PlotStyleRef::ByBlock | PlotStyleRef::Continuous => {}
    }
    match &mut c.material {
        MaterialRef::Handle(h) => m.h(h),
        MaterialRef::ByLayer | MaterialRef::ByBlock | MaterialRef::Global => {}
    }
    m.opt(&mut c.color_book);
    m.opt(&mut c.full_visual_style);
    m.opt(&mut c.face_visual_style);
    m.opt(&mut c.edge_visual_style);
    if let Some((prev, next)) = &mut c.prev_next {
        m.h(prev);
        m.h(next);
    }
}

fn entity_list(l: &mut EntityList, m: &mut Mapper) {
    match l {
        EntityList::Linked { first, last } => {
            m.h(first);
            m.h(last);
        }
        EntityList::Owned(v) => m.all(v),
    }
}

fn insert(i: &mut Insert, m: &mut Mapper) {
    m.h(&mut i.block);
    entity_list(&mut i.attribs, m);
    m.opt(&mut i.seqend);
}

fn text(t: &mut Text, m: &mut Mapper) {
    m.h(&mut t.style);
}

fn mtext(t: &mut MText, m: &mut Mapper) {
    m.h(&mut t.style);
    if let Some(c) = &mut t.context {
        m.h(&mut c.app);
    }
}

fn map_entity(e: &mut Entity, own: Handle, m: &mut Mapper) {
    match e {
        Entity::Unknown(r) => m.refs(&mut r.handles, own),
        Entity::Line(_)
        | Entity::Point(_)
        | Entity::Circle(_)
        | Entity::Arc(_)
        | Entity::Ellipse(_)
        | Entity::Ray(_)
        | Entity::Xline(_)
        | Entity::Solid(_)
        | Entity::Trace(_)
        | Entity::Face3d(_)
        | Entity::Block(_)
        | Entity::EndBlk
        | Entity::SeqEnd
        | Entity::LwPolyline(_)
        | Entity::Vertex2d(_)
        | Entity::Vertex3d(_)
        | Entity::VertexMesh(_)
        | Entity::VertexPface(_)
        | Entity::VertexPfaceFace(_)
        | Entity::Spline(_)
        | Entity::Ole2Frame(_) => {}
        Entity::Shape(s) => m.h(&mut s.style),
        Entity::Insert(i) => insert(i, m),
        Entity::MInsert(i) => insert(&mut i.insert, m),
        Entity::Text(t) => text(t, m),
        Entity::Attrib(a) => attrib(a, m),
        Entity::AttDef(a) => attrib(&mut a.attrib, m),
        Entity::MText(t) => mtext(t, m),
        Entity::Polyline2d(p) => {
            entity_list(&mut p.vertices, m);
            m.h(&mut p.seqend);
        }
        Entity::Polyline3d(p) => {
            entity_list(&mut p.vertices, m);
            m.h(&mut p.seqend);
        }
        Entity::PolyfaceMesh(p) => {
            entity_list(&mut p.vertices, m);
            m.h(&mut p.seqend);
        }
        Entity::PolygonMesh(p) => {
            entity_list(&mut p.vertices, m);
            m.h(&mut p.seqend);
        }
        Entity::MLine(l) => m.h(&mut l.style),
        Entity::Dimension(d) => {
            m.h(&mut d.common.dimstyle);
            m.h(&mut d.common.block);
        }
        Entity::Leader(l) => {
            m.h(&mut l.annotation);
            m.h(&mut l.dimstyle);
        }
        Entity::Tolerance(t) => m.h(&mut t.dimstyle),
        Entity::MultiLeader(l) => {
            for root in &mut l.context.roots {
                leader_root(root, m);
            }
            match &mut l.context.content {
                MLeaderContent::None => {}
                MLeaderContent::Text(t) => m.h(&mut t.style),
                MLeaderContent::Block(b) => m.h(&mut b.block),
            }
            m.h(&mut l.style);
            m.h(&mut l.line_type);
            m.h(&mut l.arrowhead);
            m.h(&mut l.text_style);
            m.h(&mut l.block);
            m.all(l.arrowheads.iter_mut().map(|(_, h)| h));
            m.all(l.block_labels.iter_mut().map(|b| &mut b.attdef));
        }
        Entity::Hatch(h) => {
            m.all(h.paths.iter_mut().flat_map(|p| &mut p.boundary_objects));
        }
        Entity::Viewport(v) => {
            m.all(&mut v.frozen_layers);
            m.h(&mut v.clip_boundary);
            m.opt(&mut v.vp_ent_header);
            m.h(&mut v.named_ucs);
            m.h(&mut v.base_ucs);
            m.h(&mut v.background);
            m.h(&mut v.visual_style);
            m.h(&mut v.shade_plot);
            m.h(&mut v.sun);
        }
        Entity::Image(i) | Entity::Wipeout(i) => {
            m.h(&mut i.image_def);
            m.h(&mut i.reactor);
        }
        Entity::Acis(a) => m.opt(&mut a.history),
        Entity::ProxyEntity(p) => m.refs(&mut p.object_ids, own),
    }
}

fn attrib(a: &mut super::Attrib, m: &mut Mapper) {
    text(&mut a.text, m);
    if let Some(e) = &mut a.mtext {
        // The embedded MTEXT stores no handle of its own (it stays null).
        entity_common(&mut e.common, m);
        mtext(&mut e.mtext, m);
        if let Some(d) = &mut e.annotative_data {
            m.h(&mut d.app);
        }
    }
}

fn leader_root(root: &mut LeaderRoot, m: &mut Mapper) {
    for line in &mut root.lines {
        m.h(&mut line.line_type);
        m.h(&mut line.arrow);
    }
}

fn entry(e: &mut TableEntry, m: &mut Mapper) {
    m.h(&mut e.xref_block);
}

fn dictionary(d: &mut Dictionary, m: &mut Mapper) {
    m.all(d.entries.iter_mut().map(|(_, h)| h));
}

fn dim_style(s: &mut DimStyle, m: &mut Mapper) {
    entry(&mut s.entry, m);
    for h in [
        &mut s.dimtxsty,
        &mut s.dimldrblk,
        &mut s.dimblk,
        &mut s.dimblk1,
        &mut s.dimblk2,
        &mut s.dimltype,
        &mut s.dimltex1,
        &mut s.dimltex2,
    ] {
        m.h(h);
    }
}

fn plot_settings(p: &mut PlotSettings, m: &mut Mapper) {
    m.h(&mut p.plot_view);
    m.h(&mut p.visual_style);
}

fn cell_style(s: &mut CellStyle, m: &mut Mapper) {
    if let Some(d) = &mut s.data {
        m.h(&mut d.content.text_style);
        for b in &mut d.borders {
            if let Some(b) = &mut b.data {
                m.h(&mut b.line_type);
            }
        }
    }
}

fn map_non_graphical(o: &mut NonGraphical, own: Handle, m: &mut Mapper) {
    match o {
        NonGraphical::Unknown(r) => m.refs(&mut r.handles, own),
        NonGraphical::TableControl(c) => {
            m.all(&mut c.entries);
            m.all(&mut c.extra);
        }
        NonGraphical::BlockRecord(b) => {
            entry(&mut b.entry, m);
            m.h(&mut b.block_entity);
            entity_list(&mut b.entities, m);
            m.h(&mut b.end_block);
            m.all(&mut b.inserts);
            m.h(&mut b.layout);
        }
        NonGraphical::Layer(l) => {
            entry(&mut l.entry, m);
            m.h(&mut l.plotstyle);
            m.h(&mut l.material);
            m.h(&mut l.linetype);
            m.h(&mut l.unknown_handle);
        }
        NonGraphical::TextStyle(s) => entry(&mut s.entry, m),
        NonGraphical::Linetype(l) => {
            entry(&mut l.entry, m);
            m.all(l.dashes.iter_mut().map(|d| &mut d.style));
        }
        NonGraphical::View(v) => {
            entry(&mut v.entry, m);
            m.h(&mut v.background);
            m.h(&mut v.visual_style);
            m.h(&mut v.sun);
            m.h(&mut v.base_ucs);
            m.h(&mut v.named_ucs);
            m.h(&mut v.live_section);
        }
        NonGraphical::Ucs(u) => {
            entry(&mut u.entry, m);
            m.h(&mut u.base_ucs);
            m.h(&mut u.named_ucs);
        }
        NonGraphical::Vport(v) => {
            entry(&mut v.entry, m);
            m.h(&mut v.background);
            m.h(&mut v.visual_style);
            m.h(&mut v.sun);
            m.h(&mut v.named_ucs);
            m.h(&mut v.base_ucs);
        }
        NonGraphical::AppId(a) => entry(&mut a.entry, m),
        NonGraphical::DimStyle(s) => dim_style(s, m),
        NonGraphical::VpEntHdr(v) => {
            entry(&mut v.entry, m);
            m.h(&mut v.viewport);
            m.h(&mut v.prev);
        }
        NonGraphical::Dictionary(d) => dictionary(d, m),
        NonGraphical::DictionaryWithDefault(d) => {
            dictionary(&mut d.dictionary, m);
            m.h(&mut d.default_entry);
        }
        NonGraphical::XRecord(x) => {
            for (code, v) in &mut x.items {
                match v {
                    XRecordValue::ObjectId(h) => m.h(h),
                    XRecordValue::HandleText(s) if handle_text_is_reference(*code) => {
                        if let Some(h) = parse_handle_text(s) {
                            let new = m.map(h);
                            if new != h {
                                *s = format_handle_text(new, s);
                            }
                        }
                    }
                    _ => {}
                }
            }
            m.refs(&mut x.object_ids, own);
        }
        NonGraphical::Layout(l) => {
            plot_settings(&mut l.plot, m);
            m.h(&mut l.block_record);
            m.h(&mut l.active_viewport);
            m.h(&mut l.base_ucs);
            m.h(&mut l.named_ucs);
            m.all(&mut l.viewports);
        }
        NonGraphical::PlotSettings(p) => plot_settings(p, m),
        NonGraphical::Group(g) => m.all(&mut g.entities),
        NonGraphical::MLineStyle(s) => {
            for e in &mut s.elements {
                match &mut e.linetype {
                    MLineLinetype::Handle(h) => m.h(h),
                    MLineLinetype::Index(_) => {}
                }
            }
        }
        NonGraphical::MLeaderStyle(s) => {
            m.h(&mut s.line_type);
            m.h(&mut s.arrow_head);
            m.h(&mut s.text_style);
            m.h(&mut s.block);
        }
        NonGraphical::TableStyle(t) => match t {
            TableStyle::Legacy(l) => m.all(l.rows.iter_mut().map(|r| &mut r.text_style)),
            TableStyle::Cells(c) => {
                m.h(&mut c.unknown_handle);
                cell_style(&mut c.table_style.style, m);
                for (_, s) in &mut c.cell_styles {
                    cell_style(&mut s.style, m);
                }
            }
        },
        NonGraphical::SortEntsTable(s) => {
            m.h(&mut s.block);
            // The sort handles too: they stand for the handles of the
            // objects they name, and keep their draw order with them.
            m.all(s.entries.iter_mut().flat_map(|(sort, e)| [sort, e]));
        }
        NonGraphical::ProxyObject(p) => m.refs(&mut p.object_ids, own),
        NonGraphical::SpatialIndex(s) => m.refs(&mut s.handles, own),
        NonGraphical::SpatialFilter(s) => m.refs(&mut s.handles, own),
        NonGraphical::LayerIndex(l) => m.all(l.entries.iter_mut().map(|e| &mut e.handle)),
        NonGraphical::SectionViewStyle(s) => {
            for h in [
                &mut s.identifier_style,
                &mut s.arrow_start,
                &mut s.arrow_end,
                &mut s.plane_linetype,
                &mut s.bend_linetype,
                &mut s.label_style,
            ] {
                m.h(h);
            }
        }
        NonGraphical::DetailViewStyle(s) => {
            for h in [
                &mut s.identifier_style,
                &mut s.arrow,
                &mut s.boundary_linetype,
                &mut s.label_style,
                &mut s.connection_linetype,
                &mut s.border_linetype,
            ] {
                m.h(h);
            }
        }
        NonGraphical::ImageDef(_)
        | NonGraphical::ImageDefReactor(_)
        | NonGraphical::RasterVariables(_)
        | NonGraphical::WipeoutVariables(_)
        | NonGraphical::Scale(_)
        | NonGraphical::DictionaryVar(_)
        | NonGraphical::Placeholder
        | NonGraphical::Material(_)
        | NonGraphical::VisualStyle(_) => {}
        // Its references cannot be found in the stored bytes, which are never
        // written.
        NonGraphical::Unreadable(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drawing::Drawing;
    use crate::object::stream::resolve;
    use crate::object::{ObjWriter, encode_entity_common, encode_object_common};

    /// Far above every corpus handle, so that a handle left unmapped shows.
    const OFFSET: u64 = 1 << 40;

    fn shift(h: Handle) -> Handle {
        Handle(h.0 + OFFSET)
    }

    fn rewritten(obj: &Object) -> Object {
        let mut obj = obj.clone();
        map_object_handles(&mut obj, &mut Mapper::new(&mut shift));
        obj
    }

    /// Every reference encoding `obj` writes (common and type-specific
    /// data), resolved (`None` when it does not resolve) with its code, and
    /// whether encoding got through.
    fn recorded(obj: &Object) -> (Vec<(Option<Handle>, u8)>, bool) {
        let h = obj.handle();
        let mut w = ObjWriter::recording(h);
        let ok = match obj {
            Object::Entity { common, entity } => {
                encode_entity_common(&mut w, common).and_then(|()| entity.encode(&mut w))
            }
            Object::Other { common, object } => {
                encode_object_common(&mut w, common).and_then(|()| object.encode(&mut w))
            }
        }
        .is_ok();
        let refs = w
            .recorded_refs()
            .iter()
            .map(|r| (resolve(*r, h).ok(), r.code))
            .collect();
        (refs, ok)
    }

    /// The corpus drawings, when fetched.
    fn corpus() -> Vec<(String, Drawing)> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus/files");
        let mut out: Vec<(String, Drawing)> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|f| {
                let d = Drawing::open(f.path()).ok()?;
                Some((f.file_name().to_string_lossy().into_owned(), d))
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        if out.is_empty() {
            assert!(
                std::env::var("OPENDWG_REQUIRE_CORPUS").as_deref() != Ok("1"),
                "OPENDWG_REQUIRE_CORPUS=1 but there is no corpus"
            );
            eprintln!("SKIPPED: no corpus; run scripts/fetch-corpus.sh");
        }
        out
    }

    /// Encoding a rewritten object writes the mapped references of the
    /// original, in order: absolute ones with their codes, offsets (an
    /// owner stored relative, a raw object's) as offsets recomputed or as
    /// soft pointers. The references the drawing indexes (extended data and
    /// XRECORD items besides) are the mapped ones too. Every object of every
    /// corpus file, typed or raw, in every version.
    #[test]
    fn rewrite_matches_recorded_references() {
        let mut checked = 0usize;
        for (name, d) in corpus() {
            for (h, obj) in d.objects() {
                let new = rewritten(obj);
                assert_eq!(new.handle(), shift(h), "{name} {h}");
                let (before, ok_before) = recorded(obj);
                let (after, ok_after) = recorded(&new);
                assert_eq!(ok_before, ok_after, "{name} {h}");
                assert_eq!(before.len(), after.len(), "{name} {h}");
                for (&(t, code), &(nt, ncode)) in before.iter().zip(&after) {
                    let what = format!("{name} {h} {}: {t:?} {code}", obj.type_name());
                    match (t, code) {
                        (None, _) => assert_eq!((nt, ncode), (None, code), "{what}"),
                        (Some(t), 0..=5) => {
                            assert_eq!((nt, ncode), (Some(m(t)), code), "{what}")
                        }
                        (Some(t), _) => {
                            assert_eq!(nt, Some(m(t)), "{what}");
                            assert!(matches!(ncode, 4 | 6 | 8 | 0xA | 0xC), "{what}");
                        }
                    }
                }
                let indexed = |o: &Object| -> Vec<Handle> {
                    crate::drawing::refs::object_refs(o)
                        .0
                        .into_iter()
                        .map(|(t, _)| t)
                        .collect()
                };
                let expect: Vec<Handle> = indexed(obj).into_iter().map(m).collect();
                assert_eq!(indexed(&new), expect, "{name} {h}");
                checked += 1;
            }
        }
        eprintln!("{checked} objects checked");
    }

    fn m(h: Handle) -> Handle {
        if h.0 == 0 { h } else { shift(h) }
    }

    /// No handle is left unmapped, including those an R2018 encoding does
    /// not write (R2000 list links, a viewport's entity header, sort
    /// handles): in the rewritten object's debug form every handle and
    /// every stored absolute reference is null or mapped, and no offset is
    /// left (the object moved). Every object of every corpus file and of a
    /// new drawing.
    #[test]
    fn rewrite_reaches_every_handle() {
        // A new drawing too (the every-entity fixture, built in the
        // integration tests, is checked in tests/rewrite.rs).
        let fresh = ("Drawing::new".to_string(), crate::Drawing::new());
        for (name, d) in corpus().into_iter().chain([fresh]) {
            for (h, obj) in d.objects() {
                let text = format!("{:?}", rewritten(obj));
                for v in numbers_after(&text, "Handle(") {
                    assert!(v == 0 || v >= OFFSET, "{name} {h}: Handle({v}) in {text}");
                }
                for (code, value) in href_fields(&text) {
                    let what = format!("{name} {h}: code {code} value {value}");
                    assert!(code <= 5, "{what}");
                    assert!(value == 0 || value >= OFFSET, "{what}");
                }
            }
        }
    }

    /// The decimal numbers right after each `prefix` in `text`.
    fn numbers_after(text: &str, prefix: &str) -> Vec<u64> {
        text.match_indices(prefix)
            .filter_map(|(at, _)| {
                let rest = &text[at + prefix.len()..];
                let end = rest
                    .find(|c: char| !c.is_ascii_digit())
                    .unwrap_or(rest.len());
                rest[..end].parse().ok()
            })
            .collect()
    }

    /// The (code, value) of each `HandleRef` in a debug form.
    fn href_fields(text: &str) -> Vec<(u64, u64)> {
        let number = |s: &str| -> Option<(u64, usize)> {
            let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
            Some((s[..end].parse().ok()?, end))
        };
        text.match_indices("HandleRef { code: ")
            .map(|(at, p)| {
                let rest = &text[at + p.len()..];
                let (code, end) = number(rest).unwrap();
                let rest = rest[end..].strip_prefix(", value: ").unwrap();
                (code, number(rest).unwrap().0)
            })
            .collect()
    }

    /// The scan above sees what it must: an unmapped handle and a stored
    /// offset show.
    #[test]
    fn debug_scan_finds_handles() {
        let text = format!(
            "{:?}",
            (Handle(5), Some(Handle(7)), HandleRef { code: 8, value: 0 })
        );
        assert_eq!(numbers_after(&text, "Handle("), [5, 7]);
        assert_eq!(href_fields(&text), [(8, 0)]);
    }
}
