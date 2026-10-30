//! Drawings created from scratch with [`Drawing::new`]. A new
//! drawing validates with no finding, saves, reads back with no diagnostic
//! and the same model, and takes one entity of every kind the library
//! models through the public API.

mod common;

use common::every_entity::{dictionary_entries, entry, image, line, with_every_entity};

use std::collections::{BTreeMap, BTreeSet};

use opendwg::object::{
    Dictionary, DictionaryWithDefault, Entity, EntityCommon, EntityList, ImageDef, NonGraphical,
    Object, RawObject, TableKind, WipeoutVariables,
};
use opendwg::section::{AppInfo, JulianDate};
use opendwg::{Drawing, Error, Handle, Version};

/// The creation time used throughout, so the bytes are reproducible.
const CREATED: JulianDate = JulianDate {
    day: 2_461_300,
    ms: 43_200_000,
};

/// The save time used throughout.
const SAVED: JulianDate = JulianDate {
    day: 2_461_301,
    ms: 1_000,
};

/// The smallest R2018 corpus file, whose template objects `Drawing::new`
/// reproduces.
const TEMPLATE_SOURCE: &str = "acadsharp-dynblock-lookup";

/// The last handle of the objects AutoCAD's default template (`acad.dwt`)
/// creates in [`TEMPLATE_SOURCE`]: from 0x250 on the handles are the file's
/// own content (its dynamic blocks).
const LAST_TEMPLATE_HANDLE: u64 = 0x1B8;

fn new() -> Drawing {
    Drawing::new_at(CREATED)
}

/// Write `d` and read it back; the read must have no diagnostics.
fn roundtrip(d: &Drawing) -> Drawing {
    let bytes = d.to_bytes_at(SAVED).unwrap();
    let back = Drawing::from_bytes(&bytes).unwrap();
    assert!(
        back.diagnostics().is_empty(),
        "{:?}",
        &back.diagnostics()[..back.diagnostics().len().min(10)]
    );
    back
}

/// Where the debug forms of `a` and `b` first differ.
fn debug_diff(a: &Object, b: Option<&Object>) -> String {
    let (a, b): (Vec<char>, Vec<char>) = (
        format!("{a:?}").chars().collect(),
        b.map_or("missing".into(), |b| format!("{b:?}"))
            .chars()
            .collect(),
    );
    let at = a
        .iter()
        .zip(&b)
        .position(|(x, y)| x != y)
        .unwrap_or(a.len().min(b.len()));
    let window = |s: &[char]| -> String {
        s[at.saturating_sub(100).min(s.len())..(at + 100).min(s.len())]
            .iter()
            .collect()
    };
    format!("\n  {}\n  {}", window(&a), window(&b))
}

/// A name for each object reachable from the table controls and the named
/// object dictionary, by how it is held: `TABLE <kind>/<record name>`,
/// `NOD/<key>/<key>...`, `<path>/xdict`, `<path>/BLOCK`, `<path>/ENDBLK`;
/// a layout's block record is `<layout path>/block`.
fn paths(d: &Drawing) -> BTreeMap<Handle, String> {
    let mut out: BTreeMap<Handle, String> = BTreeMap::new();
    let mut queue: Vec<Handle> = Vec::new();
    let put =
        |out: &mut BTreeMap<Handle, String>, queue: &mut Vec<Handle>, h: Handle, p: String| {
            if h.0 != 0 && d.get(h).is_some() && !out.contains_key(&h) {
                out.insert(h, p);
                queue.push(h);
            }
        };
    put(&mut out, &mut queue, d.named_objects(), "NOD".into());
    for (h, o) in d.objects() {
        if let Object::Other {
            object: NonGraphical::TableControl(c),
            ..
        } = o
        {
            put(&mut out, &mut queue, h, format!("TABLE {:?}", c.kind));
        }
    }
    // Layout blocks first, by their layout.
    let mut i = 0;
    while i < queue.len() {
        let h = queue[i];
        i += 1;
        let p = out[&h].clone();
        let Some(o) = d.get(h) else { continue };
        if let Some(x) = o.object_common().xdict {
            put(&mut out, &mut queue, x, format!("{p}/xdict"));
        }
        let Object::Other { object, .. } = o else {
            continue;
        };
        match object {
            NonGraphical::Dictionary(x) => {
                for (k, e) in &x.entries {
                    put(&mut out, &mut queue, *e, format!("{p}/{k}"));
                }
            }
            NonGraphical::DictionaryWithDefault(x) => {
                for (k, e) in &x.dictionary.entries {
                    put(&mut out, &mut queue, *e, format!("{p}/{k}"));
                }
            }
            NonGraphical::Layout(l) => {
                put(&mut out, &mut queue, l.block_record, format!("{p}/block"));
            }
            NonGraphical::TableControl(c) if c.kind != TableKind::Block => {
                for e in c.extra.iter().chain(&c.entries) {
                    if let Some(Object::Other { object: r, .. }) = d.get(*e) {
                        let name = r.table_entry().map(|t| t.name.clone()).unwrap_or_default();
                        put(&mut out, &mut queue, *e, format!("{p}/{name}"));
                    }
                }
            }
            NonGraphical::BlockRecord(b) => {
                put(&mut out, &mut queue, b.block_entity, format!("{p}/BLOCK"));
                put(&mut out, &mut queue, b.end_block, format!("{p}/ENDBLK"));
            }
            _ => {}
        }
    }
    out
}

