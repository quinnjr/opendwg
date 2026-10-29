//! Seed the fuzz corpora (`fuzz/corpus/<target>/`, gitignored) from the
//! sample drawings in `corpus/files` (fetched by `scripts/fetch-corpus.sh`)
//! and from `Drawing::new()`.
//!
//! ```sh
//! cd fuzz && cargo run --release --example seed_corpora
//! ```
//!
//! The whole-file targets (`read_drawing`, `write_roundtrip`, `container`)
//! get small files only (at most [`SMALL_FILE`] bytes): a 1 MB sample takes
//! milliseconds per run, so libFuzzer would make few runs. The object
//! decoders are fuzzed through the `object` target instead, seeded with
//! every object of every sample drawing, in every version.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use opendwg::bits::BitReader;
use opendwg::codec::{lz77_2004, lz77_2007, rs2007};
use opendwg::container::{self, SectionName};
use opendwg::object::{Circle, Entity, EntityCommon, Line, ObjReader, Object};
use opendwg::section::{DxfClass, objmap};
use opendwg::{Drawing, Version};

const VERSIONS: [Version; 6] = [
    Version::R2000,
    Version::R2004,
    Version::R2007,
    Version::R2010,
    Version::R2013,
    Version::R2018,
];

/// The largest whole file used as a seed.
const SMALL_FILE: usize = 64 * 1024;

/// The selector byte of the versioned targets (see `split` in the targets):
/// the version's index, with bit 0x80 for code page 38 (ANSI_932). The
/// targets know only code pages 30 and 38, so a drawing in another code
/// page has no selector: its bytes would be decoded in the wrong one.
fn selector(v: Version, codepage: u16) -> Option<u8> {
    let i = VERSIONS.iter().position(|&x| x == v).unwrap_or(0) as u8;
    match codepage {
        30 => Some(i),
        38 => Some(i | 0x80),
        _ => None,
    }
}

fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

struct Seeds {
    root: PathBuf,
    /// Seeds already written, by target, so that each is written once.
    seen: BTreeSet<(String, Vec<u8>)>,
}

impl Seeds {
    fn put(&mut self, target: &str, bytes: &[u8]) {
        if !self.seen.insert((target.to_owned(), bytes.to_vec())) {
            return;
        }
        let dir = self.root.join(target);
        fs::create_dir_all(&dir).expect("create corpus dir");
        let n = self.seen.len();
        fs::write(dir.join(format!("seed-{n:05}")), bytes).expect("write seed");
    }

    fn versioned(&mut self, target: &str, v: Version, bytes: &[u8]) {
        let mut b = vec![selector(v, 30).expect("code page 30 has a selector")];
        b.extend_from_slice(bytes);
        self.put(target, &b);
    }

    /// A Reed-Solomon seed: block count, data size per block, layout.
    fn rs(&mut self, blocks: usize, k: u8, interleaved: bool, src: &[u8]) {
        let Ok(count) = u8::try_from(blocks) else {
            return;
        };
        let mut b = vec![
            count,
            k,
            u8::from(interleaved) | if k == 251 { 2 } else { 0 },
        ];
        b.extend_from_slice(src);
        self.put("rs2007", &b);
    }

    /// An R2007 compression seed: the expected size, then the stream.
    fn lz2007(&mut self, expected: u64, payload: &[u8]) {
        let Ok(len) = u16::try_from(expected) else {
            return;
        };
        let mut b = len.to_le_bytes().to_vec();
        b.extend_from_slice(payload);
        self.put("lz77_2007", &b);
    }

    fn whole_file(&mut self, bytes: &[u8]) {
        if bytes.len() <= SMALL_FILE {
            for t in ["read_drawing", "write_roundtrip", "container"] {
                self.put(t, bytes);
            }
        }
    }

