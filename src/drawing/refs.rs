//! The reverse reference index behind [`Drawing::referrers`] and
//! [`Drawing::remove`].
//!
//! An object's references are those its type-specific data writes and an
//! entity's common data writes besides its ownership links (both found by
//! encoding with a recording writer, see [`crate::object::type_refs`] and
//! [`crate::object::entity_common_refs`]), the handle items of an XRECORD
//! (stored in its data; 480-481 items as text), and the application and
//! handle items of its extended data. Owner, reactors and extension
//! dictionary are ownership links; reactors are indexed separately.
//!
//! The index is built once, on first use, in one pass over the objects. After
//! that it is kept up to date incrementally: the mutation methods add and
//! drop the references they change (list entries directly, other changed
//! objects by recomputing their references), and [`Drawing::get_mut`] marks
//! its object dirty, so only that object is recomputed at the next use.
//!
//! [`Drawing::referrers`]: super::Drawing::referrers
//! [`Drawing::remove`]: super::Drawing::remove
//! [`Drawing::get_mut`]: super::Drawing::get_mut

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Mutex, MutexGuard, PoisonError};

#[cfg(doc)]
use super::Drawing;
use crate::error::Result;
use crate::handle::Handle;
use crate::object::objects::{handle_text_is_reference, parse_handle_text};
use crate::object::tables::{HARD_OWNER, HARD_POINTER, SOFT_OWNER, SOFT_POINTER};
use crate::object::{
    AcisPayload, Entity, NonGraphical, Object, XDataItem, XRecordValue, entity_common_refs,
    type_refs,
};

/// The references of `obj` (target, reference code), without null handles
/// and references to itself, and whether the list is complete. It is not when
/// the data cannot be encoded as R2018, except for an ACIS entity read
/// before R2013 (the encoder refuses it alone, before writing the history,
/// the entity's one reference, which is added here). ACIS data storage
/// records are keyed by handle but are not references: the writer drops
/// those of entities gone.
pub(crate) fn object_refs(obj: &Object) -> (Vec<(Handle, u8)>, bool) {
    let (refs, _, complete) = encoded_refs(obj);
    (refs, complete)
}

/// The references of [`object_refs`], the encoders' first error (the
/// type-specific data's, else an entity's common data's), and whether the
/// list is complete: when neither encoder failed, or for a pre-R2013 ACIS
/// entity when only its type data's encoder did (its one reference is added
/// here). `Drawing::check_new` refuses an object on the error.
pub(crate) fn encoded_refs(obj: &Object) -> (Vec<(Handle, u8)>, Result<()>, bool) {
    // An object whose common data could not be read refers to nothing the
    // drawing can know of; it is never written (see `UnreadableObject`).
    if let Object::Other {
        object: NonGraphical::Unreadable(_),
        ..
    } = obj
    {
        return (Vec::new(), Ok(()), true);
    }
    let (mut refs, mut encoded) = type_refs(obj);
    let mut complete = encoded.is_ok();
    if let Object::Entity { common, entity } = obj {
        if let Entity::Acis(a) = entity {
            if !matches!(a.payload, AcisPayload::DataStorage { .. }) {
                refs.extend(a.history.map(|h| (h, HARD_OWNER)));
                complete = true;
            }
        }
        let (common_refs, result) = entity_common_refs(common);
        refs.extend(common_refs);
        complete &= result.is_ok();
        // The type data's error first; the common data's when it has none.
        if encoded.is_ok() {
            encoded = result;
        }
    }
    if let Object::Other {
        object: NonGraphical::XRecord(x),
        ..
    } = obj
    {
        // Handle items are stored in the data, not as references: 330-369
        // and 390-399 as handles, 480-481 as hexadecimal text.
        refs.extend(x.items.iter().filter_map(|(code, value)| match value {
            XRecordValue::ObjectId(h) => Some((*h, xrecord_ref_code(*code))),
            XRecordValue::HandleText(s) if handle_text_is_reference(*code) => {
                Some((parse_handle_text(s)?, xrecord_ref_code(*code)))
            }
            _ => None,
        }));
    }
    for app in &obj.object_common().xdata {
        refs.push((app.app, SOFT_POINTER));
        refs.extend(app.items.iter().filter_map(|item| match item {
            XDataItem::LayerRef(h) | XDataItem::Handle(h) => Some((*h, SOFT_POINTER)),
            _ => None,
        }));
    }
    let own = obj.handle();
    refs.retain(|&(t, _)| t.0 != 0 && t != own);
    (refs, encoded, complete)
}

