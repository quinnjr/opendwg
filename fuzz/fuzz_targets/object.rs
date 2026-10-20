//! One object through `decode_object`, for any version, with the classes of
//! the drawing it came from; when it decodes and encodes as R2018, the
//! encoded form must decode and re-encode to the same bytes (the writer
//! never emits an object its reader rejects).
//!
//! Input: a selector byte (version, code page), a `u16` class count, each
//! class as `u16` number, `u16` item class id and a NUL-terminated DXF name,
//! then the object's bytes from its `MS` size. `examples/seed_corpora` makes
//! one input per object of every sample drawing, with that drawing's
//! classes.
#![no_main]

use libfuzzer_sys::fuzz_target;
use opendwg::object::{DecodeCtx, decode_object, encode_object};
use opendwg::section::DxfClass;
use opendwg::{Handle, Version};

const VERSIONS: [Version; 6] = [
    Version::R2000,
    Version::R2004,
    Version::R2007,
    Version::R2010,
    Version::R2013,
    Version::R2018,
];

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

/// The classes and the object bytes.
fn parse(data: &[u8]) -> Option<(Vec<DxfClass>, &[u8])> {
    let n = u16_at(data, 0)?;
    let mut at = 2;
    let mut classes = Vec::new();
    for _ in 0..n {
        let class_number = u16_at(data, at)?;
        let item_class_id = u16_at(data, at + 2)?;
        let name_len = data.get(at + 4..)?.iter().position(|&b| b == 0)?;
        let dxf_name = String::from_utf8_lossy(&data[at + 4..at + 4 + name_len]).into_owned();
        at += 4 + name_len + 1;
        classes.push(DxfClass {
            class_number,
            proxy_flags: 0,
            app_name: String::new(),
            cpp_class_name: String::new(),
            dxf_name,
            was_zombie: false,
            item_class_id,
            instance_count: 0,
            dwg_version: 0,
            maintenance_version: 0,
            unknown: [0; 2],
        });
    }
    Some((classes, data.get(at..)?))
}

fuzz_target!(|data: &[u8]| {
    let Some((&sel, rest)) = data.split_first() else {
        return;
    };
    let version = VERSIONS[usize::from(sel & 0x7F) % VERSIONS.len()];
    let codepage = if sel & 0x80 != 0 { 38 } else { 30 };
    let Some((classes, bytes)) = parse(rest) else {
        return;
    };
    let h = Handle(0x2A);
    let mut ctx = DecodeCtx::new(version, codepage, &classes);
    let Ok(obj) = decode_object(bytes, h, &mut ctx) else {
        return;
    };
    let Ok(encoded) = encode_object(&obj, &classes) else {
        return;
    };
    let mut ctx = DecodeCtx::new(Version::R2018, codepage, &classes);
    let back = decode_object(&encoded, h, &mut ctx)
        .unwrap_or_else(|e| panic!("encoded {obj:?} does not decode: {e}"));
    let again = encode_object(&back, &classes)
        .unwrap_or_else(|e| panic!("decoded {back:?} does not re-encode: {e}"));
    assert_eq!(again, encoded, "re-encoding {back:?} changed its bytes");
});
