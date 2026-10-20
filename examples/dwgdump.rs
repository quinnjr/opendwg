//! Dump information about a DWG file. Grows with each task: prints the version, code page and
//! the raw section names and sizes; `--header` also prints the header variables, and
//! `--objects` the classes, the summary, preview and app info sections, every object as
//! `handle type_name` (with the main fields of typed objects), and the diagnostics.
//! `--proxy <handle>` prints the proxy graphics primitives of the entity with that
//! (hexadecimal) handle.

use std::process::ExitCode;

use opendwg::container::{self, SectionName};
use opendwg::object::{
    AcisPayload, Dictionary, Entity, EntityList, Insert, MLeaderContent, NonGraphical, Object,
    SplineForm, TableStyle,
};
use opendwg::section::header;
use opendwg::{Drawing, Handle};

fn entity_list(list: &EntityList) -> String {
    match list {
        EntityList::Owned(v) => {
            let v: Vec<String> = v.iter().map(|h| h.to_string()).collect();
            format!("[{}]", v.join(" "))
        }
        EntityList::Linked { first, last } => format!("{first}..{last}"),
    }
}

fn describe_insert(i: &Insert) -> String {
    let attribs = match i.seqend {
        Some(seqend) => format!(", attribs {} seqend {seqend}", entity_list(&i.attribs)),
        None => String::new(),
    };
    format!(
        "block {} at {:?}, scale {:?}, rotation {}{attribs}",
        i.block, i.insertion, i.scale, i.rotation
    )
}

