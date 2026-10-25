//! The typed accessors and the mutation API of [`Drawing`].
//!
//! Accessors read the symbol tables through their control objects (the
//! header's control handles), the layouts through the layout dictionary and
//! a block's entities through its block record.
//!
//! Mutations keep the cross-object structure consistent:
//!
//! - an added entity gets a fresh handle, its owner (the entity mode for
//!   model and paper space, as AutoCAD writes them) and a place at the end of
//!   its block record's entity list; an INSERT also joins its block's insert
//!   list, and an insert's attributes or a polyline's vertices are added
//!   with it, owned by it, with their SEQEND;
//! - an added table entry joins its control object's entries; an added block
//!   gets its BLOCK and ENDBLK entities;
//! - an object added to a dictionary ([`Drawing::add_object`]) becomes its
//!   entry, owned by it and naming it among its reactors;
//! - an added IMAGE gets its IMAGEDEF_REACTOR, and an object of a class type
//!   the drawing has no class of gets its class;
//! - a removed object leaves its owner's list (entity list, attributes,
//!   vertices, control entries or dictionary entries), takes the objects it
//!   owns with it (attributes, vertices, SEQEND, BLOCK and ENDBLK, a block's
//!   entities, extension dictionaries and their contents), and leaves every
//!   group, sort table, layout viewport list, block insert list and reactor
//!   list it was in.
//!
//! Whether an object is in use comes from a reverse reference index (see
//! `drawing::refs`), built on first use and kept up to date by the
//! mutations.
//!
//! A block record's insert list is a cache of the entities that use the
//! block (AutoCAD lists ACAD_TABLEs there too): an added INSERT or MINSERT
//! joins it and a removed entity leaves it, but other users added here do
//! not join it; saving refreshes it (`drawing::write`).

use std::collections::{BTreeMap, BTreeSet, HashSet};

use super::Drawing;
use super::refs::{IndexState, RefIndex, object_refs};
use crate::diag::{Diagnostic, Severity};
use crate::error::{Error, Result};
use crate::handle::Handle;
#[cfg(doc)]
use crate::object::Dictionary;
use crate::object::tables::{HARD_OWNER, SOFT_OWNER, SOFT_POINTER};
use crate::object::{
    Attrib, Block, BlockRecord, DictionaryWithDefault, DimStyle, Entity, EntityCommon, EntityList,
    ImageDefReactor, Layer, Layout, Linetype, LinetypeRef, MInsert, NonGraphical, Object,
    ObjectCommon, TableControl, TableEntry, TableKind, TextStyle, type_refs,
};
use crate::section::DxfClass;
use crate::section::classes::{FIRST_CLASS_NUMBER, known_class};
use crate::section::header::handle_vars;

/// An [`Error::Invalid`] with one error diagnostic.
fn invalid(handle: Option<Handle>, message: impl Into<String>) -> Error {
    Error::Invalid(vec![Diagnostic {
        severity: Severity::Error,
        handle,
        message: message.into(),
    }])
}

/// Symbol table names compare without case, as in AutoCAD.
pub(super) fn names_eq(a: &str, b: &str) -> bool {
    a.chars()
        .flat_map(char::to_lowercase)
        .eq(b.chars().flat_map(char::to_lowercase))
}

/// Whether `obj` owns a list R2000 stores as a first/last pair that was not
/// built (a broken chain): it cannot be changed or followed.
fn has_linked_list(obj: &Object) -> bool {
    let list = match obj {
        Object::Other {
            object: NonGraphical::BlockRecord(b),
            ..
        } => &b.entities,
        Object::Entity {
            entity: Entity::Insert(i) | Entity::MInsert(MInsert { insert: i, .. }),
            ..
        } => &i.attribs,
        Object::Entity { entity, .. } => match entity.polyline_vertices() {
            Some(list) => list,
            None => return false,
        },
        _ => return false,
    };
    matches!(list, EntityList::Linked { .. })
}

/// Remove every link from `obj` to the objects of `xs` that
/// [`Drawing::remove`] can drop: list entries (entities, inserts,
/// attributes, vertices, control and dictionary entries, group members, sort
/// table entries, layout viewports), a SEQEND, the last active viewport, a
/// reactor and the extension dictionary. Other references stay. Each list is
/// passed over once, however many of its entries go.
fn detach(obj: &mut Object, xs: &HashSet<Handle>) {
    let gone = |h: &Handle| xs.contains(h);
    let common = match obj {
        Object::Entity { common, .. } => &mut common.obj,
        Object::Other { common, .. } => common,
    };
    common.reactors.retain(|r| !gone(r));
    if common.xdict.is_some_and(|x| gone(&x)) {
        common.xdict = None;
    }
    let drop = |v: &mut Vec<Handle>| v.retain(|h| !gone(h));
    match obj {
        Object::Other { object, .. } => match object {
            NonGraphical::BlockRecord(b) => {
                if let EntityList::Owned(v) = &mut b.entities {
                    drop(v);
                }
                drop(&mut b.inserts);
            }
            NonGraphical::TableControl(c) => {
                drop(&mut c.entries);
                // The extra dimension style references point at entries;
                // the other tables' extra entries cannot be removed.
                if c.kind == TableKind::DimStyle {
                    drop(&mut c.extra);
                }
            }
            NonGraphical::Dictionary(d) => d.entries.retain(|(_, h)| !gone(h)),
            NonGraphical::DictionaryWithDefault(d) => {
                d.dictionary.entries.retain(|(_, h)| !gone(h));
            }
            NonGraphical::Group(g) => drop(&mut g.entities),
            NonGraphical::SortEntsTable(s) => s.entries.retain(|(_, e)| !gone(e)),
            NonGraphical::Layout(l) => {
                drop(&mut l.viewports);
                if gone(&l.active_viewport) {
                    l.active_viewport = Handle(0);
                }
            }
            _ => {}
        },
        Object::Entity { entity, .. } => match entity {
            Entity::Insert(i) | Entity::MInsert(MInsert { insert: i, .. }) => {
                if let EntityList::Owned(v) = &mut i.attribs {
                    drop(v);
                }
                if i.seqend.is_some_and(|s| gone(&s)) {
                    i.seqend = None;
                }
            }
            e => {
                if let Some(EntityList::Owned(v)) = e.polyline_vertices_mut() {
                    drop(v);
                }
            }
        },
    }
}

/// How an entity owns other entities when added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    /// No owned entities.
    Plain,
    /// INSERT or MINSERT: attributes, and a SEQEND when there are any.
    Insert,
    /// A POLYLINE: vertices and a SEQEND.
    Polyline,
}

impl Family {
    fn of(e: &Entity) -> Result<Family> {
        Ok(match e {
            Entity::Block(_)
            | Entity::EndBlk
            | Entity::SeqEnd
            | Entity::Attrib(_)
            | Entity::Vertex2d(_)
            | Entity::Vertex3d(_)
            | Entity::VertexMesh(_)
            | Entity::VertexPface(_)
            | Entity::VertexPfaceFace(_) => {
                return Err(invalid(
                    None,
                    "BLOCK, ENDBLK, SEQEND, ATTRIB and VERTEX entities are added with their \
                     owner (add_block, add_insert_with_attribs, add_polyline)",
                ));
            }
            Entity::Insert(_) | Entity::MInsert(_) => Family::Insert,
            e if e.polyline_vertices().is_some() => Family::Polyline,
            _ => Family::Plain,
        })
    }
}

/// Whether `child` can be owned by `parent`: attributes by inserts, vertices
/// of the matching kind by polylines.
fn child_fits(parent: &Entity, child: &Entity) -> bool {
    match parent {
        Entity::Insert(_) | Entity::MInsert(_) => matches!(child, Entity::Attrib(_)),
        Entity::Polyline2d(_) => matches!(child, Entity::Vertex2d(_)),
        Entity::Polyline3d(_) => matches!(child, Entity::Vertex3d(_)),
        Entity::PolygonMesh(_) => matches!(child, Entity::VertexMesh(_)),
        Entity::PolyfaceMesh(_) => {
            matches!(child, Entity::VertexPface(_) | Entity::VertexPfaceFace(_))
        }
        _ => false,
    }
}

/// Set the children of an insert or polyline; other entities have none.
fn set_children(e: &mut Entity, children: Vec<Handle>, seqend: Option<Handle>) {
    match e {
        Entity::Insert(i) | Entity::MInsert(MInsert { insert: i, .. }) => {
            i.attribs = EntityList::Owned(children);
            i.seqend = seqend;
        }
        Entity::Polyline2d(p) => {
            p.vertices = EntityList::Owned(children);
            p.seqend = seqend.unwrap_or_default();
        }
        Entity::Polyline3d(p) => {
            p.vertices = EntityList::Owned(children);
            p.seqend = seqend.unwrap_or_default();
        }
        Entity::PolyfaceMesh(p) => {
            p.vertices = EntityList::Owned(children);
            p.seqend = seqend.unwrap_or_default();
        }
        Entity::PolygonMesh(p) => {
            p.vertices = EntityList::Owned(children);
            p.seqend = seqend.unwrap_or_default();
        }
        _ => {}
    }
}

