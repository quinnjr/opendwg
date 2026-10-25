//! `Drawing::validate`, the structural check run before every save.
//! Error findings stop `to_bytes`; warnings do not.

mod common;

use opendwg::object::{
    Entity, EntityCommon, EntityList, MaterialRef, NonGraphical, Object, PolygonMesh, Vertex3d,
};
use opendwg::section::JulianDate;
use opendwg::{Diagnostic, Drawing, Error, Handle, Severity, Version};

const R2018: &str = "acadsharp-sample-ac1032";

const SAVED: JulianDate = JulianDate {
    day: 2_461_300,
    ms: 0,
};

fn r2018() -> Option<Drawing> {
    let p = common::corpus_file_or_skip(R2018)?;
    Some(Drawing::open(p).unwrap())
}

fn errors(diags: &[Diagnostic]) -> Vec<&Diagnostic> {
    diags
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect()
}

/// A handle no object has.
fn missing(d: &Drawing) -> Handle {
    let h = Handle(d.header.handseed.0 + 0x1000);
    assert!(d.get(h).is_none());
    h
}

/// The first object of the type named `ty`.
fn first_of(d: &Drawing, ty: &str) -> Handle {
    d.objects()
        .find(|(_, o)| o.type_name() == ty)
        .map(|(h, _)| h)
        .unwrap_or_else(|| panic!("no {ty}"))
}

/// The error diagnostics on `h` whose message contains every one of `words`.
fn errors_on<'a>(diags: &'a [Diagnostic], h: Handle, words: &[&str]) -> Vec<&'a Diagnostic> {
    diags
        .iter()
        .filter(|d| {
            d.severity == Severity::Error
                && d.handle == Some(h)
                && words.iter().all(|w| d.message.contains(w))
        })
        .collect()
}

/// `to_bytes` refuses the drawing with `Error::Invalid` listing the errors
/// `validate` gives, and only those.
fn assert_refused(d: &Drawing) {
    let expected: Vec<Diagnostic> = errors(&d.validate()).into_iter().cloned().collect();
    assert!(!expected.is_empty());
    match d.to_bytes_at(SAVED) {
        Err(Error::Invalid(list)) => assert_eq!(list, expected),
        other => panic!("{:?}", other.map(|b| b.len())),
    }
}

fn entity_common(d: &mut Drawing, h: Handle) -> &mut EntityCommon {
    match d.get_mut(h) {
        Some(Object::Entity { common, .. }) => common,
        o => panic!("{o:?}"),
    }
}

/// Every corpus file validates without errors as read, R2000-R2010 files in
/// the form they are written in (ACIS entities converted, the R2000
/// VP_ENT_HDR table and absorbed CELLSTYLEMAPs left out); a file that can
/// be written reads back without errors too.
#[test]
fn corpus_files_validate_clean() {
    for (e, p) in common::fetched_files_or_skip() {
        let what = e.name.as_str();
        let d = Drawing::open(&p).unwrap();
        let diags = d.validate();
        assert!(errors(&diags).is_empty(), "{what}: {:#?}", errors(&diags));
        let warnings = diags.len();
        eprintln!("{what}: {warnings} warning(s)");
        for w in diags.iter().take(10) {
            eprintln!("  {w:?}");
        }
        if !d.unconvertible_objects().is_empty() {
            continue;
        }
        let back = Drawing::from_bytes(&d.to_bytes_at(SAVED).unwrap()).unwrap();
        let again = back.validate();
        assert!(errors(&again).is_empty(), "{what} read back: {again:#?}");
        assert_eq!(back.source_version(), Some(Version::R2018));
    }
}

