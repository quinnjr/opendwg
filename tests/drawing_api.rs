//! Task 20: the drawing's typed accessors and mutation API, on the R2018
//! corpus sample. After every mutation each touched object is encoded as
//! R2018 and decoded back equal, and the cross-object structure (owner lists,
//! table control entries, insert attributes, reactors, groups) has no
//! problem the drawing did not have as read.

mod common;

use std::collections::{BTreeSet, HashMap, HashSet};

use opendwg::object::{
    Attrib, DecodeCtx, Entity, EntityCommon, EntityList, Insert, Line, MInsert, NonGraphical,
    ObjReader, Object, PolyfaceMesh, PolygonMesh, Polyline3d, TableKind, Text, Vertex3d,
    VertexPfaceFace, XDataApp, XDataItem, decode_object, encode_object,
};
use opendwg::{Drawing, Error, Handle, Version};

const R2018: &str = "acadsharp-sample-ac1032";

fn r2018() -> Option<Drawing> {
    let p = common::corpus_file_or_skip(R2018)?;
    Some(Drawing::open(p).unwrap())
}

fn line(a: [f64; 3], b: [f64; 3]) -> Entity {
    Entity::Line(Line {
        start: a,
        end: b,
        thickness: 0.0,
        extrusion: [0.0, 0.0, 1.0],
    })
}

fn layer0(d: &Drawing) -> Handle {
    d.layer_by_name("0").unwrap()
}

/// Encode `h` as R2018 and decode it back: it must read back equal.
fn assert_roundtrips(d: &Drawing, h: Handle) {
    let obj = d.get(h).unwrap_or_else(|| panic!("{h} missing"));
    let bytes = encode_object(obj, &d.classes).unwrap_or_else(|e| panic!("{h}: {e}"));
    let mut ctx = DecodeCtx::new(Version::R2018, d.header.dwgcodepage, &d.classes);
    let back = decode_object(&bytes, h, &mut ctx).unwrap();
    assert_eq!(&back, obj, "{h} does not read back equal");
    assert!(ctx.diagnostics.is_empty(), "{h}: {:?}", ctx.diagnostics);
}