/// The corpus template objects `Drawing::new` leaves out, by path (and
/// everything under them): AutoCAD's records of the product and save, the
/// second layout, and the materials' packaged binary assets.
const LEFT_OUT: [&str; 6] = [
    "NOD/ACAD_CIP_PREVIOUS_PRODUCT_INFO",
    "NOD/ACAD_LAST_SAVED_VERSION_INFO",
    "NOD/ACAD_LAYOUT/Layout2",
    "NOD/ACAD_MATERIAL/ByBlock/xdict",
    "NOD/ACAD_MATERIAL/ByLayer/xdict",
    "NOD/ACAD_MATERIAL/Global/xdict/FBXASSET",
];

/// `o`'s debug form with every handle replaced by the path of the object
/// it names (`?` for one with no path, `0` for null), owner offsets and the
/// as-read version of extended records left out.
fn normalized(o: &Object, paths: &BTreeMap<Handle, String>) -> String {
    let s = format!("{o:?}")
        .replace("owner_relative: true", "owner_relative: false")
        .replace("AsRead(Some(R2018))", "AsRead(None)");
    let mut out = String::new();
    let mut rest = s.as_str();
    while let Some(at) = rest.find("Handle(") {
        let (head, tail) = rest.split_at(at + "Handle(".len());
        out.push_str(head);
        let digits = tail
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(tail.len());
        rest = match tail[..digits].parse::<u64>() {
            Ok(0) => {
                out.push('0');
                &tail[digits..]
            }
            Ok(n) => {
                out.push_str(paths.get(&Handle(n)).map_or("?", String::as_str));
                &tail[digits..]
            }
            Err(_) => tail,
        };
    }
    out.push_str(rest);
    out
}

/// The new drawing reproduces the corpus file's template objects: the same
/// objects (by type and path, the left-out ones aside), owned alike, with
/// equal values, references compared by the path of what they name.
///
/// Differences kept (the corpus file's own drawing state), patched into the
/// corpus objects before comparing: the `*Active` viewport shows the
/// limits (center, height and width); the `Model` layout plots at 1:1
/// (custom scale and scale factor) and has no data storage thumbnail. List
/// entries naming left-out objects or the file's own content are dropped
/// from the corpus objects, as are its erased (null) table entries.
#[test]
fn new_matches_the_corpus_template() {
    let Some(p) = common::corpus_file_or_skip(TEMPLATE_SOURCE) else {
        return;
    };
    let mut source = Drawing::open(p).unwrap();
    let source_paths: BTreeMap<Handle, String> = paths(&source)
        .into_iter()
        .filter(|(h, p)| {
            h.0 <= LAST_TEMPLATE_HANDLE
                && !LEFT_OUT
                    .iter()
                    .any(|x| p == x || p.starts_with(&format!("{x}/")))
        })
        .collect();
    // Drop list entries naming objects not compared, and patch the kept
    // differences.
    let keep = |h: &Handle| source_paths.contains_key(h);
    let handles: Vec<Handle> = source_paths.keys().copied().collect();
    for &h in &handles {
        let o = source.get_mut(h).unwrap();
        let common = o.object_common_mut();
        common.reactors.retain(keep);
        common.xdict = common.xdict.filter(keep);
        common.has_ds_binary_data = false;
        let Object::Other { object, .. } = o else {
            continue;
        };
        match object {
            NonGraphical::TableControl(c) => c.entries.retain(keep),
            NonGraphical::Dictionary(x) => x.entries.retain(|(_, e)| keep(e)),
            NonGraphical::BlockRecord(b) => {
                b.entities = EntityList::Owned(Vec::new());
                b.inserts.clear();
            }
            NonGraphical::Vport(v) => {
                v.view_center = [6.0, 4.5];
                v.view_height = 9.0;
                v.aspect_ratio_times_height = 12.0;
            }
            NonGraphical::Layout(l) => {
                l.plot.custom_scale = [1.0, 1.0];
                l.plot.scale_factor = 1.0;
            }
            _ => {}
        }
    }
    let d = new();
    let new_paths = paths(&d);
    // Every object of the new drawing has a path.
    assert_eq!(new_paths.len(), d.objects().count());
    let by_path = |m: &BTreeMap<Handle, String>| -> BTreeMap<String, Handle> {
        m.iter().map(|(h, p)| (p.clone(), *h)).collect()
    };
    let (want, got) = (by_path(&source_paths), by_path(&new_paths));
    assert_eq!(
        want.keys().collect::<Vec<_>>(),
        got.keys().collect::<Vec<_>>(),
        "the same objects"
    );
    let mut differ = Vec::new();
    for (path, &h) in &want {
        let a = normalized(source.get(h).unwrap(), &source_paths);
        let b = normalized(d.get(got[path]).unwrap(), &new_paths);
        if a != b {
            let at = a
                .chars()
                .zip(b.chars())
                .position(|(x, y)| x != y)
                .unwrap_or(0);
            let w = |s: &str| {
                s.chars()
                    .skip(at.saturating_sub(80))
                    .take(200)
                    .collect::<String>()
            };
            differ.push(format!("{path}:\n  corpus {}\n  new    {}", w(&a), w(&b)));
        }
    }
    assert!(differ.is_empty(), "{}", differ.join("\n"));

    // The classes: the corpus file's prefix that the other R2018 file shares.
    assert_eq!(d.classes.len(), 17);
    for (a, b) in d.classes.iter().zip(&source.classes) {
        let mut b = b.clone();
        b.instance_count = a.instance_count;
        assert_eq!(*a, b);
    }
    if let Some(p) = common::corpus_file_or_skip("acadsharp-sample-ac1032") {
        let other = Drawing::open(p).unwrap();
        for (a, b) in d.classes.iter().zip(&other.classes) {
            assert_eq!((&a.dxf_name, a.dwg_version), (&b.dxf_name, b.dwg_version));
        }
    }
}