/// An entity whose layer is changed through `get_mut` to a
/// handle no object has is an error naming the entity; saving is refused.
#[test]
fn validate_catches_dangling_layer() {
    let Some(mut d) = r2018() else { return };
    let line = first_of(&d, "LINE");
    let gone = missing(&d);
    entity_common(&mut d, line).layer = gone;
    let diags = d.validate();
    let found = errors_on(
        &diags,
        line,
        &["LINE", &line.to_string(), &gone.to_string()],
    );
    assert_eq!(found.len(), 1, "{diags:#?}");
    assert!(found[0].message.contains("does not exist"), "{found:?}");
    assert_refused(&d);
    // A layer handle naming an object that is not a layer is an error too.
    let style = d.text_styles().next().unwrap().0;
    entity_common(&mut d, line).layer = style;
    let diags = d.validate();
    assert_eq!(
        errors_on(&diags, line, &["LINE", "not a layer"]).len(),
        1,
        "{diags:#?}"
    );
}

/// Soft pointers and reactors to objects that do not exist
/// are warnings; the drawing saves and keeps their values.
#[test]
fn soft_dangling_is_warning() {
    let Some(mut d) = r2018() else { return };
    let gone = missing(&d);
    let line = first_of(&d, "LINE");
    entity_common(&mut d, line).obj.reactors.push(gone);
    // A layout's last active viewport is a soft pointer.
    let layout = d
        .layouts()
        .into_iter()
        .find(|(_, l)| l.active_viewport.0 != 0)
        .map(|(h, _)| h)
        .unwrap();
    match d.get_mut(layout) {
        Some(Object::Other {
            object: NonGraphical::Layout(l),
            ..
        }) => l.active_viewport = gone,
        o => panic!("{o:?}"),
    }
    let diags = d.validate();
    assert!(errors(&diags).is_empty(), "{diags:#?}");
    for h in [line, layout] {
        assert!(
            diags.iter().any(|x| x.severity == Severity::Warning
                && x.handle == Some(h)
                && x.message.contains(&gone.to_string())),
            "{h}: {diags:#?}"
        );
    }
    let back = Drawing::from_bytes(&d.to_bytes_at(SAVED).unwrap()).unwrap();
    assert!(
        back.get(line)
            .unwrap()
            .object_common()
            .reactors
            .contains(&gone)
    );
    assert!(matches!(
        back.get(layout),
        Some(Object::Other { object: NonGraphical::Layout(l), .. }) if l.active_viewport == gone
    ));
}

/// A drawing without layer `0` cannot be saved.
#[test]
fn missing_layer_zero_is_error() {
    let Some(mut d) = r2018() else { return };
    let zero = d.layer_by_name("0").unwrap();
    match d.get_mut(zero) {
        Some(Object::Other {
            object: NonGraphical::Layer(l),
            ..
        }) => l.entry.name = "Zero".into(),
        o => panic!("{o:?}"),
    }
    let diags = d.validate();
    assert!(
        errors(&diags)
            .iter()
            .any(|x| x.message.contains("layer \"0\"")),
        "{diags:#?}"
    );
    assert_refused(&d);
}

/// A hard owner reference to a missing object is an error, and so is an
/// entity its owner does not list.
#[test]
fn hard_owner_and_owner_lists() {
    let Some(mut d) = r2018() else { return };
    let ms = d.model_space();
    let line = first_of(&d, "LINE");
    let owner = d.owner_of(line).unwrap();
    let gone = missing(&d);
    let list = match d.get_mut(owner) {
        Some(Object::Other {
            object: NonGraphical::BlockRecord(b),
            ..
        }) => match &mut b.entities {
            EntityList::Owned(v) => v,
            l => panic!("{l:?}"),
        },
        o => panic!("{o:?}"),
    };
    let at = list.iter().position(|&e| e == line).unwrap();
    list[at] = gone;
    let diags = d.validate();
    assert_eq!(
        errors_on(
            &diags,
            owner,
            &["BLOCK_HEADER", &gone.to_string(), "does not exist"]
        )
        .len(),
        1,
        "{diags:#?}"
    );
    assert_eq!(
        errors_on(&diags, line, &["LINE", "does not list it"]).len(),
        1,
        "{diags:#?}"
    );
    assert_refused(&d);
    // An entity listed by a block it does not name as owner.
    let Some(mut d) = r2018() else { return };
    let other = d
        .block_records()
        .map(|(h, _)| h)
        .find(|&h| h != ms && h != owner && d.entities_in(h).next().is_some())
        .unwrap();
    let moved = d.entities_in(other).next().unwrap();
    if let Some(Object::Other {
        object: NonGraphical::BlockRecord(b),
        ..
    }) = d.get_mut(ms)
    {
        if let EntityList::Owned(v) = &mut b.entities {
            v.push(moved);
        }
    }
    let diags = d.validate();
    assert!(
        !errors_on(&diags, moved, &["owned by", &ms.to_string()]).is_empty(),
        "{diags:#?}"
    );
}

