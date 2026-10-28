//! The ODA File Converter as an independent oracle. Every
//! round-tripped corpus file and every drawing made from scratch (the
//! sign-off pack, `tests/common/pack.rs`) is converted DWG -> DXF
//! (AutoCAD 2018, audit on) by the converter; the test fails on any `.err`
//! file it writes, any error line in its output, a missing or empty DXF, or
//! a failed run.
//!
//! **Local opt-in only.** The converter is free software from the Open
//! Design Alliance under its Community User Agreement, which each person
//! accepts for themselves (see THIRD_PARTY.md, "ODA File Converter"):
//! running it on a developer machine is fine, running it in hosted or
//! shared CI is not clearly permitted. So these tests never download it,
//! skip when the `CI` environment variable is set, and skip with a printed
//! `SKIP` message when the converter is not installed. CI relies on
//! `Drawing::validate` and the manual AutoCAD sign-off
//! (`scripts/make-signoff.sh`) instead.
//!
//! Configuration (environment variables):
//!
//! - `ODA_FILE_CONVERTER`: the converter program. Default: `ODAFileConverter`
//!   searched on `PATH`. When set, it must exist (a wrong path fails rather
//!   than skips).
//! - `ODA_FILE_CONVERTER_ARGS`: the arguments, with `{in}` and `{out}`
//!   standing for the input and output folders. Default:
//!   `{in} {out} ACAD2018 DXF 0 1 *.dwg`, the vendor's documented order
//!   (input folder, output folder, output version, output type, recurse,
//!   audit, input filter). Change it if your converter version differs. The
//!   value is split on whitespace before `{in}` and `{out}` are replaced:
//!   folders with spaces are fine, but no other argument can hold a space
//!   (no quoting is supported).
//! - `ODA_TIMEOUT_SECS`: how long one converter run may take before it is
//!   killed and the test fails (default 120).
//! - `OPENDWG_REQUIRE_ODA=1`: fail instead of skipping when the converter is
//!   absent.
//!
//! The converter is a Qt application: on a Linux machine without a display
//! (a server, WSL without WSLg, a container), set `QT_QPA_PLATFORM=offscreen`
//! (passed on to the converter), or run the tests under `xvfb-run`;
//! otherwise it may exit at once with "could not connect to display" (a
//! failure) or hang (killed after `ODA_TIMEOUT_SECS`).
//!
//! The converter's log is scanned for error lines with simple word patterns
//! (see [`error_lines`]), written without a real log at hand: tune them
//! against the first real run's output.
//!
//! ```text
//! QT_QPA_PLATFORM=offscreen ODA_FILE_CONVERTER=/usr/bin/ODAFileConverter \
//!     cargo test --release --test oda -- --nocapture
//! ```

mod common;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::pack::{self, PackFile};

/// The default arguments (see the module documentation).
const DEFAULT_ARGS: &str = "{in} {out} ACAD2018 DXF 0 1 *.dwg";

/// The converter's program name on `PATH`.
const PROGRAM: &str = if cfg!(windows) {
    "ODAFileConverter.exe"
} else {
    "ODAFileConverter"
};

/// The default time one converter run may take.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// How to run the converter.
#[derive(Debug, Clone, PartialEq)]
struct Converter {
    program: PathBuf,
    args: String,
    timeout: Duration,
}

/// The converter from the environment values given (`ODA_FILE_CONVERTER`,
/// `ODA_FILE_CONVERTER_ARGS`, `PATH`, `CI`), or why it is not run.
fn find_converter(
    program: Option<&str>,
    args: Option<&str>,
    path: Option<&str>,
    ci: Option<&str>,
) -> Result<Converter, String> {
    if ci.is_some_and(|v| !v.is_empty() && v != "0" && v != "false") {
        return Err(
            "CI is set: the ODA File Converter is local opt-in only (see THIRD_PARTY.md)".into(),
        );
    }
    let args = args.unwrap_or(DEFAULT_ARGS).to_owned();
    if let Some(p) = program.filter(|p| !p.is_empty()) {
        let p = PathBuf::from(p);
        // A program given by name only is looked up on PATH.
        let found = if p.components().count() == 1 {
            search_path(p.to_str().unwrap_or_default(), path)
        } else {
            p.is_file().then_some(p.clone())
        };
        return match found {
            Some(program) => Ok(Converter {
                program,
                args,
                timeout: DEFAULT_TIMEOUT,
            }),
            None => panic!(
                "ODA_FILE_CONVERTER is set to {}, which does not exist",
                p.display()
            ),
        };
    }
    search_path(PROGRAM, path)
        .map(|program| Converter {
            program,
            args,
            timeout: DEFAULT_TIMEOUT,
        })
        .ok_or_else(|| format!("{PROGRAM} is not on PATH and ODA_FILE_CONVERTER is not set"))
}