/// A new drawing has no validation finding (no warning either), and its
/// header names its objects: the same objects as in the corpus file whose
/// template it reproduces.
#[test]
fn new_validates_clean() {
    let d = new();
    assert_eq!(d.validate(), vec![]);
    assert!(d.diagnostics().is_empty());
    assert!(
        d.write_diagnostics().is_empty(),
        "{:?}",
        d.write_diagnostics()
    );
    assert_eq!(d.source_version(), None);
    assert!(d.data_storage.is_none());
    assert_eq!(d.app_info, Some(AppInfo::opendwg()));
    let h = &d.header;
    assert_eq!(h.tdcreate, CREATED);
    assert_eq!(h.tdupdate, CREATED);
    assert!(d.objects().all(|(x, _)| x < h.handseed));
    assert_eq!(h.handseed.0, d.objects().last().unwrap().0.0 + 1);
    let summary = d.summary.as_ref().unwrap();
    assert_eq!((summary.create, summary.modified), (CREATED, CREATED));
    assert_eq!(summary.title, "");
    assert_eq!(summary.custom, vec![]);

    let Some(p) = common::corpus_file_or_skip(TEMPLATE_SOURCE) else {
        return;
    };
    let source = Drawing::open(p).unwrap();
    let s = &source.header;
    let pairs = |h: &opendwg::section::HeaderVars| {
        vec![
            h.block_control,
            h.layer_control,
            h.style_control,
            h.linetype_control,
            h.view_control,
            h.ucs_control,
            h.vport_control,
            h.appid_control,
            h.dimstyle_control,
            h.dict_named_objects,
            h.dict_acad_group,
            h.dict_acad_mlinestyle,
            h.dict_layouts,
            h.dict_plotsettings,
            h.dict_plotstyles,
            h.dict_materials.unwrap(),
            h.dict_colors.unwrap(),
            h.dict_visualstyle.unwrap(),
            h.clayer,
            h.textstyle,
            h.celtype,
            h.cmaterial.unwrap(),
            h.dimstyle,
            h.cmlstyle,
            h.dimtxsty,
            h.block_record_model_space,
            h.block_record_paper_space,
            h.ltype_bylayer,
            h.ltype_byblock,
            h.ltype_continuous,
            h.interfereobjvs.unwrap(),
            h.interferevpvs.unwrap(),
            h.dragvs.unwrap(),
            h.cpsnid,
            h.ucsname,
            h.pucsname,
            h.dimblk,
            h.dimldrblk,
        ]
    };
    // The same objects, by what they are (their handles differ).
    let named = |d: &Drawing, hs: Vec<Handle>| -> Vec<String> {
        let p = paths(d);
        hs.into_iter()
            .map(|h| if h.0 == 0 { "0".into() } else { p[&h].clone() })
            .collect()
    };
    assert_eq!(named(&d, pairs(h)), named(&source, pairs(s)));
    // GUIDs in AutoCAD's form, derived from the creation time.
    for g in [&h.fingerprintguid, &h.versionguid] {
        assert_eq!(g.len(), 38, "{g}");
        assert!(g.starts_with('{') && g.ends_with('}'), "{g}");
        assert_eq!(*g, g.to_uppercase());
    }
    assert_ne!(h.fingerprintguid, h.versionguid);
    let later = Drawing::new_at(SAVED);
    assert_ne!(later.header.fingerprintguid, h.fingerprintguid);
}