/// Table record names are unique without case; header handles must name
/// objects of the right kind; required dictionaries must be there.
#[test]
fn names_header_and_required_dictionaries() {
    let Some(d0) = r2018() else { return };
    let layers: Vec<(Handle, String)> = d0
        .layers()
        .map(|(h, l)| (h, l.entry.name.clone()))
        .collect();
    let (h, _) = layers.iter().find(|(_, n)| n != "0").unwrap();
    let mut d = d0.clone();
    match d.get_mut(*h) {
        Some(Object::Other {
            object: NonGraphical::Layer(l),
            ..
        }) => l.entry.name = "0".into(),
        o => panic!("{o:?}"),
    }
    let diags = d.validate();
    assert_eq!(
        errors_on(&diags, *h, &["LAYER", "same name"]).len(),
        1,
        "{diags:#?}"
    );
    assert_refused(&d);

    let mut d = d0.clone();
    d.header.clayer = missing(&d);
    let diags = d.validate();
    assert!(
        errors(&diags)
            .iter()
            .any(|x| x.message.contains("$CLAYER") && x.handle.is_none()),
        "{diags:#?}"
    );
    let mut d = d0.clone();
    d.header.clayer = Handle(0);
    let diags = d.validate();
    assert!(
        errors(&diags)
            .iter()
            .any(|x| x.message.contains("$CLAYER is null")),
        "{diags:#?}"
    );
    let mut d = d0.clone();
    d.header.celtype = d.layer_by_name("0").unwrap();
    let diags = d.validate();
    assert!(
        errors(&diags)
            .iter()
            .any(|x| x.message.contains("$CELTYPE") && x.message.contains("not a LTYPE")),
        "{diags:#?}"
    );

    let mut d = d0.clone();
    let nod = d.named_objects();
    match d.get_mut(nod) {
        Some(Object::Other {
            object: NonGraphical::Dictionary(n),
            ..
        }) => n.entries.retain(|(name, _)| name != "ACAD_MLINESTYLE"),
        o => panic!("{o:?}"),
    }
    let diags = d.validate();
    assert!(
        !errors_on(&diags, nod, &["ACAD_MLINESTYLE"]).is_empty(),
        "{diags:#?}"
    );
}

/// An object whose common data names another object's handle would give
/// two objects one handle.
#[test]
fn duplicate_handle_is_error() {
    let Some(mut d) = r2018() else { return };
    let line = first_of(&d, "LINE");
    let circle = first_of(&d, "CIRCLE");
    entity_common(&mut d, line).obj.handle = circle;
    let diags = d.validate();
    assert!(
        !errors_on(&diags, line, &["LINE", &circle.to_string(), "handle"]).is_empty(),
        "{diags:#?}"
    );
}

