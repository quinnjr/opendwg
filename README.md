# opendwg

A pure-Rust reader and writer for AutoCAD DWG files. It reads AutoCAD 2000
to 2018 drawings (AC1015-AC1032) into a typed, editable model (header
variables, classes, symbol tables, blocks, entities and non-graphical
objects) and writes AutoCAD 2018 (AC1032). Objects it does not model, such
as custom objects from ARX applications, are kept and written back as they
were. It has no `unsafe` code and no dependencies beyond `encoding_rs`,
`thiserror` and (optionally) `serde`. Licensed under MIT or Apache-2.0.

## Supported versions

| Release      | Magic  | Read | Write |
|--------------|--------|------|-------|
| AutoCAD 2000 | AC1015 | yes  | no    |
| AutoCAD 2004 | AC1018 | yes  | no    |
| AutoCAD 2007 | AC1021 | yes  | no    |
| AutoCAD 2010 | AC1024 | yes  | no    |
| AutoCAD 2013 | AC1027 | yes  | no    |
| AutoCAD 2018 | AC1032 | yes  | yes   |

Every drawing is written as AutoCAD 2018, the current format. Any other
version (R13, R14, ...) is `Error::UnsupportedVersion`, and a
password-protected drawing is `Error::Encrypted`. DXF is not supported.

## Usage

```rust,no_run
use opendwg::object::{EntityCommon, Layer, Line};
use opendwg::{Drawing, Severity};

fn main() -> opendwg::Result<()> {
    // Read any supported version.
    let mut d = Drawing::open("plan.dwg")?;
    println!("{:?}", d.source_version()); // e.g. Some(R2004)
    for diag in d.diagnostics() {
        if diag.severity == Severity::Error {
            eprintln!("{diag}");
        }
    }
    for (handle, layer) in d.layers() {
        println!("{handle}: {}", layer.entry.name);
    }
    for h in d.entities_in(d.model_space()) {
        if let Some((common, entity)) = d.entity(h) {
            println!("{h} on layer {}: {entity:?}", common.layer);
        }
    }

    // Edit it: a new layer and a line on it.
    let ms = d.model_space();
    let walls = d.add_layer(Layer::new("Walls"))?;
    d.add_entity(
        ms,
        EntityCommon::new(walls),
        Line::new([0.0, 0.0, 0.0], [10.0, 5.0, 0.0]).into(),
    )?;

    // Save it as AutoCAD 2018, with the warnings about what the write left
    // out (sections it cannot carry over, ...), for the user.
    for warning in d.save_with_diagnostics("plan-2018.dwg")? {
        eprintln!("{warning}");
    }

    // Or start from nothing: a complete drawing with the standard tables,
    // model and paper space, layouts and default styles.
    let new = Drawing::new();
    let bytes: Vec<u8> = new.to_bytes()?;
    std::fs::write("new.dwg", bytes)?;
    Ok(())
}
```

`Layer::new`, `Line::new`, `Circle::new`, `Text::new`, `MText::new` and
`Insert::new` give the common defaults; `Drawing::layer`, `block_record` and
`entity` (and their `_mut` forms) give typed access by handle.
`examples/app_workflow.rs` runs this end to end, with the recovery of an
old drawing described below (`cargo run --example app_workflow`).

Reading is fault tolerant about objects: one that cannot be decoded is kept
as raw bytes with a diagnostic (or, when even its common data is
unreadable, kept as stored and unwritable). A broken container (file
header, section maps), header section, classes section or object map (any
of its pages) fails the read.

Writing never drops data silently. What it cannot carry over (an unknown
section of an old file, a damaged preview, ...) is left out with a warning:
`Drawing::to_bytes_with_diagnostics` and `Drawing::save_with_diagnostics`
return those warnings with the file, and `Drawing::write_diagnostics` lists
them before saving. `to_bytes` and `save` write the same file without them.
The writer also adds what AutoCAD requires and other programs' files lack
(the dimension style `Standard`, the `ACAD_GROUP` and `ACAD_PLOTSETTINGS`
dictionaries), sets a null owner to the one object that lists it, and gives
an entity the block that lists it as owner, each with a warning; the drawing
itself is not changed. An object two objects list keeps its null owner
(which one owns it is not known): validation refuses it, but only warns
when the object, or every object listing it, is kept raw. `Drawing::validate`
checks ownership, references, handles and required tables; the writer runs it
before every save. `Drawing::remove` refuses to delete an object something
still refers to (`Error::InUse`).

