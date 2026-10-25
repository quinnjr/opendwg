//! The `serde` feature: a drawing serializes to JSON for an app front end,
//! and deserializes back to the same model.
#![cfg(feature = "serde")]

mod common;

use opendwg::Drawing;
use opendwg::object::Object;
use opendwg::section::JulianDate;

/// A fixed save date, so that two writes of one drawing compare equal.
const SAVED: JulianDate = JulianDate {
    day: 2_461_000,
    ms: 0,
};

fn r2018_sample() -> Option<Drawing> {
    let path = common::corpus_file_or_skip("acadsharp-sample-ac1032")?;
    Some(Drawing::open(path).unwrap())
}

#[test]
fn r2018_drawing_serializes_to_json() {
    let Some(d) = r2018_sample() else {
        return;
    };
    let json = serde_json::to_string(&d).unwrap();
    let layers: Vec<String> = d.layers().map(|(_, l)| l.entry.name.clone()).collect();
    assert!(layers.len() > 1);
    for name in &layers {
        assert!(json.contains(&format!(
            "\"name\":{}",
            serde_json::to_string(name).unwrap()
        )));
    }
    // Entities are tagged with their kind.
    for kind in [
        "\"Line\"",
        "\"Circle\"",
        "\"MText\"",
        "\"Insert\"",
        "\"Hatch\"",
        "\"Dimension\"",
    ] {
        assert!(json.contains(kind), "no {kind} in the JSON");
    }
    // Objects are keyed by handle and sections by their R2004 name.
    let model_space = d.model_space().0.to_string();
    assert!(json.contains(&format!("\"{model_space}\":")));
}

#[test]
fn json_round_trip_keeps_the_model() {
    let Some(d) = r2018_sample() else {
        return;
    };
    let json = serde_json::to_string(&d).unwrap();
    let back: Drawing = serde_json::from_str(&json).unwrap();
    assert_eq!(back.header, d.header);
    assert_eq!(back.classes, d.classes);
    assert_eq!(back.summary, d.summary);
    assert_eq!(back.preview, d.preview);
    assert_eq!(back.data_storage, d.data_storage);
    assert_eq!(back.diagnostics(), d.diagnostics());
    let objects: Vec<(_, &Object)> = d.objects().collect();
    assert_eq!(back.objects().collect::<Vec<_>>(), objects);
    assert_eq!(
        back.extra_sections().collect::<Vec<_>>(),
        d.extra_sections().collect::<Vec<_>>()
    );
    // It writes, and the result reads back with the same objects.
    let again = Drawing::from_bytes(&back.to_bytes().unwrap()).unwrap();
    assert_eq!(again.objects().count(), objects.len());
}

#[test]
fn new_drawing_round_trips_through_json() {
    let d = Drawing::new();
    let back: Drawing = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
    assert_eq!(
        back.objects().collect::<Vec<_>>(),
        d.objects().collect::<Vec<_>>()
    );
    assert_eq!(
        back.to_bytes_at(SAVED).unwrap(),
        d.to_bytes_at(SAVED).unwrap()
    );
}

#[test]
fn every_corpus_drawing_round_trips_through_json() {
    for (e, path) in common::fetched_files_or_skip() {
        let d = Drawing::open(path).unwrap();
        let json = serde_json::to_string(&d).unwrap();
        let back: Drawing =
            serde_json::from_str(&json).unwrap_or_else(|err| panic!("{}: {err}", e.name));
        assert!(
            back.objects().eq(d.objects()),
            "{}: objects changed",
            e.name
        );
        assert_eq!(back.header, d.header, "{}", e.name);
        // A deserialized drawing has no read state: it saves when the
        // original does (R2000/R2004 files with unconvertible objects fail
        // with the same error).
        match (d.to_bytes_at(SAVED), back.to_bytes_at(SAVED)) {
            (Ok(original), Ok(bytes)) => {
                let a = Drawing::from_bytes(&original).unwrap();
                let b = Drawing::from_bytes(&bytes).unwrap();
                let handles = |d: &Drawing| d.objects().map(|(h, _)| h).collect::<Vec<_>>();
                assert_eq!(handles(&b), handles(&a), "{}", e.name);
            }
            (Err(a), Err(b)) => assert_eq!(a.to_string(), b.to_string(), "{}", e.name),
            (a, b) => panic!(
                "{}: original {:?}, deserialized {:?}",
                e.name,
                a.err(),
                b.err()
            ),
        }
    }
}

/// Seeded xorshift64*.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) % n as u64) as usize
    }
}

/// The JSON pointers of every value in `v`.
fn pointers(v: &serde_json::Value, at: String, out: &mut Vec<String>) {
    match v {
        serde_json::Value::Array(a) => {
            for (i, x) in a.iter().enumerate() {
                pointers(x, format!("{at}/{i}"), out);
            }
        }
        serde_json::Value::Object(m) => {
            for (k, x) in m {
                let k = k.replace('~', "~0").replace('/', "~1");
                pointers(x, format!("{at}/{k}"), out);
            }
        }
        _ => {}
    }
    out.push(at);
}