/// Cross-object counts: a polyface mesh's face indices and counts too large
/// to store (wrong counts are recounted), a polygon mesh's M x N, a
/// multiline's lines against its style; vertex flags are set by the writer.
#[test]
fn mesh_mline_and_vertex_checks() {
    let Some(d0) = r2018() else { return };
    // A polyface mesh face using a location the mesh does not have.
    let mut d = d0.clone();
    let face = first_of(&d, "VERTEX_PFACE_FACE");
    match d.get_mut(face) {
        Some(Object::Entity {
            entity: Entity::VertexPfaceFace(f),
            ..
        }) => f.indices[1] = 99,
        o => panic!("{o:?}"),
    }
    let diags = d.validate();
    assert!(
        !errors_on(&diags, face, &["VERTEX_PFACE_FACE", "99"]).is_empty(),
        "{diags:#?}"
    );
    // Counts that do not match are derived values the writer recounts: no
    // finding. Counts its fields cannot hold are an error.
    let mut d = d0.clone();
    let mesh = first_of(&d, "POLYLINE_PFACE");
    match d.get_mut(mesh) {
        Some(Object::Entity {
            entity: Entity::PolyfaceMesh(p),
            ..
        }) => p.face_count += 1,
        o => panic!("{o:?}"),
    }
    assert!(errors(&d.validate()).is_empty());
    match d.get_mut(mesh) {
        Some(Object::Entity {
            entity: Entity::PolyfaceMesh(p),
            ..
        }) => match &mut p.vertices {
            EntityList::Owned(v) => {
                let first = v[0];
                v.extend(std::iter::repeat_n(first, 32_768));
            }
            l => panic!("{l:?}"),
        },
        o => panic!("{o:?}"),
    }
    let diags = d.validate();
    assert_eq!(
        errors_on(
            &diags,
            mesh,
            &["POLYLINE_PFACE", "32773 vertices", "can hold"]
        )
        .len(),
        1,
        "{:#?}",
        errors(&diags)
    );

    // A multiline whose line count is not its style's.
    let mut d = d0.clone();
    let mline = first_of(&d, "MLINE");
    match d.get_mut(mline) {
        Some(Object::Entity {
            entity: Entity::MLine(m),
            ..
        }) => m.lines_in_style += 1,
        o => panic!("{o:?}"),
    }
    let diags = d.validate();
    assert!(
        !errors_on(&diags, mline, &["MLINE", "MLINESTYLE"]).is_empty(),
        "{diags:#?}"
    );
    // A vertex whose flags are another kind's: the writer sets its kind's
    // bits and clears the others', keeping its other bits.
    let mut d = d0.clone();
    let v = first_of(&d, "VERTEX_3D");
    match d.get_mut(v) {
        Some(Object::Entity {
            entity: Entity::Vertex3d(x),
            ..
        }) => x.flags = 64 | 1,
        o => panic!("{o:?}"),
    }
    assert!(errors(&d.validate()).is_empty());
    let back = Drawing::from_bytes(&d.to_bytes_at(SAVED).unwrap()).unwrap();
    assert!(matches!(
        back.get(v),
        Some(Object::Entity { entity: Entity::Vertex3d(x), .. }) if x.flags == 32 | 1
    ));
    // A polygon mesh whose M x N is not its vertex count.
    let mut d = d0.clone();
    let ms = d.model_space();
    let layer = d.layer_by_name("0").unwrap();
    let vertex = |i: usize| {
        (
            EntityCommon::new(layer),
            Entity::VertexMesh(Vertex3d {
                flags: 64,
                point: [i as f64, 0.0, 0.0],
            }),
        )
    };
    let mesh = d
        .add_polyline(
            ms,
            (
                EntityCommon::new(layer),
                Entity::PolygonMesh(PolygonMesh {
                    flags: 16,
                    curve_type: 0,
                    m_count: 2,
                    n_count: 2,
                    m_density: 0,
                    n_density: 0,
                    vertices: EntityList::Owned(vec![]),
                    seqend: Handle(0),
                }),
            ),
            (0..4).map(vertex).collect(),
        )
        .unwrap();
    assert!(errors(&d.validate()).is_empty());
    match d.get_mut(mesh) {
        Some(Object::Entity {
            entity: Entity::PolygonMesh(p),
            ..
        }) => p.n_count = 3,
        o => panic!("{o:?}"),
    }
    let diags = d.validate();
    assert!(
        !errors_on(&diags, mesh, &["POLYLINE_MESH", "2 x 3", "4 vertices"]).is_empty(),
        "{diags:#?}"
    );
}

