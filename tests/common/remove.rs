//! Removing a drawing's objects through the API: the unconvertible objects
//! of an R2000/R2004 corpus file, so that the rest saves (the round-trip
//! tests, shared with the sign-off pack and the ODA converter tests).

use std::collections::HashSet;

use opendwg::object::{MaterialRef, NonGraphical, Object, XDataItem, XRecordValue};
use opendwg::{Drawing, Error, Handle};

/// Clear the references of `referrer` to `x` that keep `x` from being
/// removed: extended data handles, an entity's material and color book, an
/// XRECORD's handle items.
pub fn clear_references(d: &mut Drawing, referrer: Handle, x: Handle) {
    let obj = d.get_mut(referrer).unwrap();
    let ty = obj.type_name().into_owned();
    let (xdata, cleared) = match obj {
        Object::Entity { common, .. } => {
            let mut cleared = false;
            if common.material == MaterialRef::Handle(x) {
                common.material = MaterialRef::ByLayer;
                cleared = true;
            }
            if common.color_book == Some(x) {
                common.color_book = None;
                cleared = true;
            }
            (&mut common.obj.xdata, cleared)
        }
        Object::Other {
            common,
            object: NonGraphical::XRecord(r),
        } => {
            let mut cleared = false;
            for (_, v) in &mut r.items {
                if *v == XRecordValue::ObjectId(x) {
                    *v = XRecordValue::ObjectId(Handle(0));
                    cleared = true;
                }
            }
            (&mut common.xdata, cleared)
        }
        Object::Other { common, .. } => (&mut common.xdata, false),
        other => panic!("{referrer}: {other:?}"),
    };
    let mut in_xdata = false;
    for item in xdata.iter_mut().flat_map(|a| &mut a.items) {
        if *item == XDataItem::Handle(x) {
            *item = XDataItem::Handle(Handle(0));
            in_xdata = true;
        }
    }
    assert!(
        cleared || in_xdata,
        "{referrer} ({ty}) refers to {x} in another way"
    );
}

/// Remove `objects` through the API: in passes while some are removed (an
/// object may be in use by others of the list); then the uses
/// [`clear_references`] knows are cleared for those left, by objects not
/// themselves to be removed (an R2004 3DSOLID, which converts, keeps its
/// raw history objects through an XRECORD of its extension dictionary).
pub fn remove_all(d: &mut Drawing, objects: &[(Handle, String)], what: &str) {
    let mut left: Vec<Handle> = objects.iter().map(|&(h, _)| h).collect();
    let to_remove: HashSet<Handle> = left.iter().copied().collect();
    loop {
        let before = left.len();
        let mut in_use = Vec::new();
        for h in std::mem::take(&mut left) {
            if d.get(h).is_none() {
                continue; // removed with its owner
            }
            match d.remove(h) {
                Ok(_) => {}
                Err(Error::InUse {
                    objects,
                    header_vars,
                }) => {
                    assert!(header_vars.is_empty(), "{what}: {h} {header_vars:?}");
                    left.push(h);
                    in_use.push((h, objects));
                }
                Err(e) => panic!("{what}: {h}: {e:?}"),
            }
        }
        if left.is_empty() {
            return;
        }
        if left.len() == before {
            let mut cleared = false;
            for (h, users) in in_use {
                for r in users.into_iter().filter(|r| !to_remove.contains(r)) {
                    clear_references(d, r, h);
                    cleared = true;
                }
            }
            assert!(cleared, "{what}: {left:?} cannot be removed");
        }
    }
}