/// The drawing's structure: the tables hold their default records, the
/// layouts and their blocks name each other, the named object dictionary
/// has its standard entries.
#[test]
fn new_has_the_default_objects() {
    let d = new();
    let names = |kind: TableKind| -> Vec<String> {
        let control = match kind {
            TableKind::Layer => d.header.layer_control,
            TableKind::Style => d.header.style_control,
            TableKind::AppId => d.header.appid_control,
            TableKind::DimStyle => d.header.dimstyle_control,
            TableKind::Vport => d.header.vport_control,
            _ => unreachable!(),
        };
        let Some(Object::Other {
            object: NonGraphical::TableControl(c),
            ..
        }) = d.get(control)
        else {
            panic!("{kind:?}")
        };
        c.entries
            .iter()
            .map(|&h| match d.get(h) {
                Some(Object::Other { object, .. }) => object.table_entry().unwrap().name.clone(),
                o => panic!("{h}: {o:?}"),
            })
            .collect()
    };
    assert_eq!(names(TableKind::Layer), ["0"]);
    assert_eq!(names(TableKind::Style), ["Standard", "Annotative"]);
    assert_eq!(names(TableKind::DimStyle), ["Standard", "Annotative"]);
    assert_eq!(names(TableKind::Vport), ["*Active"]);
    assert!(names(TableKind::AppId).contains(&"ACAD".to_string()));
    let linetypes: Vec<&str> = d.linetypes().map(|(_, l)| l.entry.name.as_str()).collect();
    assert_eq!(linetypes, ["ByBlock", "ByLayer", "Continuous"]);

    let layouts: Vec<(Handle, String, Handle)> = d
        .layouts()
        .into_iter()
        .map(|(h, l)| (h, l.name.clone(), l.block_record))
        .collect();
    assert_eq!(layouts.len(), 2);
    assert_eq!(layouts[0].1, "Model");
    assert_eq!(layouts[1].1, "Layout1");
    assert_eq!(layouts[0].2, d.model_space());
    assert_eq!(layouts[1].2, d.header.block_record_paper_space);
    assert_eq!(d.paper_spaces(), vec![d.header.block_record_paper_space]);
    for (layout, _, block) in &layouts {
        let b = d.block_records().find(|(h, _)| h == block).unwrap().1;
        assert_eq!(b.layout, *layout, "{block} names its layout back");
        assert_eq!(b.entities, EntityList::Owned(vec![]));
        assert!(matches!(
            d.get(b.block_entity),
            Some(Object::Entity {
                entity: Entity::Block(_),
                ..
            })
        ));
        assert!(matches!(
            d.get(b.end_block),
            Some(Object::Entity {
                entity: Entity::EndBlk,
                ..
            })
        ));
    }

    let nod: BTreeMap<String, Handle> = dictionary_entries(&d, d.named_objects())
        .into_iter()
        .collect();
    let expected = [
        "ACAD_COLOR",
        "ACAD_DETAILVIEWSTYLE",
        "ACAD_GROUP",
        "ACAD_LAYOUT",
        "ACAD_MATERIAL",
        "ACAD_MLEADERSTYLE",
        "ACAD_MLINESTYLE",
        "ACAD_PLOTSETTINGS",
        "ACAD_PLOTSTYLENAME",
        "ACAD_SCALELIST",
        "ACAD_SECTIONVIEWSTYLE",
        "ACAD_TABLESTYLE",
        "ACAD_VISUALSTYLE",
        "AcDbVariableDictionary",
    ];
    assert_eq!(nod.keys().map(String::as_str).collect::<Vec<_>>(), expected);
    let kind = |h: Handle| d.get(h).unwrap().type_name().into_owned();
    assert_eq!(
        kind(entry(&d, nod["ACAD_MLINESTYLE"], "Standard")),
        "MLINESTYLE"
    );
    assert_eq!(
        kind(entry(&d, nod["ACAD_PLOTSTYLENAME"], "Normal")),
        "ACDBPLACEHOLDER"
    );
    assert_eq!(
        kind(entry(&d, nod["ACAD_MLEADERSTYLE"], "Standard")),
        "MLEADERSTYLE"
    );
    assert_eq!(
        kind(entry(&d, nod["ACAD_TABLESTYLE"], "Standard")),
        "TABLESTYLE"
    );
    assert_eq!(kind(entry(&d, nod["ACAD_SCALELIST"], "A0")), "SCALE");
    assert_eq!(dictionary_entries(&d, nod["ACAD_SCALELIST"]).len(), 33);
    assert_eq!(dictionary_entries(&d, nod["ACAD_MATERIAL"]).len(), 3);
    assert_eq!(dictionary_entries(&d, nod["ACAD_VISUALSTYLE"]).len(), 24);
    assert_eq!(dictionary_entries(&d, nod["ACAD_GROUP"]), vec![]);
    // The model layout plots at 1:1.
    let model = &d.layouts()[0].1.plot;
    assert_eq!((model.scale_factor, model.custom_scale), (1.0, [1.0, 1.0]));
    // No object claims data in a data storage section, which a new drawing
    // does not have.
    assert!(
        d.objects()
            .all(|(_, o)| !o.object_common().has_ds_binary_data)
    );
}