/// A group member that does not list the group among its reactors, and an
/// entity naming a group as reactor that the group does not list, are
/// warnings (reactor links, which AutoCAD repairs).
#[test]
fn group_reactors() {
    let Some(d0) = r2018() else { return };
    let group = first_of(&d0, "GROUP");
    let member = match d0.get(group) {
        Some(Object::Other {
            object: NonGraphical::Group(g),
            ..
        }) => g.entities[0],
        o => panic!("{o:?}"),
    };
    let mut d = d0.clone();
    entity_common(&mut d, member)
        .obj
        .reactors
        .retain(|&r| r != group);
    let diags = d.validate();
    assert!(errors(&diags).is_empty(), "{diags:#?}");
    assert!(
        diags.iter().any(|x| x.severity == Severity::Warning
            && x.handle == Some(member)
            && x.message.contains("GROUP")),
        "{diags:#?}"
    );
    let mut d = d0.clone();
    let line = d
        .objects()
        .find(|(_, o)| o.type_name() == "LINE" && !o.object_common().reactors.contains(&group))
        .map(|(h, _)| h)
        .unwrap();
    entity_common(&mut d, line).obj.reactors.push(group);
    let diags = d.validate();
    assert!(errors(&diags).is_empty(), "{diags:#?}");
    assert!(
        diags.iter().any(|x| x.severity == Severity::Warning
            && x.handle == Some(line)
            && x.message.contains("GROUP")
            && x.message.contains("does not list")),
        "{diags:#?}"
    );
    assert!(d.to_bytes_at(SAVED).is_ok());
}

/// The findings are sorted by handle, then message, and the same for the
/// same drawing.
#[test]
fn findings_are_sorted_and_deterministic() {
    let Some(mut d) = r2018() else { return };
    let gone = missing(&d);
    for ty in ["LINE", "CIRCLE", "ARC"] {
        let h = first_of(&d, ty);
        let c = entity_common(&mut d, h);
        c.layer = gone;
        c.material = MaterialRef::Handle(gone);
        c.obj.reactors.push(gone);
    }
    d.header.clayer = gone;
    let diags = d.validate();
    let mut sorted = diags.clone();
    sorted.sort_by(|a, b| (a.handle, &a.message).cmp(&(b.handle, &b.message)));
    assert_eq!(diags, sorted);
    assert!(diags.len() >= 10, "{diags:#?}");
    assert_eq!(d.clone().validate(), diags);
}

/// The required tables: a missing table control is an error.
#[test]
fn missing_table_is_error() {
    let Some(mut d) = r2018() else { return };
    d.header.ucs_control = missing(&d);
    let diags = d.validate();
    assert!(
        errors(&diags).iter().any(|x| x.message.contains("UCS")),
        "{diags:#?}"
    );
}

/// An XRECORD 480-481 item (a hard pointer stored as hexadecimal text)
/// whose text is not a handle is a warning naming the item; an empty one
/// (the null handle) and a valid one are not reported.
#[test]
fn bad_xrecord_handle_text_is_warning() {
    use opendwg::object::{XRecord, XRecordValue};
    let mut d = Drawing::new_at(SAVED);
    let nod = d.named_objects();
    let l0 = d.layer_by_name("0").unwrap();
    let x = d
        .add_object(
            nod,
            "OPENDWG_TEST",
            NonGraphical::XRecord(XRecord {
                items: vec![
                    (480, XRecordValue::HandleText("not hex".into())),
                    (481, XRecordValue::HandleText(String::new())),
                    (480, XRecordValue::HandleText(format!("{:X}", l0.0))),
                ],
                cloning: 1,
                ..XRecord::default()
            }),
        )
        .unwrap();
    let diags = d.validate();
    let on_x: Vec<_> = diags.iter().filter(|f| f.handle == Some(x)).collect();
    assert_eq!(on_x.len(), 1, "{diags:#?}");
    assert_eq!(on_x[0].severity, Severity::Warning);
    assert!(
        on_x[0].message.contains("480") && on_x[0].message.contains("not hex"),
        "{on_x:?}"
    );
    // A warning does not stop the save.
    d.to_bytes_at(SAVED).unwrap();
}