    fn file(&mut self, bytes: &[u8]) {
        self.whole_file(bytes);
        let Ok(raw) = container::read(bytes) else {
            return;
        };
        let v = raw.version;
        for (name, data) in &raw.sections {
            match name {
                SectionName::Header => self.versioned("header", v, data),
                SectionName::Classes => self.versioned("classes", v, data),
                SectionName::Handles => self.put("objmap", data),
                SectionName::AcDsPrototype => self.put("acds", data),
                SectionName::SummaryInfo | SectionName::AppInfo => {
                    self.versioned("summaryinfo", v, data)
                }
                SectionName::Preview => {
                    let mut b = vec![0];
                    b.extend_from_slice(data);
                    self.put("preview", &b);
                }
                _ => {}
            }
            // Compressed pages: the expected size, then the compressed data.
            let head = &data[..data.len().min(0x8000)];
            let mut b = (head.len() as u16).to_le_bytes().to_vec();
            b.extend(lz77_2004::compress(head));
            self.put("lz77_2004", &b);
            // Reed-Solomon encoder output, both codes and layouts.
            let head = &data[..data.len().min(3 * 251)];
            for k in [239u8, 251] {
                for interleaved in [true, false] {
                    let blocks = head.len().div_ceil(usize::from(k)).max(1);
                    let enc = rs2007::encode(head, usize::from(k), interleaved).unwrap();
                    self.rs(blocks, k, interleaved, &enc);
                }
            }
        }
        self.objects(bytes);
        if let Ok(d) = Drawing::from_bytes(bytes) {
            for (_, o) in d.objects() {
                if let Object::Entity { common, .. } = o
                    && let Some(g) = &common.proxy_graphics
                {
                    self.versioned("proxy_graphics", v, g);
                }
            }
        }
        if v == Version::R2007 {
            self.r2007_pages(bytes);
        }
    }

    /// One `object` input per object of the file (see the target's docs):
    /// the file's classes, then the object's bytes from its `MS` size to its
    /// CRC.
    fn objects(&mut self, bytes: &[u8]) {
        let Ok(raw) = container::read(bytes) else {
            return;
        };
        let (v, cp) = (raw.version, raw.codepage);
        let (Some(objects), Some(map), Some(classes)) = (
            raw.sections.get(&SectionName::Objects),
            raw.sections.get(&SectionName::Handles),
            raw.sections.get(&SectionName::Classes),
        ) else {
            return;
        };
        let Ok(map) = objmap::decode(map) else {
            return;
        };
        let Ok(classes) = opendwg::section::classes::decode(classes, v, cp) else {
            return;
        };
        // Seeded only in a code page the target can select.
        let Some(sel) = selector(v, cp) else {
            return;
        };
        let head = class_header(sel, &classes);
        for (_, loc) in map {
            let Some(at) = loc
                .checked_sub(raw.objects_base)
                .and_then(|o| usize::try_from(o).ok())
                .and_then(|o| objects.get(o..))
            else {
                continue;
            };
            let Ok((_, pre)) = ObjReader::split(at, v, cp) else {
                continue;
            };
            let mut r = BitReader::new(at);
            if r.read_ms().is_err() || (v >= Version::R2010 && r.read_umc().is_err()) {
                continue;
            }
            let len = (r.bit_pos() / 8) as usize + pre.size_bytes as usize + 2;
            let Some(obj) = at.get(..len) else {
                continue;
            };
            let mut b = head.clone();
            b.extend_from_slice(obj);
            self.put("object", &b);
        }
    }