/// The main fields of a typed entity, or `None` for raw entities.
fn describe_entity(entity: &Entity) -> Option<String> {
    Some(match entity {
        Entity::Unknown(_) => return None,
        Entity::Line(l) => format!("{:?} to {:?}", l.start, l.end),
        Entity::Point(p) => format!("{:?}", p.position),
        Entity::Circle(c) => format!("center {:?}, radius {}", c.center, c.radius),
        Entity::Arc(a) => format!(
            "center {:?}, radius {}, angles {} to {}",
            a.circle.center, a.circle.radius, a.start_angle, a.end_angle
        ),
        Entity::Ellipse(e) => format!(
            "center {:?}, major axis {:?}, ratio {}",
            e.center, e.major_axis, e.axis_ratio
        ),
        Entity::Ray(r) | Entity::Xline(r) => format!("{:?} direction {:?}", r.point, r.vector),
        Entity::Solid(s) | Entity::Trace(s) => format!("corners {:?}", s.corners),
        Entity::Face3d(f) => format!("corners {:?}", f.corners),
        Entity::Shape(s) => format!(
            "shape {} of style {} at {:?}",
            s.shape_number, s.style, s.insertion
        ),
        Entity::Block(b) => format!("{:?}", b.name),
        Entity::EndBlk | Entity::SeqEnd => String::new(),
        Entity::Insert(i) => describe_insert(i),
        Entity::MInsert(m) => format!(
            "{}, {} x {} array",
            describe_insert(&m.insert),
            m.columns,
            m.rows
        ),
        Entity::Text(t) => format!("{:?} at {:?}, height {}", t.value, t.insertion, t.height),
        Entity::Attrib(a) => format!(
            "{:?} = {:?}{}",
            a.tag,
            a.text.value,
            if a.mtext.is_some() {
                ", multi-line"
            } else {
                ""
            }
        ),
        Entity::AttDef(a) => format!(
            "{:?} prompt {:?}, default {:?}{}",
            a.attrib.tag,
            a.prompt,
            a.attrib.text.value,
            if a.attrib.mtext.is_some() {
                ", multi-line"
            } else {
                ""
            }
        ),
        Entity::MText(m) => format!("{:?} at {:?}", m.plain_text(), m.insertion),
        Entity::LwPolyline(p) => format!(
            "{} points{}, bulges {}, widths {}, first {:?}",
            p.points.len(),
            if p.closed() { " closed" } else { "" },
            p.bulges.len(),
            p.widths.len(),
            p.points.first()
        ),
        Entity::Polyline2d(_)
        | Entity::Polyline3d(_)
        | Entity::PolyfaceMesh(_)
        | Entity::PolygonMesh(_) => format!(
            "vertices {} seqend {}",
            entity
                .polyline_vertices()
                .map(entity_list)
                .unwrap_or_default(),
            entity.polyline_seqend().unwrap_or_default()
        ),
        Entity::Vertex2d(v) => format!("{:?} bulge {}", v.point, v.bulge),
        Entity::Vertex3d(v) | Entity::VertexMesh(v) | Entity::VertexPface(v) => {
            format!("{:?}", v.point)
        }
        Entity::VertexPfaceFace(f) => format!("{:?}", f.indices),
        Entity::Spline(s) => match &s.form {
            SplineForm::Control(c) => format!(
                "degree {}, {} control points, {} knots",
                s.degree,
                c.points.len(),
                c.knots.len()
            ),
            SplineForm::Fit(f) => format!("degree {}, {} fit points", s.degree, f.points.len()),
        },
        Entity::MLine(m) => format!(
            "{} vertices, {} lines, style {}",
            m.vertices.len(),
            m.lines_in_style,
            m.style
        ),
        Entity::Dimension(d) => format!(
            "measurement {}, text {:?}, style {}, block {}",
            d.measurement(),
            d.common.user_text,
            d.common.dimstyle,
            d.common.block
        ),
        Entity::Leader(l) => format!(
            "{} points, annotation {}, style {}",
            l.points.len(),
            l.annotation,
            l.dimstyle
        ),
        Entity::Tolerance(t) => format!("{:?} at {:?}", t.text, t.insertion),
        Entity::MultiLeader(m) => {
            let content = match &m.context.content {
                MLeaderContent::Text(t) => format!("text {:?}", t.label),
                MLeaderContent::Block(b) => format!("block {}", b.block),
                MLeaderContent::None => "no content".to_owned(),
            };
            let lines: usize = m.context.roots.iter().map(|r| r.lines.len()).sum();
            format!(
                "{content}, {} roots, {lines} lines, style {}",
                m.context.roots.len(),
                m.style
            )
        }
        Entity::Hatch(h) => format!(
            "{:?}{}, {} paths, {} pattern lines, {} seed points{}",
            h.pattern_name,
            if h.is_solid() { " solid" } else { "" },
            h.paths.len(),
            h.pattern.as_ref().map_or(0, |p| p.lines.len()),
            h.seed_points.len(),
            if h.gradient.enabled != 0 {
                format!(", gradient {:?}", h.gradient.name)
            } else {
                String::new()
            }
        ),
        Entity::Viewport(v) => format!(
            "center {:?}, {} x {}, status {}, {} frozen layers",
            v.center,
            v.width,
            v.height,
            v.status_flags,
            v.frozen_layers.len()
        ),
        Entity::Image(i) | Entity::Wipeout(i) => format!(
            "at {:?}, size {:?}, clip type {} ({} points), def {}",
            i.insertion,
            i.size,
            i.clip_type,
            i.clip_boundary.len(),
            i.image_def
        ),
        Entity::Acis(a) => {
            let payload = match &a.payload {
                AcisPayload::DataStorage { guid, .. } => format!("data in AcDs, GUID {guid}"),
                AcisPayload::Empty => "no data".to_owned(),
                AcisPayload::SatBlocks {
                    data, block_sizes, ..
                } => format!("SAT, {} blocks, {} bytes", block_sizes.len(), data.len()),
                AcisPayload::Inline { data, .. } => format!("inline, {} bytes", data.len()),
            };
            format!(
                "{:?}: {payload}, {} wireframe bits{}",
                a.kind,
                a.wireframe_bits,
                a.history
                    .map_or(String::new(), |h| format!(", history {h}"))
            )
        }
        Entity::Ole2Frame(o) => format!("{} bytes of OLE data", o.data.len()),
        Entity::ProxyEntity(p) => format!(
            "class {}, version {}.{}, {} data bits, {} references",
            p.class_id,
            p.version,
            p.maintenance_version,
            p.data_bits,
            p.object_ids.len()
        ),
        // A kind added after this tool was written.
        _ => return None,
    })
}

