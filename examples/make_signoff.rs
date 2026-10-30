//! Write the AutoCAD sign-off pack: the drawings of
//! `tests/common/pack.rs`, the originals they were made from, untouched
//! copies, and `SIGNOFF.md` filled in from `scripts/SIGNOFF-TEMPLATE.md`.
//! Run it through `scripts/make-signoff.sh`:
//!
//! ```text
//! make_signoff <out dir> [commit] [date]
//! ```
//!
//! Every drawing is read back before it is written out: one that does not
//! read back with no diagnostic and no validation error stops the run, and
//! so does a drawing left out for any reason but an unfetched corpus (a
//! corpus file that does not read or save), as in `tests/signoff.rs`.

use std::path::Path;
use std::process::ExitCode;

#[path = "../tests/common/mod.rs"]
mod common;

use common::pack::{self, Kind};

const TEMPLATE: &str = include_str!("../scripts/SIGNOFF-TEMPLATE.md");

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

fn run(out: &Path, commit: &str, date: &str) -> Result<(), String> {
    let pack = pack::build(pack::LINES);
    if !pack.errors.is_empty() {
        return Err(pack.errors.join("\n"));
    }
    let failed: Vec<String> = pack
        .skipped
        .iter()
        .filter(|(_, why)| !why.contains("fetch-corpus"))
        .map(|(name, why)| format!("{name}: {why}"))
        .collect();
    if !failed.is_empty() {
        return Err(failed.join("\n"));
    }
    for f in &pack.files {
        pack::reads_back_clean(f)?;
    }
    let originals = out.join("originals");
    let untouched = out.join("untouched");
    for dir in [out, &originals, &untouched] {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    for f in &pack.files {
        write(&out.join(&f.name), &f.bytes)?;
        write(&untouched.join(&f.name), &f.bytes)?;
        if let Kind::Corpus(p) | Kind::Edit(Some(p)) = &f.kind {
            let to = originals.join(p.file_name().ok_or("no file name")?);
            std::fs::copy(p, &to).map_err(|e| format!("{}: {e}", to.display()))?;
        }
        println!("{:>10} bytes  {}", f.bytes.len(), f.name);
    }
    for (name, why) in &pack.skipped {
        println!("   skipped  {name}: {why}");
    }
    let md = pack::signoff_markdown(&pack, TEMPLATE, commit, date);
    write(&out.join("SIGNOFF.md"), md.as_bytes())?;
    println!("wrote {}", out.join("SIGNOFF.md").display());
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(out) = args.first() else {
        eprintln!("usage: make_signoff <out dir> [commit] [date]");
        return ExitCode::FAILURE;
    };
    let commit = args.get(1).map_or("unknown", String::as_str);
    let date = args.get(2).map_or("unknown", String::as_str);
    match run(Path::new(out), commit, date) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("make_signoff: {e}");
            ExitCode::FAILURE
        }
    }
}