/// Problems in the cross-object structure: owner lists, control entries,
/// insert attributes and polyline vertices, block insert lists, groups and
/// reactors. Each is a string naming the objects, so the problems of a
/// drawing can be compared with those it had as read.
fn structure_problems(d: &Drawing) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let exists = |h: Handle| d.get(h).is_some();
    let mut listed: HashSet<Handle> = HashSet::new();
    for (h, o) in d.objects() {
        for r in &o.object_common().reactors {
            if !exists(*r) {
                out.insert(format!("{h}: reactor {r} missing"));
            }
        }
        if let Some(x) = o.object_common().xdict {
            if !exists(x) {
                out.insert(format!("{h}: xdict {x} missing"));
            }
        }
        match o {
            Object::Other {
                object: NonGraphical::BlockRecord(b),
                ..
            } => {
                let EntityList::Owned(list) = &b.entities else {
                    out.insert(format!("{h}: entity list not built"));
                    continue;
                };
                for &e in list.iter().chain([&b.block_entity, &b.end_block]) {
                    listed.insert(e);
                    if d.owner_of(e) != Some(h) {
                        out.insert(format!("{h}: entity {e} not owned by it"));
                    }
                }
                for &i in &b.inserts {
                    match d.get(i) {
                        Some(Object::Entity {
                            entity: Entity::Insert(ins),
                            ..
                        }) if ins.block == h => {}
                        Some(Object::Entity {
                            entity: Entity::MInsert(m),
                            ..
                        }) if m.insert.block == h => {}
                        _ => {
                            out.insert(format!("{h}: insert {i} not an insert of it"));
                        }
                    }
                }
            }
            Object::Other {
                object: NonGraphical::TableControl(c),
                ..
            } => {
                for &e in c.entries.iter().chain(&c.extra) {
                    if e.0 != 0 && d.owner_of(e) != Some(h) && c.kind != TableKind::DimStyle {
                        out.insert(format!("{h}: entry {e} not owned by it"));
                    }
                    if e.0 != 0 && !exists(e) {
                        out.insert(format!("{h}: entry {e} missing"));
                    }
                }
            }
            Object::Other {
                object: NonGraphical::Group(g),
                ..
            } => {
                for &e in &g.entities {
                    if !exists(e) {
                        out.insert(format!("{h}: group entity {e} missing"));
                    }
                }
            }
            Object::Other {
                object: NonGraphical::Dictionary(dict),
                ..
            } => {
                for (name, e) in &dict.entries {
                    if e.0 != 0 && !exists(*e) {
                        out.insert(format!("{h}: entry {name} {e} missing"));
                    }
                }
            }
            Object::Entity { entity, common } => {
                if !exists(common.layer) {
                    out.insert(format!("{h}: layer {} missing", common.layer));
                }
                let children = match entity {
                    Entity::Insert(i)
                    | Entity::MInsert(opendwg::object::MInsert { insert: i, .. }) => {
                        if !exists(i.block) {
                            out.insert(format!("{h}: block {} missing", i.block));
                        }
                        match d.get(i.block) {
                            Some(Object::Other {
                                object: NonGraphical::BlockRecord(b),
                                ..
                            }) if b.inserts.contains(&h) => {}
                            _ => {
                                out.insert(format!("{h}: not in its block's insert list"));
                            }
                        }
                        let EntityList::Owned(list) = &i.attribs else {
                            continue;
                        };
                        if list.is_empty() != i.seqend.is_none() {
                            out.insert(format!("{h}: attribs and seqend disagree"));
                        }
                        Some((list.clone(), i.seqend))
                    }
                    e => match (e.polyline_vertices(), e.polyline_seqend()) {
                        (Some(EntityList::Owned(list)), Some(s)) => Some((list.clone(), Some(s))),
                        _ => None,
                    },
                };
                if let Some((list, seqend)) = children {
                    for c in list.into_iter().chain(seqend) {
                        listed.insert(c);
                        if d.owner_of(c) != Some(h) {
                            out.insert(format!("{h}: child {c} not owned by it"));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    // Every entity is in its owner's list.
    for (h, o) in d.objects() {
        if matches!(o, Object::Entity { .. }) && !listed.contains(&h) {
            out.insert(format!("{h}: entity in no owner list"));
        }
    }
    out
}

/// The hard (codes 3 and 5) references `obj` stores, from its R2018
/// encoding; none for an object that cannot be encoded.
fn hard_refs(d: &Drawing, obj: &Object) -> Vec<Handle> {
    let Ok(bytes) = encode_object(obj, &d.classes) else {
        return vec![];
    };
    let (mut r, _) = ObjReader::split(&bytes, Version::R2018, d.header.dwgcodepage).unwrap();
    let mut out = vec![];
    while r.handles.remaining_bits() >= 8 {
        let href = r.handles.read_handle_ref().unwrap();
        if matches!(href.code, 3 | 5) && href.value != 0 {
            out.extend(href.resolve(obj.handle()));
        }
    }
    out
}

/// A drawing as read: its structure problems, its objects and their hard
/// references.
struct Baseline {
    problems: BTreeSet<String>,
    objects: HashMap<Handle, (Object, Vec<Handle>)>,
}

impl Baseline {
    fn of(d: &Drawing) -> Baseline {
        Baseline {
            problems: problems_with_refs(d, None),
            objects: d
                .objects()
                .map(|(h, o)| (h, (o.clone(), hard_refs(d, o))))
                .collect(),
        }
    }
}

/// [`structure_problems`], plus every non-entity's owner must exist and
/// every hard reference must be to an existing object. Objects unchanged
/// since `baseline` reuse its references.
fn problems_with_refs(d: &Drawing, baseline: Option<&Baseline>) -> BTreeSet<String> {
    let mut out = structure_problems(d);
    for (h, o) in d.objects() {
        if let Object::Other { common, .. } = o {
            if common.owner.0 != 0 && d.get(common.owner).is_none() {
                out.insert(format!("{h}: owner {} missing", common.owner));
            }
        }
        let refs = match baseline.and_then(|b| b.objects.get(&h)) {
            Some((was, refs)) if was == o => refs.clone(),
            _ => hard_refs(d, o),
        };
        for t in refs {
            if d.get(t).is_none() {
                out.insert(format!("{h}: hard reference to missing {t}"));
            }
        }
    }
    out
}

/// No structure problem the drawing did not have as read.
fn assert_no_new_problems(before: &Baseline, d: &Drawing) {
    let after = problems_with_refs(d, Some(before));
    let new: Vec<_> = after.difference(&before.problems).collect();
    assert!(new.is_empty(), "new structure problems: {new:?}");
}

fn block_list(d: &Drawing, block: Handle) -> Vec<Handle> {
    match d.get(block) {
        Some(Object::Other {
            object: NonGraphical::BlockRecord(b),
            ..
        }) => match &b.entities {
            EntityList::Owned(v) => v.clone(),
            l => panic!("{l:?}"),
        },
        o => panic!("{o:?}"),
    }
}

#[test]
fn add_line_to_model_space() {
    let Some(mut d) = r2018() else { return };
    let before = Baseline::of(&d);
    let ms = d.model_space();
    let n = d.entities_in(ms).count();
    assert!(n > 0);
    let h = d
        .add_entity(
            ms,
            EntityCommon::new(layer0(&d)),
            line([0.0; 3], [1.0, 2.0, 0.0]),
        )
        .unwrap();
    let list: Vec<Handle> = d.entities_in(ms).collect();
    assert_eq!(list.len(), n + 1);
    assert_eq!(list.last(), Some(&h));
    assert_eq!(d.owner_of(h), Some(ms));
    // Model space entities store no owner: entity mode 2, as AutoCAD writes.
    let Some(Object::Entity { common, .. }) = d.get(h) else {
        panic!()
    };
    assert_eq!(common.entmode, 2);
    assert_eq!(common.obj.handle, h);
    assert_roundtrips(&d, h);
    assert_roundtrips(&d, ms);
    assert_no_new_problems(&before, &d);
    // In a user block the owner is stored (entity mode 0).
    let block = d.add_block("MyNewBlock").unwrap();
    let e = d
        .add_entity(
            block,
            EntityCommon::new(layer0(&d)),
            line([0.0; 3], [1.0; 3]),
        )
        .unwrap();
    assert!(d.entities_in(block).eq([e]));
    let Some(Object::Entity { common, .. }) = d.get(e) else {
        panic!()
    };
    assert_eq!((common.entmode, common.obj.owner), (0, block));
    assert_roundtrips(&d, e);
    assert_no_new_problems(&before, &d);
}

#[test]
fn remove_in_use_layer_refused() {
    let Some(mut d) = r2018() else { return };
    let before = Baseline::of(&d);
    let ms = d.model_space();
    let layer = d.layer_by_name("Layer2").unwrap();
    let e = d
        .add_entity(ms, EntityCommon::new(layer), line([0.0; 3], [1.0; 3]))
        .unwrap();
    match d.remove(layer) {
        Err(Error::InUse { objects, .. }) => assert!(objects.contains(&e), "{objects:?}"),
        r => panic!("{r:?}"),
    }
    assert!(d.get(layer).is_some());
    assert!(d.referrers(layer).contains(&e));
    // Once nothing uses it, it can be removed (its extension dictionary
    // goes with it) and its control entry goes too.
    let mut layer_users = d.referrers(layer);
    let control = d.header.layer_control;
    layer_users.retain(|&r| r != control);
    for u in layer_users {
        assert!(
            matches!(d.get(u), Some(Object::Entity { .. })),
            "{u} uses the layer"
        );
        if d.get(u).is_some() {
            d.remove(u).unwrap();
        }
    }
    let xdict = d.get(layer).unwrap().object_common().xdict;
    let removed = d.remove(layer).unwrap();
    assert_eq!(removed.handle(), layer);
    assert!(d.layer_by_name("Layer2").is_none());
    if let Some(x) = xdict {
        assert!(d.get(x).is_none(), "the extension dictionary is removed");
    }
    assert_roundtrips(&d, control);
    assert_no_new_problems(&before, &d);
}

#[test]
fn remove_refuses_protected_and_header_referenced() {
    let Some(mut d) = r2018() else { return };
    for name in ["0", "defpoints"] {
        let h = d.layer_by_name(name).unwrap();
        assert!(matches!(d.remove(h), Err(Error::Protected(_))), "{name}");
    }
    let ms = d.model_space();
    assert!(matches!(d.remove(ms), Err(Error::Protected(_))));
    for ps in d.paper_spaces() {
        assert!(matches!(d.remove(ps), Err(Error::Protected(_))));
    }
    for (h, lt) in d
        .linetypes()
        .map(|(h, l)| (h, l.entry.name.clone()))
        .collect::<Vec<_>>()
    {
        if ["bylayer", "byblock", "continuous"].contains(&lt.to_lowercase().as_str()) {
            assert!(matches!(d.remove(h), Err(Error::Protected(_))), "{lt}");
        }
    }
    let standard: Vec<Handle> = d
        .text_styles()
        .filter(|(_, s)| s.entry.name == "Standard")
        .map(|(h, _)| h)
        .collect();
    for h in standard {
        assert!(matches!(d.remove(h), Err(Error::Protected(_))));
    }
    let standard: Vec<Handle> = d
        .dim_styles()
        .filter(|(_, s)| s.entry.name == "Standard")
        .map(|(h, _)| h)
        .collect();
    for h in standard {
        assert!(matches!(d.remove(h), Err(Error::Protected(_))));
    }
    // Layouts are protected with their block records.
    let layout = d.layouts()[1].0;
    match d.remove(layout) {
        Err(Error::Protected(why)) => assert!(why.contains("layout"), "{why}"),
        r => panic!("{r:?}"),
    }
    // A layer that only the header uses ($CLAYER): in use by the header
    // variable, named.
    let mut layer = d
        .get(layer0(&d))
        .map(|o| match o {
            Object::Other {
                object: NonGraphical::Layer(l),
                ..
            } => l.clone(),
            _ => panic!(),
        })
        .unwrap();
    layer.entry.name = "Current".into();
    let h = d.add_layer(layer).unwrap();
    d.header.clayer = h;
    match d.remove(h) {
        Err(Error::InUse {
            objects,
            header_vars,
        }) => {
            assert!(objects.is_empty(), "{objects:?}");
            assert_eq!(header_vars, ["$CLAYER"]);
        }
        r => panic!("{r:?}"),
    }
    assert_eq!(d.referrers(h), [d.header.layer_control]);
    assert_eq!(d.header_references(h), ["$CLAYER"]);
    d.header.clayer = layer0(&d);
    d.remove(h).unwrap();
    assert!(matches!(d.remove(h), Err(Error::NotFound(_))));
}

#[test]
fn remove_entity_with_attribs_removes_children() {
    let Some(mut d) = r2018() else { return };
    let before = Baseline::of(&d);
    let ms = d.model_space();
    let l0 = layer0(&d);
    let block = d.add_block("Tagged").unwrap();
    let style = d.text_styles().next().unwrap().0;
    let attrib_tpl = attrib_template(&d);
    let insert = Insert {
        insertion: [5.0, 5.0, 0.0],
        scale: [1.0; 3],
        rotation: 0.0,
        extrusion: [0.0, 0.0, 1.0],
        block,
        attribs: EntityList::Owned(vec![]),
        seqend: None,
    };
    let attrib = |tag: &str| {
        let text = Text {
            insertion: [5.0, 5.0, 0.0],
            alignment: None,
            extrusion: [0.0, 0.0, 1.0],
            thickness: 0.0,
            oblique_angle: 0.0,
            rotation: 0.0,
            height: 1.0,
            width_factor: 1.0,
            value: format!("value of {tag}"),
            generation: 0,
            horizontal_alignment: 0,
            vertical_alignment: 0,
            style,
        };
        (
            EntityCommon::new(l0),
            Attrib {
                text,
                tag: tag.into(),
                ..attrib_tpl.clone()
            },
        )
    };
    let attribs = vec![attrib("A"), attrib("B")];
    let ih = d
        .add_insert_with_attribs(ms, (EntityCommon::new(l0), Entity::Insert(insert)), attribs)
        .unwrap();
    let Some(Object::Entity {
        entity: Entity::Insert(i),
        ..
    }) = d.get(ih)
    else {
        panic!()
    };
    let EntityList::Owned(attribs) = i.attribs.clone() else {
        panic!()
    };
    let seqend = i.seqend.unwrap();
    assert_eq!(attribs.len(), 2);
    for &c in attribs.iter().chain([&seqend]) {
        assert_eq!(d.owner_of(c), Some(ih));
        assert_roundtrips(&d, c);
    }
    assert_roundtrips(&d, ih);
    assert_roundtrips(&d, block);
    assert!(d.referrers(block).contains(&ih));
    assert_no_new_problems(&before, &d);

    // The block is in use by the insert.
    assert!(matches!(d.remove(block), Err(Error::InUse { objects, .. }) if objects == [ih]));

    // Removing one attribute keeps the other and the SEQEND; removing the
    // last one removes the SEQEND too.
    d.remove(attribs[0]).unwrap();
    assert_roundtrips(&d, ih);
    assert!(d.get(seqend).is_some());
    d.remove(attribs[1]).unwrap();
    assert!(d.get(seqend).is_none());
    let Some(Object::Entity {
        entity: Entity::Insert(i),
        ..
    }) = d.get(ih)
    else {
        panic!()
    };
    assert_eq!((&i.attribs, i.seqend), (&EntityList::Owned(vec![]), None));
    assert_roundtrips(&d, ih);
    assert_no_new_problems(&before, &d);
    let first = ih;

    // A fresh insert with attributes: removing it removes them all, and it
    // leaves the block's insert list.
    let ih = d
        .add_insert_with_attribs(
            ms,
            (
                EntityCommon::new(l0),
                Entity::Insert(Insert {
                    insertion: [0.0; 3],
                    scale: [1.0; 3],
                    rotation: 0.0,
                    extrusion: [0.0, 0.0, 1.0],
                    block,
                    attribs: EntityList::Owned(vec![]),
                    seqend: None,
                }),
            ),
            vec![attrib("C")],
        )
        .unwrap();
    let Some(Object::Entity {
        entity: Entity::Insert(i),
        ..
    }) = d.get(ih)
    else {
        panic!()
    };
    let children: Vec<Handle> = match &i.attribs {
        EntityList::Owned(v) => v.iter().copied().chain(i.seqend).collect(),
        _ => panic!(),
    };
    let n = d.entities_in(ms).count();
    let removed = d.remove(ih).unwrap();
    assert!(matches!(
        removed,
        Object::Entity {
            entity: Entity::Insert(_),
            ..
        }
    ));
    for c in children {
        assert!(d.get(c).is_none(), "{c} removed with its insert");
    }
    assert_eq!(d.entities_in(ms).count(), n - 1);
    assert!(!block_list(&d, ms).contains(&ih));
    let Some(Object::Other {
        object: NonGraphical::BlockRecord(b),
        ..
    }) = d.get(block)
    else {
        panic!()
    };
    assert_eq!(b.inserts, [first]);
    assert_roundtrips(&d, ms);
    assert_roundtrips(&d, block);
    // Once the first insert is gone too, the block is no longer used and
    // can be removed, with its BLOCK and ENDBLK.
    assert!(matches!(d.remove(block), Err(Error::InUse { objects, .. }) if objects == [first]));
    d.remove(first).unwrap();
    let Some(Object::Other {
        object: NonGraphical::BlockRecord(b),
        ..
    }) = d.get(block)
    else {
        panic!()
    };
    let (be, ee) = (b.block_entity, b.end_block);
    d.remove(block).unwrap();
    assert!(d.get(be).is_none() && d.get(ee).is_none());
    assert_roundtrips(&d, d.header.block_control);
    assert_no_new_problems(&before, &d);
}

/// An ATTRIB and TEXT read from the sample, as templates for the fields the
/// test does not set.
fn attrib_template(d: &Drawing) -> Attrib {
    d.objects()
        .find_map(|(_, o)| match o {
            Object::Entity {
                entity: Entity::Attrib(a),
                ..
            } if a.mtext.is_none() => Some((**a).clone()),
            _ => None,
        })
        .expect("the sample has a single-line ATTRIB")
}

#[test]
fn alloc_handle_monotonic_and_bumps_handseed() {
    let Some(mut d) = r2018() else { return };
    let seed = d.header.handseed;
    let max = d.objects().map(|(h, _)| h).max().unwrap();
    assert!(seed > max);
    let a = d.alloc_handle().unwrap();
    let b = d.alloc_handle().unwrap();
    assert_eq!(a, seed);
    assert_eq!(b.0, a.0 + 1);
    assert_eq!(d.header.handseed.0, b.0 + 1);
    // A seed at or below an existing handle never gives that handle.
    d.header.handseed = Handle(0x10);
    let c = d.alloc_handle().unwrap();
    assert!(d.get(c).is_none());
    assert!(c > max);
    assert_eq!(d.header.handseed.0, c.0 + 1);
    // Added objects take allocated handles.
    let ms = d.model_space();
    let e = d
        .add_entity(ms, EntityCommon::new(layer0(&d)), line([0.0; 3], [1.0; 3]))
        .unwrap();
    assert_eq!(e.0, c.0 + 1);
}

#[test]
fn layer_by_name_case_insensitive() {
    let Some(d) = r2018() else { return };
    let h = d.layer_by_name("Layer_Off").unwrap();
    assert_eq!(d.layer_by_name("LAYER_OFF"), Some(h));
    assert_eq!(d.layer_by_name("layer_off"), Some(h));
    assert_eq!(d.layer_by_name("Defpoints"), d.layer_by_name("DEFPOINTS"));
    assert!(d.layer_by_name("no such layer").is_none());
    let (_, l) = d.layers().find(|&(x, _)| x == h).unwrap();
    assert_eq!(l.entry.name, "Layer_Off");
}

#[test]
fn accessors_on_the_sample() {
    let Some(d) = r2018() else { return };
    assert_eq!(d.model_space(), d.header.block_record_model_space);
    assert_eq!(d.named_objects(), d.header.dict_named_objects);
    let layouts = d.layouts();
    assert!(layouts.len() >= 2);
    assert!(
        layouts
            .windows(2)
            .all(|w| w[0].1.tab_order <= w[1].1.tab_order)
    );
    // Paper spaces follow the layout tabs; model space is not one.
    let ps = d.paper_spaces();
    let from_layouts: Vec<Handle> = layouts
        .iter()
        .map(|(_, l)| l.block_record)
        .filter(|&b| b != d.model_space())
        .collect();
    assert_eq!(ps, from_layouts);
    assert!(ps.contains(&d.header.block_record_paper_space));
    let names: Vec<_> = d
        .block_records()
        .map(|(_, b)| b.entry.name.clone())
        .collect();
    assert!(names.contains(&"*Model_Space".to_string()));
    assert!(names.contains(&"MyBlock".to_string()));
    assert!(
        d.linetypes()
            .any(|(_, l)| l.entry.name.eq_ignore_ascii_case("ByLayer"))
    );
    assert!(d.text_styles().any(|(_, s)| s.entry.name == "Standard"));
    assert!(d.dim_styles().any(|(_, s)| s.entry.name == "Standard"));
    // Every model space entity, in owner order.
    let ms: Vec<Handle> = d.entities_in(d.model_space()).collect();
    assert_eq!(ms, block_list(&d, d.model_space()));
    assert!(d.entities_in(d.header.layer_control).next().is_none());
}

#[test]
fn table_adds_append_to_controls_and_refuse_duplicates() {
    let Some(mut d) = r2018() else { return };
    let before = Baseline::of(&d);
    let pick = |d: &Drawing, kind: TableKind| -> NonGraphical {
        let control = match kind {
            TableKind::Layer => d.header.layer_control,
            TableKind::Linetype => d.header.linetype_control,
            TableKind::Style => d.header.style_control,
            TableKind::DimStyle => d.header.dimstyle_control,
            _ => unreachable!(),
        };
        let Some(Object::Other {
            object: NonGraphical::TableControl(c),
            ..
        }) = d.get(control)
        else {
            panic!()
        };
        // The last named entry that exists (AutoCAD keeps erased entries'
        // handles; unnamed shape files may share the empty name).
        c.entries
            .iter()
            .rev()
            .find_map(|&e| match d.get(e) {
                Some(Object::Other { object, .. })
                    if object.table_entry().is_some_and(|t| !t.name.is_empty()) =>
                {
                    Some(object.clone())
                }
                _ => None,
            })
            .unwrap()
    };
    let mut added = vec![];
    for kind in [
        TableKind::Layer,
        TableKind::Linetype,
        TableKind::Style,
        TableKind::DimStyle,
    ] {
        let template = pick(&d, kind);
        let (h, dup) = match template {
            NonGraphical::Layer(mut l) => {
                let dup = d.add_layer(l.clone());
                l.entry.name = "Added".into();
                (d.add_layer(l).unwrap(), dup)
            }
            NonGraphical::Linetype(mut l) => {
                let dup = d.add_linetype(l.clone());
                l.entry.name = "Added".into();
                (d.add_linetype(l).unwrap(), dup)
            }
            NonGraphical::TextStyle(mut s) => {
                let dup = d.add_text_style(s.clone());
                s.entry.name = "Added".into();
                (d.add_text_style(s).unwrap(), dup)
            }
            NonGraphical::DimStyle(mut s) => {
                let mut upper = (*s).clone();
                upper.entry.name = upper.entry.name.to_uppercase();
                let dup = d.add_dim_style(upper);
                s.entry.name = "Added".into();
                (d.add_dim_style(*s).unwrap(), dup)
            }
            o => panic!("{o:?}"),
        };
        assert!(matches!(dup, Err(Error::Invalid(_))), "{kind:?} duplicate");
        let control = d.get(h).unwrap().object_common().owner;
        let Some(Object::Other {
            object: NonGraphical::TableControl(c),
            ..
        }) = d.get(control)
        else {
            panic!()
        };
        assert_eq!(c.kind, kind);
        assert_eq!(c.entries.last(), Some(&h));
        assert_roundtrips(&d, h);
        assert_roundtrips(&d, control);
        added.push(h);
    }
    assert!(d.layers().any(|(h, _)| h == added[0]));
    assert!(d.linetypes().any(|(h, _)| h == added[1]));
    assert!(d.text_styles().any(|(h, _)| h == added[2]));
    assert!(d.dim_styles().any(|(h, _)| h == added[3]));
    assert!(d.add_block("mynewblock").is_ok());
    assert!(matches!(d.add_block("MYNEWBLOCK"), Err(Error::Invalid(_))));
    assert!(matches!(d.add_block("MyBlock"), Err(Error::Invalid(_))));
    assert!(matches!(d.add_block(""), Err(Error::Invalid(_))));
    assert!(matches!(d.add_block("*U9"), Err(Error::Invalid(_))));
    assert_no_new_problems(&before, &d);
    // Unused added entries can be removed again.
    for h in added {
        d.remove(h).unwrap();
    }
    assert_no_new_problems(&before, &d);
}

#[test]
fn polyline_with_vertices_add_and_remove() {
    let Some(mut d) = r2018() else { return };
    let before = Baseline::of(&d);
    let ms = d.model_space();
    let l0 = layer0(&d);
    let poly = Entity::Polyline3d(Polyline3d {
        spline_flags: 0,
        closed_flags: 1,
        vertices: EntityList::Owned(vec![]),
        seqend: Handle(0),
    });
    let vertex = |p: [f64; 3]| {
        (
            EntityCommon::new(l0),
            Entity::Vertex3d(Vertex3d {
                flags: 32,
                point: p,
            }),
        )
    };
    let vs = vec![
        vertex([0.0; 3]),
        vertex([1.0, 0.0, 0.0]),
        vertex([1.0, 1.0, 0.0]),
    ];
    let ph = d
        .add_polyline(ms, (EntityCommon::new(l0), poly.clone()), vs)
        .unwrap();
    let Some(Object::Entity { entity, .. }) = d.get(ph) else {
        panic!()
    };
    let EntityList::Owned(vertices) = entity.polyline_vertices().unwrap().clone() else {
        panic!()
    };
    let seqend = entity.polyline_seqend().unwrap();
    assert_eq!(vertices.len(), 3);
    for &c in vertices.iter().chain([&seqend]) {
        assert_eq!(d.owner_of(c), Some(ph));
        assert_roundtrips(&d, c);
    }
    assert_roundtrips(&d, ph);
    assert_no_new_problems(&before, &d);
    // A vertex of the wrong kind is refused.
    let bad = vec![(EntityCommon::new(l0), line([0.0; 3], [1.0; 3]))];
    assert!(matches!(
        d.add_polyline(ms, (EntityCommon::new(l0), poly), bad),
        Err(Error::Invalid(_))
    ));
    // Removing a vertex unlinks it; removing the polyline removes the rest.
    d.remove(vertices[1]).unwrap();
    assert_roundtrips(&d, ph);
    d.remove(ph).unwrap();
    for c in [vertices[0], vertices[2], seqend] {
        assert!(d.get(c).is_none());
    }
    assert_no_new_problems(&before, &d);
    // The structural entities cannot be removed on their own.
    let Some(Object::Other {
        object: NonGraphical::BlockRecord(b),
        ..
    }) = d.get(ms)
    else {
        panic!()
    };
    let block_entity = b.block_entity;
    assert!(matches!(d.remove(block_entity), Err(Error::Invalid(_))));
}

#[test]
fn add_entity_checks_its_references() {
    let Some(mut d) = r2018() else { return };
    let ms = d.model_space();
    let seed = d.header.handseed;
    let n = d.objects().count();
    // A missing layer, a layer that is not one, a missing block, a block
    // that is not a block record, structural entities: all refused, and
    // nothing changes.
    let missing = Handle(0xFFFF_FFFF);
    assert!(matches!(
        d.add_entity(ms, EntityCommon::new(missing), line([0.0; 3], [1.0; 3])),
        Err(Error::NotFound(h)) if h == missing
    ));
    assert!(matches!(
        d.add_entity(
            ms,
            EntityCommon::new(d.header.layer_control),
            line([0.0; 3], [1.0; 3])
        ),
        Err(Error::Invalid(_))
    ));
    let l0 = layer0(&d);
    assert!(matches!(
        d.add_entity(missing, EntityCommon::new(l0), line([0.0; 3], [1.0; 3])),
        Err(Error::NotFound(_))
    ));
    assert!(matches!(
        d.add_entity(l0, EntityCommon::new(l0), line([0.0; 3], [1.0; 3])),
        Err(Error::Invalid(_))
    ));
    for e in [Entity::SeqEnd, Entity::EndBlk] {
        assert!(matches!(
            d.add_entity(ms, EntityCommon::new(l0), e),
            Err(Error::Invalid(_))
        ));
    }
    let mut with_xdict = EntityCommon::new(l0);
    with_xdict.obj.xdict = Some(d.named_objects());
    assert!(matches!(
        d.add_entity(ms, with_xdict, line([0.0; 3], [1.0; 3])),
        Err(Error::Invalid(_))
    ));
    assert_eq!(d.header.handseed, seed);
    assert_eq!(d.objects().count(), n);
}

#[test]
fn referrers_index_follows_mutation() {
    let Some(mut d) = r2018() else { return };
    let ms = d.model_space();
    let layer = d.layer_by_name("Layer1").unwrap();
    let before = d.referrers(layer);
    let e = d
        .add_entity(ms, EntityCommon::new(layer), line([0.0; 3], [1.0; 3]))
        .unwrap();
    let with = d.referrers(layer);
    assert_eq!(with.len(), before.len() + 1);
    assert!(with.contains(&e));
    // Moving the entity to another layer through get_mut: the index is
    // rebuilt.
    let l0 = layer0(&d);
    if let Some(Object::Entity { common, .. }) = d.get_mut(e) {
        common.layer = l0;
    }
    assert_eq!(d.referrers(layer), before);
    assert!(d.referrers(l0).contains(&e));
    d.remove(e).unwrap();
    assert!(!d.referrers(l0).contains(&e));
    // The block record referrers of model space: its control.
    assert!(d.referrers(ms).contains(&d.header.block_control));
}

#[test]
fn removing_an_entity_drops_group_and_reactor_references() {
    let Some(mut d) = r2018() else { return };
    let before = Baseline::of(&d);
    // An entity in a group lists the group among its reactors.
    let found = d.objects().find_map(|(g, o)| match o {
        Object::Other {
            object: NonGraphical::Group(grp),
            ..
        } => grp.entities.first().map(|&e| (g, e)),
        _ => None,
    });
    let Some((group, e)) = found else {
        common::skip("a GROUP in the R2018 sample");
        return;
    };
    assert!(d.referrers(e).contains(&group));
    d.remove(e).unwrap();
    let Some(Object::Other {
        object: NonGraphical::Group(g),
        ..
    }) = d.get(group)
    else {
        panic!()
    };
    assert!(!g.entities.contains(&e));
    assert!(
        d.objects()
            .all(|(_, o)| !o.object_common().reactors.contains(&e))
    );
    assert_roundtrips(&d, group);
    assert_no_new_problems(&before, &d);
}

/// R2000 drawings' lists are built after reading; one whose chain was
/// broken stays linked, and adding to it fails rather than corrupting it.
#[test]
fn linked_block_list_refuses_mutation() {
    let Some(p) = common::corpus_file_or_skip("acadsharp-sample-ac1015") else {
        return;
    };
    let mut d = Drawing::open(p).unwrap();
    let ms = d.model_space();
    // Every list of the sample is built, and it can be changed: its
    // pre-R2013 solids cannot be written, but their one reference (the
    // history) is known, so removal is not refused for them.
    assert!(!block_list(&d, ms).is_empty());
    let l0 = layer0(&d);
    let e = d
        .add_entity(ms, EntityCommon::new(l0), line([0.0; 3], [1.0; 3]))
        .unwrap();
    assert_eq!(d.entities_in(ms).last(), Some(e));
    d.remove(e).unwrap();
    let unused = d.add_layer(match d.get(l0) {
        Some(Object::Other {
            object: NonGraphical::Layer(l),
            ..
        }) => opendwg::object::Layer {
            entry: opendwg::object::TableEntry {
                name: "Unused".into(),
                ..l.entry.clone()
            },
            ..l.clone()
        },
        _ => panic!(),
    });
    d.remove(unused.unwrap()).unwrap();
    let first = block_list(&d, ms)[0];
    if let Some(Object::Other {
        object: NonGraphical::BlockRecord(b),
        ..
    }) = d.get_mut(ms)
    {
        b.entities = EntityList::Linked {
            first,
            last: Handle(0xFFFF_FFFF),
        };
    }
    assert!(matches!(
        d.add_entity(ms, EntityCommon::new(l0), line([0.0; 3], [1.0; 3])),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(d.remove(first), Err(Error::Invalid(_))));
    assert!(d.get(first).is_some());
    // The linked chain is walked as far as it goes.
    assert_eq!(d.entities_in(ms).next(), Some(first));
}

/// Removing each object of the sample in turn: either it is removed, every
/// object it changed reads back equal after encoding and the structure has
/// no new problem, or it is refused and nothing changed.
#[test]
fn remove_every_object_keeps_structure() {
    let Some(d) = r2018() else { return };
    let before = Baseline::of(&d);
    // Build the reference index once: every copy starts with it.
    d.referrers(d.model_space());
    let (mut removed, mut refused) = (0, 0);
    let handles: Vec<Handle> = d.objects().map(|(h, _)| h).collect();
    for h in handles {
        let mut copy = d.clone();
        match copy.remove(h) {
            Ok(obj) => {
                removed += 1;
                assert_eq!(obj.handle(), h);
                assert!(copy.get(h).is_none());
                for (x, o) in copy.objects() {
                    if d.get(x) != Some(o) {
                        assert_roundtrips(&copy, x);
                    }
                }
                assert_no_new_problems(&before, &copy);
                assert_eq!(copy.header, d.header);
            }
            Err(e) => {
                refused += 1;
                assert!(
                    matches!(
                        e,
                        Error::InUse { .. } | Error::Invalid(_) | Error::Protected(_)
                    ),
                    "{h}: {e}"
                );
                assert!(copy.objects().eq(d.objects()), "{h}: refused but changed");
                assert_eq!(copy.header, d.header, "{h}: refused but the header changed");
            }
        }
    }
    assert!(
        removed > 500 && refused > 100,
        "{removed} removed, {refused} refused"
    );
}

/// An entity referred to by a hard pointer (a LEADER's annotation) is in
/// use; one referred to only by soft pointers (an extended data handle) is
/// removed and the pointer left dangling, as real files often have.
#[test]
fn entity_hard_references_refuse_soft_ones_dangle() {
    let Some(mut d) = r2018() else { return };
    let leader = d.objects().find_map(|(h, o)| match o {
        Object::Entity {
            entity: Entity::Leader(l),
            ..
        } if l.annotation.0 != 0 => Some((h, l.annotation)),
        _ => None,
    });
    let (leader, annotation) = leader.expect("the sample has an annotated LEADER");
    assert!(
        matches!(d.remove(annotation), Err(Error::InUse { objects, .. }) if objects == [leader])
    );

    let ms = d.model_space();
    let l0 = layer0(&d);
    let target = d
        .add_entity(ms, EntityCommon::new(l0), line([0.0; 3], [1.0; 3]))
        .unwrap();
    let holder = d
        .add_entity(ms, EntityCommon::new(l0), line([0.0; 3], [2.0; 3]))
        .unwrap();
    let acad = d
        .objects()
        .find_map(|(h, o)| match o {
            Object::Other {
                object: NonGraphical::AppId(a),
                ..
            } if a.entry.name == "ACAD" => Some(h),
            _ => None,
        })
        .unwrap();
    if let Some(Object::Entity { common, .. }) = d.get_mut(holder) {
        common.obj.xdata.push(XDataApp {
            app: acad,
            items: vec![XDataItem::Handle(target)],
        });
    }
    assert!(d.referrers(target).contains(&holder));
    d.remove(target).unwrap();
    let Some(Object::Entity { common, .. }) = d.get(holder) else {
        panic!()
    };
    assert_eq!(common.obj.xdata[0].items, [XDataItem::Handle(target)]);
}

fn plain_insert(block: Handle) -> Insert {
    Insert {
        insertion: [0.0; 3],
        scale: [1.0; 3],
        rotation: 0.0,
        extrusion: [0.0, 0.0, 1.0],
        block,
        attribs: EntityList::Owned(vec![]),
        seqend: None,
    }
}

/// Inserting A into C when C holds B and B holds A (or C itself) would
/// make a circular block reference at any depth: refused.
#[test]
fn circular_block_references_refused_at_any_depth() {
    let Some(mut d) = r2018() else { return };
    let l0 = layer0(&d);
    let [a, b, c] = ["A", "B", "C"].map(|n| d.add_block(n).unwrap());
    let ins = |d: &mut Drawing, into: Handle, of: Handle| {
        d.add_entity(
            into,
            EntityCommon::new(l0),
            Entity::Insert(plain_insert(of)),
        )
    };
    ins(&mut d, a, b).unwrap();
    ins(&mut d, b, c).unwrap();
    for (into, of) in [(c, a), (c, b), (c, c), (b, a)] {
        assert!(
            matches!(ins(&mut d, into, of), Err(Error::Invalid(_))),
            "{of} into {into}"
        );
    }
    // A MINSERT is followed too.
    let minsert = Entity::MInsert(MInsert {
        insert: plain_insert(a),
        columns: 2,
        rows: 2,
        column_spacing: 1.0,
        row_spacing: 1.0,
    });
    assert!(matches!(
        d.add_entity(c, EntityCommon::new(l0), minsert.clone()),
        Err(Error::Invalid(_))
    ));
    // Not circular: A into model space, and a MINSERT of C into A.
    let ms = d.model_space();
    ins(&mut d, ms, a).unwrap();
    let Entity::MInsert(mut m) = minsert else {
        unreachable!()
    };
    m.insert.block = c;
    d.add_entity(a, EntityCommon::new(l0), Entity::MInsert(m))
        .unwrap();
}

/// A MINSERT with attributes owns them and a SEQEND like an INSERT.
#[test]
fn minsert_with_attribs() {
    let Some(mut d) = r2018() else { return };
    let before = Baseline::of(&d);
    let l0 = layer0(&d);
    let block = d.add_block("Grid").unwrap();
    let attrib = attrib_template(&d);
    let minsert = Entity::MInsert(MInsert {
        insert: plain_insert(block),
        columns: 3,
        rows: 2,
        column_spacing: 2.0,
        row_spacing: 2.0,
    });
    let ms = d.model_space();
    let h = d
        .add_insert_with_attribs(
            ms,
            (EntityCommon::new(l0), minsert),
            vec![(EntityCommon::new(l0), attrib)],
        )
        .unwrap();
    let Some(Object::Entity {
        entity: Entity::MInsert(m),
        ..
    }) = d.get(h)
    else {
        panic!()
    };
    let EntityList::Owned(attribs) = m.insert.attribs.clone() else {
        panic!()
    };
    let seqend = m.insert.seqend.unwrap();
    for c in attribs.iter().chain([&seqend]) {
        assert_eq!(d.owner_of(*c), Some(h));
        assert_roundtrips(&d, *c);
    }
    assert_roundtrips(&d, h);
    assert_no_new_problems(&before, &d);
    // Only inserts are taken.
    assert!(matches!(
        d.add_insert_with_attribs(
            ms,
            (EntityCommon::new(l0), line([0.0; 3], [1.0; 3])),
            vec![]
        ),
        Err(Error::Invalid(_))
    ));
    d.remove(h).unwrap();
    assert!(d.get(seqend).is_none());
    assert_no_new_problems(&before, &d);
}

/// A block's ATTDEF flag follows its ATTDEFs.
#[test]
fn has_attdefs_follows_attdefs() {
    let Some(mut d) = r2018() else { return };
    let l0 = layer0(&d);
    let block = d.add_block("WithAttdefs").unwrap();
    let attdef = d
        .objects()
        .find_map(|(_, o)| match o {
            Object::Entity {
                entity: Entity::AttDef(a),
                ..
            } if a.attrib.mtext.is_none() => Some(Entity::AttDef(a.clone())),
            _ => None,
        })
        .unwrap();
    let has = |d: &Drawing| match d.get(block) {
        Some(Object::Other {
            object: NonGraphical::BlockRecord(b),
            ..
        }) => b.has_attdefs,
        _ => panic!(),
    };
    assert!(!has(&d));
    let a1 = d
        .add_entity(block, EntityCommon::new(l0), attdef.clone())
        .unwrap();
    let a2 = d.add_entity(block, EntityCommon::new(l0), attdef).unwrap();
    assert!(has(&d));
    assert_roundtrips(&d, block);
    d.remove(a1).unwrap();
    assert!(has(&d), "one ATTDEF is left");
    d.remove(a2).unwrap();
    assert!(!has(&d));
    assert_roundtrips(&d, block);
}

/// A polyface mesh's vertices and faces must match its counts, and its
/// vertices cannot be removed one by one; a polygon mesh has M x N vertices.
#[test]
fn mesh_counts_are_checked_and_mesh_vertices_kept() {
    let Some(mut d) = r2018() else { return };
    let before = Baseline::of(&d);
    let ms = d.model_space();
    let l0 = layer0(&d);
    let pface = |vertices: i16, faces: i16| {
        (
            EntityCommon::new(l0),
            Entity::PolyfaceMesh(PolyfaceMesh {
                vertex_count: vertices,
                face_count: faces,
                vertices: EntityList::Owned(vec![]),
                seqend: Handle(0),
            }),
        )
    };
    let v = |p: [f64; 3]| {
        (
            EntityCommon::new(l0),
            Entity::VertexPface(Vertex3d {
                flags: 192,
                point: p,
            }),
        )
    };
    let f = |indices: [i16; 4]| {
        (
            EntityCommon::new(l0),
            Entity::VertexPfaceFace(VertexPfaceFace { indices }),
        )
    };
    let children = || {
        vec![
            v([0.0; 3]),
            v([1.0, 0.0, 0.0]),
            v([0.0, 1.0, 0.0]),
            f([1, 2, 3, 0]),
        ]
    };
    for (counts, kids) in [
        ((4, 1), children()),
        ((3, 2), children()),
        (
            (3, 1),
            vec![v([0.0; 3]), v([1.0; 3]), f([1, 2, 3, 0]), v([2.0; 3])],
        ),
        ((3, 1), {
            let mut c = children();
            c[3] = f([1, 2, 4, 0]);
            c
        }),
    ] {
        assert!(
            matches!(
                d.add_polyline(ms, pface(counts.0, counts.1), kids),
                Err(Error::Invalid(_))
            ),
            "{counts:?}"
        );
    }
    let mesh = d.add_polyline(ms, pface(3, 1), children()).unwrap();
    assert_roundtrips(&d, mesh);
    let Some(Object::Entity { entity, .. }) = d.get(mesh) else {
        panic!()
    };
    let EntityList::Owned(vertices) = entity.polyline_vertices().unwrap().clone() else {
        panic!()
    };
    for &x in &vertices {
        assert!(matches!(d.remove(x), Err(Error::Invalid(_))));
    }
    assert_no_new_problems(&before, &d);
    d.remove(mesh).unwrap();
    assert!(vertices.iter().all(|&x| d.get(x).is_none()));
    // A 2 x 2 polygon mesh needs four vertices.
    let grid = |vertices: usize| {
        let poly = Entity::PolygonMesh(PolygonMesh {
            flags: 16,
            curve_type: 0,
            m_count: 2,
            n_count: 2,
            m_density: 0,
            n_density: 0,
            vertices: EntityList::Owned(vec![]),
            seqend: Handle(0),
        });
        let vs = (0..vertices)
            .map(|i| {
                (
                    EntityCommon::new(l0),
                    Entity::VertexMesh(Vertex3d {
                        flags: 64,
                        point: [i as f64, 0.0, 0.0],
                    }),
                )
            })
            .collect();
        ((EntityCommon::new(l0), poly), vs)
    };
    let (p, vs) = grid(3);
    assert!(matches!(d.add_polyline(ms, p, vs), Err(Error::Invalid(_))));
    let (p, vs) = grid(4);
    let mesh = d.add_polyline(ms, p, vs).unwrap();
    assert_roundtrips(&d, mesh);
    assert_no_new_problems(&before, &d);
}

/// A seed below the existing handles (a damaged file) is moved past them
/// on the first mutation, so removing the newest object never lets its
/// handle be given again.
#[test]
fn removed_handles_are_not_reused() {
    let Some(mut d) = r2018() else { return };
    let ms = d.model_space();
    let l0 = layer0(&d);
    let newest = d
        .add_entity(ms, EntityCommon::new(l0), line([0.0; 3], [1.0; 3]))
        .unwrap();
    d.header.handseed = Handle(0x10);
    // A refused removal changes nothing, the stale seed included.
    let used = d.layer_by_name("Layer1").unwrap();
    assert!(matches!(d.remove(used), Err(Error::InUse { .. })));
    assert_eq!(d.header.handseed, Handle(0x10));
    d.remove(newest).unwrap();
    let next = d.alloc_handle().unwrap();
    assert!(next > newest, "{next} after removing {newest}");
}

/// An XRECORD added to the named object dictionary as `name`, with an object
/// id item per handle of `refs` (DXF 340, hard pointers).
fn add_xrecord(d: &mut Drawing, name: &str, refs: &[Handle]) -> Handle {
    use opendwg::object::{XRecord, XRecordValue};
    let items = refs
        .iter()
        .map(|&h| (340, XRecordValue::ObjectId(h)))
        .collect();
    let nod = d.named_objects();
    d.add_object(
        nod,
        name,
        NonGraphical::XRecord(XRecord {
            items,
            cloning: 1,
            ..XRecord::default()
        }),
    )
    .unwrap()
}

/// Point XRECORD `x`'s object id items at `refs`.
fn set_xrecord_refs(d: &mut Drawing, x: Handle, refs: &[Handle]) {
    use opendwg::object::XRecordValue;
    let Some(Object::Other {
        object: NonGraphical::XRecord(r),
        ..
    }) = d.get_mut(x)
    else {
        panic!("{x} is not an XRECORD")
    };
    r.items = refs
        .iter()
        .map(|&h| (340, XRecordValue::ObjectId(h)))
        .collect();
}

/// Objects that refer to each other cannot be removed one by one, but go
/// together with `remove_all`, which returns them in the order given (a
/// handle given twice, once).
#[test]
fn remove_all_takes_mutual_references_together() {
    let mut d = Drawing::new();
    let a = add_xrecord(&mut d, "A", &[]);
    let b = add_xrecord(&mut d, "B", &[a]);
    set_xrecord_refs(&mut d, a, &[b]);
    let c = add_xrecord(&mut d, "C", &[]);
    for (x, by) in [(a, b), (b, a)] {
        match d.remove(x) {
            Err(Error::InUse { objects, .. }) => assert_eq!(objects, [by]),
            other => panic!("{other:?}"),
        }
    }
    let removed = d.remove_all(&[b, a, b, c]).unwrap();
    let handles: Vec<Handle> = removed.iter().map(Object::handle).collect();
    assert_eq!(handles, [b, a, c]);
    for x in [a, b, c] {
        assert!(d.get(x).is_none());
    }
    let errors: Vec<_> = d
        .validate()
        .into_iter()
        .filter(|x| x.severity == opendwg::Severity::Error)
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    let back = Drawing::from_bytes(&d.to_bytes().unwrap()).unwrap();
    assert!(back.get(a).is_none() && back.get(b).is_none());
}

/// A refused `remove_all` changes nothing: a missing handle, a protected
/// object, a referrer outside the set (named by `InUse`), a SEQEND whose
/// owner is not in the set.
#[test]
fn remove_all_is_atomic() {
    let mut d = Drawing::new();
    let a = add_xrecord(&mut d, "A", &[]);
    let b = add_xrecord(&mut d, "B", &[a]);
    let before = d.clone();
    let objects = |d: &Drawing| d.objects().map(|(h, _)| h).collect::<Vec<_>>();
    let l0 = layer0(&d);
    assert!(matches!(
        d.remove_all(&[b, Handle(0xFF_FFFF)]),
        Err(Error::NotFound(Handle(0xFF_FFFF)))
    ));
    assert!(matches!(d.remove_all(&[b, l0]), Err(Error::Protected(_))));
    match d.remove_all(&[a]) {
        Err(Error::InUse { objects, .. }) => assert_eq!(objects, [b]),
        other => panic!("{other:?}"),
    }
    assert_eq!(objects(&d), objects(&before));
    assert_eq!(d.header.handseed, before.header.handseed);
    // A SEQEND goes with its polyline, not alone.
    let ms = d.model_space();
    let vertex = |x: f64| {
        (
            EntityCommon::new(l0),
            Entity::Vertex3d(Vertex3d {
                flags: 32,
                point: [x, 0.0, 0.0],
            }),
        )
    };
    let poly = d
        .add_polyline(
            ms,
            (
                EntityCommon::new(l0),
                Entity::Polyline3d(Polyline3d {
                    spline_flags: 0,
                    closed_flags: 0,
                    vertices: EntityList::Owned(vec![]),
                    seqend: Handle(0),
                }),
            ),
            vec![vertex(0.0), vertex(1.0)],
        )
        .unwrap();
    let Some(Object::Entity {
        entity: Entity::Polyline3d(p),
        ..
    }) = d.get(poly)
    else {
        panic!()
    };
    let seqend = p.seqend;
    let with_poly = d.clone();
    assert!(matches!(
        d.remove_all(&[a, b, seqend]),
        Err(Error::Invalid(_))
    ));
    assert_eq!(objects(&d), objects(&with_poly));
    // With its owner it is fine, as is a child given before its owner.
    let removed = d.remove_all(&[seqend, poly, a, b]).unwrap();
    assert_eq!(removed.len(), 4);
    assert!(d.get(seqend).is_none() && d.get(poly).is_none());
}

/// A block given with its BLOCK entity and one of its entities: they go
/// with the block record.
#[test]
fn remove_all_children_given_with_their_owner() {
    let mut d = Drawing::new();
    let block = d.add_block("B").unwrap();
    let l0 = layer0(&d);
    let e = d
        .add_entity(block, EntityCommon::new(l0), line([0.0; 3], [1.0; 3]))
        .unwrap();
    let Some(Object::Other {
        object: NonGraphical::BlockRecord(b),
        ..
    }) = d.get(block)
    else {
        panic!()
    };
    let begin = b.block_entity;
    let removed = d.remove_all(&[e, begin, block]).unwrap();
    assert_eq!(
        removed.iter().map(Object::handle).collect::<Vec<_>>(),
        [e, begin, block]
    );
    assert!(d.block_records().all(|(h, _)| h != block));
}

/// What an application might do with an object that still refers to one
/// being removed: an entity's material goes back to ByLayer and its color
/// book is dropped (its ACI color stays); extended data handles (R2000
/// attaches materials through `RTMaterial` extended data) and an XRECORD's
/// object ids naming a removed object become null.
fn release(d: &mut Drawing, referrer: Handle, removed: &HashSet<Handle>) {
    use opendwg::object::{MaterialRef, XRecordValue};
    let gone = |h: &Handle| removed.contains(h);
    let Some(obj) = d.get_mut(referrer) else {
        panic!("{referrer} is missing")
    };
    if let Object::Entity { common, .. } = obj {
        if matches!(common.material, MaterialRef::Handle(m) if gone(&m)) {
            common.material = MaterialRef::ByLayer;
        }
        if common.color_book.is_some_and(|b| gone(&b)) {
            common.color_book = None;
        }
    }
    if let Object::Other {
        object: NonGraphical::XRecord(x),
        ..
    } = obj
    {
        for (_, v) in &mut x.items {
            if matches!(v, XRecordValue::ObjectId(h) if gone(h)) {
                *v = XRecordValue::ObjectId(Handle(0));
            }
        }
    }
    for app in &mut obj.object_common_mut().xdata {
        for item in &mut app.items {
            if matches!(item, XDataItem::Handle(h) if gone(h)) {
                *item = XDataItem::Handle(Handle(0));
            }
        }
    }
}

/// The README's recovery for R2000 and R2004 drawings: remove the
/// unconvertible objects together with `remove_all`, after releasing what
/// still refers to them (entities' materials and color books, XRECORD
/// object ids); the drawing then saves, reads back and validates clean.
#[test]
fn unconvertible_objects_removed_together() {
    let mut done = 0;
    for name in ["acadsharp-sample-ac1015", "acadsharp-sample-ac1018"] {
        let Some(path) = common::corpus_file_or_skip(name) else {
            continue;
        };
        let mut d = Drawing::open(path).unwrap();
        let unconvertible: Vec<Handle> = d
            .unconvertible_objects()
            .into_iter()
            .map(|(h, _)| h)
            .collect();
        assert!(!unconvertible.is_empty(), "{name}");
        assert!(matches!(d.to_bytes(), Err(Error::UnconvertibleObjects(_))));
        let set: HashSet<Handle> = unconvertible.iter().copied().collect();
        let before = d.clone();
        match d.remove_all(&unconvertible) {
            Err(Error::InUse {
                objects,
                header_vars,
            }) => {
                assert!(header_vars.is_empty(), "{name}: {header_vars:?}");
                assert_eq!(
                    d.objects().count(),
                    before.objects().count(),
                    "{name}: atomic"
                );
                for r in objects {
                    release(&mut d, r, &set);
                }
            }
            other => panic!("{name}: {:?}", other.map(|v| v.len())),
        }
        let removed = d.remove_all(&unconvertible).unwrap();
        assert_eq!(removed.len(), unconvertible.len(), "{name}");
        assert!(d.unconvertible_objects().is_empty(), "{name}");
        let back = Drawing::from_bytes(&d.to_bytes().unwrap()).unwrap();
        let errors: Vec<_> = back
            .validate()
            .into_iter()
            .chain(back.diagnostics().iter().cloned())
            .filter(|x| x.severity == opendwg::Severity::Error)
            .collect();
        assert!(errors.is_empty(), "{name}: {errors:?}");
        for h in &unconvertible {
            assert!(back.get(*h).is_none(), "{name}: {h}");
        }
        done += 1;
    }
    if done > 0 {
        assert_eq!(done, 2);
    }
}

/// The types an application needs are at the crate root.
#[test]
fn root_reexports() {
    let _: opendwg::Color = opendwg::Color::ByLayer;
    let _: opendwg::Result<opendwg::Handle> = Ok(opendwg::Handle(1));
    let d: Option<opendwg::Diagnostic> = None;
    assert!(d.is_none());
    let _ = (opendwg::Severity::Error, opendwg::Version::R2018);
    let _: fn() -> opendwg::Drawing = opendwg::Drawing::new;
    let e: opendwg::Error = opendwg::Error::NotFound(opendwg::Handle(1));
    assert_eq!(e.to_string(), "object not found: 1");
}

/// `to_bytes_with_diagnostics` and `save_with_diagnostics` give the file
/// and the warnings `write_diagnostics` lists (the AEC sample's AuxHeader
/// has a layout the writer does not know, and is replaced).
#[test]
fn write_with_diagnostics() {
    let Some(path) = common::corpus_file_or_skip("acadsharp-aec-objects") else {
        return;
    };
    let d = Drawing::open(path).unwrap();
    let (bytes, diags) = d.to_bytes_with_diagnostics().unwrap();
    assert!(!diags.is_empty());
    assert_eq!(diags, d.write_diagnostics());
    assert!(Drawing::from_bytes(&bytes).is_ok());
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("with-diags.dwg");
    assert_eq!(d.save_with_diagnostics(&out).unwrap(), diags);
    assert!(Drawing::open(&out).is_ok());
    let fresh = Drawing::new().to_bytes_with_diagnostics().unwrap();
    assert!(fresh.1.is_empty(), "{:?}", fresh.1);
}

/// `has_attdefs` is refreshed at save when a block's ATTDEFs come or go
/// other than through the API (an ATTDEF moved in by `get_mut`, a flag
/// cleared by hand): it then says whether the block has one. While a
/// block's ATTDEFs are as read, the flag is kept as read (AutoCAD leaves it
/// set on model space after its ATTDEFs are gone, in every sample).
#[test]
fn has_attdefs_is_refreshed_at_save() {
    let Some(mut d) = r2018() else { return };
    let flag = |d: &Drawing, b: Handle| match d.get(b) {
        Some(Object::Other {
            object: NonGraphical::BlockRecord(r),
            ..
        }) => r.has_attdefs,
        _ => panic!(),
    };
    let ms = d.model_space();
    assert!(
        flag(&d, ms)
            && !d.entities_in(ms).any(|e| matches!(
                d.get(e),
                Some(Object::Entity {
                    entity: Entity::AttDef(_),
                    ..
                })
            ))
    );
    // A block with ATTDEFs whose flag is cleared by hand.
    let (with, _) = d
        .block_records()
        .find(|(_, b)| b.has_attdefs && b.layout.0 == 0)
        .unwrap();
    if let Some(Object::Other {
        object: NonGraphical::BlockRecord(b),
        ..
    }) = d.get_mut(with)
    {
        b.has_attdefs = false;
    }
    // A block with no ATTDEF flagged by hand.
    let plain = d.add_block("Plain").unwrap();
    if let Some(Object::Other {
        object: NonGraphical::BlockRecord(b),
        ..
    }) = d.get_mut(plain)
    {
        b.has_attdefs = true;
    }
    let back = Drawing::from_bytes(&d.to_bytes().unwrap()).unwrap();
    assert!(flag(&back, ms), "kept as read");
    assert!(flag(&back, with));
    assert!(!flag(&back, plain));
}

/// The constructors give usable defaults: a layer added with
/// `Layer::new` gets colour 7 and, left null, the Continuous linetype and
/// layer 0's plot style and material; text and MTEXT added with a null
/// style get $TEXTSTYLE. Everything saves, reads back and validates.
#[test]
fn constructors_add_and_save() {
    use opendwg::object::{Circle, Insert, Layer, MText};
    let mut d = Drawing::new();
    let walls = d.add_layer(Layer::new("Walls")).unwrap();
    let l0 = d.layer(layer0(&d)).unwrap().clone();
    let layer = d.layer(walls).unwrap();
    assert_eq!(layer.entry.name, "Walls");
    assert_eq!(layer.color, opendwg::Color::Index(7));
    assert_eq!(layer.linetype, d.header.ltype_continuous);
    assert_eq!(
        (layer.plotstyle, layer.material),
        (l0.plotstyle, l0.material)
    );
    assert!(layer.plot && !layer.off && !layer.frozen && !layer.locked);
    let ms = d.model_space();
    let on = |l| EntityCommon::new(l);
    let line = d
        .add_entity(
            ms,
            on(walls),
            Entity::Line(Line::new([0.0; 3], [10.0, 5.0, 0.0])),
        )
        .unwrap();
    d.add_entity(
        ms,
        on(walls),
        Entity::Circle(Circle::new([1.0, 1.0, 0.0], 2.5)),
    )
    .unwrap();
    let text = d
        .add_entity(
            ms,
            on(walls),
            Entity::Text(Text::new([0.0, 3.0, 0.0], 0.25, "Hello")),
        )
        .unwrap();
    let mtext = d
        .add_entity(
            ms,
            on(walls),
            MText::new([0.0, 6.0, 0.0], 0.25, 10.0, "Multi\\Pline").into(),
        )
        .unwrap();
    let block = d.add_block("Door").unwrap();
    d.add_entity(
        block,
        on(layer0(&d)),
        Entity::Line(Line::new([0.0; 3], [1.0, 0.0, 0.0])),
    )
    .unwrap();
    let insert = d
        .add_entity(ms, on(walls), Insert::new(block, [5.0, 5.0, 0.0]).into())
        .unwrap();
    let style = |d: &Drawing, h| match d.entity(h) {
        Some((_, Entity::Text(t))) => t.style,
        Some((_, Entity::MText(m))) => m.style,
        other => panic!("{other:?}"),
    };
    assert_eq!(style(&d, text), d.header.textstyle);
    assert_eq!(style(&d, mtext), d.header.textstyle);
    let errors: Vec<_> = d
        .validate()
        .into_iter()
        .filter(|x| x.severity == opendwg::Severity::Error)
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    let back = Drawing::from_bytes(&d.to_bytes().unwrap()).unwrap();
    assert!(back.diagnostics().is_empty(), "{:?}", back.diagnostics());
    for h in [line, text, mtext, insert] {
        assert_eq!(
            back.get(h).map(|o| o.type_name()),
            d.get(h).map(|o| o.type_name())
        );
    }
    assert_eq!(back.layer(walls).unwrap().entry.name, "Walls");
}

/// Typed getters: `layer`, `block_record` and `entity` give the typed value
/// of the right kind only; their `_mut` forms change it and keep the
/// reference index up to date, like `get_mut`.
#[test]
fn typed_getters() {
    use opendwg::object::Layer;
    let mut d = Drawing::new();
    let l0 = layer0(&d);
    let ms = d.model_space();
    assert!(d.layer(ms).is_none() && d.block_record(l0).is_none());
    assert_eq!(d.block_record(ms).unwrap().entry.name, "*Model_Space");
    let e = d
        .add_entity(ms, EntityCommon::new(l0), line([0.0; 3], [1.0; 3]))
        .unwrap();
    assert!(d.entity(l0).is_none());
    let (common, entity) = d.entity(e).unwrap();
    assert_eq!(common.layer, l0);
    assert!(matches!(entity, Entity::Line(_)));
    // Moving the line to a new layer through entity_mut: the old layer is
    // free, the new one in use.
    let red = d.add_layer(Layer::new("Red")).unwrap();
    let (common, _) = d.entity_mut(e).unwrap();
    common.layer = red;
    assert!(d.referrers(red).contains(&e));
    assert!(!d.referrers(l0).contains(&e));
    assert!(d.layer_mut(red).is_some());
    d.layer_mut(red).unwrap().locked = true;
    assert!(d.layer(red).unwrap().locked);
    d.block_record_mut(ms).unwrap().description = "model".into();
    assert_eq!(d.block_record(ms).unwrap().description, "model");
    // A layer's linetype changed through layer_mut is a reference too.
    let dashed = d
        .linetypes()
        .find(|(_, l)| l.entry.name == "ByBlock")
        .map(|(h, _)| h)
        .unwrap();
    d.layer_mut(red).unwrap().linetype = dashed;
    assert!(d.referrers(dashed).contains(&red));
}

/// An entity whose linetype is not a linetype, or a multiline whose line
/// count is not its style's element count, is refused when added, as
/// `validate()` would refuse it at save; with a real linetype and the
/// style's count it is added and the drawing validates.
#[test]
fn add_entity_checks_reference_kinds_as_validate_does() {
    use opendwg::Severity;
    use opendwg::object::{LinetypeRef, MLine};
    let mut d = Drawing::new();
    let ms = d.model_space();
    let l0 = layer0(&d);
    let mut common = EntityCommon::new(l0);
    common.linetype = LinetypeRef::Handle(l0);
    let refused = d.add_entity(ms, common.clone(), line([0.0; 3], [1.0, 0.0, 0.0]));
    assert!(
        matches!(&refused, Err(Error::Invalid(e)) if e[0].message.contains("not a linetype")),
        "{refused:?}"
    );
    let (continuous, _) = d
        .linetypes()
        .find(|(_, l)| l.entry.name == "Continuous")
        .unwrap();
    common.linetype = LinetypeRef::Handle(continuous);
    d.add_entity(ms, common, line([0.0; 3], [1.0, 0.0, 0.0]))
        .unwrap();

    let (style, elements) = d
        .objects()
        .find_map(|(h, o)| match o {
            Object::Other {
                object: NonGraphical::MLineStyle(s),
                ..
            } => Some((h, s.elements.len())),
            _ => None,
        })
        .unwrap();
    let mline = |lines: usize| {
        Entity::MLine(Box::new(MLine {
            scale: 1.0,
            justification: 0,
            base_point: [0.0; 3],
            extrusion: [0.0, 0.0, 1.0],
            flags: 0,
            lines_in_style: lines as u8,
            vertices: Vec::new(),
            style,
        }))
    };
    let refused = d.add_entity(ms, EntityCommon::new(l0), mline(elements + 1));
    assert!(
        matches!(&refused, Err(Error::Invalid(e)) if e[0].message.contains("elements")),
        "{refused:?}"
    );
    d.add_entity(ms, EntityCommon::new(l0), mline(elements))
        .unwrap();
    let errors: Vec<_> = d
        .validate()
        .into_iter()
        .filter(|x| x.severity == Severity::Error)
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
}