/// The reference code a DXF group code's handle stands for: 330-339 soft
/// pointer, 340-349, 390-399 and 480-481 hard pointer, 350-359 soft owner,
/// 360-369 hard owner; any other (1005-1009) a soft pointer. 320-329 items
/// (arbitrary handles, not translated) are not references.
fn xrecord_ref_code(group: i16) -> u8 {
    match group {
        340..=349 | 390..=399 | 480..=481 => HARD_POINTER,
        350..=359 => SOFT_OWNER,
        360..=369 => HARD_OWNER,
        _ => SOFT_POINTER,
    }
}

/// One object's outgoing links in the index.
#[derive(Debug, Clone, Default)]
struct Node {
    refs: Vec<(Handle, u8)>,
    reactors: Vec<Handle>,
}

/// Who refers to whom.
#[derive(Debug, Clone, Default)]
pub(crate) struct RefIndex {
    nodes: HashMap<Handle, Node>,
    /// For each referenced object, its referrers in handle order, with the
    /// codes of their references to it.
    referrers: HashMap<Handle, BTreeMap<Handle, Vec<u8>>>,
    /// For each object, the objects that list it among their reactors.
    reactor_holders: HashMap<Handle, BTreeSet<Handle>>,
    /// Objects whose references cannot all be listed (see [`object_refs`]).
    incomplete: BTreeSet<Handle>,
}

impl RefIndex {
    /// The referrers of `h` with their reference codes, in handle order.
    pub(crate) fn referrers(&self, h: Handle) -> impl Iterator<Item = (Handle, &[u8])> {
        self.referrers
            .get(&h)
            .into_iter()
            .flatten()
            .map(|(&r, codes)| (r, codes.as_slice()))
    }

    /// The objects listing `h` among their reactors.
    pub(crate) fn reactor_holders(&self, h: Handle) -> impl Iterator<Item = Handle> {
        self.reactor_holders.get(&h).into_iter().flatten().copied()
    }

    /// The references of `h` as indexed.
    pub(crate) fn refs_of(&self, h: Handle) -> &[(Handle, u8)] {
        self.nodes.get(&h).map_or(&[], |n| n.refs.as_slice())
    }

    pub(crate) fn incomplete(&self) -> impl Iterator<Item = Handle> {
        self.incomplete.iter().copied()
    }

    /// Whether the references of `h` could not all be listed.
    pub(crate) fn is_incomplete(&self, h: Handle) -> bool {
        self.incomplete.contains(&h)
    }

    /// Whether `holder` lists `r` among its reactors.
    pub(crate) fn holds_reactor(&self, holder: Handle, r: Handle) -> bool {
        self.reactor_holders
            .get(&r)
            .is_some_and(|s| s.contains(&holder))
    }

    /// Whether `from` refers to `to` (with any code).
    pub(crate) fn refers(&self, from: Handle, to: Handle) -> bool {
        self.referrers
            .get(&to)
            .is_some_and(|m| m.contains_key(&from))
    }

    /// Everything indexed, in a canonical order (codes sorted), to compare
    /// an index kept up to date with one built afresh.
    #[cfg(test)]
    #[allow(clippy::type_complexity)]
    pub(crate) fn snapshot(
        &self,
    ) -> (
        BTreeMap<Handle, BTreeMap<Handle, Vec<u8>>>,
        BTreeMap<Handle, BTreeSet<Handle>>,
        BTreeSet<Handle>,
    ) {
        let referrers = self
            .referrers
            .iter()
            .map(|(&t, m)| {
                let m = m
                    .iter()
                    .map(|(&r, codes)| {
                        let mut codes = codes.clone();
                        codes.sort_unstable();
                        (r, codes)
                    })
                    .collect();
                (t, m)
            })
            .collect();
        let holders = self
            .reactor_holders
            .iter()
            .map(|(&r, s)| (r, s.clone()))
            .collect();
        (referrers, holders, self.incomplete.clone())
    }

    /// Index `obj` as object `h` (which must not be indexed).
    fn insert(&mut self, h: Handle, obj: &Object) {
        let (refs, complete) = object_refs(obj);
        if !complete {
            self.incomplete.insert(h);
        }
        for &(t, code) in &refs {
            self.referrers
                .entry(t)
                .or_default()
                .entry(h)
                .or_default()
                .push(code);
        }
        let reactors = obj.object_common().reactors.clone();
        for &r in &reactors {
            self.reactor_holders.entry(r).or_default().insert(h);
        }
        self.nodes.insert(h, Node { refs, reactors });
    }

    /// Drop everything object `h` refers to. References to it stay.
    pub(crate) fn delete(&mut self, h: Handle) {
        self.incomplete.remove(&h);
        let Some(node) = self.nodes.remove(&h) else {
            return;
        };
        for (t, _) in node.refs {
            self.drop_referrer(t, h);
        }
        for r in node.reactors {
            self.drop_holder(r, h);
        }
    }