/// The class error findings of `d`, as messages.
fn class_errors(d: &Drawing) -> Vec<String> {
    errors(&d.validate())
        .into_iter()
        .filter(|x| x.message.contains("class"))
        .map(|x| x.message.clone())
        .collect()
}

/// A class number below 500 or used twice is an error: the reader would
/// refuse the file or read it differently.
#[test]
fn bad_class_numbers_are_errors() {
    let d = Drawing::new();
    assert!(d.classes.len() > 2);
    assert_eq!(class_errors(&d), Vec::<String>::new());

    let mut low = d.clone();
    low.classes[0].class_number = 499;
    let found = class_errors(&low);
    assert!(found.iter().any(|m| m.contains("below 500")), "{found:?}");
    assert!(matches!(low.to_bytes_at(SAVED), Err(Error::Invalid(_))));

    let mut dup = d.clone();
    dup.classes[1].class_number = dup.classes[0].class_number;
    let found = class_errors(&dup);
    assert!(found.iter().any(|m| m.contains("used twice")), "{found:?}");
    assert!(matches!(dup.to_bytes_at(SAVED), Err(Error::Invalid(_))));
}

/// An item class id other than entity (0x1F2) or object (0x1F3), which the
/// reader accepts and reads as an object class, validates and is written as
/// the kind the reader gave it: 0x1F3. Objects of the class read back the
/// same; an entity class with such an id has its entities refused (the
/// reader would read them as objects).
#[test]
fn hostile_item_class_id_round_trips() {
    use opendwg::section::classes::ITEM_CLASS_OBJECT;
    let mut d = Drawing::new();
    let counts =
        |d: &Drawing, name: &str| d.objects().filter(|(_, o)| o.type_name() == name).count();
    let i = d
        .classes
        .iter()
        .position(|c| !c.is_entity() && counts(&d, &c.dxf_name) > 0)
        .expect("a new drawing has objects of an object class");
    let name = d.classes[i].dxf_name.clone();
    d.classes[i].item_class_id = 0x123;
    assert_eq!(class_errors(&d), Vec::<String>::new());
    let back = Drawing::from_bytes(&d.to_bytes_at(SAVED).unwrap()).unwrap();
    let c = back.classes.iter().find(|c| c.dxf_name == name).unwrap();
    assert_eq!(c.item_class_id, ITEM_CLASS_OBJECT);
    assert_eq!(counts(&back, &name), counts(&d, &name));
    assert!(errors(&back.validate()).is_empty());
    // A class without objects, whatever it said, is written as an object
    // class, as it would be read.
    let mut d = Drawing::new();
    if let Some(c) = d.classes.iter_mut().find(|c| c.is_entity()) {
        let name = c.dxf_name.clone();
        c.item_class_id = 0xFFFF;
        if counts(&d, &name) == 0 {
            let back = Drawing::from_bytes(&d.to_bytes_at(SAVED).unwrap()).unwrap();
            let c = back.classes.iter().find(|c| c.dxf_name == name).unwrap();
            assert_eq!(c.item_class_id, ITEM_CLASS_OBJECT);
        } else {
            assert!(matches!(d.to_bytes_at(SAVED), Err(Error::Invalid(_))));
        }
    }
}