/// Saving a new drawing and reading it back gives the same drawing, but
/// for the save time and the application information.
#[test]
fn new_roundtrips() {
    let d = new();
    let back = roundtrip(&d);
    let mut header = d.header.clone();
    header.tdupdate = SAVED;
    assert_eq!(back.header, header);
    assert_eq!(back.classes, d.classes);
    assert_eq!(
        back.objects().map(|(h, _)| h).collect::<Vec<_>>(),
        d.objects().map(|(h, _)| h).collect::<Vec<_>>()
    );
    for (h, o) in d.objects() {
        assert_eq!(back.get(h), Some(o), "{h}");
    }
    let mut summary = d.summary.clone().unwrap();
    summary.modified = SAVED;
    assert_eq!(back.summary, Some(summary));
    assert_eq!(back.preview, d.preview);
    assert_eq!(back.app_info, d.app_info);
    assert!(back.data_storage.is_none());
    assert_eq!(back.validate(), vec![]);

    // $MEASUREMENT (stored in the Template section) reads back.
    assert_eq!(back.header.measurement, Some(0));
    let mut metric = new();
    metric.header.measurement = Some(1);
    assert_eq!(roundtrip(&metric).header.measurement, Some(1));
}

/// The same creation time gives the same drawing and the same bytes.
#[test]
fn new_is_deterministic() {
    let a = new().to_bytes_at(SAVED).unwrap();
    assert_eq!(a, new().to_bytes_at(SAVED).unwrap());
    assert_ne!(a, Drawing::new_at(SAVED).to_bytes_at(SAVED).unwrap());
    // `new` and `default` use the clock: the creation time lies between
    // the times taken around the calls.
    let before = now();
    let d = Drawing::default();
    let e = Drawing::new();
    let after = now();
    for x in [&d, &e] {
        let t = (x.header.tdcreate.day, x.header.tdcreate.ms);
        assert!(before <= t && t <= after, "{before:?} {t:?} {after:?}");
        assert_eq!(x.header.tdupdate, x.header.tdcreate);
    }
    assert_eq!(d.objects().count(), new().objects().count());
    // Two drawings created in the same millisecond still get their own
    // GUIDs.
    assert_ne!(d.header.fingerprintguid, e.header.fingerprintguid);
    assert_ne!(d.header.versionguid, e.header.versionguid);
}

/// The time now as (Julian day, milliseconds), as `Drawing::new` takes it
/// (UTC).
fn now() -> (i32, i32) {
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    let secs = since.as_secs();
    let day = 2_440_588 + (secs / 86_400) as i32;
    let ms = ((secs % 86_400) * 1000 + u64::from(since.subsec_millis())) as i32;
    (day, ms)
}

