//! Sets `opendwg_fixtures` when the test-only fixture directories are present.
//! They are excluded from the published crate (see `Cargo.toml` and
//! THIRD_PARTY.md), so the unit tests that read them compile only in a
//! checkout of the repository.

use std::path::Path;

const FIXTURES: [&str; 2] = ["src/object/testdata", "src/section/testdata"];

fn main() {
    println!("cargo::rustc-check-cfg=cfg(opendwg_fixtures)");
    // A missing path would make Cargo rerun the script on every build; the
    // directories are absent from the published crate.
    println!("cargo::rerun-if-changed=build.rs");
    for dir in FIXTURES.iter().filter(|d| Path::new(d).is_dir()) {
        println!("cargo::rerun-if-changed={dir}");
    }
    if FIXTURES.iter().all(|d| Path::new(d).is_dir()) {
        println!("cargo::rustc-cfg=opendwg_fixtures");
    }
}