    /// Real R2007 pages (ODS 5.2-5.4): the file header page, the pages map
    /// and sections map (RS(255,239), compressed) and every data page
    /// (RS(255,251)).
    fn r2007_pages(&mut self, bytes: &[u8]) {
        let Some(page) = bytes.get(0x80..0x80 + 3 * 255) else {
            return;
        };
        self.rs(3, 239, true, page);
        let Ok(d) = rs2007::decode(page, 3, 239, 3) else {
            return;
        };
        let Some(len) = u64_at(&d, 0x18).map(|l| l as u32 as i32) else {
            return;
        };
        let Some(payload) = d.get(0x20..0x20 + len.unsigned_abs() as usize) else {
            return;
        };
        self.lz2007(0x110, payload);
        let Ok(h) = lz77_2007::decompress(payload, 0x110) else {
            return;
        };
        let f = |at| u64_at(&h, at).unwrap_or(0);
        let system =
            |s: &mut Seeds, address: u64, c: u64, u: u64, repeat: u64| -> Option<Vec<u8>> {
                let blocks = (c.div_ceil(8) * 8)
                    .checked_mul(repeat)?
                    .div_ceil(239)
                    .max(1);
                let start = usize::try_from(address).ok()?;
                let src = bytes.get(start..start + usize::try_from(blocks).ok()? * 255)?;
                s.rs(blocks as usize, 239, true, src);
                let decoded = rs2007::decode(src, blocks as usize, 239, blocks as usize).ok()?;
                let payload = decoded.get(..c as usize)?;
                s.lz2007(u, payload);
                lz77_2007::decompress(payload, u as usize).ok()
            };
        let Some(pages_map) = system(self, 0x480 + f(0x38), f(0x50), f(0x58), f(0x18)) else {
            return;
        };
        let mut address = 0x480u64;
        for pair in pages_map.as_chunks::<16>().0 {
            let size = u64_at(pair, 0).unwrap_or(0);
            let id = u64_at(pair, 8).unwrap_or(0) as i64;
            if id.unsigned_abs() == f(0xC0) {
                system(self, address, f(0xB0), f(0xC8), f(0xD8));
            } else if id > 0
                && let Some(src) = usize::try_from(address)
                    .ok()
                    .and_then(|a| bytes.get(a..a + size as usize))
            {
                let blocks = src.len() / 255;
                self.rs(blocks, 251, true, src);
                if let Ok(decoded) = rs2007::decode(src, blocks, 251, blocks) {
                    self.lz2007(0x7400, &decoded);
                }
            }
            address = address.saturating_add(size);
        }
    }
}

/// The `object` target's class list for `classes`, after the selector byte.
fn class_header(sel: u8, classes: &[DxfClass]) -> Vec<u8> {
    let mut b = vec![sel];
    b.extend((classes.len() as u16).to_le_bytes());
    for c in classes {
        b.extend(c.class_number.to_le_bytes());
        b.extend(c.item_class_id.to_le_bytes());
        b.extend(c.dxf_name.as_bytes());
        b.push(0);
    }
    b
}

/// Small drawings made with the library: an empty one and one with a few
/// entities.
fn small_drawings() -> Vec<Vec<u8>> {
    let mut out = vec![Drawing::new().to_bytes().expect("Drawing::new() writes")];
    let mut d = Drawing::new();
    let ms = d.model_space();
    let layer = d.layer_by_name("0").expect("layer 0");
    for i in 0..3 {
        let x = f64::from(i);
        let line = Entity::Line(Line {
            start: [x, 0.0, 0.0],
            end: [x, 1.0, 0.0],
            thickness: 0.0,
            extrusion: [0.0, 0.0, 1.0],
        });
        d.add_entity(ms, EntityCommon::new(layer), line)
            .expect("add line");
    }
    let circle = Circle {
        center: [0.0, 0.0, 0.0],
        radius: 2.0,
        thickness: 0.0,
        extrusion: [0.0, 0.0, 1.0],
    };
    d.add_entity(ms, EntityCommon::new(layer), Entity::Circle(circle))
        .expect("add circle");
    out.push(d.to_bytes().expect("small drawing writes"));
    out
}

fn main() {
    let fuzz = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut seeds = Seeds {
        root: fuzz.join("corpus"),
        seen: BTreeSet::new(),
    };
    let files = fuzz.join("../corpus/files");
    let mut found = 0;
    if let Ok(entries) = fs::read_dir(&files) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dwg")) {
                let bytes = fs::read(&p).expect("read sample");
                // The bit reader and LZ77 decoders get raw file slices too.
                for chunk in bytes.chunks(4096).take(8) {
                    seeds.put("bitreader", chunk);
                    seeds.put("lz77_2007", chunk);
                }
                seeds.file(&bytes);
                found += 1;
            }
        }
    }
    for bytes in small_drawings() {
        seeds.file(&bytes);
    }
    println!(
        "{} seeds from {found} sample drawings and Drawing::new() in {}",
        seeds.seen.len(),
        seeds.root.display()
    );
    let mut per: Vec<(String, usize)> = Vec::new();
    for (t, _) in &seeds.seen {
        match per.last_mut() {
            Some((last, n)) if last == t => *n += 1,
            _ => per.push((t.clone(), 1)),
        }
    }
    for (t, n) in per {
        println!("  {t}: {n}");
    }
}
