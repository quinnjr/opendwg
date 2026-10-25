//! A stable mutation fuzzer that runs with `cargo test`: every corpus file
//! (when fetched) and `Drawing::new()` output gets seeded mutations (bit
//! flips, byte replacements, truncations, splices, length-field bumps), and
//! each mutated file is read and, when that succeeds, written. No case may
//! panic, and each must finish within a time bound.
//!
//! `OPENDWG_MUTATION_CASES` sets the cases per input (default 8) and
//! `OPENDWG_MUTATION_SEED` the base seed. A failure prints the input, case
//! and mutations so it can be replayed. The `cargo-fuzz` targets in `fuzz/`
//! search much deeper; this keeps a floor in every test run.

mod common;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use opendwg::Drawing;

/// Seeded xorshift64*: the same seed gives the same mutations everywhere.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A value in `0..n` (`n > 0`).
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// A position to mutate: half the time in the first 4 KiB, where the file
/// header, section maps and (R2000) the small sections live.
fn position(rng: &mut Rng, len: usize) -> usize {
    if rng.below(2) == 0 {
        rng.below(len.min(4096))
    } else {
        rng.below(len)
    }
}

/// Apply one random mutation to `b` (non-empty) and describe it.
fn mutate(rng: &mut Rng, b: &mut Vec<u8>) -> String {
    let len = b.len();
    match rng.below(5) {
        0 => {
            let at = position(rng, len);
            let bit = rng.below(8);
            b[at] ^= 1 << bit;
            format!("flip bit {bit} of byte {at:#x}")
        }
        1 => {
            let at = position(rng, len);
            let v = [0x00, 0xFF, 0x7F, 0x80, 0x01, rng.next() as u8][rng.below(6)];
            b[at] = v;
            format!("byte {at:#x} = {v:#04x}")
        }
        2 => {
            let at = rng.below(len);
            b.truncate(at.max(1));
            format!("truncate to {} bytes", b.len())
        }
        3 => {
            // Copy a chunk of the file over another place, or insert it.
            let from = rng.below(len);
            let n = (1 + rng.below(256)).min(len - from);
            let chunk = b[from..from + n].to_vec();
            let to = position(rng, len);
            if rng.below(2) == 0 {
                let end = (to + n).min(len);
                b[to..end].copy_from_slice(&chunk[..end - to]);
                format!("splice {n} bytes from {from:#x} over {to:#x}")
            } else {
                b.splice(to..to, chunk);
                format!("insert {n} bytes from {from:#x} at {to:#x}")
            }
        }
        _ => {
            // Treat 4 aligned bytes as a little-endian length or offset and
            // make it large, zero or slightly off.
            if len < 8 {
                return "too short for a length field".to_string();
            }
            let at = position(rng, len - 4) & !3;
            let Some(field) = b.get_mut(at..at + 4) else {
                return "no length field".to_string();
            };
            let old = u32::from_le_bytes(field.try_into().unwrap());
            let new = match rng.below(5) {
                0 => u32::MAX,
                1 => 0,
                2 => old.wrapping_add(1),
                3 => old.wrapping_sub(1),
                _ => old.wrapping_mul(0x1000).max(0x7FFF_FFFF),
            };
            field.copy_from_slice(&new.to_le_bytes());
            format!("length field at {at:#x}: {old:#x} -> {new:#x}")
        }
    }
}

/// How far a case got.
#[derive(Default)]
struct Outcome {
    read: bool,
    written: bool,
    /// Why a written file did not read back (a failure).
    unreadable: Option<String>,
}

/// Read `bytes`, write the drawing when it reads, and read the written file
/// back.
fn read_and_write(bytes: &[u8]) -> Outcome {
    let Ok(d) = Drawing::from_bytes(bytes) else {
        return Outcome::default();
    };
    let Ok(written) = d.to_bytes() else {
        return Outcome {
            read: true,
            ..Outcome::default()
        };
    };
    Outcome {
        read: true,
        written: true,
        unreadable: Drawing::from_bytes(&written).err().map(|e| e.to_string()),
    }
}

/// Run `read_and_write` on another thread; `Err` describes a panic or a run
/// over `limit` (a hang: the thread is left behind and the test fails).
fn run_case(bytes: Vec<u8>, limit: Duration) -> Result<(Outcome, Duration), String> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let start = Instant::now();
        let outcome = catch_unwind(AssertUnwindSafe(|| read_and_write(&bytes)));
        let _ = tx.send(outcome.map(|o| (o, start.elapsed())));
    });
    match rx.recv_timeout(limit) {
        Ok(Ok(t)) => Ok(t),
        Ok(Err(p)) => Err(format!(
            "panicked: {}",
            p.downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| p.downcast_ref::<&str>().copied())
                .unwrap_or("?")
        )),
        Err(_) => Err(format!("ran over {limit:?}")),
    }
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

#[test]
fn mutated_files_never_panic_or_hang() {
    let cases = env_u64("OPENDWG_MUTATION_CASES", 8);
    let seed = env_u64("OPENDWG_MUTATION_SEED", 0x0D3C_F022);
    let mut inputs: Vec<(String, Vec<u8>)> = common::fetched_files_or_skip()
        .into_iter()
        .map(|(e, p)| (e.name, std::fs::read(p).unwrap()))
        .collect();
    inputs.push(("Drawing::new()".into(), Drawing::new().to_bytes().unwrap()));

    let mut failures = Vec::new();
    let (mut read, mut written) = (0, 0);
    for (n, (name, original)) in inputs.iter().enumerate() {
        // The work bound: a mutated file may take far longer than the
        // original (a corrupt page is re-read, objects fall back to raw),
        // but not without limit.
        let (_, base) = run_case(original.clone(), Duration::from_secs(120))
            .unwrap_or_else(|e| panic!("{name} unmutated: {e}"));
        let limit = base * 20 + Duration::from_secs(5);
        for case in 0..cases {
            let mut rng = Rng::new(seed ^ ((n as u64) << 32) ^ case);
            let mut bytes = original.clone();
            let steps: Vec<String> = (0..1 + rng.below(3))
                .map(|_| mutate(&mut rng, &mut bytes))
                .collect();
            match run_case(bytes, limit) {
                Ok((o, _)) => {
                    read += usize::from(o.read);
                    written += usize::from(o.written);
                    if let Some(e) = o.unreadable {
                        failures.push(format!(
                            "{name} case {case} ({}): the written file does not read back: {e}",
                            steps.join("; ")
                        ));
                    }
                }
                Err(e) => failures.push(format!("{name} case {case} ({}): {e}", steps.join("; "))),
            }
        }
    }
    let total = inputs.len() as u64 * cases;
    eprintln!("{total} mutated files: {read} read, {written} also written and read back");
    assert!(
        failures.is_empty(),
        "seed {seed:#x}, {cases} cases per input:\n{}",
        failures.join("\n")
    );
    // The check is hollow if no mutated file gets as far as the writer
    // (27 read and 16 written of the 72 default cases on the main corpus).
    if total >= 20 {
        assert!(
            read > 0 && written > 0,
            "{total} mutated files: {read} read, {written} written"
        );
    }
}

#[test]
fn mutations_are_deterministic() {
    let original: Vec<u8> = (0..=255).cycle().take(10_000).collect();
    let run = || {
        let mut rng = Rng::new(7);
        let mut b = original.clone();
        let steps: Vec<String> = (0..20).map(|_| mutate(&mut rng, &mut b)).collect();
        (b, steps)
    };
    let (a, steps) = run();
    assert_eq!((a.clone(), steps.clone()), run());
    assert_ne!(a, original);
    assert_eq!(steps.len(), 20);
}