/// The main fields of a typed object, or `None` for raw objects.
fn describe(obj: &Object) -> Option<String> {
    let object = match obj {
        Object::Entity { entity, .. } => return describe_entity(entity),
        Object::Other { object, .. } => object,
        _ => return None,
    };
    Some(match object {
        NonGraphical::Unknown(_) => return None,
        NonGraphical::TableControl(c) => {
            let extra: Vec<String> = c.extra.iter().map(|h| h.to_string()).collect();
            format!(
                "{:?} table: {} entries, extra [{}]",
                c.kind,
                c.entries.len(),
                extra.join(" ")
            )
        }
        NonGraphical::BlockRecord(b) => {
            let entities = match &b.entities {
                EntityList::Owned(v) => format!("{} entities", v.len()),
                EntityList::Linked { first, last } => format!("entities {first}..{last}"),
            };
            format!(
                "{:?} {entities}, {} inserts, block {} endblk {} layout {}",
                b.entry.name,
                b.inserts.len(),
                b.block_entity,
                b.end_block,
                b.layout
            )
        }
        NonGraphical::Layer(l) => {
            let mut flags = vec![];
            for (set, name) in [
                (l.off, "off"),
                (l.frozen, "frozen"),
                (l.frozen_in_new_viewports, "vp-frozen"),
                (l.locked, "locked"),
                (!l.plot, "no-plot"),
            ] {
                if set {
                    flags.push(name);
                }
            }
            format!(
                "{:?} color {:?}, linetype {}, lineweight index {}{}{}",
                l.entry.name,
                l.color,
                l.linetype,
                l.lineweight,
                if flags.is_empty() { "" } else { ", " },
                flags.join(" ")
            )
        }
        NonGraphical::TextStyle(t) => format!(
            "{:?} font {:?} bigfont {:?}, height {}, width {}{}",
            t.entry.name,
            t.font_name,
            t.bigfont_name,
            t.fixed_height,
            t.width_factor,
            if t.shape_file { ", shape file" } else { "" }
        ),
        NonGraphical::Linetype(l) => {
            let texts: Vec<String> = (0..l.dashes.len()).filter_map(|i| l.text(i)).collect();
            format!(
                "{:?} {:?}, length {}, {} dashes, texts {texts:?}",
                l.entry.name,
                l.description,
                l.pattern_length,
                l.dashes.len()
            )
        }
        NonGraphical::View(v) => format!(
            "{:?} center {:?}, {} x {}",
            v.entry.name, v.center, v.width, v.height
        ),
        NonGraphical::Ucs(u) => format!("{:?} origin {:?}", u.entry.name, u.origin),
        NonGraphical::Vport(v) => format!(
            "{:?} center {:?}, height {}",
            v.entry.name, v.view_center, v.view_height
        ),
        NonGraphical::AppId(a) => format!("{:?}", a.entry.name),
        NonGraphical::DimStyle(d) => format!(
            "{:?} dimscale {}, dimtxt {}, dimasz {}, text style {}",
            d.entry.name, d.dimscale, d.dimtxt, d.dimasz, d.dimtxsty
        ),
        NonGraphical::VpEntHdr(v) => format!("{:?} viewport {}", v.entry.name, v.viewport),
        NonGraphical::Dictionary(d) => describe_dictionary(d),
        NonGraphical::DictionaryWithDefault(d) => format!(
            "{}, default {}",
            describe_dictionary(&d.dictionary),
            d.default_entry
        ),
        NonGraphical::XRecord(x) => {
            let items: Vec<String> = x.items.iter().map(|(c, v)| format!("{c}={v:?}")).collect();
            format!("[{}], {} object ids", items.join(" "), x.object_ids.len())
        }
        NonGraphical::Layout(l) => format!(
            "{:?} tab {}, block {}, page {:?}, {} viewports",
            l.name,
            l.tab_order,
            l.block_record,
            l.plot.page_setup_name,
            l.viewports.len()
        ),
        NonGraphical::PlotSettings(p) => format!(
            "{:?} printer {:?}, paper {:?}",
            p.page_setup_name, p.printer, p.paper_size
        ),
        NonGraphical::Group(g) => format!("{:?}, {} entities", g.name, g.entities.len()),
        NonGraphical::MLineStyle(m) => {
            let lines: Vec<String> = m
                .elements
                .iter()
                .map(|e| format!("{} {:?} {:?}", e.offset, e.color, e.linetype))
                .collect();
            format!(
                "{:?} flags {}, lines [{}]",
                m.name,
                m.flags,
                lines.join(", ")
            )
        }
        NonGraphical::MLeaderStyle(m) => format!(
            "{:?} content {}, text style {}, text height {}, arrow {}",
            m.description, m.content_type, m.text_style, m.text_height, m.arrow_head_size
        ),
        NonGraphical::TableStyle(t) => match t {
            TableStyle::Legacy(l) => format!("{:?} (pre-R2010 format)", l.description),
            TableStyle::Cells(c) => {
                let names: Vec<&str> = c.cell_styles.iter().map(|(_, s)| s.name.as_str()).collect();
                format!("{:?} cell styles {names:?}", c.description)
            }
        },
        NonGraphical::ImageDef(i) => format!("{:?} {:?} pixels", i.path, i.size),
        NonGraphical::ImageDefReactor(i) => format!("class version {}", i.class_version),
        NonGraphical::RasterVariables(v) => format!(
            "frame {}, quality {}, units {}",
            v.display_frame, v.display_quality, v.units
        ),
        NonGraphical::WipeoutVariables(v) => format!("frame {}", v.display_frame),
        NonGraphical::Scale(s) => format!(
            "{:?} {}:{}{}",
            s.name,
            s.paper_units,
            s.drawing_units,
            if s.unit_scale { ", unit scale" } else { "" }
        ),
        NonGraphical::DictionaryVar(v) => format!("{:?}", v.value),
        NonGraphical::Placeholder => String::new(),
        NonGraphical::SortEntsTable(s) => {
            format!("block {}, {} entries", s.block, s.entries.len())
        }
        NonGraphical::ProxyObject(p) => format!(
            "class {}, {} data bits, {} object ids",
            p.class_id,
            p.data_bits,
            p.object_ids.len()
        ),
        NonGraphical::SpatialIndex(s) => format!(
            "timestamps {:?}, {} bits kept",
            s.timestamps, s.bits.data_bits
        ),
        NonGraphical::LayerIndex(l) => format!("{} entries", l.entries.len()),
        NonGraphical::SpatialFilter(f) => format!("{} bits kept", f.bits.data_bits),
        NonGraphical::Material(m) => format!("{:?}", m.name),
        NonGraphical::VisualStyle(v) => format!("{:?} type {}", v.name, v.style_type),
        NonGraphical::SectionViewStyle(v) => format!("{:?}", v.base.name),
        NonGraphical::DetailViewStyle(v) => format!("{:?}", v.base.name),
        // A kind added after this tool was written.
        _ => return None,
    })
}