/// The run timeout an `ODA_TIMEOUT_SECS` value gives (whole seconds, more
/// than 0); [`DEFAULT_TIMEOUT`] when unset. A bad value fails.
fn timeout_from(v: Option<&str>) -> Duration {
    match v.filter(|v| !v.is_empty()) {
        None => DEFAULT_TIMEOUT,
        Some(v) => match v.trim().parse::<u64>() {
            Ok(n) if n > 0 => Duration::from_secs(n),
            _ => panic!("ODA_TIMEOUT_SECS={v:?} is not a whole number of seconds above 0"),
        },
    }
}

/// `name` in the directories of `path` (a `PATH` value).
fn search_path(name: &str, path: Option<&str>) -> Option<PathBuf> {
    std::env::split_paths(path?)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

/// The converter from this process's environment, or `None` after printing
/// why the test is skipped (a failure with `OPENDWG_REQUIRE_ODA=1`).
fn converter_or_skip() -> Option<Converter> {
    let var = |k: &str| std::env::var(k).ok();
    match find_converter(
        var("ODA_FILE_CONVERTER").as_deref(),
        var("ODA_FILE_CONVERTER_ARGS").as_deref(),
        var("PATH").as_deref(),
        var("CI").as_deref(),
    ) {
        Ok(c) => Some(Converter {
            timeout: timeout_from(var("ODA_TIMEOUT_SECS").as_deref()),
            ..c
        }),
        Err(why) => {
            if var("OPENDWG_REQUIRE_ODA").as_deref() == Some("1") {
                panic!("OPENDWG_REQUIRE_ODA=1 but the converter is not run: {why}");
            }
            eprintln!("SKIP: ODA File Converter oracle: {why}");
            None
        }
    }
}

/// The argument list: `template` split on whitespace, then `{in}` and
/// `{out}` replaced by the folders (which may hold spaces).
fn expand_args(template: &str, input: &Path, output: &Path) -> Vec<String> {
    template
        .split_whitespace()
        .map(|a| {
            a.replace("{in}", &input.to_string_lossy())
                .replace("{out}", &output.to_string_lossy())
        })
        .collect()
}

/// The lines of the converter's output that report an error: those with a
/// word starting "error" or "fail" (any case) that no zero count goes with
/// ("0 errors", "no errors", "Errors: 0", "Audit errors: 0", "Total errors
/// found 0 fixed 0", "failed: 0"). These patterns were written without a
/// real converter log: tune them against the first real run's output.
fn error_lines(log: &str) -> Vec<String> {
    log.lines()
        .filter(|l| reports_problem(l))
        .map(|l| l.trim().to_owned())
        .collect()
}

fn reports_problem(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    let words: Vec<&str> = l
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let zero = |w: Option<&&str>| matches!(w.copied(), Some("0" | "no" | "none" | "zero"));
    words.iter().enumerate().any(|(i, w)| {
        if !(w.starts_with("error") || w.starts_with("fail")) {
            return false;
        }
        let before = i.checked_sub(1).and_then(|j| words.get(j));
        let after = words.get(i + 1);
        let counted =
            matches!(after.copied(), Some("found" | "count" | "total")) && zero(words.get(i + 2));
        !(zero(before) || zero(after) || counted)
    })
}

/// `spawn`, tried again for up to a second while it fails with "text file
/// busy" (ETXTBSY): a program just written (the stand-in converter scripts
/// of the tests) cannot be run while another thread's child, forked
/// between its write and its close, still holds it open for writing. That
/// child execs or exits at once, so the wait is short.
fn spawn_retrying(
    mut spawn: impl FnMut() -> std::io::Result<std::process::Child>,
) -> std::io::Result<std::process::Child> {
    const ETXTBSY: i32 = 26;
    let start = Instant::now();
    loop {
        match spawn() {
            Err(e)
                if cfg!(unix)
                    && e.raw_os_error() == Some(ETXTBSY)
                    && start.elapsed() < Duration::from_secs(1) =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            r => return r,
        }
    }
}

/// Run `conv` with `args`, killing it after its timeout: its exit status
/// (`None` when killed) and its output (stdout, then stderr).
fn run_with_timeout(
    conv: &Converter,
    args: &[String],
) -> std::io::Result<(Option<std::process::ExitStatus>, String)> {
    let mut child = spawn_retrying(|| {
        Command::new(&conv.program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
    })?;
    // Read both pipes on threads, so a chatty converter never blocks. A
    // process the converter started may keep a pipe open after it is
    // killed: the output is then waited for a few seconds only.
    let reader = |pipe: Option<Box<dyn Read + Send>>| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut text = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut text);
            }
            let _ = tx.send(String::from_utf8_lossy(&text).into_owned());
        });
        rx
    };
    let out = reader(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let err = reader(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if start.elapsed() >= conv.timeout {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    let text = |rx: std::sync::mpsc::Receiver<String>| {
        rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap_or_else(|_| "(output still open after the converter ended)".into())
    };
    let log = format!("{}\n{}", text(out), text(err));
    Ok((status, log))
}

/// Convert `files` in one run of `conv` in a fresh folder under `work`;
/// the problems found, one per line (empty: all accepted).
fn convert(conv: &Converter, files: &[PackFile], work: &Path) -> Vec<String> {
    let (input, output) = (work.join("in"), work.join("out"));
    let _ = std::fs::remove_dir_all(work);
    std::fs::create_dir_all(&input).unwrap();
    std::fs::create_dir_all(&output).unwrap();
    for f in files {
        std::fs::write(input.join(&f.name), &f.bytes).unwrap();
    }
    let args = expand_args(&conv.args, &input, &output);
    let mut problems = Vec::new();
    match run_with_timeout(conv, &args) {
        Err(e) => problems.push(format!("{} could not run: {e}", conv.program.display())),
        Ok((status, log)) => {
            match status {
                None => problems.push(format!(
                    "the converter did not finish within {} s and was killed (ODA_TIMEOUT_SECS; \
                     without a display set QT_QPA_PLATFORM=offscreen)",
                    conv.timeout.as_secs()
                )),
                Some(s) if !s.success() => problems.push(format!("the converter exited with {s}")),
                Some(_) => {}
            }
            problems.extend(error_lines(&log).into_iter().map(|l| format!("log: {l}")));
        }
    }
    let mut written: Vec<PathBuf> = std::fs::read_dir(&output)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    written.sort();
    for p in &written {
        if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("err")) {
            let text = std::fs::read_to_string(p).unwrap_or_default();
            problems.push(format!(
                "{}: {}",
                p.file_name().unwrap().to_string_lossy(),
                text.trim()
            ));
        }
    }
    for f in files {
        let stem = Path::new(&f.name).file_stem().unwrap().to_string_lossy();
        let dxf = written.iter().find(|p| {
            p.file_stem().is_some_and(|s| s.to_string_lossy() == stem)
                && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dxf"))
        });
        match dxf.map(|p| std::fs::metadata(p).map_or(0, |m| m.len())) {
            Some(n) if n > 0 => {}
            Some(_) => problems.push(format!("{}: the DXF written is empty", f.name)),
            None => problems.push(format!("{}: no DXF written", f.name)),
        }
    }
    problems
}

/// Convert `files`, failing with every problem found.
fn assert_accepted(files: &[PackFile], what: &str) {
    let Some(conv) = converter_or_skip() else {
        return;
    };
    assert!(!files.is_empty(), "{what}: nothing to convert");
    let work = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("oda")
        .join(what);
    let problems = convert(&conv, files, &work);
    assert!(
        problems.is_empty(),
        "{what}: the ODA File Converter rejected opendwg's output (files in {}):\n{}",
        work.display(),
        problems.join("\n")
    );
    eprintln!("{what}: {} files accepted", files.len());
}

/// Every corpus file the round trip writes (R2000/R2004 ones with their
/// unconvertible objects removed), and the edits of the sign-off pack.
#[test]
fn oda_accepts_roundtripped_corpus() {
    if converter_or_skip().is_none() {
        return;
    }
    let mut p = pack::Pack::default();
    pack::corpus_files(&mut p);
    pack::edit_files(&mut p);
    // As the sign-off pack checks: nothing failed to build, and a file is
    // left out only for a corpus not fetched.
    assert!(p.errors.is_empty(), "{:?}", p.errors);
    for (name, why) in &p.skipped {
        assert!(why.contains("fetch-corpus"), "{name}: {why}");
    }
    let files: Vec<PackFile> = p
        .files
        .into_iter()
        .filter(|f| f.name != "edit_dimlfac.dwg")
        .collect();
    if files.is_empty() {
        common::skip("any files");
        return;
    }
    assert_accepted(&files, "corpus");
}

/// The drawings made from scratch: empty, every entity kind, 200k lines,
/// and the DIMLFAC dimension edit.
#[test]
fn oda_accepts_new_drawings() {
    if converter_or_skip().is_none() {
        return;
    }
    let mut p = pack::Pack::default();
    pack::new_files(&mut p, pack::LINES);
    pack::edit_files(&mut p);
    let files: Vec<PackFile> = p
        .files
        .into_iter()
        .filter(|f| matches!(f.kind, pack::Kind::New) || f.name == "edit_dimlfac.dwg")
        .collect();
    assert_eq!(files.len(), 4);
    assert_accepted(&files, "new");
}

// ---- the harness itself (runs without the converter) ----

#[test]
fn args_follow_the_template() {
    let (i, o) = (Path::new("/t/in dir"), Path::new("/t/out"));
    assert_eq!(
        expand_args(DEFAULT_ARGS, i, o),
        ["/t/in dir", "/t/out", "ACAD2018", "DXF", "0", "1", "*.dwg"]
    );
    assert_eq!(
        expand_args("-i={in} -o={out} DXF", i, o),
        ["-i=/t/in dir", "-o=/t/out", "DXF"]
    );
}

#[test]
fn error_lines_skip_zero_counts() {
    let log = "Processing a.dwg\nTotal errors found 0 fixed 0\n0 errors\nNo errors found\n\
               Errors: 0\nAudit errors: 0\nFailed: 0, converted: 3\n\
               Error: invalid handle in b.dwg\nConversion FAILED for c.dwg\n\
               Errors: 2\n";
    assert_eq!(
        error_lines(log),
        [
            "Error: invalid handle in b.dwg",
            "Conversion FAILED for c.dwg",
            "Errors: 2"
        ]
    );
    assert!(error_lines("Done.\n").is_empty());
}

#[test]
fn converter_lookup() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("oda-lookup");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.to_string_lossy().into_owned();
    // Absent: a skip reason.
    assert!(find_converter(None, None, Some(&path), None).is_err());
    std::fs::write(dir.join(PROGRAM), "").unwrap();
    // On PATH, with the default arguments.
    let found = find_converter(None, None, Some(&path), None).unwrap();
    assert_eq!(found.program, dir.join(PROGRAM));
    assert_eq!(found.args, DEFAULT_ARGS);
    // Named by the variable, with other arguments.
    let named = dir.join(PROGRAM).to_string_lossy().into_owned();
    let c = find_converter(Some(&named), Some("{in} {out}"), None, None).unwrap();
    assert_eq!(
        (c.program, c.args.as_str()),
        (dir.join(PROGRAM), "{in} {out}")
    );
    // CI: never run.
    let why = find_converter(Some(&named), None, Some(&path), Some("true")).unwrap_err();
    assert!(why.contains("THIRD_PARTY"), "{why}");
    assert!(find_converter(None, None, Some(&path), Some("")).is_ok());
}