/// An object whose class says entity (or an entity whose class says
/// object) would be read back as the other kind.
#[test]
fn class_kind_must_match_its_objects() {
    let mut d = Drawing::new();
    let (h, i) = d
        .objects()
        .find_map(|(h, o)| {
            let name = o.type_name();
            let i = d.classes.iter().position(|c| c.dxf_name == name)?;
            Some((h, i))
        })
        .expect("a new drawing has an object of a class");
    d.classes[i].item_class_id ^= 0x1F2 ^ 0x1F3;
    let found = errors(&d.validate())
        .into_iter()
        .filter(|x| x.handle == Some(h))
        .map(|x| x.message.clone())
        .collect::<Vec<_>>();
    assert!(found.iter().any(|m| m.contains("class")), "{found:?}");
    assert!(matches!(d.to_bytes_at(SAVED), Err(Error::Invalid(_))));
}

/// An entity one block lists but whose owner names another block is
/// written with the listing block as owner, with a warning; listed by two
/// blocks it is refused.
#[test]
fn entity_owner_follows_its_block_list() {
    use opendwg::object::{Entity, EntityCommon, Line};
    let mut d = Drawing::new();
    let ms = d.model_space();
    let l0 = d.layer_by_name("0").unwrap();
    let line = d
        .add_entity(
            ms,
            EntityCommon::new(l0),
            Entity::Line(Line {
                start: [0.0; 3],
                end: [1.0, 0.0, 0.0],
                thickness: 0.0,
                extrusion: [0.0, 0.0, 1.0],
            }),
        )
        .unwrap();
    let other = d.add_block("OTHER").unwrap();
    {
        let (common, _) = d.entity_mut(line).unwrap();
        common.entmode = 0;
        common.obj.owner = other;
    }
    let v = d.validate();
    assert!(errors(&v).is_empty(), "{v:#?}");
    assert!(
        d.write_diagnostics()
            .iter()
            .any(|x| x.handle == Some(line) && x.message.contains("lists it"))
    );
    let (bytes, warnings) = d.to_bytes_with_diagnostics().unwrap();
    assert!(warnings.iter().any(|x| x.handle == Some(line)));
    let back = Drawing::from_bytes(&bytes).unwrap();
    assert_eq!(back.owner_of(line), Some(ms));
    assert!(errors(&back.validate()).is_empty());
    // Listed by both blocks: ambiguous, refused.
    if let Some(opendwg::object::EntityList::Owned(list)) =
        d.block_record_mut(other).map(|b| &mut b.entities)
    {
        list.push(line);
    }
    assert!(!errors(&d.validate()).is_empty());
    assert!(matches!(d.to_bytes(), Err(Error::Invalid(_))));
}

/// An xref block cannot own entities: `add_entity` refuses one, and an
/// entity put in its list through `block_record_mut` is a validate error,
/// so the save fails with `Error::Invalid`, not in the encoder.
#[test]
fn xref_blocks_own_no_entities() {
    use opendwg::object::{Entity, EntityCommon, Line};
    let mut d = Drawing::new();
    let l0 = d.layer_by_name("0").unwrap();
    let line = || {
        Entity::Line(Line {
            start: [0.0; 3],
            end: [1.0, 0.0, 0.0],
            thickness: 0.0,
            extrusion: [0.0, 0.0, 1.0],
        })
    };
    let x = d.add_block("X").unwrap();
    let e = d.add_entity(x, EntityCommon::new(l0), line()).unwrap();
    d.block_record_mut(x).unwrap().is_xref = true;
    let refused = d.add_entity(x, EntityCommon::new(l0), line());
    assert!(matches!(refused, Err(Error::Invalid(_))), "{refused:?}");
    let v = d.validate();
    assert!(
        errors(&v)
            .iter()
            .any(|f| f.handle == Some(x) && f.message.contains("xref")),
        "{v:#?}"
    );
    assert!(matches!(d.to_bytes(), Err(Error::Invalid(_))));
    // Not an xref again, it saves with its entity.
    d.block_record_mut(x).unwrap().is_xref = false;
    assert!(errors(&d.validate()).is_empty());
    let back = Drawing::from_bytes(&d.to_bytes().unwrap()).unwrap();
    assert_eq!(back.owner_of(e), Some(x));
}
