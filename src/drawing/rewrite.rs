//! Renumbering a whole drawing's handles: [`Drawing::rewrite_handles`].
//!
//! The mapping is taken over every handle the drawing stores: the objects'
//! own handles, every handle in their data (see `object::remap`: typed
//! references, references kept as stored, extended data, XRECORD items),
//! the header's handle variables, the keys of the data storage records and
//! the lists the writer keeps from the file read (`write::ReadState`). It is
//! computed once per handle and checked before anything changes, so that a
//! rewrite either applies whole or leaves the drawing as it was.

use std::collections::{HashMap, HashSet};

use super::Drawing;
use super::refs::IndexState;
use crate::container::SectionName;
use crate::diag::{Diagnostic, Severity};
use crate::error::{Error, Result};
use crate::handle::Handle;
use crate::object::remap::{Mapper, map_object_handles};
use crate::version::Version;

fn invalid(handle: Option<Handle>, message: String) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        handle,
        message,
    }
}

impl Drawing {
    /// Renumber the drawing's handles: every handle the drawing stores
    /// becomes `f(handle)`, the objects' own handles and every reference to
    /// them alike, so that the drawing stays consistent (for merging
    /// drawings: move one's handles above the other's). `f` is called once
    /// per handle and never for the null handle, which stays null.
    ///
    /// Covered: every object's handle and the references in its data,
    /// typed or kept as stored (a raw object's, a proxy's: see
    /// [`crate::object::RawObject::rewrite_handles`] for offsets), extended
    /// data handles, XRECORD handle items (480-481 hard pointers stored as
    /// text included; 320-329 arbitrary handles are not translated, as in
    /// DXF), a sort table's sort handles, the
    /// header's handle variables, the data storage records
    /// ([`crate::section::DataStorage::remap_handles`]), and the handles
    /// the writer keeps from the file read. Handles inside a raw object's
    /// data bits cannot be found and are kept.
    ///
    /// Afterwards $HANDSEED is above every handle (kept when it already
    /// was), and the reference index is rebuilt on next use.
    ///
    /// What cannot be rewritten faithfully is reported as warnings added to
    /// [`Drawing::diagnostics`], naming the object by its new handle, each
    /// message once per object: a raw XRECORD or SORTENTSTABLE that moved
    /// (its data may hold handles), and a stored offset reference that named
    /// no handle (cleared, see [`crate::object::RawObject::rewrite_handles`]).
    /// When any handle moves, a read diagnostic about a handle the drawing
    /// no longer stores loses its handle; a mapping that moves nothing
    /// changes nothing.
    ///
    /// Fails with [`Error::Invalid`], changing nothing, when `f` maps two
    /// handles to one, a handle to null or to the largest handle (which
    /// would leave no room for $HANDSEED), or moves a handle while the
    /// drawing keeps an R2013+ data storage section it could not decode
    /// (written as read, keyed by the old handles). An error of
    /// [`crate::section::DataStorage::remap_handles`] is passed on, also
    /// changing nothing.
    pub fn rewrite_handles(&mut self, f: impl Fn(Handle) -> Handle) -> Result<()> {
        let table = self.handle_table(&f)?;
        let mut map = |h: Handle| table.get(&h).copied().unwrap_or(h);
        let data_storage = match &self.data_storage {
            Some(s) => {
                let mut s = s.clone();
                s.remap_handles(|h| (h.0 != 0).then(|| map(h)))?;
                Some(s)
            }
            None => None,
        };

        // Nothing fails from here on.
        self.data_storage = data_storage;
        let objects = std::mem::take(&mut self.objects);
        let mut notes = Vec::new();
        self.objects = objects
            .into_iter()
            .map(|(h, mut obj)| {
                let mut m = Mapper::new(&mut map);
                map_object_handles(&mut obj, &mut m);
                let new = m.map(h);
                // One note per message and object.
                let mut seen = HashSet::new();
                m.notes.retain(|n| seen.insert(n.clone()));
                notes.extend(m.notes.into_iter().map(|message| Diagnostic {
                    severity: Severity::Warning,
                    handle: Some(new),
                    message,
                }));
                (new, obj)
            })
            .collect();
        self.header.map_handles(&mut map);
        let largest = table.values().max().map_or(0, |h| h.0);
        // Every mapped handle is below the largest possible one (checked).
        self.header.handseed = Handle(self.header.handseed.0.max(largest + 1));
        self.read.map_handles(&mut Mapper::new(&mut map));
        // A diagnostic about a handle the drawing no longer stores (an
        // object skipped while reading) loses it when anything moves: kept,
        // it could name the object that now has that handle.
        if table.iter().any(|(old, new)| old != new) {
            for d in &mut self.diagnostics {
                d.handle = d.handle.and_then(|h| table.get(&h).copied());
            }
        }
        self.diagnostics.extend(notes);
        *self.ref_index.get_mut() = IndexState::default();
        Ok(())
    }