## Objects that cannot be converted

Objects opendwg does not model are kept raw. Raw objects read from an
AutoCAD 2007 or later file already have the layout AutoCAD 2018 needs and
are written back byte for byte. Raw objects read from an AutoCAD 2000 or
2004 file store their text differently, and without knowing their layout
they cannot be converted safely, so saving such a drawing fails:

```rust,no_run
use std::collections::HashSet;

use opendwg::object::{MaterialRef, NonGraphical, Object, XDataItem, XRecordValue};
use opendwg::{Drawing, Error, Handle};

fn main() -> opendwg::Result<()> {
    let mut d = Drawing::open("old-r2004.dwg")?;
    let objects = match d.to_bytes() {
        Ok(bytes) => {
            std::fs::write("converted.dwg", bytes)?;
            return Ok(());
        }
        // (handle, class name) of each object; nothing has been written.
        Err(Error::UnconvertibleObjects(objects)) => objects,
        Err(e) => return Err(e),
    };
    for (handle, class) in &objects {
        eprintln!("cannot convert {class} {handle}");
    }
    // The application decides: tell the user, or remove them and save.
    // They often refer to each other, so they are removed as one set.
    let handles: Vec<Handle> = objects.iter().map(|&(h, _)| h).collect();
    match d.remove_all(&handles) {
        Ok(_) => {}
        // Objects outside the set still use some of them (nothing was
        // removed): here, entities' materials and color books, extended
        // data handles (R2000 attaches materials that way) and XRECORD
        // object ids. Let go of them, then remove again.
        Err(Error::InUse { objects, .. }) => {
            let gone: HashSet<Handle> = handles.iter().copied().collect();
            for referrer in objects {
                if let Some(obj) = d.get_mut(referrer) {
                    release(obj, &gone);
                }
            }
            d.remove_all(&handles)?;
        }
        Err(e) => return Err(e),
    }
    d.save("converted.dwg")
}

/// Point `obj`'s references to removed objects elsewhere: an entity's
/// material back to ByLayer, no color book, null handles in its extended
/// data and (for an XRECORD) its object ids.
fn release(obj: &mut Object, gone: &HashSet<Handle>) {
    if let Object::Entity { common, .. } = obj {
        if matches!(common.material, MaterialRef::Handle(m) if gone.contains(&m)) {
            common.material = MaterialRef::ByLayer;
        }
        if common.color_book.is_some_and(|b| gone.contains(&b)) {
            common.color_book = None;
        }
    }
    if let Object::Other { object: NonGraphical::XRecord(x), .. } = obj {
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
```

`Drawing::remove_all` removes the set atomically: references between its
members, and objects one of them owns, do not block it; `Error::InUse` names
the objects outside the set that still refer to one of them (the R2000 and
R2004 sample drawings need the steps above, `tests/drawing_api.rs` checks it).
`Drawing::unconvertible_objects` lists them before saving. There is no lossy
mode: opendwg never drops data silently. Proxy objects and typed objects
convert normally; the few typed values AutoCAD 2018 has no form for are
listed the same way (for example AutoCAD 2000 3D solid SAT data, an
AutoCAD 2000 entity list whose chain is broken, an old-style table style, or
an object whose common data could not be read; see the documentation of `Drawing::unconvertible_objects`). An AutoCAD 2000
layout that names a plot view no VIEW has is saved with no plot view and a
warning (`Drawing::write_diagnostics`).

## Features

