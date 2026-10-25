//! Shared helpers for integration tests: the public sample corpus.
//!
//! The corpus files are not in the repository. Fetch them with `scripts/fetch-corpus.sh`.
#![allow(dead_code)]

pub mod every_entity;
pub mod model;
pub mod pack;
pub mod remove;

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;

use opendwg::Handle;
use opendwg::container::{self, SectionName};
use opendwg::section::objmap;

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub file: Vec<Entry>,
}

#[derive(Debug, Deserialize)]
pub struct Entry {
    pub name: String,
    pub url: String,
    pub sha256: String,
    /// Magic string, e.g. `"AC1032"`.
    pub version: String,
    pub features: Vec<String>,
    pub license_note: String,
    /// The program that wrote the file (`corpus/extended.toml` only).
    #[serde(default)]
    pub producer: Option<String>,
    /// Expected values, e.g. `object_count`.
    #[serde(default)]
    pub expect: BTreeMap<String, toml::Value>,
}

pub const FEATURES: [&str; 9] = [
    "hatch",
    "dimension",
    "attributes",
    "mtext",
    "image",
    "xref",
    "custom_objects",
    "proxy_graphics",
    "non_ascii",
];

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("corpus")
}

/// The corpus manifest; an empty one (with a skip, see [`skip`]) when it is
/// absent, as in the published crate, which leaves `corpus/` out.
pub fn load_manifest() -> Manifest {
    let Ok(text) = std::fs::read_to_string(corpus_dir().join("manifest.toml")) else {
        skip("corpus/manifest.toml");
        return Manifest { file: Vec::new() };
    };
    toml::from_str(&text).expect("corpus/manifest.toml parses")
}

/// The extended corpus manifest (`corpus/extended.toml`); an empty one (with
/// a skip, see [`skip`]) when it is absent.
pub fn load_extended() -> Manifest {
    let Ok(text) = std::fs::read_to_string(corpus_dir().join("extended.toml")) else {
        skip("corpus/extended.toml");
        return Manifest { file: Vec::new() };
    };
    toml::from_str(&text).expect("corpus/extended.toml parses")
}

/// Path of a fetched corpus file, or `None` when `scripts/fetch-corpus.sh` has not been run.
pub fn corpus_file(name: &str) -> Option<PathBuf> {
    let p = corpus_dir().join("files").join(format!("{name}.dwg"));
    p.is_file().then_some(p)
}

/// Report a corpus skip. With `OPENDWG_REQUIRE_CORPUS=1` a missing corpus is a failure
/// instead, so CI cannot pass vacuously.
pub fn skip(what: &str) {
    if std::env::var("OPENDWG_REQUIRE_CORPUS").as_deref() == Ok("1") {
        panic!("OPENDWG_REQUIRE_CORPUS=1 but the corpus lacks {what}; run scripts/fetch-corpus.sh");
    }
    eprintln!("SKIPPED: the corpus lacks {what}; run scripts/fetch-corpus.sh");
}

/// [`corpus_file`], reporting a skip (see [`skip`]) when the file is missing.
pub fn corpus_file_or_skip(name: &str) -> Option<PathBuf> {
    let p = corpus_file(name);
    if p.is_none() {
        skip(name);
    }
    p
}

/// All fetched corpus files as `(entry, path)`; unfetched entries are skipped.
pub fn fetched_files() -> Vec<(Entry, PathBuf)> {
    load_manifest()
        .file
        .into_iter()
        .filter_map(|e| corpus_file(&e.name).map(|p| (e, p)))
        .collect()
}

/// Like [`fetched_files`] restricted to one version magic. Reports a skip (see [`skip`])
/// when nothing is fetched, so an unfetched corpus is visible rather than a silent pass.
pub fn fetched_files_for(version: &str) -> Vec<(Entry, PathBuf)> {
    let v: Vec<_> = fetched_files()
        .into_iter()
        .filter(|(e, _)| e.version == version)
        .collect();
    if v.is_empty() {
        skip(&format!("{version} files"));
    }
    v
}

/// Like [`fetched_files`], but reports a skip (see [`skip`]) when the result is empty.
pub fn fetched_files_or_skip() -> Vec<(Entry, PathBuf)> {
    let v = fetched_files();
    if v.is_empty() {
        skip("any files");
    }
    v
}

/// The bytes of every object, by handle, as stored in the file.
pub fn object_bytes(bytes: &[u8]) -> BTreeMap<Handle, Vec<u8>> {
    let raw = container::read(bytes).unwrap();
    let objects = &raw.sections[&SectionName::Objects];
    let map = objmap::decode(&raw.sections[&SectionName::Handles]).unwrap();
    map.into_iter()
        .map(|(h, loc)| {
            let at = &objects[loc as usize..];
            let (_, pre) =
                opendwg::object::ObjReader::split(at, raw.version, raw.codepage).unwrap();
            // MS size, MC handle stream size (R2010+), the bytes and the CRC.
            let mut r = opendwg::bits::BitReader::new(at);
            r.read_ms().unwrap();
            if raw.version >= opendwg::Version::R2010 {
                r.read_umc().unwrap();
            }
            let head = (r.bit_pos() / 8) as usize;
            (h, at[..head + pre.size_bytes as usize + 2].to_vec())
        })
        .collect()
}