    /// `f` over every non-null handle the drawing stores (see
    /// [`Drawing::rewrite_handles`]), checked.
    ///
    /// It takes `&mut self` only because the object walker
    /// (`map_object_handles`) is written once, for mutation: walked here
    /// with the identity (which records each handle and returns it), every
    /// object is left exactly as it was. Nothing else is touched, so a
    /// refused mapping leaves the drawing unchanged.
    fn handle_table(&mut self, f: &impl Fn(Handle) -> Handle) -> Result<HashMap<Handle, Handle>> {
        let mut table: HashMap<Handle, Handle> = HashMap::new();
        // Walk with the identity, which changes nothing, to list the handles.
        let mut seen = |h: Handle| {
            table.entry(h).or_insert_with(|| f(h));
            h
        };
        for (&h, obj) in &mut self.objects {
            seen(h);
            map_object_handles(obj, &mut Mapper::new(&mut seen));
        }
        self.header.map_handles(&mut seen);
        if let Some(s) = &self.data_storage {
            for r in &s.records {
                if r.handle.0 != 0 {
                    seen(r.handle);
                }
            }
        }
        for h in self.read.handles() {
            if h.0 != 0 {
                seen(h);
            }
        }

        let mut errors = Vec::new();
        let mut sorted: Vec<(Handle, Handle)> = table.iter().map(|(&a, &b)| (a, b)).collect();
        sorted.sort_unstable();
        let mut by_new: HashMap<Handle, Handle> = HashMap::with_capacity(sorted.len());
        for &(old, new) in &sorted {
            if new.0 == 0 {
                errors.push(invalid(
                    Some(old),
                    format!("{old} would become the null handle"),
                ));
            } else if new.0 == u64::MAX {
                errors.push(invalid(
                    Some(old),
                    format!(
                        "{old} would become the largest handle, which leaves no room for $HANDSEED"
                    ),
                ));
            } else if let Some(&first) = by_new.get(&new) {
                errors.push(invalid(
                    Some(first),
                    format!("both {first} and {old} would become {new}"),
                ));
            } else {
                by_new.insert(new, old);
            }
        }
        let undecoded_storage = self.source_version.is_some_and(|v| v >= Version::R2013)
            && self.data_storage.is_none()
            && self
                .extra_sections
                .contains_key(&SectionName::AcDsPrototype);
        if undecoded_storage && sorted.iter().any(|(old, new)| old != new) {
            errors.push(invalid(
                None,
                "the data storage section could not be decoded and is written as read: its \
                 records cannot be renumbered"
                    .into(),
            ));
        }
        if errors.is_empty() {
            Ok(table)
        } else {
            Err(Error::Invalid(errors))
        }
    }
}