- `serde` (off by default): `Serialize` and `Deserialize` for `Drawing` and
  every public model type (header, classes, objects, entities, sections,
  diagnostics), for example to pass a drawing to a JavaScript front end as
  JSON. Objects are keyed by handle and extra sections by their section
  name. Not serialized: values kept only to write unchanged objects back
  bit for bit (the `AsRead` fields), the reference index (rebuilt on use)
  and what the writer keeps from the file read; a deserialized drawing is
  written like one built in memory. Deserializing gives back the same model
  (round trip tested on every corpus file); with `serde_json`, enable its
  `float_roundtrip` feature so coordinates come back exact. `Error`
  serializes (only) as `{ "kind", "message", "handles" }`, so a Tauri
  command can return it as is. The codec tools (`BitReader`, `BitWriter`,
  `ObjReader`, `ObjWriter`, `DecodeCtx`) are not serializable.

## Hostile input

Any bytes may be passed to `Drawing::from_bytes`: the reader never panics,
never loops without end and allocates no more than the input size allows.
`fuzz/` holds `cargo-fuzz` targets for the whole reader, the write round
trip, the container layer, single objects and each codec and section
decoder; see [Fuzzing](#fuzzing).

A drawing built from untrusted data some other way (edited through the API,
or deserialized with the `serde` feature) is checked when it is saved:
`to_bytes` and `save` run `Drawing::validate` and refuse a drawing with
error findings (`Error::Invalid`), and fail with an error on values a
field cannot hold (`Error::Encode`). A saved file is designed to always read
back, and this is fuzz-checked by `tests/serde.rs` on randomly edited JSON.

## Format reference and clean-room policy

opendwg is written from the Open Design Alliance's public *Open Design
Specification for .dwg files* (v5.4.1) and from observation of real files.
Where the specification is wrong or silent, permissively licensed
implementations (ACadSharp, MIT) were consulted; every such point is listed
in `THIRD_PARTY.md` and credited where it is used. No GPL or LGPL material
(LibreDWG code or documentation, or any other copyleft implementation) was
consulted, and contributions must not be based on any.

## License

Licensed under either of

- Apache License, Version 2.0 (`LICENSE-APACHE`)
- MIT license (`LICENSE-MIT`)

at your option. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in this crate, as defined in the
Apache-2.0 license, shall be dual licensed as above, without any additional
terms or conditions.

## Testing

```sh
scripts/fetch-corpus.sh   # the public sample drawings (see corpus/README.md)
cargo test
```

Tests that need the sample corpus skip when it is not fetched;
`OPENDWG_REQUIRE_CORPUS=1` makes them fail instead. The corpus is not in
the repository (the sample licenses do not allow redistribution in every
case): `corpus/manifest.toml` lists each file's URL and SHA-256, and the
script downloads and checks them into `corpus/files/` (gitignored).

```sh
cargo test --features serde                       # the serde tests too
OPENDWG_REQUIRE_CORPUS=1 cargo test --release -- --include-ignored
```

The ignored tests are the slow ones, such as the 200,000-entity round trip
(`large_drawing_roundtrip_is_linear`), meant for release builds.

`scripts/check-msrv.sh` builds the library, tests and examples, with and
without `serde`, on the crate's `rust-version` (1.85) with warnings as
errors: newer syntax such as let chains does not build there. It needs that
toolchain (`rustup toolchain install 1.85`).

CI (`.github/workflows/ci.yml`) runs on every pull request and on pushes
to `main` and `develop`: it fetches the corpus once, runs the release tests
(ignored ones included, with `OPENDWG_REQUIRE_CORPUS=1`) on Linux, Windows
and macOS, the `serde` tests on Linux, `rustfmt`, `clippy` and `rustdoc`
with warnings as errors, a build of the fuzz targets, and
`scripts/check-msrv.sh`.

### Fuzzing

`tests/mutation_fuzz.rs` runs in every `cargo test`: it mutates each corpus
file and a new drawing (bit flips, byte replacements, truncations, splices,
length fields) and reads and writes the result, failing on a panic or a
case that runs too long. `OPENDWG_MUTATION_CASES` (default 8 per file) and
`OPENDWG_MUTATION_SEED` scale and vary it.

Deeper fuzzing uses [`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz),
which needs a nightly toolchain:

```sh
rustup toolchain install nightly --profile minimal
cargo install cargo-fuzz
cd fuzz
cargo run --release --example seed_corpora    # seeds fuzz/corpus/ from corpus/files
cargo +nightly fuzz list
cargo +nightly fuzz run read_drawing -- -max_total_time=600 -rss_limit_mb=2048
```

The targets:

- whole files: `read_drawing`, `write_roundtrip` (read, write at a fixed
  save time, read again: the written file must read back and validate with
  no error) and `container` (slicing into sections only);
- `object`: one object through `decode_object`, in any version, with the
  classes of the drawing it came from; what decodes is encoded as R2018,
  which must decode and re-encode to the same bytes. The seeder makes one
  input per object of every sample drawing;
- one codec or section each: `bitreader`, `lz77_2004`, `lz77_2007`,
  `rs2007` (also checks that decoding the encoder's output gives the data
  back), `objmap`, `header`, `classes`, `summaryinfo` (with the app info
  section), `preview`, `acds` and `proxy_graphics`.

The seeder gives the whole-file targets small drawings only (64 KiB at
most): libFuzzer runs a target about as many times per second as its
inputs allow, and a 1 MB sample takes milliseconds to read. Keep inputs
small with `-max_len`, and shrink a corpus that has grown with `cmin`:

```sh
cargo +nightly fuzz run read_drawing -- -max_len=65536 -max_total_time=600
cargo +nightly fuzz cmin object
```

A crash is saved under `fuzz/artifacts/<target>/`; replay it with
`cargo +nightly fuzz run <target> <file>` and shrink it with
`cargo +nightly fuzz tmin <target> <file>`.

### ODA File Converter oracle (local opt-in)

`tests/oda.rs` converts every round-tripped corpus file and every drawing
made from scratch to DXF with the Open Design Alliance's free
[ODA File Converter](https://www.opendesign.com/guestfiles/oda_file_converter)
(AutoCAD 2018, audit on), and fails on any `.err` file or error it reports.
It is an independent check that the files opendwg writes are readable.

The converter is not bundled or downloaded. Its terms (the ODA Community User
Agreement, see `THIRD_PARTY.md`) are accepted by each person for themselves:
running it on your own machine is fine, running it in hosted or shared CI is
not clearly permitted. So the oracle is local opt-in only: the tests print
`SKIP` and pass when the converter is not installed, and never run it when
the `CI` environment variable is set. CI relies on `Drawing::validate` and
the manual AutoCAD sign-off below.

To run it, install the converter yourself, then:

```sh
ODA_FILE_CONVERTER=/path/to/ODAFileConverter \
  cargo test --release --test oda -- --nocapture
```

`ODAFileConverter` on `PATH` is found without the variable. If your
converter version takes its arguments differently, set
`ODA_FILE_CONVERTER_ARGS` (default `{in} {out} ACAD2018 DXF 0 1 *.dwg`;
`{in}` and `{out}` are the input and output folders). The value is split on
whitespace, without quoting, before the folders are put in: only `{in}` and
`{out}` may contain spaces. A run that takes longer than `ODA_TIMEOUT_SECS`
(default 120) is killed and fails. On Linux without a display (a server,
WSL without WSLg), add `QT_QPA_PLATFORM=offscreen`, or the converter may
fail to start or hang. `OPENDWG_REQUIRE_ODA=1` makes a missing converter a
failure.

### AutoCAD sign-off

```sh
scripts/fetch-corpus.sh
scripts/make-signoff.sh   # writes signoff/ (gitignored)
```

`scripts/make-signoff.sh` writes the drawings to check in AutoCAD on
Windows (every corpus round trip, new drawings, and edits whose result only
AutoCAD can confirm), the originals to compare with, untouched copies, and
`signoff/SIGNOFF.md`: a step-by-step checklist, written for someone who is
not a developer, with a place to record each result. The checklist comes
from `scripts/SIGNOFF-TEMPLATE.md` and the file list from
`tests/common/pack.rs`.

The script stops when the corpus is not fetched (`--allow-partial` makes a
pack without the corpus files) and keeps a pack whose `SIGNOFF.md` already
has results recorded (`--force` replaces it).
