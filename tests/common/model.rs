//! Comparing a drawing's objects with those of its round trip.

use std::collections::HashSet;

use opendwg::object::{
    AcisGuid, DecodeCtx, Entity, NonGraphical, Object, decode_object, encode_object,
};
use opendwg::{Drawing, Handle, Version};

/// `o` with its raw bits marked as R2018 ones: a raw object read from an
/// R2007+ file is written unchanged, and reads back as R2018 bits.
pub fn normalized(o: &Object) -> Object {
    let mut o = o.clone();
    if let Object::Entity {
        entity: Entity::Unknown(r),
        ..
    }
    | Object::Other {
        object: NonGraphical::Unknown(r),
        ..
    } = &mut o
    {
        r.source_version = Version::R2018;
    }
    o
}

/// Object `h` of `d` as an R2018 read gives it back: encoded and decoded
/// alone (an ACIS entity read before R2013 converted first, as the drawing
/// writer converts it). `Err` when it does not encode or decode cleanly.
pub fn as_r2018(d: &Drawing, h: Handle, o: &Object) -> Result<Object, String> {
    let mut o = o.clone();
    if let Object::Entity {
        common,
        entity: Entity::Acis(a),
    } = &mut o
    {
        let guid = AcisGuid::derived(&d.header.fingerprintguid, h);
        let (converted, data) = a.to_r2013(guid).ok_or("ACIS not convertible")?;
        **a = converted;
        if data.is_some() {
            common.obj.has_ds_binary_data = true;
        }
    }
    let bytes = encode_object(&o, &d.classes).map_err(|e| format!("{h}: {e}"))?;
    let mut ctx = DecodeCtx::new(Version::R2018, d.header.dwgcodepage, &d.classes);
    let back = decode_object(&bytes, h, &mut ctx).map_err(|e| format!("{h}: {e}"))?;
    if !ctx.diagnostics.is_empty() {
        return Err(format!("{h}: {:?}", ctx.diagnostics));
    }
    Ok(back)
}

/// Whether `b` (read back) is object `h` of `d` as written: for a file read
/// from R2013 or later the object itself (raw bits normalized), for an
/// older one the object as R2018 gives it back (see [`as_r2018`]).
pub fn same_object(d: &Drawing, h: Handle, o: &Object, b: &Object) -> bool {
    if d.source_version().is_some_and(|v| v >= Version::R2013) {
        normalized(o) == normalized(b)
    } else {
        as_r2018(d, h, o).is_ok_and(|x| x == *b)
    }
}

/// Where the debug forms of `a` and `b` first differ.
pub fn debug_diff(a: &Object, b: &Object) -> String {
    let (a, b): (Vec<char>, Vec<char>) = (
        format!("{a:?}").chars().collect(),
        format!("{b:?}").chars().collect(),
    );
    let at = a
        .iter()
        .zip(&b)
        .position(|(x, y)| x != y)
        .unwrap_or(a.len().min(b.len()));
    let window = |s: &[char]| -> String {
        s[at.saturating_sub(120).min(s.len())..(at + 120).min(s.len())]
            .iter()
            .collect()
    };
    format!("\n  {}\n  {}", window(&a), window(&b))
}

/// `b`, the object `h` read back, with the documented writer refreshes
/// undone so it can be compared with the object as read: entries for the
/// objects the writer added (a repaired `Standard` dimension style, an
/// `ACAD_PLOTSETTINGS` or `ACAD_GROUP` dictionary) dropped from table
/// controls and dictionaries; the owner and entity mode of an object a
/// write warning names (null owners set to their lister, entities given
/// the block that lists them) taken from `o`, and `has_ds_binary_data`
/// taken from `o` when the read-back value is the one the written data
/// storage records give (the writer derives it; ODA-based writers leave it
/// clear on objects with a record, `gdal-02`).
///
/// The per-writer exceptions this covers in the extended corpus: BricsCAD,
/// Geo7/Teigha, ezdxf and SolidWorks files lacking the `Standard`
/// dimension style (`brx-*`, `geo7-01`, `ezdxf-04`, `dsh-*`, `gdal-03`..),
/// civilcraft's writer without `ACAD_PLOTSETTINGS` (`cc-06/08/09/12/13`),
/// AutoCAD 2002/2004's null-owner round-trip dictionaries (`arx-02/03`),
/// and the ODA-based `gdal-02`'s data storage flags.
pub fn undo_refreshes(
    d: &Drawing,
    back: &Drawing,
    o: &Object,
    b: &Object,
    warned: &HashSet<Handle>,
) -> Object {
    let mut b = b.clone();
    let known = |h: Handle| h.0 == 0 || d.get(h).is_some();
    match &mut b {
        Object::Other {
            object: NonGraphical::TableControl(c),
            ..
        } => c.entries.retain(|&e| known(e)),
        Object::Other {
            object: NonGraphical::Dictionary(x),
            ..
        } => x.entries.retain(|&(_, e)| known(e)),
        _ => {}
    }
    let h = o.handle();
    if warned.contains(&h) {
        b.object_common_mut().owner = o.object_common().owner;
        if let (Object::Entity { common: bc, .. }, Object::Entity { common: oc, .. }) = (&mut b, o)
        {
            bc.entmode = oc.entmode;
        }
    }
    let has_record = back
        .data_storage
        .as_ref()
        .is_some_and(|s| s.records.iter().any(|r| r.handle == h));
    if b.object_common().has_ds_binary_data == has_record {
        b.object_common_mut().has_ds_binary_data = o.object_common().has_ds_binary_data;
    }
    b
}