/// The Visio-style export path: a new drawing takes one entity of every
/// kind the library models (ACIS entities aside: without data they need a
/// data storage record) through the public API, with the objects they use
/// (a block with attribute definitions, a shape file, an image definition;
/// see `common::every_entity`), saves, reads back with no diagnostic,
/// validates clean and gives every entity back.
#[test]
fn new_with_every_entity_kind() {
    let (d, every) = with_every_entity(new());
    let _ms = d.model_space();
    let ps = d.header.block_record_paper_space;
    let (img, def) = (every.image, every.image_def);
    // The image owns its reactor, which the definition lists.
    let Some(Object::Entity {
        entity: Entity::Image(i),
        ..
    }) = d.get(img)
    else {
        panic!()
    };
    let reactor = i.reactor;
    assert_eq!(d.owner_of(reactor), Some(img));
    assert!(matches!(
        d.get(reactor),
        Some(Object::Other {
            object: NonGraphical::ImageDefReactor(_),
            ..
        })
    ));
    assert!(
        d.get(def)
            .unwrap()
            .object_common()
            .reactors
            .contains(&reactor)
    );

    let kinds: BTreeSet<String> = d
        .objects()
        .filter(|(_, o)| matches!(o, Object::Entity { .. }))
        .map(|(_, o)| o.type_name().into_owned())
        .collect();
    // Every typed entity kind but the ACIS ones and the owned ones (ATTRIB,
    // VERTEX, SEQEND, BLOCK, ENDBLK).
    for k in [
        "LINE",
        "POINT",
        "CIRCLE",
        "ARC",
        "ELLIPSE",
        "RAY",
        "XLINE",
        "SOLID",
        "TRACE",
        "3DFACE",
        "SHAPE",
        "TEXT",
        "MTEXT",
        "ATTDEF",
        "INSERT",
        "MINSERT",
        "LWPOLYLINE",
        "POLYLINE_2D",
        "POLYLINE_3D",
        "POLYLINE_PFACE",
        "POLYLINE_MESH",
        "SPLINE",
        "MLINE",
        "DIMENSION_LINEAR",
        "DIMENSION_ALIGNED",
        "DIMENSION_ANG3PT",
        "DIMENSION_ANG2LN",
        "DIMENSION_RADIUS",
        "DIMENSION_DIAMETER",
        "DIMENSION_ORDINATE",
        "ARC_DIMENSION",
        "LARGE_RADIAL_DIMENSION",
        "LEADER",
        "TOLERANCE",
        "MULTILEADER",
        "HATCH",
        "IMAGE",
        "WIPEOUT",
        "OLE2FRAME",
        "VIEWPORT",
        "ACAD_PROXY_ENTITY",
    ] {
        assert!(kinds.contains(k), "{k} not added: {kinds:?}");
    }

    assert_eq!(d.validate(), vec![]);
    let back = roundtrip(&d);
    assert_eq!(back.validate(), vec![]);
    // Every object comes back: the entities, their attributes, vertices and
    // SEQENDs, the block, the image definition, ...; the paper space layout
    // lists its new viewport (a list the writer refreshes).
    let (layout1, _) = d.layouts()[1];
    let mut expected = d.clone();
    if let Some(Object::Other {
        object: NonGraphical::Layout(l),
        ..
    }) = expected.get_mut(layout1)
    {
        assert_eq!(l.viewports, vec![]);
        l.viewports = d.entities_in(ps).collect();
    }
    let differ: Vec<String> = expected
        .objects()
        .filter(|&(h, o)| back.get(h) != Some(o))
        .map(|(h, o)| format!("{h} {}: {}", o.type_name(), debug_diff(o, back.get(h))))
        .collect();
    assert!(differ.is_empty(), "{}", differ.join("\n"));
    assert_eq!(back.objects().count(), d.objects().count());
    assert_eq!(back.classes.len(), d.classes.len());
    for (a, b) in back.classes.iter().zip(&d.classes) {
        assert_eq!((&a.dxf_name, a.class_number), (&b.dxf_name, b.class_number));
    }
}