fn describe_dictionary(d: &Dictionary) -> String {
    let entries: Vec<String> = d
        .entries
        .iter()
        .map(|(n, h)| format!("{n:?}={h}"))
        .collect();
    format!(
        "[{}]{}",
        entries.join(" "),
        if d.is_hard_owner() {
            ", hard owner"
        } else {
            ""
        }
    )
}

/// Print the proxy graphics of the entity `handle`, one primitive per line.
fn dump_proxy(bytes: &[u8], path: &str, raw: &container::RawSections, handle: Handle) -> ExitCode {
    let d = match Drawing::from_bytes(bytes) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("dwgdump: {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let Some(Object::Entity { common, .. }) = d.get(handle) else {
        eprintln!("dwgdump: {path}: {handle} is not an entity");
        return ExitCode::FAILURE;
    };
    match common.proxy_primitives_with_codepage(raw.version, raw.codepage) {
        None => {
            eprintln!("dwgdump: {path}: {handle} has no proxy graphics");
            ExitCode::FAILURE
        }
        Some(Err(e)) => {
            eprintln!("dwgdump: {path}: {handle}: proxy graphics: {e}");
            ExitCode::FAILURE
        }
        Some(Ok(prims)) => {
            println!("{handle}: {} proxy graphics primitives", prims.len());
            for p in &prims {
                println!("{p:?}");
            }
            ExitCode::SUCCESS
        }
    }
}

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut show_header = false;
    let mut show_objects = false;
    let mut proxy: Option<Handle> = None;
    while let Some(flag) = args.first().filter(|a| a.starts_with("--")) {
        match flag.as_str() {
            "--header" => show_header = true,
            "--objects" => show_objects = true,
            "--proxy" => {
                args.remove(0);
                let value = args.first().map(|h| h.trim_start_matches("0x"));
                match value.and_then(|h| u64::from_str_radix(h, 16).ok()) {
                    Some(h) => proxy = Some(Handle(h)),
                    None => {
                        eprintln!("dwgdump: --proxy needs a hexadecimal handle");
                        return ExitCode::from(2);
                    }
                }
            }
            _ => break,
        }
        args.remove(0);
    }
    let [path] = args.as_slice() else {
        eprintln!("usage: dwgdump [--header] [--objects] [--proxy <handle>] <file.dwg>");
        return ExitCode::from(2);
    };
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("dwgdump: {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let raw = match container::read(&bytes) {
        Ok(raw) => raw,
        Err(e) => {
            eprintln!("dwgdump: {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!("{}", String::from_utf8_lossy(raw.version.magic()));
    println!("maintenance {}, codepage {}", raw.maintenance, raw.codepage);
    for (name, data) in &raw.sections {
        println!("{name:?}: {} bytes", data.len());
    }
    if show_header {
        let bytes = raw
            .sections
            .get(&SectionName::Header)
            .map_or(&[][..], Vec::as_slice);
        match header::decode(bytes, raw.version, raw.codepage) {
            Ok(h) => println!("{h:#?}"),
            Err(e) => {
                eprintln!("dwgdump: {path}: header: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    if let Some(handle) = proxy {
        return dump_proxy(&bytes, path, &raw, handle);
    }
    if show_objects {
        let d = match Drawing::from_bytes(&bytes) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("dwgdump: {path}: {e}");
                return ExitCode::FAILURE;
            }
        };
        println!("{} classes", d.classes.len());
        for c in &d.classes {
            println!(
                "class {} {} ({})",
                c.class_number, c.dxf_name, c.cpp_class_name
            );
        }
        if let Some(s) = &d.summary {
            println!(
                "summary: title {:?}, author {:?}, last saved by {:?}, created {:?}, modified {:?}, \
                 {} custom properties",
                s.title,
                s.author,
                s.last_saved_by,
                s.create,
                s.modified,
                s.custom.len()
            );
        }
        if let Some(p) = &d.preview {
            let size = |i: &Option<Vec<u8>>| i.as_ref().map(Vec::len);
            println!(
                "preview: header {} bytes, bmp {:?}, wmf {:?}, png {:?} bytes",
                p.header.len(),
                size(&p.bmp),
                size(&p.wmf),
                size(&p.png)
            );
        }
        if let Some(a) = &d.app_info {
            let texts: Vec<&str> = a.entries.iter().map(|e| e.text.as_str()).collect();
            println!("app info: {:?} {texts:?}", a.name);
        }
        println!("{} objects", d.objects().count());
        for (h, obj) in d.objects() {
            match describe(obj) {
                Some(fields) => println!("{h} {} {fields}", obj.type_name()),
                None => println!("{h} {}", obj.type_name()),
            }
        }
        for diag in d.diagnostics() {
            let at = diag.handle.map(|h| format!(" {h}")).unwrap_or_default();
            println!("{:?}{at}: {}", diag.severity, diag.message);
        }
    }
    ExitCode::SUCCESS
}
