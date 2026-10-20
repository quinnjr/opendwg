//! What an application does with opendwg, end to end: open a drawing, list
//! its layers and model space, add a layer and a line, save it as AutoCAD
//! 2018 and open the result again; then recover an AutoCAD 2004 drawing
//! whose unconvertible objects block saving, by removing them together.
//!
//! ```sh
//! cargo run --example app_workflow [drawing.dwg [old-r2004.dwg]]
//! ```
//!
//! Without arguments it uses the sample corpus (`scripts/fetch-corpus.sh`),
//! or a new drawing when the corpus is not fetched.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use opendwg::object::{
    EntityCommon, Layer, Line, MaterialRef, NonGraphical, Object, XDataItem, XRecordValue,
};
use opendwg::{Drawing, Error, Handle, Severity};

fn corpus(name: &str) -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("corpus/files")
        .join(name);
    p.is_file().then_some(p)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let drawing = args
        .next()
        .map(PathBuf::from)
        .or_else(|| corpus("acadsharp-sample-ac1032.dwg"));
    let old = args
        .next()
        .map(PathBuf::from)
        .or_else(|| corpus("acadsharp-sample-ac1018.dwg"));
    let out = std::env::temp_dir().join(format!("opendwg-app-workflow-{}", std::process::id()));
    std::fs::create_dir_all(&out)?;

    edit_and_save(drawing.as_deref(), &out.join("edited.dwg"))?;
    match old {
        Some(old) => recover(&old, &out.join("recovered.dwg"))?,
        None => println!("no R2004 drawing given and no corpus: recovery skipped"),
    }
    std::fs::remove_dir_all(&out)?;
    Ok(())
}

/// Open (or create), list, add a layer and a line, save, reopen.
fn edit_and_save(path: Option<&Path>, out: &Path) -> opendwg::Result<()> {
    let mut d = match path {
        Some(p) => Drawing::open(p)?,
        None => Drawing::new(),
    };
    match d.source_version() {
        Some(v) => println!(
            "opened {}: {v}",
            path.map_or("?".into(), |p| p.display().to_string())
        ),
        None => println!("new drawing"),
    }
    for diag in d
        .diagnostics()
        .iter()
        .filter(|x| x.severity == Severity::Error)
    {
        println!("  {diag}");
    }
    for (_, layer) in d.layers() {
        println!(
            "  layer {:?}{}",
            layer.entry.name,
            if layer.off { " (off)" } else { "" }
        );
    }
    let ms = d.model_space();
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for h in d.entities_in(ms) {
        if let Some(o) = d.get(h) {
            *kinds.entry(o.type_name().into_owned()).or_default() += 1;
        }
    }
    println!("  model space: {kinds:?}");

    let walls = match d.layer_by_name("Walls") {
        Some(h) => h,
        None => d.add_layer(Layer::new("Walls"))?,
    };
    if let Some(layer) = d.layer_mut(walls) {
        layer.color = opendwg::Color::Index(1);
    }
    let line = d.add_entity(
        ms,
        EntityCommon::new(walls),
        Line::new([0.0, 0.0, 0.0], [10.0, 5.0, 0.0]).into(),
    )?;
    let warnings = match d.save_with_diagnostics(out) {
        // An old drawing may hold objects AutoCAD 2018 cannot store.
        Err(Error::UnconvertibleObjects(objects)) => {
            remove_unconvertible(&mut d, &objects)?;
            d.save_with_diagnostics(out)?
        }
        r => r?,
    };
    for warning in warnings {
        println!("  saved with: {warning}");
    }

    let back = Drawing::open(out)?;
    let (common, _) = back.entity(line).ok_or(Error::NotFound(line))?;
    assert_eq!(common.layer, walls);
    assert_eq!(
        back.layer(walls).map(|l| l.entry.name.as_str()),
        Some("Walls")
    );
    let errors = back
        .validate()
        .into_iter()
        .filter(|x| x.severity == Severity::Error)
        .count();
    assert_eq!(errors, 0);
    println!(
        "  saved and reopened: {} entities in model space, line {line} on layer Walls",
        back.entities_in(back.model_space()).count()
    );
    Ok(())
}

/// Save an old drawing whose unconvertible objects block saving: remove
/// them as one set, after releasing what outside the set still uses them.
fn recover(path: &Path, out: &Path) -> opendwg::Result<()> {
    let mut d = Drawing::open(path)?;
    println!(
        "opened {}: {}",
        path.display(),
        d.source_version().map_or("?".into(), |v| v.to_string())
    );
    let objects = match d.save(out) {
        Ok(()) => {
            println!("  saved as is");
            return Ok(());
        }
        Err(Error::UnconvertibleObjects(objects)) => objects,
        Err(e) => return Err(e),
    };
    remove_unconvertible(&mut d, &objects)?;
    d.save(out)?;
    let back = Drawing::open(out)?;
    let errors = back
        .validate()
        .into_iter()
        .filter(|x| x.severity == Severity::Error)
        .count();
    assert_eq!(errors, 0);
    println!(
        "  removed {} objects, saved and reopened as {}",
        objects.len(),
        back.source_version().map_or("?".into(), |v| v.to_string())
    );
    Ok(())
}

/// Remove the unconvertible `objects` as one set, after releasing what
/// outside the set still uses them.
fn remove_unconvertible(d: &mut Drawing, objects: &[(Handle, String)]) -> opendwg::Result<()> {
    println!("  {}", Error::UnconvertibleObjects(objects.to_vec()));
    let handles: Vec<Handle> = objects.iter().map(|&(h, _)| h).collect();
    match d.remove_all(&handles) {
        Ok(_) => Ok(()),
        Err(Error::InUse { objects, .. }) => {
            println!(
                "  {} objects outside the set use them; releasing",
                objects.len()
            );
            let gone: HashSet<Handle> = handles.iter().copied().collect();
            for referrer in objects {
                if let Some(obj) = d.get_mut(referrer) {
                    release(obj, &gone);
                }
            }
            d.remove_all(&handles).map(|_| ())
        }
        Err(e) => Err(e),
    }
}

/// Point `obj`'s references to removed objects elsewhere (as in the
/// README): material back to ByLayer, no color book, null extended data
/// handles and XRECORD object ids.
fn release(obj: &mut Object, gone: &HashSet<Handle>) {
    if let Object::Entity { common, .. } = obj {
        if matches!(common.material, MaterialRef::Handle(m) if gone.contains(&m)) {
            common.material = MaterialRef::ByLayer;
        }
        if common.color_book.is_some_and(|b| gone.contains(&b)) {
            common.color_book = None;
        }
    }
    if let Object::Other {
        object: NonGraphical::XRecord(x),
        ..
    } = obj
    {
        for (_, value) in &mut x.items {
            if matches!(value, XRecordValue::ObjectId(h) if gone.contains(h)) {
                *value = XRecordValue::ObjectId(Handle(0));
            }
        }
    }
    for app in &mut obj.object_common_mut().xdata {
        for item in &mut app.items {
            if matches!(item, XDataItem::Handle(h) if gone.contains(h)) {
                *item = XDataItem::Handle(Handle(0));
            }
        }
    }
}