/// `add_object` adds a dictionary entry owned by its dictionary, which it
/// names among its reactors; a class type's first object adds its class
/// once; refusals change nothing.
#[test]
fn add_object_entries_and_classes() {
    let mut d = new();
    let nod = d.named_objects();
    let classes = d.classes.len();
    let seed = d.header.handseed;
    let empty = || {
        NonGraphical::Dictionary(Dictionary {
            entries: Vec::new(),
            cloning: 1,
            hard_owner: 0,
        })
    };

    // Refusals: not a dictionary, a missing one, a taken or empty name, a
    // table record, an unknown class type.
    let layer0 = d.layer_by_name("0").unwrap();
    assert!(matches!(
        d.add_object(layer0, "X", empty()),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        d.add_object(Handle(0xFFFF), "X", empty()),
        Err(Error::NotFound(_))
    ));
    assert!(matches!(
        d.add_object(nod, "acad_group", empty()),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        d.add_object(nod, "", empty()),
        Err(Error::Invalid(_))
    ));
    let style = d.text_styles().next().unwrap().1.clone();
    assert!(matches!(
        d.add_object(nod, "X", NonGraphical::TextStyle(style)),
        Err(Error::Invalid(_))
    ));
    let raw = RawObject {
        type_code: 600,
        class_name: Some("SOME_APP_OBJECT".into()),
        source_version: Version::R2018,
        data: Vec::new(),
        data_bits: 0,
        strings: Some((Vec::new(), 0)),
        handles: Vec::new(),
        tail_handles_raw: Vec::new(),
        tail_handle_bits: 0,
    };
    assert!(matches!(
        d.add_object(nod, "X", NonGraphical::Unknown(raw)),
        Err(Error::Invalid(_))
    ));
    // A dictionary that comes with entries (here the named object
    // dictionary's, which would make a cycle) is refused: its entries
    // would not be owned by it.
    let group = entry(&d, nod, "ACAD_GROUP");
    for with in [
        NonGraphical::Dictionary(Dictionary {
            entries: vec![("loop".into(), nod)],
            cloning: 1,
            hard_owner: 0,
        }),
        NonGraphical::DictionaryWithDefault(DictionaryWithDefault {
            dictionary: Dictionary {
                entries: vec![("g".into(), group)],
                cloning: 1,
                hard_owner: 0,
            },
            default_entry: group,
        }),
    ] {
        assert!(matches!(
            d.add_object(nod, "X", with),
            Err(Error::Invalid(_))
        ));
    }
    assert_eq!(d.header.handseed, seed);
    assert_eq!(d.classes.len(), classes);
    assert_eq!(new().validate(), d.validate());

    // A wipeout settings object: its class is added after the last one.
    let vars = NonGraphical::WipeoutVariables(WipeoutVariables { display_frame: 1 });
    let h = d.add_object(nod, "ACAD_WIPEOUT_VARS", vars).unwrap();
    assert_eq!(h, seed);
    assert_eq!(d.owner_of(h), Some(nod));
    assert_eq!(d.get(h).unwrap().object_common().reactors, vec![nod]);
    assert_eq!(entry(&d, nod, "ACAD_WIPEOUT_VARS"), h);
    assert_eq!(d.classes.len(), classes + 1);
    let added = d.classes.last().unwrap();
    assert_eq!(
        (added.dxf_name.as_str(), added.class_number),
        ("WIPEOUTVARIABLES", 517)
    );
    // A second object of the type adds no class.
    let dict = d.add_object(nod, "MY_DICT", empty()).unwrap();
    let vars = NonGraphical::WipeoutVariables(WipeoutVariables { display_frame: 0 });
    d.add_object(dict, "more", vars).unwrap();
    assert_eq!(d.classes.len(), classes + 1);
    assert_eq!(d.validate(), vec![]);
    let back = roundtrip(&d);
    for (h, o) in d.objects() {
        assert_eq!(back.get(h), Some(o), "{h}");
    }
    // Saved counts follow the objects.
    assert_eq!(back.classes.last().unwrap().instance_count, 2);

    // Removing the dictionary takes its entry with it.
    d.remove(dict).unwrap();
    assert_eq!(d.validate(), vec![]);
}

/// An image added without its reactor gets one; removing the image takes
/// the reactor and its place in the definition's reactors. A definition that
/// is not one is refused.
#[test]
fn image_reactor_follows_the_image() {
    let mut d = new();
    let ms = d.model_space();
    let l0 = d.layer_by_name("0").unwrap();
    let nod = d.named_objects();
    let seed = d.header.handseed;
    let bad = Entity::Image(Box::new(image([0.0; 3], l0)));
    assert!(matches!(
        d.add_entity(ms, EntityCommon::new(l0), bad),
        Err(Error::Invalid(_))
    ));
    let missing = Entity::Image(Box::new(image([0.0; 3], Handle(0xFFFF))));
    assert!(matches!(
        d.add_entity(ms, EntityCommon::new(l0), missing),
        Err(Error::NotFound(_))
    ));
    assert_eq!(d.header.handseed, seed);

    let dict = Dictionary {
        entries: Vec::new(),
        cloning: 1,
        hard_owner: 0,
    };
    let images = d
        .add_object(nod, "ACAD_IMAGE_DICT", NonGraphical::Dictionary(dict))
        .unwrap();
    let def = ImageDef {
        class_version: 0,
        size: [10.0, 10.0],
        path: "a.png".into(),
        loaded: true,
        resolution_units: 0,
        pixel_size: [1.0, 1.0],
    };
    let def = d
        .add_object(images, "a", NonGraphical::ImageDef(def))
        .unwrap();
    let img = d
        .add_entity(
            ms,
            EntityCommon::new(l0),
            Entity::Image(Box::new(image([0.0; 3], def))),
        )
        .unwrap();
    let Some(Object::Entity {
        entity: Entity::Image(i),
        ..
    }) = d.get(img)
    else {
        panic!()
    };
    let reactor = i.reactor;
    assert_eq!(reactor.0, img.0 + 1);
    assert_eq!(
        d.get(def).unwrap().object_common().reactors,
        vec![images, reactor]
    );
    assert_eq!(d.validate(), vec![]);
    d.remove(img).unwrap();
    assert!(d.get(reactor).is_none());
    assert_eq!(d.get(def).unwrap().object_common().reactors, vec![images]);
    assert_eq!(d.validate(), vec![]);
}