    fn drop_referrer(&mut self, t: Handle, r: Handle) {
        if let Some(m) = self.referrers.get_mut(&t) {
            m.remove(&r);
            if m.is_empty() {
                self.referrers.remove(&t);
            }
        }
    }

    fn drop_holder(&mut self, r: Handle, holder: Handle) {
        if let Some(s) = self.reactor_holders.get_mut(&r) {
            s.remove(&holder);
            if s.is_empty() {
                self.reactor_holders.remove(&r);
            }
        }
    }

    /// `from` now also refers to `to` (a list entry it gained).
    pub(crate) fn add_edge(&mut self, from: Handle, to: Handle, code: u8) {
        if let Some(node) = self.nodes.get_mut(&from) {
            node.refs.push((to, code));
            self.referrers
                .entry(to)
                .or_default()
                .entry(from)
                .or_default()
                .push(code);
        }
    }

    /// `from` no longer refers to any of `to` at all (one pass over its
    /// references, however many go).
    pub(crate) fn remove_edges(&mut self, from: Handle, to: &HashSet<Handle>) {
        if let Some(node) = self.nodes.get_mut(&from) {
            node.refs.retain(|(t, _)| !to.contains(t));
            for &t in to {
                self.drop_referrer(t, from);
            }
        }
    }

    /// `h`'s reactors are now `reactors`.
    pub(crate) fn set_reactors(&mut self, h: Handle, reactors: &[Handle]) {
        let Some(node) = self.nodes.get_mut(&h) else {
            return;
        };
        let old = std::mem::replace(&mut node.reactors, reactors.to_vec());
        for r in old {
            self.drop_holder(r, h);
        }
        for &r in reactors {
            self.reactor_holders.entry(r).or_default().insert(h);
        }
    }
}

/// The index, whether built, the objects changed through `get_mut` since,
/// and a count of the objects whose references were computed for it (the
/// work, for tests).
#[derive(Debug, Clone, Default)]
pub(crate) struct IndexState {
    index: Option<RefIndex>,
    dirty: HashSet<Handle>,
    pub(crate) work: usize,
    /// The number of objects links were dropped from by removals (each
    /// object's lists passed over once per removal), for tests.
    #[cfg(test)]
    pub(crate) detached: usize,
    /// For the block records looked at by the circular insert check, the
    /// blocks their entities insert (INSERT and MINSERT); kept up to date by
    /// the add methods, and an entry dropped when its block's list or one of
    /// its entities may have changed (see [`IndexState::forget_inserts`]).
    pub(crate) block_inserts: HashMap<Handle, HashSet<Handle>>,
    /// The number of block entity lists scanned to fill `block_inserts`, for
    /// tests.
    #[cfg(test)]
    pub(crate) block_scans: usize,
    /// For the table controls and dictionaries whose entries were looked up
    /// by name, their entries by name folded to lower case (see
    /// `api::fold_name`), each name's in list order; kept up to date by the
    /// add methods, and an entry dropped when its list, or the name of one
    /// of its table entries, may have changed (see
    /// [`IndexState::forget_names`]).
    pub(crate) names: HashMap<Handle, HashMap<String, Vec<Handle>>>,
    /// For each table entry indexed in `names`, the table controls it is
    /// indexed under (its name is in it, not in the control).
    pub(crate) name_holders: HashMap<Handle, BTreeSet<Handle>>,
    /// The number of lists scanned to fill `names`, for tests.
    #[cfg(test)]
    pub(crate) name_scans: usize,
    /// The new handles `Drawing::check_new` compared, for tests.
    #[cfg(test)]
    pub(crate) pending_probes: usize,
    /// The questions `Drawing::check_new` asked about new handles, for
    /// tests.
    #[cfg(test)]
    pub(crate) pending_lookups: usize,
}

impl IndexState {
    /// The index, built or brought up to date with `objects` first.
    pub(crate) fn ensure(&mut self, objects: &BTreeMap<Handle, Object>) -> &mut RefIndex {
        if self.index.is_none() {
            let mut index = RefIndex::default();
            for (&h, obj) in objects {
                index.insert(h, obj);
            }
            self.work += objects.len();
            self.dirty.clear();
            self.index = Some(index);
        }
        let index = self.index.get_or_insert_with(RefIndex::default);
        for h in self.dirty.drain() {
            index.delete(h);
            if let Some(obj) = objects.get(&h) {
                index.insert(h, obj);
                self.work += 1;
            }
        }
        index
    }

    /// Bring the index up to date (recompute the objects changed through
    /// `get_mut`) if it was built; a mutation calls this before changing
    /// anything.
    pub(crate) fn sync(&mut self, objects: &BTreeMap<Handle, Object>) {
        if self.index.is_some() {
            self.ensure(objects);
        }
    }