impl super::write::ReadState {
    /// The handles kept from the file read.
    fn handles(&self) -> impl Iterator<Item = Handle> + '_ {
        let lists = |m: &'_ HashMap<Handle, Vec<Handle>>| {
            m.iter()
                .flat_map(|(&k, v)| std::iter::once(k).chain(v.iter().copied()))
                .collect::<Vec<_>>()
        };
        self.order
            .iter()
            .copied()
            .chain(lists(&self.layout_viewports))
            .chain(lists(&self.block_users))
            .chain(self.layout_blocks.iter().flat_map(|(&k, &v)| [k, v]))
            .chain(self.undecoded_storage_acis.iter().copied())
            .chain(self.acis_without_record.iter().copied())
            .chain(self.attdefs.keys().copied())
    }

    /// Pass the handles kept from the file read through `m` (those
    /// [`Self::handles`] lists).
    fn map_handles(&mut self, m: &mut Mapper) {
        for h in &mut self.order {
            m.h(h);
        }
        let map_lists = |lists: &mut HashMap<Handle, Vec<Handle>>, m: &mut Mapper| {
            *lists = std::mem::take(lists)
                .into_iter()
                .map(|(k, mut v)| {
                    for h in &mut v {
                        m.h(h);
                    }
                    (m.map(k), v)
                })
                .collect();
        };
        map_lists(&mut self.layout_viewports, m);
        map_lists(&mut self.block_users, m);
        self.layout_blocks = std::mem::take(&mut self.layout_blocks)
            .into_iter()
            .map(|(k, v)| (m.map(k), m.map(v)))
            .collect();
        for h in &mut self.undecoded_storage_acis {
            m.h(h);
        }
        self.acis_without_record = std::mem::take(&mut self.acis_without_record)
            .into_iter()
            .map(|h| m.map(h))
            .collect::<HashSet<_>>();
        self.attdefs = std::mem::take(&mut self.attdefs)
            .into_iter()
            .map(|(k, v)| (m.map(k), v))
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::super::refs::IndexState;
    use super::*;
    use crate::container::SectionName;
    use crate::diag::Severity;
    use crate::error::Error;
    use crate::handle::HandleRef;
    use crate::object::{
        Entity, EntityCommon, Line, NonGraphical, Object, ObjectCommon, RawObject, XDataApp,
        XDataItem, XRecord, XRecordValue,
    };
    use crate::section::DataStorage;
    use crate::section::acds::DataRecord;
    use crate::section::header::handle_vars;
    use crate::version::Version;

    fn line(d: &mut Drawing, x: f64) -> Handle {
        let ms = d.model_space();
        let l0 = d.layer_by_name("0").unwrap();
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

    fn drawing() -> Drawing {
        let mut d = Drawing::new_at(crate::section::JulianDate {
            day: 2_461_300,
            ms: 0,
        });
        for i in 0..3 {
            line(&mut d, f64::from(i));
        }
        d
    }

    /// Everything a rewrite may change, to show a refused one changed
    /// nothing.
    fn state(d: &Drawing) -> String {
        format!(
            "{:?} {:?} {:?} {:?} {:?}",
            d.objects, d.header, d.read, d.data_storage, d.diagnostics
        )
    }

    fn shift(h: Handle) -> Handle {
        Handle(h.0 + 0x1000)
    }

    /// The index kept by the drawing equals one built afresh.
    fn assert_index_fresh(d: &Drawing) {
        let kept = d.ref_index.lock().ensure(&d.objects).snapshot();
        let fresh = IndexState::default().ensure(&d.objects).snapshot();
        assert!(kept == fresh, "the kept index differs from a fresh one");
    }

    /// A mapping that sends two handles to one, a handle to null or to the
    /// largest handle (which leaves no room for $HANDSEED) is refused with
    /// [`Error::Invalid`] naming the handle, and the drawing is unchanged.
    #[test]
    fn rewrite_refuses_bad_mappings() {
        let mut d = drawing();
        let lines: Vec<Handle> = d.entities_in(d.model_space()).collect();
        let (a, b) = (lines[0], lines[1]);
        // A soft pointer to an object that does not exist.
        let gone = Handle(0x7777);
        assert!(!d.objects.contains_key(&gone));
        let l0 = d.layer_by_name("0").unwrap();
        let Some(Object::Entity { common, .. }) = d.get_mut(b) else {
            panic!()
        };
        common.obj.xdata.push(XDataApp {
            app: l0,
            items: vec![XDataItem::Handle(gone)],
        });
        let before = state(&d);
        type Mapping = Box<dyn Fn(Handle) -> Handle>;
        let to = |t: Handle| -> Mapping { Box::new(move |h| if h == a { t } else { h }) };
        let cases: [(Mapping, &str); 4] = [
            (to(b), "both"),
            (to(Handle(0)), "null"),
            (to(Handle(u64::MAX)), "largest"),
            // A collision with a handle only referred to counts too.
            (to(gone), "both"),
        ];
        for (f, what) in cases {
            match d.rewrite_handles(f) {
                Err(Error::Invalid(diags)) => {
                    assert!(!diags.is_empty());
                    assert!(
                        diags.iter().all(|x| x.severity == Severity::Error),
                        "{diags:?}"
                    );
                    assert!(
                        diags
                            .iter()
                            .any(|x| x.handle == Some(a) && x.message.contains(what)),
                        "{what}: {diags:?}"
                    );
                }
                other => panic!("{what}: {other:?}"),
            }
            assert_eq!(state(&d), before, "{what}");
        }
    }

    /// Every handle moves: the objects (keys and own handles), their
    /// references, the header's handle variables, the writer's read state;
    /// $HANDSEED lands above the new largest handle; the drawing validates
    /// and saves.
    #[test]
    fn rewrite_renumbers_everything() {
        let mut d = drawing();
        d.read.order = d.objects.keys().copied().rev().collect();
        let old = d.clone();
        d.rewrite_handles(shift).unwrap();
        let keys: Vec<Handle> = d.objects.keys().copied().collect();
        let expect: Vec<Handle> = old.objects.keys().copied().map(shift).collect();
        assert_eq!(keys, expect);
        for (h, o) in &d.objects {
            assert_eq!(o.handle(), *h);
        }
        let ms = d.model_space();
        assert_eq!(ms, shift(old.model_space()));
        assert_eq!(
            d.entities_in(ms).collect::<Vec<_>>(),
            old.entities_in(old.model_space())
                .map(shift)
                .collect::<Vec<_>>()
        );
        let vars: Vec<_> = handle_vars(&old.header)
            .into_iter()
            .map(|(n, h)| (n, shift(h)))
            .collect();
        assert_eq!(handle_vars(&d.header), vars);
        let largest = d.objects.last_key_value().unwrap().0;
        assert!(d.header.handseed > *largest, "{}", d.header.handseed);
        assert_eq!(
            d.read.order,
            old.read
                .order
                .iter()
                .copied()
                .map(shift)
                .collect::<Vec<_>>()
        );
        let errors: Vec<_> = d
            .validate()
            .into_iter()
            .filter(|x| x.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "{errors:?}");
        let back = Drawing::from_bytes(&d.to_bytes().unwrap()).unwrap();
        assert_eq!(
            back.objects.keys().collect::<Vec<_>>(),
            d.objects.keys().collect::<Vec<_>>()
        );
        // The identity changes nothing (the seed was already past every
        // handle).
        let before = state(&d);
        d.rewrite_handles(|h| h).unwrap();
        assert_eq!(state(&d), before);
    }

    /// $HANDSEED never moves down: a seed above the new largest handle (a
    /// mapping that compacts) is kept.
    #[test]
    fn rewrite_keeps_a_higher_seed() {
        let mut d = drawing();
        d.header.handseed = Handle(0x9000);
        d.rewrite_handles(shift).unwrap();
        assert_eq!(d.header.handseed, Handle(0x9000));
    }

    /// The reference index, built and partly stale (an object changed
    /// through `get_mut`) before the rewrite, matches one built afresh
    /// after it, and answers with the new handles.
    #[test]
    fn rewrite_keeps_the_reference_index_fresh() {
        let mut d = drawing();
        let l0 = d.layer_by_name("0").unwrap();
        let users = d.referrers(l0);
        assert!(!users.is_empty());
        let first = d.entities_in(d.model_space()).next().unwrap();
        let Some(Object::Entity { common, .. }) = d.get_mut(first) else {
            panic!()
        };
        common.color_book = None;
        d.rewrite_handles(shift).unwrap();
        assert_index_fresh(&d);
        assert_eq!(
            d.referrers(shift(l0)),
            users.into_iter().map(shift).collect::<Vec<_>>()
        );
        assert!(d.referrers(l0).is_empty());
    }

    /// Data storage records follow their objects; the read-state lists of
    /// ACIS entities and the diagnostics too.
    #[test]
    fn rewrite_remaps_data_storage_and_read_state() {
        let mut d = drawing();
        let a = d.entities_in(d.model_space()).next().unwrap();
        d.data_storage = Some(DataStorage {
            records: vec![DataRecord {
                handle: a,
                schema: 0,
                data: vec![1, 2, 3],
            }],
            ..DataStorage::default()
        });
        d.read.acis_without_record.insert(a);
        d.read.undecoded_storage_acis.push(a);
        d.read.layout_blocks.insert(a, a);
        d.read.block_users = [(a, vec![a])].into();
        d.read.layout_viewports = [(a, vec![a])].into();
        d.read.attdefs = [(a, (false, true))].into();
        d.diagnostics.push(crate::diag::Diagnostic {
            severity: Severity::Warning,
            handle: Some(a),
            message: "read".into(),
        });
        d.rewrite_handles(shift).unwrap();
        let n = shift(a);
        assert_eq!(d.data_storage.as_ref().unwrap().records[0].handle, n);
        assert!(d.read.acis_without_record.contains(&n));
        assert_eq!(d.read.undecoded_storage_acis, [n]);
        assert_eq!(d.read.layout_blocks.get(&n), Some(&n));
        assert_eq!(d.read.block_users.get(&n), Some(&vec![n]));
        assert_eq!(d.read.layout_viewports.get(&n), Some(&vec![n]));
        assert_eq!(d.read.attdefs.get(&n), Some(&(false, true)));
        assert_eq!(d.diagnostics.last().unwrap().handle, Some(n));
    }

    /// An R2013+ data storage section that could not be decoded is written
    /// as read, keyed by the old handles: a rewrite that moves any handle is
    /// refused (nothing changes); one that moves none is not.
    #[test]
    fn rewrite_refuses_undecoded_data_storage() {
        let mut d = drawing();
        d.source_version = Some(Version::R2018);
        d.extra_sections
            .insert(SectionName::AcDsPrototype, vec![1, 2, 3]);
        let before = state(&d);
        match d.rewrite_handles(shift) {
            Err(Error::Invalid(diags)) => {
                assert!(diags[0].message.contains("data storage"), "{diags:?}")
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(state(&d), before);
        d.rewrite_handles(|h| h).unwrap();
        // A pre-R2013 one is left out on save: no matter.
        d.source_version = Some(Version::R2010);
        d.rewrite_handles(shift).unwrap();
    }

    /// Insert `object` as `h`, owned and listed by the named objects
    /// dictionary.
    fn insert(d: &mut Drawing, h: Handle, object: NonGraphical) {
        let nod = d.named_objects();
        let common = ObjectCommon {
            handle: h,
            owner: nod,
            reactors: vec![nod],
            ..ObjectCommon::new()
        };
        d.objects.insert(h, Object::Other { common, object });
        if let Some(Object::Other {
            object: NonGraphical::Dictionary(dict),
            ..
        }) = d.objects.get_mut(&nod)
        {
            dict.entries.push((format!("E{h}"), h));
        }
        *d.ref_index.get_mut() = IndexState::default();
    }

    /// XRECORD 480-481 items, hard pointers stored as hexadecimal text, are
    /// references: indexed (with the hard pointer code) and rewritten in
    /// their stored form. 320-329 items (arbitrary handles, "not
    /// translated" in DXF) are neither.
    #[test]
    fn rewrite_follows_xrecord_hard_pointer_text() {
        let mut d = drawing();
        let l0 = d.layer_by_name("0").unwrap();
        let x = Handle(0x7000);
        let text = |h: Handle| format!("{:x}", h.0);
        insert(
            &mut d,
            x,
            NonGraphical::XRecord(XRecord {
                items: vec![
                    (480, XRecordValue::HandleText(text(l0))),
                    (481, XRecordValue::HandleText(String::new())),
                    (320, XRecordValue::HandleText(text(l0))),
                ],
                ..XRecord::default()
            }),
        );
        assert!(d.referrers(l0).contains(&x));
        let refs = crate::drawing::refs::object_refs(d.get(x).unwrap()).0;
        assert_eq!(
            refs,
            [(l0, crate::object::tables::HARD_POINTER)],
            "only the 480 item"
        );
        d.rewrite_handles(shift).unwrap();
        let Some(Object::Other {
            object: NonGraphical::XRecord(r),
            ..
        }) = d.get(shift(x))
        else {
            panic!()
        };
        assert_eq!(
            r.items,
            [
                (480, XRecordValue::HandleText(text(shift(l0)))),
                (481, XRecordValue::HandleText(String::new())),
                (320, XRecordValue::HandleText(text(l0))),
            ]
        );
        assert!(d.referrers(shift(l0)).contains(&shift(x)));
        assert_index_fresh(&d);
        // Saved and read back, the item still names the moved layer.
        let back = Drawing::from_bytes(&d.to_bytes().unwrap()).unwrap();
        assert!(back.referrers(shift(l0)).contains(&shift(x)));
    }

    fn raw(type_code: u16, class_name: Option<&str>, handles: Vec<HandleRef>) -> NonGraphical {
        NonGraphical::Unknown(RawObject {
            type_code,
            class_name: class_name.map(Into::into),
            source_version: Version::R2018,
            data: vec![],
            data_bits: 0,
            strings: None,
            handles,
            tail_handles_raw: vec![],
            tail_handle_bits: 0,
        })
    }

    /// What a rewrite cannot carry faithfully is reported as a warning on
    /// the moved object: a raw XRECORD or SORTENTSTABLE (handles in its data
    /// bits) and a stored offset that names no handle (cleared to a null
    /// soft pointer). A read diagnostic about a handle no longer stored
    /// loses it; one about a moved object follows it.
    #[test]
    fn rewrite_warns_what_it_cannot_carry() {
        let mut d = drawing();
        let (xr, sort, off) = (Handle(0x7000), Handle(0x7001), Handle(0x7002));
        insert(&mut d, xr, raw(0x4F, None, vec![]));
        insert(&mut d, sort, raw(600, Some("SORTENTSTABLE"), vec![]));
        let bad = HandleRef {
            code: 0xC,
            value: 0x8000,
        };
        // The same unresolvable offset twice: reported once.
        insert(&mut d, off, raw(601, Some("ACME_THING"), vec![bad, bad]));
        let line = d.entities_in(d.model_space()).next().unwrap();
        for h in [Handle(0x9999), line] {
            d.diagnostics.push(crate::diag::Diagnostic {
                severity: Severity::Warning,
                handle: Some(h),
                message: "read".into(),
            });
        }
        let read = d.diagnostics.len();
        // Unmoved, nothing is reported or cleared: the read diagnostics keep
        // their handles, that of an object no longer stored included.
        let diagnostics = d.diagnostics.clone();
        d.rewrite_handles(|h| h).unwrap();
        assert_eq!(d.diagnostics, diagnostics);
        d.rewrite_handles(shift).unwrap();
        let handles: Vec<Option<Handle>> = d.diagnostics[read - 2..read]
            .iter()
            .map(|x| x.handle)
            .collect();
        assert_eq!(handles, [None, Some(shift(line))]);
        let notes = &d.diagnostics[read..];
        assert_eq!(notes.len(), 3, "{notes:?}");
        assert!(notes.iter().all(|x| x.severity == Severity::Warning));
        let on = |h: Handle, what: &str| {
            notes
                .iter()
                .any(|x| x.handle == Some(shift(h)) && x.message.contains(what))
        };
        assert!(on(xr, "XRECORD kept as bits"), "{notes:?}");
        assert!(on(sort, "SORTENTSTABLE kept as bits"), "{notes:?}");
        assert!(on(off, "cleared"), "{notes:?}");
        assert_eq!(
            d.get(shift(off)).unwrap().raw().unwrap().handles,
            [HandleRef { code: 4, value: 0 }; 2]
        );
    }
}