/// The large-drawing test of tests/roundtrip.rs builds on `Drawing::new()`;
/// this checks the
/// same path at a small size: lines added to a new drawing save and read
/// back.
#[test]
fn new_takes_many_lines() {
    let mut d = new();
    let ms = d.model_space();
    let l0 = d.layer_by_name("0").unwrap();
    for i in 0..1000 {
        let x = f64::from(i);
        d.add_entity(
            ms,
            EntityCommon::new(l0),
            line([x, 0.0, 0.0], [x, 1.0, 0.0]),
        )
        .unwrap();
    }
    let back = roundtrip(&d);
    assert_eq!(back.entities_in(ms).count(), 1000);
    assert_eq!(back.validate(), vec![]);
}

/// The AuxHeader fields of ODS 27 in their R2018 layout (the maintenance
/// versions widened to `RL`): name and size in bytes.
const AUX_FIELDS: [(&str, usize); 39] = [
    ("magic", 3),
    ("dwg version", 2),
    ("maintenance", 4),
    ("saves", 4),
    ("-1", 4),
    ("saves part 1", 2),
    ("saves part 2", 2),
    ("0 a", 4),
    ("version string 1", 2),
    ("maintenance 1", 4),
    ("version string 2", 2),
    ("maintenance 2", 4),
    ("0x0005 a", 2),
    ("0x0893 a", 2),
    ("0x0005 b", 2),
    ("0x0893 b", 2),
    ("0 b", 2),
    ("1", 2),
    ("0 c", 4),
    ("0 d", 4),
    ("0 e", 4),
    ("0 f", 4),
    ("0 g", 4),
    ("tdcreate day", 4),
    ("tdcreate ms", 4),
    ("tdupdate day", 4),
    ("tdupdate ms", 4),
    ("handseed", 4),
    ("educational", 4),
    ("0 h", 2),
    ("part 1 - part 2", 2),
    ("0 i", 4),
    ("0 j", 4),
    ("0 k", 4),
    ("saves again", 4),
    ("0 l", 4),
    ("0 m", 4),
    ("0 n", 4),
    ("0 o", 4),
];

fn aux_fields(b: &[u8]) -> BTreeMap<&'static str, i64> {
    assert_eq!(b.len(), AUX_FIELDS.iter().map(|f| f.1).sum::<usize>());
    let mut at = 0;
    let mut out = BTreeMap::new();
    for (name, n) in AUX_FIELDS {
        let v = match n {
            2 => i64::from(i16::from_le_bytes([b[at], b[at + 1]])),
            4 => i64::from(i32::from_le_bytes(b[at..at + 4].try_into().unwrap())),
            _ => i64::from(b[at]) << 16 | i64::from(b[at + 1]) << 8 | i64::from(b[at + 2]),
        };
        out.insert(name, v);
        at += n;
    }
    out
}

fn aux_section(d: &Drawing) -> Vec<u8> {
    d.extra_sections()
        .find(|(n, _)| **n == opendwg::container::SectionName::AuxHeader)
        .unwrap()
        .1
        .to_vec()
}

/// A new drawing's AuxHeader, field by field against AutoCAD's in the
/// corpus file. Differences kept: the maintenance version (this crate's 0,
/// as in its file header), the save counts (one save, by ODS's rules), the
/// dates and handle seed (the drawing's), and the two words ODS gives as
/// zero (in the "0 f" and "0 g" fields) where AutoCAD stores per-file
/// values.
#[test]
fn new_aux_header_fields() {
    let d = new();
    let back = roundtrip(&d);
    let got = aux_fields(&aux_section(&back));
    let h = &back.header;
    let expected_diffs: BTreeMap<&str, i64> = [
        ("maintenance", 0),
        ("saves", 1),
        ("saves part 1", 1),
        ("saves part 2", 0),
        ("0 f", 0),
        ("0 g", 0),
        ("tdcreate day", i64::from(h.tdcreate.day)),
        ("tdcreate ms", i64::from(h.tdcreate.ms)),
        ("tdupdate day", i64::from(h.tdupdate.day)),
        ("tdupdate ms", i64::from(h.tdupdate.ms)),
        ("handseed", i64::try_from(h.handseed.0).unwrap()),
        ("part 1 - part 2", 1),
        ("saves again", 1),
    ]
    .into_iter()
    .collect();
    for (name, v) in &expected_diffs {
        assert_eq!(got[name], *v, "{name}");
    }
    let Some(p) = common::corpus_file_or_skip(TEMPLATE_SOURCE) else {
        return;
    };
    let want = aux_fields(&aux_section(&Drawing::open(p).unwrap()));
    for (name, v) in &want {
        if !expected_diffs.contains_key(name) {
            assert_eq!(got[name], *v, "{name}");
        }
    }
}