    /// Index the new objects `new` and the list entries `edges` (from, to,
    /// code) other objects gained, if the index was built. The rest of the
    /// index must be up to date (see [`IndexState::sync`]).
    pub(crate) fn added(
        &mut self,
        objects: &BTreeMap<Handle, Object>,
        new: &[Handle],
        edges: &[(Handle, Handle, u8)],
    ) {
        let Some(index) = &mut self.index else {
            return;
        };
        for &h in new {
            if let Some(obj) = objects.get(&h) {
                index.insert(h, obj);
                self.work += 1;
            }
        }
        for &(from, to, code) in edges {
            index.add_edge(from, to, code);
        }
    }

    /// The built index, if any.
    pub(crate) fn index_mut(&mut self) -> Option<&mut RefIndex> {
        self.index.as_mut()
    }

    /// Recompute the references of `h` (gone from `objects` if removed).
    pub(crate) fn refresh(&mut self, objects: &BTreeMap<Handle, Object>, h: Handle) {
        if let Some(index) = &mut self.index {
            index.delete(h);
            if let Some(obj) = objects.get(&h) {
                index.insert(h, obj);
                self.work += 1;
            }
        }
    }

    /// Drop what `block_inserts` holds of block record `block`.
    pub(crate) fn forget_inserts(&mut self, block: Handle) {
        self.block_inserts.remove(&block);
    }

    /// Drop what `names` holds of `h`'s list (a table control or dictionary)
    /// and of the tables `h` is an entry of.
    pub(crate) fn forget_names(&mut self, h: Handle) {
        self.names.remove(&h);
        for holder in self.name_holders.remove(&h).unwrap_or_default() {
            self.names.remove(&holder);
        }
    }

    /// Record in `names` the new entry `h` of table control `control`, named
    /// `folded` (none when it is no table entry), if the table's names are
    /// indexed: as indexing the table finds it, it names the control among
    /// its holders either way.
    pub(crate) fn name_added_to_table(
        &mut self,
        control: Handle,
        folded: Option<String>,
        h: Handle,
    ) {
        if let Some(names) = self.names.get_mut(&control) {
            if let Some(folded) = folded {
                names.entry(folded).or_default().push(h);
            }
            self.name_holders.entry(h).or_default().insert(control);
        }
    }

    /// Record in `names` the new entry `h` named `folded` of dictionary
    /// `dictionary`, if its names are indexed (its names are its own: it is
    /// no holder).
    pub(crate) fn name_added_to_dictionary(
        &mut self,
        dictionary: Handle,
        folded: String,
        h: Handle,
    ) {
        if let Some(names) = self.names.get_mut(&dictionary) {
            names.entry(folded).or_default().push(h);
        }
    }

    pub(crate) fn mark_dirty(&mut self, h: Handle) {
        if self.index.is_some() {
            self.dirty.insert(h);
        }
    }
}

/// [`IndexState`] behind a lock, so that `&self` queries can build it.
#[derive(Debug, Default)]
pub(crate) struct IndexCell(Mutex<IndexState>);

impl IndexCell {
    /// The state. A lock poisoned by a panic while the index was being
    /// changed may hold a half-updated index: it is dropped, to be rebuilt.
    pub(crate) fn lock(&self) -> MutexGuard<'_, IndexState> {
        self.0.lock().unwrap_or_else(|poisoned| {
            self.0.clear_poison();
            let mut state = poisoned.into_inner();
            *state = IndexState::default();
            state
        })
    }

    /// The state, to change; a poisoned one is dropped as in
    /// [`IndexCell::lock`].
    pub(crate) fn get_mut(&mut self) -> &mut IndexState {
        if self.0.is_poisoned() {
            self.0.clear_poison();
            *self.0.get_mut().unwrap_or_else(PoisonError::into_inner) = IndexState::default();
        }
        self.0.get_mut().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Clone for IndexCell {
    fn clone(&self) -> Self {
        IndexCell(Mutex::new(self.lock().clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A panic while the lock is held drops the index rather than keep a
    /// possibly half-updated one; both accessors recover.
    #[test]
    fn poisoned_index_is_dropped() {
        let poison = |cell: &IndexCell| {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut state = cell.lock();
                state.work = 7;
                state.index = Some(RefIndex::default());
                panic!("while updating the index");
            }));
            assert!(result.is_err());
            assert!(cell.0.is_poisoned());
        };
        let cell = IndexCell::default();
        poison(&cell);
        let state = cell.lock();
        assert!(state.index.is_none() && state.work == 0);
        drop(state);
        assert!(!cell.0.is_poisoned());
        let mut cell = IndexCell::default();
        poison(&cell);
        let state = cell.get_mut();
        assert!(state.index.is_none() && state.work == 0);
    }
}