/// A polyface mesh's children are its vertices then its faces, as many as
/// its counts, and its faces only use those vertices; a polygon mesh has M x
/// N vertices.
fn check_mesh(parent: &Entity, children: &[(EntityCommon, Entity)]) -> Result<()> {
    match parent {
        Entity::PolyfaceMesh(p) => {
            let vertices = children
                .iter()
                .take_while(|(_, c)| matches!(c, Entity::VertexPface(_)))
                .count();
            let faces = &children[vertices..];
            if faces
                .iter()
                .any(|(_, c)| !matches!(c, Entity::VertexPfaceFace(_)))
            {
                return Err(invalid(
                    None,
                    "a polyface mesh's vertices come before its faces",
                ));
            }
            if usize::try_from(p.vertex_count).ok() != Some(vertices)
                || usize::try_from(p.face_count).ok() != Some(faces.len())
            {
                return Err(invalid(
                    None,
                    format!(
                        "a polyface mesh with vertex count {} and face count {} has {vertices} \
                         vertices and {} faces",
                        p.vertex_count,
                        p.face_count,
                        faces.len()
                    ),
                ));
            }
            let out_of_range = faces.iter().any(|(_, c)| match c {
                Entity::VertexPfaceFace(f) => f
                    .indices
                    .iter()
                    .any(|&i| usize::from(i.unsigned_abs()) > vertices),
                _ => false,
            });
            if out_of_range {
                return Err(invalid(None, "a polyface mesh face uses a missing vertex"));
            }
        }
        Entity::PolygonMesh(p) => {
            let expected = i32::from(p.m_count) * i32::from(p.n_count);
            if usize::try_from(expected).ok() != Some(children.len()) {
                return Err(invalid(
                    None,
                    format!(
                        "a {} x {} polygon mesh has {} vertices",
                        p.m_count,
                        p.n_count,
                        children.len()
                    ),
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

fn insert_block(e: &Entity) -> Option<Handle> {
    match e {
        Entity::Insert(i) | Entity::MInsert(MInsert { insert: i, .. }) => Some(i.block),
        _ => None,
    }
}

impl Drawing {
    // ---- reading ----

    /// The control object of a table, from the header's control handle.
    pub(super) fn table_control(&self, kind: TableKind) -> Option<(Handle, &TableControl)> {
        let h = &self.header;
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
        match self.objects.get(&control) {
            Some(Object::Other {
                object: NonGraphical::TableControl(c),
                ..
            }) if c.kind == kind => Some((control, c)),
            _ => None,
        }
    }

    /// The entries of a table, once each: for blocks and linetypes the
    /// control's extra entries first (`*Model_Space` and `*Paper_Space`;
    /// `ByBlock` and `ByLayer`), as AutoCAD lists them; then the counted
    /// entries in file order.
    pub(super) fn table_handles(&self, kind: TableKind) -> Vec<Handle> {
        let Some((_, c)) = self.table_control(kind) else {
            return Vec::new();
        };
        let extra: &[Handle] = match kind {
            TableKind::Block | TableKind::Linetype => &c.extra,
            _ => &[],
        };
        let mut seen = HashSet::new();
        extra
            .iter()
            .chain(&c.entries)
            .copied()
            .filter(|&h| h.0 != 0 && seen.insert(h))
            .collect()
    }

    fn table_iter<'a, T: 'a>(
        &'a self,
        kind: TableKind,
        pick: impl Fn(&'a NonGraphical) -> Option<&'a T> + 'a,
    ) -> impl Iterator<Item = (Handle, &'a T)> + 'a {
        self.table_handles(kind)
            .into_iter()
            .filter_map(move |h| match self.objects.get(&h) {
                Some(Object::Other { object, .. }) => pick(object).map(|t| (h, t)),
                _ => None,
            })
    }

    /// The layers, in table order.
    pub fn layers(&self) -> impl Iterator<Item = (Handle, &Layer)> {
        self.table_iter(TableKind::Layer, |o| match o {
            NonGraphical::Layer(l) => Some(l),
            _ => None,
        })
    }

    /// The linetypes, `ByBlock` and `ByLayer` first.
    pub fn linetypes(&self) -> impl Iterator<Item = (Handle, &Linetype)> {
        self.table_iter(TableKind::Linetype, |o| match o {
            NonGraphical::Linetype(l) => Some(l),
            _ => None,
        })
    }

    /// The text styles (and shape files), in table order.
    pub fn text_styles(&self) -> impl Iterator<Item = (Handle, &TextStyle)> {
        self.table_iter(TableKind::Style, |o| match o {
            NonGraphical::TextStyle(s) => Some(s),
            _ => None,
        })
    }

    /// The dimension styles, in table order.
    pub fn dim_styles(&self) -> impl Iterator<Item = (Handle, &DimStyle)> {
        self.table_iter(TableKind::DimStyle, |o| match o {
            NonGraphical::DimStyle(s) => Some(&**s),
            _ => None,
        })
    }

    /// The block records, `*Model_Space` and `*Paper_Space` first.
    pub fn block_records(&self) -> impl Iterator<Item = (Handle, &BlockRecord)> {
        self.table_iter(TableKind::Block, |o| match o {
            NonGraphical::BlockRecord(b) => Some(b),
            _ => None,
        })
    }

    /// The block record `h`; `None` when `h` is not a block record.
    pub fn block_record(&self, h: Handle) -> Option<&BlockRecord> {
        match self.objects.get(&h) {
            Some(Object::Other {
                object: NonGraphical::BlockRecord(b),
                ..
            }) => Some(b),
            _ => None,
        }
    }

    /// The block record `h`, to change in place (see [`Drawing::get_mut`]
    /// for what is not checked).
    pub fn block_record_mut(&mut self, h: Handle) -> Option<&mut BlockRecord> {
        match self.get_mut(h) {
            Some(Object::Other {
                object: NonGraphical::BlockRecord(b),
                ..
            }) => Some(b),
            _ => None,
        }
    }

    /// The layer `h`; `None` when `h` is not a layer.
    pub fn layer(&self, h: Handle) -> Option<&Layer> {
        match self.objects.get(&h) {
            Some(Object::Other {
                object: NonGraphical::Layer(l),
                ..
            }) => Some(l),
            _ => None,
        }
    }

    /// The layer `h`, to change in place (see [`Drawing::get_mut`] for what
    /// is not checked).
    pub fn layer_mut(&mut self, h: Handle) -> Option<&mut Layer> {
        match self.get_mut(h) {
            Some(Object::Other {
                object: NonGraphical::Layer(l),
                ..
            }) => Some(l),
            _ => None,
        }
    }

    /// The entity `h`: its common data (layer, colour, ...) and its
    /// type-specific data; `None` when `h` is not an entity.
    pub fn entity(&self, h: Handle) -> Option<(&EntityCommon, &Entity)> {
        match self.objects.get(&h) {
            Some(Object::Entity { common, entity }) => Some((common, entity)),
            _ => None,
        }
    }

    /// The entity `h`, to change in place (see [`Drawing::get_mut`] for
    /// what is not checked).
    pub fn entity_mut(&mut self, h: Handle) -> Option<(&mut EntityCommon, &mut Entity)> {
        match self.get_mut(h) {
            Some(Object::Entity { common, entity }) => Some((common, entity)),
            _ => None,
        }
    }

    /// The layer named `name`, compared without case as AutoCAD does.
    pub fn layer_by_name(&self, name: &str) -> Option<Handle> {
        self.layers()
            .find(|(_, l)| names_eq(&l.entry.name, name))
            .map(|(h, _)| h)
    }

    /// The model space block record: the header's, or the block record named
    /// `*Model_Space` when the header's is not a block record; null when
    /// there is neither.
    pub fn model_space(&self) -> Handle {
        let h = self.header.block_record_model_space;
        if self.block_record(h).is_some() {
            return h;
        }
        self.block_records()
            .find(|(_, b)| names_eq(&b.entry.name, "*Model_Space"))
            .map_or(Handle(0), |(h, _)| h)
    }

    /// The paper space block records, in layout tab order: the block records
    /// of the layouts (model space's excepted), and the header's paper space
    /// if no layout has it.
    pub fn paper_spaces(&self) -> Vec<Handle> {
        let ms = self.model_space();
        let mut out = Vec::new();
        let layouts = self.layouts().into_iter().map(|(_, l)| l.block_record);
        for b in layouts.chain([self.header.block_record_paper_space]) {
            if b != ms && self.block_record(b).is_some() && !out.contains(&b) {
                out.push(b);
            }
        }
        out
    }

    /// The entities of block record `block` in owner order; empty when it is
    /// not a block record. An R2000 list whose chain is broken (see
    /// [`EntityList::Linked`]) is followed from its first entity as far as
    /// it goes.
    pub fn entities_in(&self, block: Handle) -> impl Iterator<Item = Handle> + '_ {
        let (owned, linked): (&[Handle], Vec<Handle>) =
            match self.block_record(block).map(|b| &b.entities) {
                Some(EntityList::Owned(list)) => (list, Vec::new()),
                Some(&EntityList::Linked { first, last }) => (&[], self.follow_links(first, last)),
                None => (&[], Vec::new()),
            };
        owned.iter().copied().chain(linked)
    }

    /// The R2000 entities from `first` along their next links (the next
    /// handle when an entity stores none), until `last`, a missing or
    /// non-entity object, a null link or a loop.
    fn follow_links(&self, first: Handle, last: Handle) -> Vec<Handle> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut cur = first;
        while let Some(Object::Entity { common, .. }) = self.objects.get(&cur) {
            if !seen.insert(cur) {
                break;
            }
            out.push(cur);
            if cur == last {
                break;
            }
            cur = match common.prev_next {
                Some((_, next)) => next,
                None => match cur.0.checked_add(1) {
                    Some(n) => Handle(n),
                    None => break,
                },
            };
        }
        out
    }

    /// The layout dictionary: the header's, or the named object
    /// dictionary's `ACAD_LAYOUT` entry.
    fn layout_dictionary(&self) -> Option<&[(String, Handle)]> {
        let entries = |h: Handle| match self.objects.get(&h) {
            Some(Object::Other {
                object: NonGraphical::Dictionary(d),
                ..
            }) => Some(d.entries.as_slice()),
            Some(Object::Other {
                object: NonGraphical::DictionaryWithDefault(d),
                ..
            }) => Some(d.dictionary.entries.as_slice()),
            _ => None,
        };
        entries(self.header.dict_layouts).or_else(|| {
            let (_, h) = entries(self.named_objects())?
                .iter()
                .find(|(name, _)| name == "ACAD_LAYOUT")?;
            entries(*h)
        })
    }

    /// The layouts, in tab order (model space's first).
    pub fn layouts(&self) -> Vec<(Handle, &Layout)> {
        let mut out: Vec<(Handle, &Layout)> = self
            .layout_dictionary()
            .unwrap_or_default()
            .iter()
            .filter_map(|&(_, h)| match self.objects.get(&h) {
                Some(Object::Other {
                    object: NonGraphical::Layout(l),
                    ..
                }) => Some((h, &**l)),
                _ => None,
            })
            .collect();
        out.sort_by_key(|(_, l)| l.tab_order);
        out
    }

    /// The named object dictionary (the root dictionary).
    pub fn named_objects(&self) -> Handle {
        self.header.dict_named_objects
    }

    /// The owner of `h`: the stored owner, or for an entity that stores none
    /// the block record its entity mode names (model space, or the header's
    /// paper space). `None` for a missing object or a null owner.
    pub fn owner_of(&self, h: Handle) -> Option<Handle> {
        let owner = match self.objects.get(&h)? {
            Object::Entity { common, .. } => match common.entmode {
                0 => common.obj.owner,
                1 => self.header.block_record_paper_space,
                2 => self.model_space(),
                _ => return None,
            },
            Object::Other { common, .. } => common.owner,
        };
        (owner.0 != 0).then_some(owner)
    }

    // ---- references ----

    /// The objects that refer to `h`, in handle order, each once: through
    /// their type-specific data, an entity's layer, linetype, material, plot
    /// style, color book or visual styles, or their extended data (see
    /// `drawing::refs`). An object's own owner, reactor and extension
    /// dictionary links do not count; but an owner that lists the object in
    /// its data (a table control's entries, a block record's entities, a
    /// dictionary's entries, an insert's attributes) refers to it and is
    /// returned. Header variables are listed by
    /// [`Drawing::header_references`].
    ///
    /// The first call builds a reverse index of every reference, in time
    /// linear in their number; mutations keep it up to date, and an object
    /// changed through [`Drawing::get_mut`] is recomputed at the next call.
    /// The index sits behind a lock so that this `&self` method can build
    /// it: concurrent calls from several threads are serialized.
    pub fn referrers(&self, h: Handle) -> Vec<Handle> {
        let mut state = self.ref_index.lock();
        let index = state.ensure(&self.objects);
        index.referrers(h).map(|(r, _)| r).collect()
    }

    /// The header variables that point at `h`, by name (`"$CLAYER"`,
    /// `"$DIMSTYLE"`, `"LAYER CONTROL OBJECT"`, ...).
    pub fn header_references(&self, h: Handle) -> Vec<&'static str> {
        handle_vars(&self.header)
            .into_iter()
            .filter(|&(_, v)| v == h)
            .map(|(name, _)| name)
            .collect()
    }

    // ---- handles ----

    /// Move a `$HANDSEED` that is not above every handle (a damaged file)
    /// past them, so a handle once used is never given again, even after its
    /// object is removed. Every mutation starts with it. When the largest
    /// handle is the largest possible one (a hostile file) the seed becomes
    /// that handle: the handles are used up, and stay so after removals.
    fn normalise_seed(&mut self) {
        if let Some((&last, _)) = self.objects.last_key_value() {
            if self.header.handseed <= last {
                self.header.handseed = Handle(last.0.saturating_add(1));
            }
        }
    }

    /// Allocate a handle for a new object: the header's `$HANDSEED`, which
    /// is then bumped. A seed that is not above every existing handle (a
    /// damaged file) is moved past them first, so the handle is never one an
    /// object has or had.
    ///
    /// Handles are not reused: when the seed reaches the largest possible
    /// handle (which is never given, so that a seed above every handle can
    /// be written) the handles are used up, and this fails with
    /// [`Error::Invalid`].
    pub fn alloc_handle(&mut self) -> Result<Handle> {
        Ok(self.alloc_handles(1)?[0])
    }

    /// Allocate `n` consecutive handles (see [`Drawing::alloc_handle`]); on
    /// error nothing changes, the seed included.
    fn alloc_handles(&mut self, n: usize) -> Result<Vec<Handle>> {
        let seed = self.header.handseed;
        self.normalise_seed();
        let first = self.header.handseed.0.max(1);
        let past = self
            .objects
            .last_key_value()
            .is_none_or(|(last, _)| last.0 < first);
        match u64::try_from(n).ok().and_then(|n| first.checked_add(n)) {
            // `end` is at most the largest handle, never given.
            Some(end) if past => {
                self.header.handseed = Handle(end);
                Ok((first..end).map(Handle).collect())
            }
            _ => {
                self.header.handseed = seed;
                Err(invalid(
                    None,
                    format!(
                        "the handles are used up: {n} more cannot be allocated above \
                         $HANDSEED {}",
                        Handle(first)
                    ),
                ))
            }
        }
    }

    // ---- adding ----

    /// Check a new object: its common data has no extension dictionary (it
    /// cannot share one) and every reactor exists; every reference it makes
    /// is to an existing object or one in `pending` (added with it), and it
    /// can be encoded as R2018; an entity's layer is a layer, its linetype a
    /// linetype, and a multiline has as many lines as its style has
    /// elements (the kind checks of `validate()`).
    fn check_new(&self, obj: &Object, pending: &[Handle]) -> Result<()> {
        let common = obj.object_common();
        if common.xdict.is_some() {
            return Err(invalid(
                Some(common.handle),
                "a new object cannot have an extension dictionary",
            ));
        }
        let exists = |h: Handle| self.objects.contains_key(&h) || pending.contains(&h);
        if let Some(&r) = common.reactors.iter().find(|&&r| !exists(r)) {
            return Err(Error::NotFound(r));
        }
        type_refs(obj).1?;
        if let Some((t, _)) = object_refs(obj).0.into_iter().find(|&(t, _)| !exists(t)) {
            return Err(Error::NotFound(t));
        }
        if let Object::Entity { common, .. } = obj {
            if !matches!(
                self.objects.get(&common.layer),
                Some(Object::Other {
                    object: NonGraphical::Layer(_),
                    ..
                })
            ) && !pending.contains(&common.layer)
            {
                return Err(invalid(
                    Some(common.obj.handle),
                    format!("{} is not a layer", common.layer),
                ));
            }
            // The kind checks `validate()` makes of an entity's references,
            // so that nothing added here is refused at save.
            if let LinetypeRef::Handle(lt) = common.linetype {
                if !matches!(
                    self.objects.get(&lt),
                    Some(Object::Other {
                        object: NonGraphical::Linetype(_),
                        ..
                    })
                ) && !pending.contains(&lt)
                {
                    return Err(invalid(
                        Some(common.obj.handle),
                        format!("{lt} is not a linetype"),
                    ));
                }
            }
        }
        if let Object::Entity {
            entity: Entity::MLine(m),
            ..
        } = obj
        {
            if let Some(Object::Other {
                object: NonGraphical::MLineStyle(s),
                ..
            }) = self.objects.get(&m.style)
            {
                if usize::from(m.lines_in_style) != s.elements.len() {
                    return Err(invalid(
                        Some(common.handle),
                        format!(
                            "the multiline has {} lines, but its style {} has {} elements",
                            m.lines_in_style,
                            m.style,
                            s.elements.len()
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Check that `block` is a block record whose entity list can take an
    /// entity.
    fn check_block(&self, block: Handle) -> Result<()> {
        match self.objects.get(&block) {
            None => Err(Error::NotFound(block)),
            Some(Object::Other {
                object: NonGraphical::BlockRecord(b),
                ..
            }) => match b.entities {
                EntityList::Owned(_) => Ok(()),
                EntityList::Linked { .. } => Err(invalid(
                    Some(block),
                    "the block's entity list was not built (a broken R2000 chain); it cannot \
                     be changed",
                )),
            },
            Some(_) => Err(invalid(
                Some(block),
                format!("{block} is not a block record"),
            )),
        }
    }

    /// The entity mode and owner of an entity of `block`: model space and
    /// the header's paper space store none (modes 2 and 1), as AutoCAD writes
    /// them; other blocks are stored owners (mode 0).
    fn place(&self, block: Handle, common: &mut EntityCommon) {
        let (entmode, owner) = if block == self.model_space() {
            (2, Handle(0))
        } else if block == self.header.block_record_paper_space {
            (1, Handle(0))
        } else {
            (0, block)
        };
        common.entmode = entmode;
        common.obj.owner = owner;
        common.obj.owner_relative = false;
        common.prev_next = None;
    }

    /// Add an entity to the end of block record `block`'s entity list and
    /// return its new handle. The entity's handle, owner and entity mode are
    /// set here. An INSERT or MINSERT joins its block's insert list (a cache
    /// of the block's users that saving refreshes; other users, such as a
    /// new ACAD_TABLE, are not added to it here). An ATTDEF sets its block's
    /// `has_attdefs`.
    ///
    /// Refused ([`Error::Invalid`]) for a `block` that is not a block record
    /// or whose R2000 list was not built, for entities owned by other
    /// entities (BLOCK, ENDBLK, SEQEND, ATTRIB, VERTEX), for an insert with
    /// attributes or a polyline with vertices (see
    /// [`Drawing::add_insert_with_attribs`] and [`Drawing::add_polyline`];
    /// a polyline without vertices gets its SEQEND here), for an insert of
    /// a layout's block or one that would make a circular block reference
    /// (at any depth of nested inserts), for common data with an
    /// extension dictionary, for a layer that is not a layer or a linetype
    /// that is not a linetype, for a multiline whose line count is not its
    /// style's element count, for an entity that cannot be encoded as R2018
    /// and for an entity of a class
    /// type the library has no class definition of. A reference to a missing
    /// object is [`Error::NotFound`]. On error nothing changes.
    ///
    /// A TEXT or MTEXT with a null style (as [`crate::object::Text::new`]
    /// and [`crate::object::MText::new`] leave it) gets the current text
    /// style ($TEXTSTYLE, else `Standard`).
    ///
    /// An entity of a class type (IMAGE, WIPEOUT, MULTILEADER, ARC_DIMENSION,
    /// LARGE_RADIAL_DIMENSION) the drawing has no class of gets its class,
    /// as AutoCAD registers a class with its first object. An IMAGE with an
    /// image definition but no reactor gets its IMAGEDEF_REACTOR, owned by
    /// the image and listed among the definition's reactors, as AutoCAD
    /// links them; a definition that is not an IMAGEDEF is refused.
    pub fn add_entity(
        &mut self,
        block: Handle,
        common: EntityCommon,
        entity: Entity,
    ) -> Result<Handle> {
        let has_children = match &entity {
            Entity::Insert(i) | Entity::MInsert(MInsert { insert: i, .. }) => {
                i.attribs != EntityList::Owned(Vec::new()) || i.seqend.is_some()
            }
            e => e
                .polyline_vertices()
                .is_some_and(|v| *v != EntityList::Owned(Vec::new())),
        };
        if has_children {
            return Err(invalid(
                None,
                "an insert with attributes or a polyline with vertices is added with \
                 add_insert_with_attribs or add_polyline",
            ));
        }
        self.add_family(block, (common, entity), Vec::new())
    }

    /// Add an INSERT or MINSERT with its attributes, owned by it and ended by
    /// a SEQEND (none without attributes), to block record `block`; see
    /// [`Drawing::add_entity`]. The insert's attribute list and SEQEND are
    /// set here.
    pub fn add_insert_with_attribs(
        &mut self,
        block: Handle,
        insert: (EntityCommon, Entity),
        attribs: Vec<(EntityCommon, Attrib)>,
    ) -> Result<Handle> {
        if insert_block(&insert.1).is_none() {
            return Err(invalid(
                None,
                "add_insert_with_attribs takes an INSERT or MINSERT",
            ));
        }
        let attribs = attribs
            .into_iter()
            .map(|(c, a)| (c, Entity::Attrib(Box::new(a))))
            .collect();
        self.add_family(block, insert, attribs)
    }

    /// Add a 2D, 3D, polyface mesh or polygon mesh POLYLINE with its vertices
    /// (of the matching VERTEX kind), owned by it and ended by a SEQEND, to
    /// block record `block`; see [`Drawing::add_entity`]. The polyline's
    /// vertex list and SEQEND are set here. A polyface mesh's vertices come
    /// before its faces, their numbers must be its vertex and face counts,
    /// and its faces may only use those vertices; a polygon mesh has M x N
    /// vertices ([`Error::Invalid`] otherwise).
    pub fn add_polyline(
        &mut self,
        block: Handle,
        poly: (EntityCommon, Entity),
        vertices: Vec<(EntityCommon, Entity)>,
    ) -> Result<Handle> {
        if poly.1.polyline_vertices().is_none() {
            return Err(invalid(None, "add_polyline takes a POLYLINE"));
        }
        self.add_family(block, poly, vertices)
    }

    /// Add an entity and the entities it owns (attributes or vertices, and a
    /// SEQEND); nothing changes on error.
    fn add_family(
        &mut self,
        block: Handle,
        parent: (EntityCommon, Entity),
        children: Vec<(EntityCommon, Entity)>,
    ) -> Result<Handle> {
        let (common, mut entity) = parent;
        self.check_block(block)?;
        // Text added without a style (`Text::new`, `MText::new`) gets the
        // current one.
        let style = match &mut entity {
            Entity::Text(t) => Some(&mut t.style),
            Entity::MText(m) => Some(&mut m.style),
            _ => None,
        };
        if let Some(style) = style.filter(|s| s.0 == 0) {
            *style = self.current_text_style().unwrap_or_default();
        }
        let family = Family::of(&entity)?;
        if let Some(i) = children.iter().position(|(_, c)| !child_fits(&entity, c)) {
            return Err(invalid(
                None,
                format!(
                    "child {i} cannot be owned by this entity: inserts own ATTRIBs, polylines \
                     VERTEXes of their kind"
                ),
            ));
        }
        check_mesh(&entity, &children)?;
        let target = insert_block(&entity);
        if let Some(target) = target {
            match self.block_record(target) {
                None if !self.objects.contains_key(&target) => {
                    return Err(Error::NotFound(target));
                }
                None => {
                    return Err(invalid(None, format!("{target} is not a block record")));
                }
                Some(b) if b.layout.0 != 0 => {
                    return Err(invalid(Some(target), "a layout's block cannot be inserted"));
                }
                Some(_) if self.block_reaches(target, block) => {
                    return Err(invalid(
                        Some(target),
                        format!(
                            "inserting block {target} into {block} would make a circular block \
                             reference"
                        ),
                    ));
                }
                Some(_) => {}
            }
        }
        let image_def = self.new_image_def(&entity)?;
        let is_attdef = matches!(entity, Entity::AttDef(_));
        let with_seqend = match family {
            Family::Plain => false,
            Family::Insert => !children.is_empty(),
            Family::Polyline => true,
        };
        self.sync_index();
        let seed = self.header.handseed;
        let n = 1 + children.len() + usize::from(with_seqend) + usize::from(image_def.is_some());
        let handles = self.alloc_handles(n)?;
        let built = self
            .build_family(
                block,
                &handles,
                (common, entity),
                children,
                with_seqend,
                image_def.is_some(),
            )
            .and_then(|objects| Ok((self.missing_classes(&objects)?, objects)));
        let (classes, objects) = match built {
            Ok(built) => built,
            Err(e) => {
                self.header.handseed = seed;
                return Err(e);
            }
        };
        let h = handles[0];
        for obj in objects {
            self.objects.insert(obj.handle(), obj);
        }
        self.classes.extend(classes);
        if let Some(def) = image_def {
            let reactor = handles[handles.len() - 1];
            if let Some(Object::Other { common, .. }) = self.objects.get_mut(&def) {
                common.reactors.push(reactor);
            }
            self.ref_index.get_mut().refresh(&self.objects, def);
        }
        if let Some(Object::Other {
            object: NonGraphical::BlockRecord(b),
            ..
        }) = self.objects.get_mut(&block)
        {
            if let EntityList::Owned(list) = &mut b.entities {
                list.push(h);
            }
            b.has_attdefs |= is_attdef;
        }
        let mut edges = vec![(block, h, HARD_OWNER)];
        if let Some(Object::Other {
            object: NonGraphical::BlockRecord(b),
            ..
        }) = target.and_then(|t| self.objects.get_mut(&t))
        {
            b.inserts.push(h);
            edges.extend(target.map(|t| (t, h, SOFT_POINTER)));
        }
        if let (Some(t), Some(known)) = (
            target,
            self.ref_index.get_mut().block_inserts.get_mut(&block),
        ) {
            known.insert(t);
        }
        self.ref_index
            .get_mut()
            .added(&self.objects, &handles, &edges);
        Ok(h)
    }

    /// The image definition of a new IMAGE that has one but no reactor, for
    /// which [`Drawing::add_entity`] creates the reactor; `None` for other
    /// entities. A definition that is not an IMAGEDEF is refused
    /// ([`Error::NotFound`] when missing, else [`Error::Invalid`]).
    fn new_image_def(&self, entity: &Entity) -> Result<Option<Handle>> {
        let Entity::Image(i) = entity else {
            return Ok(None);
        };
        if i.image_def.0 == 0 || i.reactor.0 != 0 {
            return Ok(None);
        }
        match self.objects.get(&i.image_def) {
            None => Err(Error::NotFound(i.image_def)),
            Some(Object::Other {
                object: NonGraphical::ImageDef(_),
                ..
            }) => Ok(Some(i.image_def)),
            Some(_) => Err(invalid(
                None,
                format!("{} is not an image definition", i.image_def),
            )),
        }
    }

    /// The classes the drawing lacks for `objects`: for each class type it
    /// has no class of, the library's definition of that class
    /// ([`known_class`]), numbered after the drawing's last class, as
    /// AutoCAD adds a class with its first object. A class type the library
    /// has no definition of (another application's class) is refused with
    /// [`Error::Invalid`].
    fn missing_classes(&self, objects: &[Object]) -> Result<Vec<DxfClass>> {
        let mut next = self
            .classes
            .iter()
            .map(|c| c.class_number.saturating_add(1))
            .max()
            .unwrap_or(FIRST_CLASS_NUMBER)
            .max(FIRST_CLASS_NUMBER);
        let mut out: Vec<DxfClass> = Vec::new();
        for obj in objects {
            let Some(name) = obj.class_name() else {
                continue;
            };
            let have = |c: &DxfClass| c.dxf_name == name;
            if self.classes.iter().any(have) || out.iter().any(have) {
                continue;
            }
            let class = (next < u16::MAX)
                .then(|| known_class(name, next))
                .flatten()
                .ok_or_else(|| {
                    invalid(
                        None,
                        format!(
                            "the drawing has no class {name} and it cannot be added (an unknown \
                             class, or no class number left)"
                        ),
                    )
                })?;
            next += 1;
            out.push(class);
        }
        Ok(out)
    }

    /// Whether block record `from` is `to` or holds, through INSERTs and
    /// MINSERTs nested at any depth, an insert of `to`. The blocks each
    /// block inserts are cached (see `IndexState::block_inserts`), so a
    /// block's entities are scanned once, not at every insert added.
    fn block_reaches(&self, from: Handle, to: Handle) -> bool {
        let mut state = self.ref_index.lock();
        let mut stack = vec![from];
        let mut seen = HashSet::from([from]);
        while let Some(b) = stack.pop() {
            if b == to {
                return true;
            }
            if !state.block_inserts.contains_key(&b) {
                let targets: HashSet<Handle> = self
                    .entities_in(b)
                    .filter_map(|e| match self.objects.get(&e) {
                        Some(Object::Entity { entity, .. }) => insert_block(entity),
                        _ => None,
                    })
                    .collect();
                state.block_scans += 1;
                state.block_inserts.insert(b, targets);
            }
            for &t in &state.block_inserts[&b] {
                if seen.insert(t) {
                    stack.push(t);
                }
            }
        }
        false
    }

    /// Bring the reference index up to date before a mutation (see
    /// [`IndexState::sync`]).
    fn sync_index(&mut self) {
        self.ref_index.get_mut().sync(&self.objects);
    }

    /// The objects of [`Drawing::add_family`], checked, parent first, then
    /// its children, its SEQEND and an image's new reactor (in `handles`'
    /// order).
    fn build_family(
        &self,
        block: Handle,
        handles: &[Handle],
        parent: (EntityCommon, Entity),
        children: Vec<(EntityCommon, Entity)>,
        with_seqend: bool,
        with_reactor: bool,
    ) -> Result<Vec<Object>> {
        let (mut common, mut entity) = parent;
        let h = handles[0];
        let child_handles = handles[1..=children.len()].to_vec();
        let seqend = with_seqend.then(|| handles[children.len() + 1]);
        // An image added without its reactor gets one (see
        // `Drawing::add_entity`), allocated last.
        let reactor = match &mut entity {
            Entity::Image(i) if with_reactor => {
                let r = handles[handles.len() - 1];
                i.reactor = r;
                Some(r)
            }
            _ => None,
        };
        common.obj.handle = h;
        self.place(block, &mut common);
        let owned = |handle: Handle, mut c: EntityCommon| {
            c.obj.handle = handle;
            c.entmode = 0;
            c.obj.owner = h;
            c.obj.owner_relative = false;
            c.prev_next = None;
            c
        };
        let mut objects = Vec::with_capacity(handles.len());
        for (&ch, (c, e)) in child_handles.iter().zip(children) {
            objects.push(Object::Entity {
                common: owned(ch, c),
                entity: e,
            });
        }
        if let Some(s) = seqend {
            objects.push(Object::Entity {
                common: owned(s, EntityCommon::new(common.layer)),
                entity: Entity::SeqEnd,
            });
        }
        set_children(&mut entity, child_handles, seqend);
        objects.insert(0, Object::Entity { common, entity });
        if let Some(r) = reactor {
            // As AutoCAD writes them: owned by the image, class version 2.
            objects.push(Object::Other {
                common: ObjectCommon {
                    handle: r,
                    owner: h,
                    ..ObjectCommon::new()
                },
                object: NonGraphical::ImageDefReactor(ImageDefReactor { class_version: 2 }),
            });
        }
        for obj in &objects {
            self.check_new(obj, handles)?;
        }
        Ok(objects)
    }

    /// Add an entry to the table of `kind`, owned by its control object;
    /// nothing changes on error.
    pub(super) fn add_table_entry(
        &mut self,
        kind: TableKind,
        object: NonGraphical,
    ) -> Result<Handle> {
        let Some((control, _)) = self.table_control(kind) else {
            return Err(invalid(
                None,
                format!("the drawing has no {kind:?} table control"),
            ));
        };
        let name = object
            .table_entry()
            .map(|e| e.name.clone())
            .unwrap_or_default();
        let unnamed_shape = matches!(&object, NonGraphical::TextStyle(s) if s.shape_file);
        self.check_new_name(kind, &name, unnamed_shape)?;
        self.sync_index();
        let seed = self.header.handseed;
        let h = self.alloc_handle()?;
        let obj = Object::Other {
            common: ObjectCommon {
                handle: h,
                owner: control,
                ..ObjectCommon::new()
            },
            object,
        };
        if let Err(e) = self.check_new(&obj, &[h]) {
            self.header.handseed = seed;
            return Err(e);
        }
        self.objects.insert(h, obj);
        if let Some(Object::Other {
            object: NonGraphical::TableControl(c),
            ..
        }) = self.objects.get_mut(&control)
        {
            c.entries.push(h);
        }
        self.ref_index
            .get_mut()
            .added(&self.objects, &[h], &[(control, h, SOFT_OWNER)]);
        Ok(h)
    }

    /// A new entry's name must not be empty (an unnamed shape file excepted)
    /// nor that of another entry of the table, compared without case.
    fn check_new_name(&self, kind: TableKind, name: &str, may_be_empty: bool) -> Result<()> {
        if name.is_empty() {
            return if may_be_empty {
                Ok(())
            } else {
                Err(invalid(
                    None,
                    format!("a {kind:?} table entry needs a name"),
                ))
            };
        }
        let taken = self.table_handles(kind).into_iter().any(|h| {
            matches!(self.objects.get(&h), Some(Object::Other { object, .. })
                if object.table_entry().is_some_and(|e| names_eq(&e.name, name)))
        });
        if taken {
            return Err(invalid(
                None,
                format!("the {kind:?} table already has an entry named {name:?}"),
            ));
        }
        Ok(())
    }

    /// Add a non-graphical object as the entry `name` of `dictionary` (a
    /// dictionary, with or without default), owned by it and naming it among
    /// its reactors, as AutoCAD keeps dictionary entries, and return its new
    /// handle. The object's handle and owner are set here; a class type the
    /// drawing has no class of gets its class (see [`Drawing::add_entity`]).
    ///
    /// For an image: create `ACAD_IMAGE_DICT` (a [`Dictionary`]) in the
    /// named object dictionary ([`Drawing::named_objects`]), add the
    /// IMAGEDEF to it, then the IMAGE with [`Drawing::add_entity`].
    ///
    /// Refused ([`Error::Invalid`]) for a `dictionary` that is not one, a
    /// name that is empty or another entry's (compared without case, as
    /// AutoCAD looks keys up), objects other ways add (table controls and
    /// entries, layouts) or that own other objects (a group, sort table or
    /// other object listing entities, an image definition reactor, a
    /// dictionary that already has entries: add it empty, then its
    /// entries), common
    /// data with an extension dictionary, and an object that cannot be
    /// encoded as R2018. A reference to a missing object is
    /// [`Error::NotFound`]. On error nothing changes.
    pub fn add_object(
        &mut self,
        dictionary: Handle,
        name: &str,
        object: NonGraphical,
    ) -> Result<Handle> {
        let entries = match self.objects.get(&dictionary) {
            None => return Err(Error::NotFound(dictionary)),
            Some(Object::Other {
                object: NonGraphical::Dictionary(d),
                ..
            }) => &d.entries,
            Some(Object::Other {
                object: NonGraphical::DictionaryWithDefault(d),
                ..
            }) => &d.dictionary.entries,
            Some(_) => {
                return Err(invalid(
                    Some(dictionary),
                    format!("{dictionary} is not a dictionary"),
                ));
            }
        };
        if name.is_empty() || entries.iter().any(|(n, _)| names_eq(n, name)) {
            return Err(invalid(
                Some(dictionary),
                format!("the dictionary already has an entry named {name:?}, or it is empty"),
            ));
        }
        let refused = match &object {
            NonGraphical::TableControl(_) | NonGraphical::Layout(_) => true,
            o if o.table_entry().is_some() => true,
            NonGraphical::Group(_)
            | NonGraphical::SortEntsTable(_)
            | NonGraphical::SpatialIndex(_)
            | NonGraphical::LayerIndex(_)
            | NonGraphical::ImageDefReactor(_) => true,
            NonGraphical::Dictionary(d)
            | NonGraphical::DictionaryWithDefault(DictionaryWithDefault {
                dictionary: d, ..
            }) => !d.entries.is_empty(),
            _ => false,
        };
        if refused {
            return Err(invalid(
                None,
                "table records, table controls and layouts are added by their own methods; \
                 groups, sort tables, indexes, image reactors and dictionaries with entries \
                 (whose entries would not be owned by them) are not added as dictionary \
                 entries: add an empty dictionary, then its entries",
            ));
        }
        self.sync_index();
        let seed = self.header.handseed;
        let h = self.alloc_handle()?;
        let obj = Object::Other {
            common: ObjectCommon {
                handle: h,
                owner: dictionary,
                reactors: vec![dictionary],
                ..ObjectCommon::new()
            },
            object,
        };
        let classes = self
            .check_new(&obj, &[h])
            .and_then(|()| self.missing_classes(std::slice::from_ref(&obj)));
        let classes = match classes {
            Ok(c) => c,
            Err(e) => {
                self.header.handseed = seed;
                return Err(e);
            }
        };
        self.objects.insert(h, obj);
        self.classes.extend(classes);
        match self.objects.get_mut(&dictionary) {
            Some(Object::Other {
                object: NonGraphical::Dictionary(d),
                ..
            }) => d.entries.push((name.to_owned(), h)),
            Some(Object::Other {
                object: NonGraphical::DictionaryWithDefault(d),
                ..
            }) => d.dictionary.entries.push((name.to_owned(), h)),
            _ => {}
        }
        self.ref_index
            .get_mut()
            .added(&self.objects, &[h], &[(dictionary, h, SOFT_OWNER)]);
        Ok(h)
    }

    /// Add a layer to the layer table; see [`Drawing::add_entity`] for what
    /// is checked. A duplicate name (compared without case) is
    /// [`Error::Invalid`].
    ///
    /// A null linetype becomes the Continuous linetype, and a null plot
    /// style or material layer `0`'s (as [`Layer::new`] leaves them), when
    /// the drawing has them.
    pub fn add_layer(&mut self, mut layer: Layer) -> Result<Handle> {
        if layer.linetype.0 == 0 {
            layer.linetype = self.continuous_linetype().unwrap_or_default();
        }
        if let Some(l0) = self.layer_by_name("0").and_then(|h| self.layer(h)) {
            if layer.plotstyle.0 == 0 {
                layer.plotstyle = l0.plotstyle;
            }
            if layer.material.0 == 0 {
                layer.material = l0.material;
            }
        }
        self.add_table_entry(TableKind::Layer, NonGraphical::Layer(layer))
    }

    /// The Continuous linetype: the header's, or the linetype of that name.
    fn continuous_linetype(&self) -> Option<Handle> {
        let h = self.header.ltype_continuous;
        if matches!(
            self.objects.get(&h),
            Some(Object::Other {
                object: NonGraphical::Linetype(_),
                ..
            })
        ) {
            return Some(h);
        }
        self.linetypes()
            .find(|(_, l)| names_eq(&l.entry.name, "Continuous"))
            .map(|(h, _)| h)
    }

    /// The current text style: $TEXTSTYLE when it is a text style, else
    /// `Standard`.
    pub(super) fn current_text_style(&self) -> Option<Handle> {
        let h = self.header.textstyle;
        if matches!(
            self.objects.get(&h),
            Some(Object::Other {
                object: NonGraphical::TextStyle(_),
                ..
            })
        ) {
            return Some(h);
        }
        self.text_styles()
            .find(|(_, s)| names_eq(&s.entry.name, "Standard"))
            .map(|(h, _)| h)
    }

    /// Add a linetype; see [`Drawing::add_layer`].
    pub fn add_linetype(&mut self, linetype: Linetype) -> Result<Handle> {
        self.add_table_entry(TableKind::Linetype, NonGraphical::Linetype(linetype))
    }

    /// Add a text style (an unnamed shape file may share the empty name);
    /// see [`Drawing::add_layer`].
    pub fn add_text_style(&mut self, style: TextStyle) -> Result<Handle> {
        self.add_table_entry(TableKind::Style, NonGraphical::TextStyle(style))
    }

    /// Add a dimension style; see [`Drawing::add_layer`].
    pub fn add_dim_style(&mut self, style: DimStyle) -> Result<Handle> {
        self.add_table_entry(TableKind::DimStyle, NonGraphical::DimStyle(Box::new(style)))
    }

    /// Add an empty block named `name`: a block record with its BLOCK and
    /// ENDBLK entities (on layer `0`), base point at the origin. The name
    /// must not be empty, start with `*` (anonymous and layout blocks) or be
    /// another block's, compared without case ([`Error::Invalid`]).
    pub fn add_block(&mut self, name: &str) -> Result<Handle> {
        if name.starts_with('*') {
            return Err(invalid(
                None,
                format!("block name {name:?}: names starting with * are reserved"),
            ));
        }
        self.check_new_name(TableKind::Block, name, false)?;
        let Some((control, _)) = self.table_control(TableKind::Block) else {
            return Err(invalid(None, "the drawing has no block table control"));
        };
        let Some(layer0) = self.layer_by_name("0") else {
            return Err(invalid(None, "the drawing has no layer 0"));
        };
        self.sync_index();
        let seed = self.header.handseed;
        let [record, begin, end] = self.alloc_handles(3)?[..] else {
            unreachable!("three handles were allocated")
        };
        let marker = |h: Handle, entity: Entity| {
            let mut common = EntityCommon::new(layer0);
            common.obj.handle = h;
            common.obj.owner = record;
            Object::Entity { common, entity }
        };
        let objects = [
            Object::Other {
                common: ObjectCommon {
                    handle: record,
                    owner: control,
                    ..ObjectCommon::new()
                },
                object: NonGraphical::BlockRecord(BlockRecord {
                    entry: TableEntry {
                        name: name.to_owned(),
                        ..TableEntry::default()
                    },
                    anonymous: false,
                    has_attdefs: false,
                    is_xref: false,
                    xref_overlaid: false,
                    xref_unloaded: false,
                    base_point: [0.0; 3],
                    xref_path: String::new(),
                    description: String::new(),
                    preview: Vec::new(),
                    insert_units: 0,
                    explodable: true,
                    block_scaling: 0,
                    block_entity: begin,
                    entities: EntityList::Owned(Vec::new()),
                    end_block: end,
                    inserts: Vec::new(),
                    layout: Handle(0),
                }),
            },
            marker(
                begin,
                Entity::Block(Block {
                    name: name.to_owned(),
                }),
            ),
            marker(end, Entity::EndBlk),
        ];
        if let Some(e) = objects
            .iter()
            .find_map(|o| self.check_new(o, &[record, begin, end]).err())
        {
            self.header.handseed = seed;
            return Err(e);
        }
        for obj in objects {
            self.objects.insert(obj.handle(), obj);
        }
        if let Some(Object::Other {
            object: NonGraphical::TableControl(c),
            ..
        }) = self.objects.get_mut(&control)
        {
            c.entries.push(record);
        }
        self.ref_index.get_mut().added(
            &self.objects,
            &[record, begin, end],
            &[(control, record, SOFT_OWNER)],
        );
        Ok(record)
    }

    // ---- removing ----

    /// What object `x` is and why it can never be removed, if so: table
    /// controls, the named object dictionary, layers `0` and `Defpoints`,
    /// the `ByLayer`, `ByBlock` and `Continuous` linetypes, the `Standard`
    /// text and dimension styles, layouts, and model and paper space blocks.
    fn protected(&self, x: Handle) -> Option<String> {
        let h = &self.header;
        let named = |name: &str, names: &[&str]| names.iter().any(|n| names_eq(name, n));
        let why = match self.objects.get(&x)? {
            Object::Other {
                object: NonGraphical::TableControl(c),
                ..
            } => format!("the {:?} table control holds the table", c.kind),
            _ if x == self.named_objects() => {
                "the named object dictionary is the root of the drawing's objects".into()
            }
            Object::Other {
                object: NonGraphical::Layer(l),
                ..
            } if named(&l.entry.name, &["0", "Defpoints"]) => format!(
                "layer {:?} is required by AutoCAD (layer 0 holds block definitions, Defpoints \
                 dimension points)",
                l.entry.name
            ),
            Object::Other {
                object: NonGraphical::Linetype(l),
                ..
            } if [h.ltype_bylayer, h.ltype_byblock, h.ltype_continuous].contains(&x)
                || named(&l.entry.name, &["ByLayer", "ByBlock", "Continuous"]) =>
            {
                format!(
                    "linetype {:?} is one of the built-in ByLayer, ByBlock and Continuous \
                     linetypes",
                    l.entry.name
                )
            }
            Object::Other {
                object: NonGraphical::TextStyle(s),
                ..
            } if named(&s.entry.name, &["Standard"]) => {
                "text style Standard is the default text style".into()
            }
            Object::Other {
                object: NonGraphical::DimStyle(s),
                ..
            } if named(&s.entry.name, &["Standard"]) => {
                "dimension style Standard is the default dimension style".into()
            }
            Object::Other {
                object: NonGraphical::Layout(l),
                ..
            } => format!(
                "layout {:?} and its block record make a layout tab together",
                l.name
            ),
            Object::Other {
                object: NonGraphical::BlockRecord(b),
                ..
            } if x == self.model_space()
                || x == h.block_record_paper_space
                || b.layout.0 != 0
                || names_eq(&b.entry.name, "*Model_Space")
                || b.entry.name.to_lowercase().starts_with("*paper_space") =>
            {
                format!(
                    "block {:?} is model space or a layout's paper space",
                    b.entry.name
                )
            }
            _ => return None,
        };
        Some(format!("{x}: {why}"))
    }

    /// `roots` and the objects they own, recursively: the objects each
    /// refers to (in the index) whose owner is it, and its extension
    /// dictionary. Removing every attribute of an insert takes its SEQEND
    /// too. Lists not built (broken R2000 chains) cannot be followed or
    /// changed. Each handle is listed once, roots first.
    fn removal_set(&self, roots: &[Handle], index: &RefIndex) -> Result<Vec<Handle>> {
        let linked = |x: Handle| {
            invalid(
                Some(x),
                "its entity list was not built (a broken R2000 chain); it cannot be changed",
            )
        };
        let mut out: Vec<Handle> = Vec::with_capacity(roots.len());
        let mut seen: HashSet<Handle> = HashSet::with_capacity(roots.len());
        for &h in roots {
            if seen.insert(h) {
                out.push(h);
            }
        }
        for &h in roots {
            let Some(owner) = self.owner_of(h) else {
                continue;
            };
            match self.objects.get(&owner) {
                Some(o) if has_linked_list(o) => return Err(linked(owner)),
                Some(Object::Entity {
                    entity: Entity::Insert(i) | Entity::MInsert(MInsert { insert: i, .. }),
                    ..
                }) => {
                    let all_go = matches!(&i.attribs, EntityList::Owned(list)
                        if !list.is_empty() && list.iter().all(|a| seen.contains(a)));
                    if let Some(s) = i.seqend.filter(|s| all_go && !seen.contains(s)) {
                        seen.insert(s);
                        out.push(s);
                    }
                }
                _ => {}
            }
        }
        let mut next = 0;
        while let Some(&x) = out.get(next) {
            next += 1;
            let Some(obj) = self.objects.get(&x) else {
                continue;
            };
            if has_linked_list(obj) {
                return Err(linked(x));
            }
            let refs = index.refs_of(x).iter().map(|&(t, _)| t);
            for c in refs.chain(obj.object_common().xdict) {
                if !seen.contains(&c) && self.owner_of(c) == Some(x) {
                    seen.insert(c);
                    out.push(c);
                }
            }
        }
        Ok(out)
    }

    /// Why entity `obj` is never removed on its own (it goes with its
    /// owner), if so.
    fn removed_with_owner(obj: &Object) -> Option<&'static str> {
        let Object::Entity { entity, .. } = obj else {
            return None;
        };
        match entity {
            Entity::Block(_) | Entity::EndBlk | Entity::SeqEnd => {
                Some("BLOCK, ENDBLK and SEQEND entities are removed with their owner")
            }
            Entity::VertexMesh(_) | Entity::VertexPface(_) | Entity::VertexPfaceFace(_) => Some(
                "a mesh's vertices and faces are removed with the mesh: its counts and face \
                 indices depend on every one",
            ),
            _ => None,
        }
    }

    /// Remove object `h` and the objects it owns, and return it.
    ///
    /// Owned objects go with it: an insert's attributes and SEQEND, a
    /// polyline's vertices and SEQEND, a block record's BLOCK, ENDBLK and
    /// entities, a dictionary's owned entries, every extension dictionary
    /// and its contents. The removed objects leave their owner's list
    /// (entity list, attributes, vertices, control or dictionary entries),
    /// groups, sort tables, layout viewport lists, block insert lists,
    /// reactor lists and extension dictionary links. Removing an insert's
    /// last attribute removes its SEQEND; removing a block's last ATTDEF
    /// clears its `has_attdefs`.
    ///
    /// Refused, with nothing changed:
    ///
    /// - [`Error::NotFound`] for a missing object;
    /// - [`Error::Protected`] for objects that are never removed: layers `0`
    ///   and `Defpoints`, the `ByLayer`, `ByBlock` and `Continuous`
    ///   linetypes, the `Standard` text and dimension styles, layouts and
    ///   model and paper space blocks, table controls, the named object
    ///   dictionary;
    /// - [`Error::Invalid`] for BLOCK, ENDBLK and SEQEND entities and a
    ///   polyface or polygon mesh's vertices and faces (remove their owner:
    ///   a mesh's counts and face indices depend on every vertex), for
    ///   owners whose R2000 list was not built, and when some object's
    ///   references cannot all be listed (it cannot be encoded as R2018), as
    ///   the removal could then leave a reference behind;
    /// - [`Error::InUse`] with the referring objects and header variables
    ///   when a removed object is still referred to other than through the
    ///   links above: a layer, linetype, text style, dimension style,
    ///   material or plot style used by an entity, a layer or a style; a
    ///   block used by an INSERT, MINSERT or dimension; a header variable
    ///   (`$CLAYER`, `$CELTYPE`, `$TEXTSTYLE`, `$DIMSTYLE`, ...); any
    ///   reference to a non-entity. An entity referred to only through soft
    ///   pointers (a hatch's boundary, ...) is removed and the pointers left
    ///   dangling, as AutoCAD files often have; a hard reference refuses it.
    ///
    /// To remove several objects that refer to each other (the
    /// unconvertible objects of an old drawing, say), use
    /// [`Drawing::remove_all`]. It is also the way to remove many entities:
    /// each call passes once over the lists the removed objects leave (an
    /// owning block's entity list, say), however many go, while removing
    /// them one by one passes over such a list once per call.
    ///
    /// The work is that of the removed objects and the objects that refer to
    /// them, not of the drawing (see `drawing::refs`).
    pub fn remove(&mut self, h: Handle) -> Result<Object> {
        let mut removed = self.remove_all(&[h])?;
        removed.pop().ok_or(Error::NotFound(h))
    }

    /// Remove the objects `handles` as one removal, and return them in the
    /// order given (a handle given twice, once).
    ///
    /// It is [`Drawing::remove`] for the whole set at once: each object
    /// takes the objects it owns with it, and references from one object of
    /// the set (or owned by one) to another do not block the removal, so
    /// objects that refer to each other, or that one of them owns, go
    /// together. A handle owned by another one of the set (a BLOCK, SEQEND
    /// or mesh vertex whose owner is in the set, say) is removed with its
    /// owner.
    ///
    /// The removal is atomic: when anything refuses it, nothing changes.
    /// The refusals are those of [`Drawing::remove`]: [`Error::NotFound`]
    /// for a handle with no object, [`Error::Protected`],
    /// [`Error::Invalid`], and [`Error::InUse`] naming the objects outside
    /// the set (and header variables) that still refer to it. The caller
    /// decides what to do with those (reset an entity's material to
    /// `ByLayer`, add the referrer to the set, ...) and calls again.
    ///
    /// On an R2000 or R2004 drawing the set often has referrers outside it
    /// (entities' materials and color books, extended data handles, XRECORD
    /// object ids), so the call can return [`Error::InUse`]; the application
    /// lets go of those references and calls again. The README and
    /// `examples/app_workflow.rs` show the whole recovery.
    ///
    /// ```no_run
    /// # use opendwg::Error;
    /// # fn main() -> opendwg::Result<()> {
    /// let mut d = opendwg::Drawing::open("old-r2004.dwg")?;
    /// let handles: Vec<_> = d.unconvertible_objects().into_iter().map(|(h, _)| h).collect();
    /// match d.remove_all(&handles) {
    ///     Ok(_) => {}
    ///     Err(Error::InUse { objects, .. }) => {
    ///         // Release what `objects` hold of the set (see the README),
    ///         // then remove again.
    /// #       let _ = objects;
    ///         d.remove_all(&handles)?;
    ///     }
    ///     Err(e) => return Err(e),
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn remove_all(&mut self, handles: &[Handle]) -> Result<Vec<Object>> {
        let mut roots: Vec<Handle> = Vec::with_capacity(handles.len());
        let mut seen = HashSet::with_capacity(handles.len());
        for &h in handles {
            if !self.objects.contains_key(&h) {
                return Err(Error::NotFound(h));
            }
            if seen.insert(h) {
                roots.push(h);
            }
        }
        for &h in &roots {
            if let Some(why) = self.protected(h) {
                return Err(Error::Protected(why));
            }
        }
        let mut state = std::mem::take(self.ref_index.get_mut());
        let result = self.remove_with(&roots, &mut state);
        *self.ref_index.get_mut() = state;
        result
    }

    /// [`Drawing::remove_all`] with the index taken out of the drawing.
    fn remove_with(&mut self, roots: &[Handle], state: &mut IndexState) -> Result<Vec<Object>> {
        let index = state.ensure(&self.objects);
        // Objects never removed on their own are taken with their owner,
        // which must then be in the set.
        let (with_owner, own): (Vec<Handle>, Vec<Handle>) = roots.iter().copied().partition(|h| {
            self.objects
                .get(h)
                .and_then(Self::removed_with_owner)
                .is_some()
        });
        let set = self.removal_set(&own, index)?;
        let in_set: HashSet<Handle> = set.iter().copied().collect();
        if let Some(&h) = with_owner.iter().find(|h| !in_set.contains(h)) {
            let why = self
                .objects
                .get(&h)
                .and_then(Self::removed_with_owner)
                .unwrap_or_default();
            return Err(invalid(Some(h), why));
        }
        if let Some(why) = set.iter().find_map(|&x| self.protected(x)) {
            return Err(Error::Protected(why));
        }
        let (plan, work) = self.plan_removal(&set, index)?;
        state.work += work;
        // Only now: a refused removal changes nothing, the seed included.
        self.normalise_seed();
        let attdef_blocks: BTreeSet<Handle> = set
            .iter()
            .filter(|x| {
                matches!(
                    self.objects.get(x),
                    Some(Object::Entity {
                        entity: Entity::AttDef(_),
                        ..
                    })
                )
            })
            .filter_map(|&x| self.owner_of(x))
            .filter(|b| !in_set.contains(b))
            .collect();
        for (&r, (xs, _)) in &plan {
            if let Some(obj) = self.objects.get_mut(&r) {
                detach(obj, xs);
                state.detached += 1;
            }
        }
        for &x in &set {
            let block = match self.objects.get(&x) {
                Some(Object::Entity { .. }) => self.owner_of(x),
                _ => Some(x),
            };
            if let Some(block) = block {
                state.forget_inserts(block);
            }
        }
        let mut removed: BTreeMap<Handle, Object> = BTreeMap::new();
        for &x in &set {
            if let Some(obj) = self.objects.remove(&x) {
                removed.insert(x, obj);
            }
        }
        if let Some(index) = state.index_mut() {
            for &x in &set {
                index.delete(x);
            }
        }
        for (r, (xs, recompute)) in plan {
            if recompute {
                state.refresh(&self.objects, r);
            } else if let Some(index) = state.index_mut() {
                index.remove_edges(r, &xs);
                if let Some(obj) = self.objects.get(&r) {
                    index.set_reactors(r, &obj.object_common().reactors);
                }
            }
        }
        for b in attdef_blocks {
            let has = self.entities_in(b).any(|e| {
                matches!(
                    self.objects.get(&e),
                    Some(Object::Entity {
                        entity: Entity::AttDef(_),
                        ..
                    })
                )
            });
            if let Some(Object::Other {
                object: NonGraphical::BlockRecord(rec),
                ..
            }) = self.objects.get_mut(&b)
            {
                rec.has_attdefs = has;
            }
        }
        Ok(roots.iter().filter_map(|h| removed.remove(h)).collect())
    }

    /// For removing `set`: the links to drop, by object, with whether that
    /// object's references must be recomputed afterwards (false when only
    /// list entries and reactors go, which the index drops directly); and
    /// the number of objects whose references were computed. Or why the
    /// removal is refused (see [`Drawing::remove`]).
    #[allow(clippy::type_complexity)]
    fn plan_removal(
        &self,
        set: &[Handle],
        index: &RefIndex,
    ) -> Result<(BTreeMap<Handle, (HashSet<Handle>, bool)>, usize)> {
        let in_set: HashSet<Handle> = set.iter().copied().collect();
        let unknown: Vec<Diagnostic> = index
            .incomplete()
            .filter(|h| !in_set.contains(h))
            .map(|u| Diagnostic {
                severity: Severity::Error,
                handle: Some(u),
                message: "the object's references cannot all be listed (it cannot be \
                          encoded as R2018), so nothing can be removed safely"
                    .into(),
            })
            .collect();
        if !unknown.is_empty() {
            return Err(Error::Invalid(unknown));
        }
        let header_vars: Vec<&'static str> = handle_vars(&self.header)
            .into_iter()
            .filter(|(_, v)| in_set.contains(v))
            .map(|(name, _)| name)
            .collect();
        let mut blocking = BTreeSet::new();
        // Each referrer outside the set, with the removed objects it refers to.
        let mut referred: BTreeMap<Handle, HashSet<Handle>> = BTreeMap::new();
        for &x in set {
            for (r, _) in index.referrers(x) {
                if !in_set.contains(&r) {
                    referred.entry(r).or_default().insert(x);
                }
            }
        }
        let mut work = 0;
        let mut plan: BTreeMap<Handle, (HashSet<Handle>, bool)> = BTreeMap::new();
        for (r, xs) in referred {
            let Some(obj) = self.objects.get(&r) else {
                continue;
            };
            if let Some(only_lists) = xs
                .iter()
                .map(|&x| list_entries_only(obj, x))
                .collect::<Option<Vec<bool>>>()
            {
                if only_lists.iter().all(|&ok| ok) {
                    plan.insert(r, (xs, false));
                } else {
                    blocking.insert(r);
                }
                continue;
            }
            let mut after = obj.clone();
            detach(&mut after, &xs);
            let (left, complete) = object_refs(&after);
            work += 1;
            let mut ok = complete;
            for &x in &xs {
                let is_entity = matches!(self.objects.get(&x), Some(Object::Entity { .. }));
                ok &= left
                    .iter()
                    .filter(|&&(t, _)| t == x)
                    .all(|&(_, code)| is_entity && matches!(code, SOFT_OWNER | SOFT_POINTER));
            }
            if ok {
                plan.insert(r, (xs, true));
            } else {
                blocking.insert(r);
            }
        }
        if !blocking.is_empty() || !header_vars.is_empty() {
            return Err(Error::InUse {
                objects: blocking.into_iter().collect(),
                header_vars,
            });
        }
        for &x in set {
            let owner = self.owner_of(x).filter(|o| {
                self.objects
                    .get(o)
                    .is_some_and(|ob| ob.object_common().xdict == Some(x))
            });
            for r in index.reactor_holders(x).chain(owner) {
                if !in_set.contains(&r) {
                    let (xs, _) = plan.entry(r).or_insert_with(|| (HashSet::new(), false));
                    xs.insert(x);
                }
            }
        }
        Ok((plan, work))
    }
}

/// For the lists the index can update without recomputing (a block record's
/// entities, a table control's entries, a dictionary's entries): whether
/// every reference of `obj` to `x` goes when [`detach`] drops `x`; `None`
/// for other objects, and when `obj`'s extended data refers to `x` (the
/// general check then decides).
fn list_entries_only(obj: &Object, x: Handle) -> Option<bool> {
    let Object::Other { object, common } = obj else {
        return None;
    };
    let in_xdata = common.xdata.iter().any(|app| {
        app.app == x
            || app.items.iter().any(|item| {
                matches!(item, crate::object::XDataItem::Handle(h)
                    | crate::object::XDataItem::LayerRef(h) if *h == x)
            })
    });
    if in_xdata {
        return None;
    }
    Some(match object {
        NonGraphical::BlockRecord(b) => {
            ![b.block_entity, b.end_block, b.layout, b.entry.xref_block].contains(&x)
        }
        NonGraphical::TableControl(c) => c.kind == TableKind::DimStyle || !c.extra.contains(&x),
        NonGraphical::Dictionary(_) => true,
        NonGraphical::DictionaryWithDefault(d) => d.default_entry != x,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::{Dictionary, Line, XDataItem};
    use crate::section::HeaderVars;

    fn drawing(objects: impl IntoIterator<Item = (u64, NonGraphical)>) -> Drawing {
        Drawing {
            source_version: None,
            header: HeaderVars::default(),
            classes: Vec::new(),
            objects: objects
                .into_iter()
                .map(|(h, object)| {
                    let common = ObjectCommon {
                        handle: Handle(h),
                        ..ObjectCommon::new()
                    };
                    (Handle(h), Object::Other { common, object })
                })
                .collect(),
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

    fn dictionary(entries: &[u64]) -> NonGraphical {
        NonGraphical::Dictionary(Dictionary {
            entries: entries
                .iter()
                .map(|&h| (format!("E{h}"), Handle(h)))
                .collect(),
            ..Dictionary::default()
        })
    }

    /// The R2018 corpus sample, when fetched.
    fn sample() -> Option<Drawing> {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("corpus/files/acadsharp-sample-ac1032.dwg");
        if !p.is_file() {
            assert!(
                std::env::var("OPENDWG_REQUIRE_CORPUS").as_deref() != Ok("1"),
                "OPENDWG_REQUIRE_CORPUS=1 but there is no corpus"
            );
            eprintln!("SKIPPED: no corpus; run scripts/fetch-corpus.sh");
            return None;
        }
        Some(Drawing::open(p).unwrap())
    }

    fn work(d: &Drawing) -> usize {
        d.ref_index.lock().work
    }

    fn line(d: &mut Drawing, i: usize) -> Handle {
        let ms = d.model_space();
        let l0 = d.layer_by_name("0").unwrap();
        let x = i as f64;
        d.add_entity(
            ms,
            EntityCommon::new(l0),
            Entity::Line(Line {
                start: [x, 0.0, 0.0],
                end: [x, 1.0, 0.0],
                thickness: 0.0,
                extrusion: [0.0, 0.0, 1.0],
            }),
        )
        .unwrap()
    }

    /// The index kept up to date equals one built afresh.
    fn assert_index_fresh(d: &Drawing) {
        let kept = d.ref_index.lock().ensure(&d.objects).snapshot();
        let mut fresh = IndexState::default();
        let fresh = fresh.ensure(&d.objects).snapshot();
        if kept != fresh {
            let diff = |a: &BTreeMap<Handle, BTreeMap<Handle, Vec<u8>>>,
                        b: &BTreeMap<Handle, BTreeMap<Handle, Vec<u8>>>| {
                a.iter()
                    .filter(|(k, v)| b.get(k) != Some(v))
                    .map(|(k, v)| format!("{k}: {v:?} vs {:?}", b.get(k)))
                    .take(5)
                    .collect::<Vec<_>>()
            };
            panic!(
                "the kept index differs from a fresh one: referrers {:?} {:?}; holders {}; \
                 incomplete {}",
                diff(&kept.0, &fresh.0),
                diff(&fresh.0, &kept.0),
                kept.1 == fresh.1,
                kept.2 == fresh.2
            );
        }
    }

    /// Handles are never reused: with the largest handle taken, or once the
    /// seed reaches it, allocation fails (the seed unchanged), also after
    /// that object is removed; several handles at once are consecutive, and
    /// a stale seed moves past every handle.
    #[test]
    fn alloc_handle_at_the_end_of_the_handle_space() {
        let exhausted = |r: Result<Vec<Handle>>| matches!(r, Err(Error::Invalid(d)) if d[0].message.contains("used up"));
        let mut d = drawing([
            (1, NonGraphical::Placeholder),
            (3, NonGraphical::Placeholder),
            (u64::MAX, NonGraphical::Placeholder),
        ]);
        d.header.handseed = Handle(2);
        assert!(exhausted(d.alloc_handle().map(|h| vec![h])));
        assert_eq!(d.header.handseed, Handle(2));
        assert!(exhausted(d.alloc_handles(2)));
        d.remove(Handle(u64::MAX)).unwrap();
        assert_eq!(d.header.handseed, Handle(u64::MAX));
        assert!(exhausted(d.alloc_handles(1)));
        // The seed reaches the largest handle, which is never given.
        let mut d = drawing([(0x10, NonGraphical::Placeholder)]);
        d.header.handseed = Handle(u64::MAX - 2);
        assert!(exhausted(d.alloc_handles(3)));
        assert_eq!(d.header.handseed, Handle(u64::MAX - 2));
        assert_eq!(
            d.alloc_handles(2).unwrap(),
            [Handle(u64::MAX - 2), Handle(u64::MAX - 1)]
        );
        assert_eq!(d.header.handseed, Handle(u64::MAX));
        assert!(exhausted(d.alloc_handles(1)));
        assert_eq!(d.alloc_handles(0).unwrap(), []);
        // A normal seed is used as is; a stale one moves past every handle.
        let mut d = drawing([(0x10, NonGraphical::Placeholder)]);
        d.header.handseed = Handle(0x20);
        assert_eq!(d.alloc_handles(2).unwrap(), [Handle(0x20), Handle(0x21)]);
        assert_eq!(d.header.handseed, Handle(0x22));
        d.header.handseed = Handle(3);
        assert_eq!(d.alloc_handle().unwrap(), Handle(0x11));
    }

    /// The reverse index is built on the first query and reused; a change
    /// through `get_mut` recomputes that one object at the next query.
    #[test]
    fn reference_index_is_built_once_and_kept_up_to_date() {
        let mut d = drawing([
            (0x10, dictionary(&[0x11, 0x12])),
            (0x11, NonGraphical::Placeholder),
            (0x12, dictionary(&[0x11])),
        ]);
        assert_eq!(work(&d), 0);
        assert_eq!(d.referrers(Handle(0x11)), [Handle(0x10), Handle(0x12)]);
        assert_eq!(work(&d), 3);
        assert_eq!(d.referrers(Handle(0x12)), [Handle(0x10)]);
        assert_eq!(work(&d), 3);
        if let Some(Object::Other {
            object: NonGraphical::Dictionary(dict),
            ..
        }) = d.get_mut(Handle(0x12))
        {
            dict.entries.clear();
        }
        assert_eq!(d.referrers(Handle(0x11)), [Handle(0x10)]);
        assert_eq!(work(&d), 4);
        assert_index_fresh(&d);
    }

    /// Removing k entities from a drawing re-encodes O(k) objects whatever
    /// the drawing's size, adding k entities computes the references of
    /// those k, and the index kept up to date equals a fresh one.
    #[test]
    fn mutation_work_follows_the_change_not_the_drawing() {
        let Some(base) = sample() else { return };
        let k = 50;
        let mut deltas = vec![];
        for extra in [100, 1000] {
            let mut d = base.clone();
            let lines: Vec<Handle> = (0..extra).map(|i| line(&mut d, i)).collect();
            d.referrers(d.model_space());
            let built = work(&d);
            assert_eq!(built, d.objects.len());
            for &h in &lines[..k] {
                d.remove(h).unwrap();
            }
            let removing = work(&d) - built;
            assert!(removing <= k, "removing {k} took {removing} encodes");
            for i in 0..k {
                line(&mut d, i);
            }
            assert_eq!(work(&d) - built - removing, k, "adding {k}");
            deltas.push(removing);
            assert_index_fresh(&d);
        }
        assert_eq!(deltas[0], deltas[1], "independent of the drawing's size");
    }

    /// Removing k entities at once from a block of 1k or of 100k entities
    /// passes once over the block's lists and recomputes no references,
    /// whatever the block's size; the entities left keep their order.
    #[test]
    fn removal_work_is_independent_of_the_block_size() {
        let k = 200;
        let mut counts = vec![];
        for size in [1_000, 100_000] {
            let mut d = Drawing::new();
            let lines: Vec<Handle> = (0..size).map(|i| line(&mut d, i)).collect();
            d.referrers(d.model_space());
            let (built, detached) = {
                let s = d.ref_index.lock();
                (s.work, s.detached)
            };
            // Every other line, from the middle of the block.
            let gone: Vec<Handle> = lines[size / 2..]
                .iter()
                .step_by(2)
                .take(k)
                .copied()
                .collect();
            let removed = d.remove_all(&gone).unwrap();
            assert_eq!(removed.len(), k);
            let s = d.ref_index.lock();
            counts.push((s.work - built, s.detached - detached));
            drop(s);
            let gone: HashSet<Handle> = gone.into_iter().collect();
            let left: Vec<Handle> = d.entities_in(d.model_space()).collect();
            let expected: Vec<Handle> = lines
                .iter()
                .copied()
                .filter(|h| !gone.contains(h))
                .collect();
            assert_eq!(left, expected, "order kept");
            assert_index_fresh(&d);
        }
        assert_eq!(counts[0], counts[1], "independent of the block's size");
        assert_eq!(counts[0], (0, 1), "one pass over model space, no encodes");
    }

    /// The circular insert check scans each block's entities once: adding
    /// more inserts of a block reuses what it found, while a change through
    /// `get_mut` or a removal makes it look again, so cycles made that way
    /// are still refused.
    #[test]
    fn insert_cycle_check_scans_each_block_once() {
        use crate::object::Insert;
        let mut d = Drawing::new();
        let l0 = d.layer_by_name("0").unwrap();
        let [a, b, c] = ["A", "B", "C"].map(|n| d.add_block(n).unwrap());
        let ins = |d: &mut Drawing, into: Handle, of: Handle| {
            d.add_entity(
                into,
                EntityCommon::new(l0),
                Entity::Insert(Insert::new(of, [0.0; 3])),
            )
        };
        let scans = |d: &Drawing| d.ref_index.lock().block_scans;
        ins(&mut d, a, b).unwrap();
        let line_in_b = {
            let x = Entity::Line(Line {
                start: [0.0; 3],
                end: [1.0, 0.0, 0.0],
                thickness: 0.0,
                extrusion: [0.0, 0.0, 1.0],
            });
            d.add_entity(b, EntityCommon::new(l0), x).unwrap()
        };
        // A into C scans A and B, once.
        ins(&mut d, c, a).unwrap();
        let first = scans(&d);
        assert_eq!(first, 2);
        for _ in 0..10 {
            ins(&mut d, c, a).unwrap();
        }
        assert_eq!(scans(&d), first, "cached");
        // B's line made an insert of C through get_mut: C into A is now
        // circular (C holds A holds B holds C).
        if let Some((_, e)) = d.entity_mut(line_in_b) {
            *e = Entity::Insert(Insert::new(c, [0.0; 3]));
        }
        assert!(matches!(ins(&mut d, a, c), Err(Error::Invalid(_))));
        assert!(matches!(ins(&mut d, c, a), Err(Error::Invalid(_))));
        // Removed again, A goes into C once more.
        d.remove(line_in_b).unwrap();
        ins(&mut d, c, a).unwrap();
        assert!(matches!(ins(&mut d, b, c), Err(Error::Invalid(_))));
    }

    /// Removing every removable object of the sample in turn, each from the
    /// same copy, keeps the index equal to a fresh one.
    #[test]
    fn index_kept_up_to_date_through_removals() {
        let Some(mut d) = sample() else { return };
        d.referrers(d.model_space());
        let handles: Vec<Handle> = d.objects.keys().copied().collect();
        let mut removed = 0;
        for h in handles {
            if d.objects.contains_key(&h) && d.remove(h).is_ok() {
                removed += 1;
            }
        }
        assert!(removed > 100, "{removed}");
        assert_index_fresh(&d);
    }

    /// A tiny xorshift generator: the mixed-operation test is reproducible
    /// without a dependency.
    struct XorShift(u64);

    impl XorShift {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: usize) -> usize {
            (self.next() % n.max(1) as u64) as usize
        }

        fn pick<T: Copy>(&mut self, v: &[T]) -> Option<T> {
            (!v.is_empty()).then(|| v[self.below(v.len())])
        }
    }

    /// A seeded run of mixed mutations on the sample keeps the index equal to
    /// a fresh build, checked along the way and at the end: lines, inserts
    /// and MINSERTs with attributes (block insert lists), 3D polylines and
    /// polyface meshes (children and SEQENDs), layers and blocks (control
    /// entries, BLOCK and ENDBLK), `get_mut` edits of a layer's linetype or
    /// an entity's extended data followed by more mutations, and removals of
    /// added and original objects.
    #[test]
    fn index_kept_up_to_date_through_mixed_mutations() {
        use crate::object::{
            Insert, PolyfaceMesh, Polyline3d, Vertex3d, VertexPfaceFace, XDataApp,
        };
        let Some(mut d) = sample() else { return };
        let attrib = d
            .objects
            .values()
            .find_map(|o| match o {
                Object::Entity {
                    entity: Entity::Attrib(a),
                    ..
                } if a.mtext.is_none() => Some((**a).clone()),
                _ => None,
            })
            .unwrap();
        let acad = d
            .objects
            .iter()
            .find_map(|(&h, o)| match o {
                Object::Other {
                    object: NonGraphical::AppId(a),
                    ..
                } if a.entry.name == "ACAD" => Some(h),
                _ => None,
            })
            .unwrap();
        let originals: Vec<Handle> = d.objects.keys().copied().collect();
        let ms = d.model_space();
        let l0 = d.layer_by_name("0").unwrap();
        let mut rng = XorShift(0x9E37_79B9_7F4A_7C15);
        let mut added: Vec<Handle> = vec![];
        let mut blocks: Vec<Handle> = vec![];
        let mut layers: Vec<Handle> = vec![l0];
        let (mut ok, mut refused) = (0, 0);
        // Successes per operation (arms 0 and 1 are one operation).
        let mut done = [0usize; 12];
        d.referrers(ms);
        for step in 0..400 {
            let layer = rng.pick(&layers).unwrap_or(l0);
            let insert = |block| Insert {
                insertion: [0.0; 3],
                scale: [1.0; 3],
                rotation: 0.0,
                extrusion: [0.0, 0.0, 1.0],
                block,
                attribs: EntityList::Owned(vec![]),
                seqend: None,
            };
            let attribs = |n: usize| {
                (0..n)
                    .map(|_| (EntityCommon::new(layer), attrib.clone()))
                    .collect::<Vec<_>>()
            };
            let arm = rng.below(12);
            let result: Result<Handle> = match arm {
                0 | 1 => {
                    let into = rng
                        .pick(&blocks)
                        .filter(|_| rng.below(2) == 0)
                        .unwrap_or(ms);
                    Ok(line(&mut d, step)).and_then(|h| {
                        if into == ms {
                            Ok(h)
                        } else {
                            d.remove(h)?;
                            d.add_entity(
                                into,
                                EntityCommon::new(layer),
                                Entity::Line(Line {
                                    start: [0.0; 3],
                                    end: [1.0; 3],
                                    thickness: 0.0,
                                    extrusion: [0.0, 0.0, 1.0],
                                }),
                            )
                        }
                    })
                }
                2 => match rng.pick(&blocks) {
                    Some(b) => {
                        let n = rng.below(3);
                        d.add_insert_with_attribs(
                            ms,
                            (EntityCommon::new(layer), Entity::Insert(insert(b))),
                            attribs(n),
                        )
                    }
                    None => continue,
                },
                3 => match rng.pick(&blocks) {
                    Some(b) => d.add_insert_with_attribs(
                        ms,
                        (
                            EntityCommon::new(layer),
                            Entity::MInsert(MInsert {
                                insert: insert(b),
                                columns: 2,
                                rows: 2,
                                column_spacing: 1.0,
                                row_spacing: 1.0,
                                read_attrib_code: Default::default(),
                            }),
                        ),
                        attribs(1),
                    ),
                    None => continue,
                },
                4 => {
                    let vertex = |x: f64| {
                        (
                            EntityCommon::new(layer),
                            Entity::Vertex3d(Vertex3d {
                                flags: 32,
                                point: [x, 0.0, 0.0],
                            }),
                        )
                    };
                    d.add_polyline(
                        ms,
                        (
                            EntityCommon::new(layer),
                            Entity::Polyline3d(Polyline3d {
                                spline_flags: 0,
                                closed_flags: 0,
                                vertices: EntityList::Owned(vec![]),
                                seqend: Handle(0),
                            }),
                        ),
                        (0..3).map(|i| vertex(f64::from(i))).collect(),
                    )
                }
                5 => {
                    let v = |x: f64| {
                        (
                            EntityCommon::new(layer),
                            Entity::VertexPface(Vertex3d {
                                flags: 192,
                                point: [x, x, 0.0],
                            }),
                        )
                    };
                    d.add_polyline(
                        ms,
                        (
                            EntityCommon::new(layer),
                            Entity::PolyfaceMesh(PolyfaceMesh {
                                vertex_count: 3,
                                face_count: 1,
                                vertices: EntityList::Owned(vec![]),
                                seqend: Handle(0),
                            }),
                        ),
                        vec![
                            v(0.0),
                            v(1.0),
                            v(2.0),
                            (
                                EntityCommon::new(layer),
                                Entity::VertexPfaceFace(VertexPfaceFace {
                                    indices: [1, 2, 3, 0],
                                }),
                            ),
                        ],
                    )
                }
                6 => {
                    let Some(Object::Other {
                        object: NonGraphical::Layer(l),
                        ..
                    }) = d.objects.get(&l0)
                    else {
                        panic!()
                    };
                    let mut l = l.clone();
                    l.entry.name = format!("Mixed{step}");
                    d.add_layer(l).inspect(|&h| layers.push(h))
                }
                7 => d
                    .add_block(&format!("MixedBlock{step}"))
                    .inspect(|&h| blocks.push(h)),
                8 => {
                    // A layer's linetype changes through get_mut.
                    let linetypes: Vec<Handle> = d.linetypes().map(|(h, _)| h).collect();
                    let lt = rng.pick(&linetypes).unwrap();
                    let target = rng.pick(&layers).unwrap();
                    if let Some(Object::Other {
                        object: NonGraphical::Layer(l),
                        ..
                    }) = d.get_mut(target)
                    {
                        l.linetype = lt;
                        done[8] += 1;
                    }
                    continue;
                }
                9 => {
                    // An entity's extended data gains a handle to another
                    // object through get_mut.
                    let Some(e) = rng.pick(&added) else { continue };
                    let other = rng.pick(&originals).unwrap();
                    if let Some(Object::Entity { common, .. }) = d.get_mut(e) {
                        common.obj.xdata.push(XDataApp {
                            app: acad,
                            items: vec![XDataItem::Handle(other)],
                        });
                        done[9] += 1;
                    }
                    continue;
                }
                10 => match rng.pick(&added) {
                    Some(h) if d.objects.contains_key(&h) => d.remove(h).map(|_| h),
                    _ => continue,
                },
                _ => {
                    let h = rng.pick(&originals).unwrap();
                    if !d.objects.contains_key(&h) {
                        continue;
                    }
                    d.remove(h).map(|_| h)
                }
            };
            match result {
                Ok(h) => {
                    ok += 1;
                    done[arm.max(1)] += 1;
                    added.push(h);
                }
                Err(_) => refused += 1,
            }
            if step % 25 == 0 {
                assert_index_fresh(&d);
            }
        }
        assert_index_fresh(&d);
        assert!(ok > 150 && refused > 10, "{ok} done, {refused} refused");
        // Every operation succeeded at least once (arm 0 is counted with 1).
        for (arm, &n) in done.iter().enumerate().skip(1) {
            assert!(n > 0, "operation {arm} never succeeded: {done:?}");
        }
    }

    /// The reference lists rest on every encoder writing its references
    /// through `ObjWriter::href`: for every typed corpus object (when the
    /// corpus is fetched), the references recorded while encoding its common
    /// and type-specific data are exactly those in its encoded handle
    /// stream, in order, with their codes; and an entity's common references
    /// start with its ownership links, which `entity_common_refs` leaves out.
    #[test]
    fn recorded_references_are_the_whole_handle_stream() {
        use crate::object::{
            ObjReader, ObjWriter, encode_entity_common, encode_object, encode_object_common,
            entity_common_refs,
        };
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus/files");
        let files = std::fs::read_dir(&dir).into_iter().flatten();
        let mut checked = 0;
        for file in files.flatten() {
            let Ok(d) = Drawing::open(file.path()) else {
                continue;
            };
            for (h, obj) in d.objects() {
                if obj.raw().is_some() {
                    continue;
                }
                let Ok(bytes) = encode_object(obj, &d.classes) else {
                    continue;
                };
                let resolve = |r: &crate::handle::HandleRef| (r.resolve(h).unwrap(), r.code);
                let mut w = ObjWriter::recording(h);
                match obj {
                    Object::Entity { common, .. } => encode_entity_common(&mut w, common),
                    Object::Other { common, .. } => encode_object_common(&mut w, common),
                }
                .unwrap();
                let common: Vec<(Handle, u8)> = w.recorded_refs().iter().map(resolve).collect();
                let c = obj.object_common();
                let ownership: Vec<Handle> = match obj {
                    Object::Entity { common, .. } if common.entmode != 0 => None,
                    _ => Some(c.owner),
                }
                .into_iter()
                .chain(c.reactors.iter().copied())
                .chain(c.xdict)
                .collect();
                let (first, rest) = common.split_at(ownership.len());
                assert_eq!(first.iter().map(|r| r.0).collect::<Vec<_>>(), ownership);
                if let Object::Entity { common: ec, .. } = obj {
                    assert_eq!(entity_common_refs(ec).0, rest, "{h}");
                } else {
                    assert!(rest.is_empty());
                }
                let mut recorded = common.clone();
                recorded.extend(type_refs(obj).0);
                let (mut r, _) = ObjReader::split(&bytes, ObjWriter::VERSION, 30).unwrap();
                let mut stored = vec![];
                while r.handles.remaining_bits() >= 8 {
                    stored.push(resolve(&r.handles.read_handle_ref().unwrap()));
                }
                assert_eq!(recorded, stored, "{h} {}", obj.type_name());
                checked += 1;
            }
        }
        if checked == 0 {
            assert!(
                std::env::var("OPENDWG_REQUIRE_CORPUS").as_deref() != Ok("1"),
                "OPENDWG_REQUIRE_CORPUS=1 but there is no corpus"
            );
            eprintln!("SKIPPED: no corpus; run scripts/fetch-corpus.sh");
        }
    }

    #[test]
    fn names_compare_without_case() {
        assert!(names_eq("Defpoints", "DEFPOINTS"));
        assert!(names_eq("Ébauche", "éBAUCHE"));
        assert!(!names_eq("Layer1", "Layer"));
    }

    /// The class definitions added with a class type's first object are
    /// those of the R2018 corpus sample, for every class type it has.
    #[test]
    fn known_classes_match_the_corpus() {
        let Some(d) = sample() else { return };
        let mut checked = 0;
        for c in &d.classes {
            let Some(mut k) = known_class(&c.dxf_name, c.class_number) else {
                continue;
            };
            k.instance_count = c.instance_count;
            assert_eq!(&k, c);
            checked += 1;
        }
        // All but ARC_DIMENSION, LARGE_RADIAL_DIMENSION and PLOTSETTINGS.
        assert_eq!(checked, 26);
        assert!(known_class("NO_SUCH_CLASS", 600).is_none());
    }
}