#[test]
fn timeout_setting() {
    assert_eq!(timeout_from(None), DEFAULT_TIMEOUT);
    assert_eq!(timeout_from(Some("")), DEFAULT_TIMEOUT);
    assert_eq!(timeout_from(Some("5")), Duration::from_secs(5));
    for bad in ["0", "-3", "two"] {
        let r = std::panic::catch_unwind(|| timeout_from(Some(bad)));
        assert!(r.is_err(), "{bad}");
    }
}

/// A converter that hangs is killed after the timeout, and that is a
/// failure.
#[cfg(unix)]
#[test]
fn hung_converter_is_killed() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("oda-hang");
    let conv = Converter {
        program: "/bin/sh".into(),
        args: "-c sleep${IFS}30".into(),
        timeout: Duration::from_secs(1),
    };
    let start = Instant::now();
    let problems = convert(&conv, &[], &dir);
    assert!(start.elapsed() < Duration::from_secs(10));
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].contains("did not finish within 1 s"),
        "{problems:?}"
    );
}

#[test]
#[should_panic(expected = "does not exist")]
fn converter_named_but_missing_fails() {
    let _ = find_converter(Some("/no/such/ODAFileConverter"), None, None, None);
}

/// A stand-in converter (a shell script) that writes a DXF per input, and an
/// `.err` file and an error line for inputs named `bad*`: the harness
/// passes the good files and reports each bad one.
#[cfg(unix)]
#[test]
fn harness_reports_rejections() {
    use std::os::unix::fs::PermissionsExt;
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("oda-fake");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("fake-converter.sh");
    std::fs::write(
        &script,
        "#!/bin/sh\n\
         # args: in out version type recurse audit filter\n\
         for f in \"$1\"/*.dwg; do\n\
           b=$(basename \"$f\" .dwg)\n\
           case \"$b\" in\n\
             bad*) echo 'bad handle' > \"$2/$b.dwg.err\"; echo \"Error in $b\";;\n\
             *) echo '0\nSECTION' > \"$2/$b.dxf\";;\n\
           esac\n\
         done\n\
         echo 'Total errors found 0 fixed 0'\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let conv = Converter {
        program: script,
        args: DEFAULT_ARGS.into(),
        timeout: DEFAULT_TIMEOUT,
    };
    let file = |name: &str| PackFile {
        name: name.into(),
        bytes: vec![1, 2, 3],
        kind: pack::Kind::New,
        what: String::new(),
        expected: Vec::new(),
    };
    let good = [file("good_a.dwg"), file("good_b.dwg")];
    assert_eq!(
        convert(&conv, &good, &dir.join("good")),
        Vec::<String>::new()
    );
    let mixed = [file("good_a.dwg"), file("bad_c.dwg")];
    let problems = convert(&conv, &mixed, &dir.join("mixed"));
    assert_eq!(
        problems,
        [
            "log: Error in bad_c",
            "bad_c.dwg.err: bad handle",
            "bad_c.dwg: no DXF written"
        ],
        "{problems:#?}"
    );
}

/// A spawn that fails with "text file busy" is tried again; other errors
/// are returned at once.
#[cfg(unix)]
#[test]
fn busy_program_is_retried() {
    let mut tries = 0;
    let child = spawn_retrying(|| {
        tries += 1;
        if tries < 3 {
            Err(std::io::Error::from_raw_os_error(26))
        } else {
            Command::new("/bin/sh").args(["-c", "exit 0"]).spawn()
        }
    });
    assert_eq!(tries, 3);
    assert!(child.unwrap().wait().unwrap().success());
    let mut tries = 0;
    let missing = spawn_retrying(|| {
        tries += 1;
        Command::new("/no/such/program").spawn()
    });
    assert!(missing.is_err());
    assert_eq!(tries, 1);
}