/// Change the value at a random pointer: numbers to edge values, strings,
/// booleans, array elements removed or repeated.
fn mutate_json(rng: &mut Rng, v: &mut serde_json::Value, ptrs: &[String]) -> String {
    use serde_json::{Value, json};
    let p = &ptrs[rng.below(ptrs.len())];
    let Some(x) = v.pointer_mut(p) else {
        return format!("{p}: gone");
    };
    match x {
        Value::Number(_) => {
            let edge = [
                json!(0),
                json!(-1),
                json!(1),
                json!(499),
                json!(500),
                json!(32767),
                json!(65535),
                json!(i32::MAX),
                json!(u64::MAX),
                json!(-1e308),
                json!(1e-300),
            ];
            let new = edge[rng.below(edge.len())].clone();
            let w = format!("{p} = {new}");
            *x = new;
            w
        }
        Value::String(s) => {
            let new = [
                "",
                "0",
                "FFFFFFFF",
                "*Model_Space",
                "\u{FFFD}\u{10FFFF}",
                "x",
            ][rng.below(6)]
            .to_string();
            let w = format!("{p}: {s:?} -> {new:?}");
            *s = new;
            w
        }
        Value::Bool(b) => {
            *b = !*b;
            format!("{p}: flipped")
        }
        Value::Array(a) if !a.is_empty() => {
            let i = rng.below(a.len());
            if rng.below(2) == 0 {
                a.remove(i);
                format!("{p}/{i} removed")
            } else {
                let e = a[i].clone();
                a.push(e);
                format!("{p}/{i} repeated")
            }
        }
        _ => format!("{p}: unchanged"),
    }
}

/// Hostile JSON: a deserialized drawing with edited values either fails to
/// save (an error, from `validate` or the encoders) or saves a file the
/// reader reads back. It never panics.
#[test]
fn hostile_json_never_panics_or_writes_unreadable_files() {
    let cases: u64 = std::env::var("OPENDWG_SERDE_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(150);
    let d = Drawing::new();
    let base = serde_json::to_value(&d).unwrap();
    let mut ptrs = Vec::new();
    pointers(&base, String::new(), &mut ptrs);
    let (mut deserialized, mut saved) = (0, 0);
    let mut failures = Vec::new();
    for case in 0..cases {
        let mut rng = Rng((case + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let mut v = base.clone();
        let steps: Vec<String> = (0..1 + rng.below(3))
            .map(|_| mutate_json(&mut rng, &mut v, &ptrs))
            .collect();
        let Ok(back) = serde_json::from_value::<Drawing>(v) else {
            continue;
        };
        deserialized += 1;
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            back.to_bytes_at(SAVED)
                .map(|b| Drawing::from_bytes(&b).map(drop))
        }));
        match outcome {
            Err(_) => failures.push(format!("case {case} ({}): panicked", steps.join("; "))),
            Ok(Ok(Err(e))) => failures.push(format!(
                "case {case} ({}): saved a file that does not read back: {e}",
                steps.join("; ")
            )),
            Ok(Ok(Ok(()))) => saved += 1,
            Ok(Err(_)) => {}
        }
    }
    eprintln!("{cases} cases: {deserialized} deserialized, {saved} saved and read back");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    // The check is hollow if the edits leave nothing to save: a quarter of
    // the cases deserialize and a quarter of those save (about half and 90%
    // at the default 150 cases).
    assert!(
        deserialized * 4 >= cases && saved * 4 >= deserialized,
        "{deserialized} deserialized, {saved} saved of {cases} cases"
    );
}

/// An error serializes as `{ kind, message, handles? }`, so a Tauri command
/// can return it as is.
#[test]
fn error_serializes_for_a_front_end() {
    use opendwg::{Error, Handle};
    let json = |e: &Error| serde_json::to_value(e).unwrap();
    assert_eq!(
        json(&Error::NotFound(Handle(0x2A))),
        serde_json::json!({ "kind": "NotFound", "message": "object not found: 2A", "handles": [42] })
    );
    assert_eq!(
        json(&Error::Corrupt("bad".into())),
        serde_json::json!({ "kind": "Corrupt", "message": "corrupt data: bad" })
    );
    let in_use = json(&Error::InUse {
        objects: vec![Handle(1), Handle(2)],
        header_vars: vec!["$CLAYER"],
    });
    assert_eq!(in_use["kind"], "InUse");
    assert_eq!(in_use["handles"], serde_json::json!([1, 2]));
    let unconvertible = json(&Error::UnconvertibleObjects(vec![(
        Handle(9),
        "MATERIAL".into(),
    )]));
    assert_eq!(unconvertible["handles"], serde_json::json!([9]));
    assert!(
        unconvertible["message"]
            .as_str()
            .unwrap()
            .contains("MATERIAL 9")
    );
    let io = json(&Error::Io(std::io::Error::other("disk")));
    assert_eq!(io["kind"], "Io");
}
