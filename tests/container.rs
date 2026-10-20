mod common;

use opendwg::codec;
use opendwg::container::{self, SectionName};
use opendwg::{Error, Version};

const HEADER_START: [u8; 16] = [
    0xCF, 0x7B, 0x1F, 0x23, 0xFD, 0xDE, 0x38, 0xA9, 0x5F, 0x7C, 0x68, 0xB8, 0x4E, 0x6D, 0x33, 0x5F,
];
const CLASSES_START: [u8; 16] = [
    0x8D, 0xA1, 0xC4, 0xB8, 0xC4, 0xA9, 0xF8, 0xC5, 0xC0, 0xDC, 0xF4, 0x5F, 0xE7, 0xCF, 0xB6, 0x8A,
];

#[test]
fn r2000_corpus_sections() {
    for (e, path) in common::fetched_files_for("AC1015") {
        let bytes = std::fs::read(&path).unwrap();
        let raw = container::read(&bytes).unwrap_or_else(|err| panic!("{}: {err}", e.name));
        assert_eq!(raw.version, Version::R2000);
        assert_eq!(raw.objects_base, 0);
        assert_eq!(raw.codepage, bytes[0x13] as u16 | (bytes[0x14] as u16) << 8);
        assert_eq!(raw.maintenance, bytes[0x0B]);
        let s = &raw.sections;
        assert!(
            s[&SectionName::Header].starts_with(&HEADER_START),
            "{}",
            e.name
        );
        assert!(
            s[&SectionName::Classes].starts_with(&CLASSES_START),
            "{}",
            e.name
        );
        assert!(!s[&SectionName::Handles].is_empty());
        assert_eq!(s[&SectionName::Objects], bytes);
        if let Some(off) = raw.preview_offset {
            assert!(bytes[off as usize..].starts_with(&s[&SectionName::Preview][..16]));
        }
    }
}

#[test]
fn r2000_sample_details() {
    let Some(path) = common::corpus_file_or_skip("acadsharp-sample-ac1015") else {
        return;
    };
    let raw = container::read(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(raw.codepage, 30);
    assert_eq!(raw.preview_offset, Some(0xdc));
    assert!(raw.sections.contains_key(&SectionName::AuxHeader));
}

#[test]
fn truncated_r2000_errors() {
    let Some(path) = common::corpus_file_or_skip("acadsharp-sample-ac1015") else {
        return;
    };
    let bytes = std::fs::read(path).unwrap();
    for n in 0..0x80 {
        assert!(
            container::read(&bytes[..n]).is_err(),
            "prefix {n} should fail"
        );
    }
}

#[test]
fn corrupted_header_crc_is_bad_crc() {
    let Some(path) = common::corpus_file_or_skip("acadsharp-sample-ac1015") else {
        return;
    };
    let mut bytes = std::fs::read(path).unwrap();
    bytes[0x13] ^= 1; // codepage is covered by the CRC
    assert!(matches!(container::read(&bytes), Err(Error::BadCrc { .. })));
}

#[test]
fn bad_magic() {
    let r = container::read(b"AC1099\0\0\0\0\0\0\0\0");
    assert!(matches!(r, Err(Error::UnsupportedVersion(s)) if s == "AC1099"));
}

/// R2004-family corpus files: every AC1018/1024/1027/1032 entry.
fn r2004_family() -> Vec<(common::Entry, std::path::PathBuf)> {
    ["AC1018", "AC1024", "AC1027", "AC1032"]
        .into_iter()
        .flat_map(common::fetched_files_for)
        .collect()
}

/// Decrypted 0x6C-byte R2004 file header data (ODS 4.1).
fn r2004_header_data(bytes: &[u8]) -> Vec<u8> {
    let mut data = bytes[0x80..0x80 + 0x6C].to_vec();
    codec::xor_r2004_file_header(&mut data);
    data
}

fn u32_le(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

#[test]
fn r2004_family_corpus_sections() {
    let files = r2004_family();
    for (e, path) in &files {
        let bytes = std::fs::read(path).unwrap();
        let raw = container::read(&bytes).unwrap_or_else(|err| panic!("{}: {err}", e.name));
        let want = Version::from_magic(bytes[..6].try_into().unwrap()).unwrap();
        assert_eq!(raw.version, want, "{}", e.name);
        assert_eq!(raw.objects_base, 0);
        assert_eq!(raw.maintenance, bytes[0x0B]);
        assert_eq!(raw.codepage, u16::from_le_bytes([bytes[0x13], bytes[0x14]]));
        assert!(raw.warnings.is_empty(), "{}: {:?}", e.name, raw.warnings);
        let s = &raw.sections;
        assert!(
            s[&SectionName::Header].starts_with(&HEADER_START),
            "{}",
            e.name
        );
        assert!(
            s[&SectionName::Classes].starts_with(&CLASSES_START),
            "{}",
            e.name
        );
        assert!(!s[&SectionName::Handles].is_empty(), "{}", e.name);
        assert!(!s[&SectionName::Objects].is_empty(), "{}", e.name);
        if let (Some(off), Some(p)) = (raw.preview_offset, s.get(&SectionName::Preview)) {
            let off = off as usize;
            assert_eq!(&bytes[off..off + 16], &p[..16], "{}", e.name);
        }
    }
}

#[test]
fn r2004_family_versions_covered() {
    let files = r2004_family();
    if files.is_empty() {
        return;
    }
    for v in ["AC1018", "AC1024", "AC1027", "AC1032"] {
        assert!(files.iter().any(|(e, _)| e.version == v), "no {v} file");
    }
}

#[test]
fn r2004_file_header_magic_and_crc32() {
    for (e, path) in r2004_family() {
        let bytes = std::fs::read(&path).unwrap();
        let mut data = r2004_header_data(&bytes);
        assert_eq!(&data[..12], b"AcFssFcAJMB\0", "{}", e.name);
        let stored = u32_le(&data, 0x68);
        data[0x68..0x6C].fill(0);
        assert_eq!(codec::crc32(0, &data), stored, "{}", e.name);
    }
}

#[test]
fn r2004_real_page_checksums() {
    for (e, path) in r2004_family() {
        let bytes = std::fs::read(&path).unwrap();
        let data = r2004_header_data(&bytes);

        // System page (the section page map): header with seed 0, then the data.
        let at = u64::from_le_bytes(data[0x54..0x5C].try_into().unwrap()) as usize + 0x100;
        let mut header = bytes[at..at + 0x14].to_vec();
        assert_eq!(u32_le(&header, 0), 0x4163_0E3B, "{}", e.name);
        let stored = u32_le(&header, 0x10);
        let comp = u32_le(&header, 0x08) as usize;
        header[0x10..].fill(0);
        let seed = codec::page_checksum(0, &header);
        let page = &bytes[at + 0x14..at + 0x14 + comp];
        assert_eq!(
            codec::page_checksum(seed, page),
            stored,
            "{}: system",
            e.name
        );

        // The first data page is at 0x100. Data checksum with seed 0, then the header (its
        // checksum field zeroed) seeded with that.
        let mask = codec::page_header_mask(0x100);
        let mut header: Vec<u8> = bytes[0x100..0x120]
            .chunks_exact(4)
            .flat_map(|w| (u32::from_le_bytes(w.try_into().unwrap()) ^ mask).to_le_bytes())
            .collect();
        assert_eq!(u32_le(&header, 0), 0x4163_043B, "{}", e.name);
        let size = u32_le(&header, 0x08) as usize;
        let data_sum = codec::page_checksum(0, &bytes[0x120..0x120 + size]);
        assert_eq!(data_sum, u32_le(&header, 0x1C), "{}: data", e.name);
        let stored = u32_le(&header, 0x18);
        header[0x18..0x1C].fill(0);
        assert_eq!(
            codec::page_checksum(data_sum, &header),
            stored,
            "{}: data header",
            e.name
        );
    }
}

#[test]
fn corrupt_page_checksum_errors() {
    for (e, path) in r2004_family() {
        let bytes = std::fs::read(&path).unwrap();
        let data = r2004_header_data(&bytes);

        // System page: a flipped byte in the compressed data is an error.
        let at = u64::from_le_bytes(data[0x54..0x5C].try_into().unwrap()) as usize + 0x100;
        let mut bad = bytes.clone();
        bad[at + 0x14] ^= 0x01;
        assert!(
            matches!(container::read(&bad), Err(Error::BadCrc { .. })),
            "{}",
            e.name
        );

        // Data page: a flipped bit in the stored (masked) data checksum is a warning only.
        let mut bad = bytes.clone();
        bad[0x100 + 0x1C] ^= 0x01;
        let raw = container::read(&bad).unwrap_or_else(|err| panic!("{}: {err}", e.name));
        assert_eq!(raw.warnings.len(), 1, "{}: {:?}", e.name, raw.warnings);
        assert_eq!(
            raw.sections,
            container::read(&bytes).unwrap().sections,
            "{}",
            e.name
        );
    }
}

#[test]
fn r2004_truncated_at_every_page_boundary() {
    // ODS 4.3: every page starts on a 0x20 boundary, so this covers each page start.
    for (e, path) in r2004_family() {
        let bytes = std::fs::read(&path).unwrap();
        let whole = container::read(&bytes).unwrap();
        for n in (0..bytes.len()).step_by(0x20).chain([bytes.len() - 1]) {
            // Without the trailing header copy but with every section, the file reads, with
            // a warning (see `missing_trailing_header_copy_is_a_warning`).
            match container::read(&bytes[..n]) {
                Err(Error::Truncated { .. }) => {}
                Err(err) => panic!("{}: prefix {n:#x}: {err}", e.name),
                Ok(raw) => {
                    assert_eq!(raw.sections, whole.sections, "{}: prefix {n:#x}", e.name);
                    assert!(!raw.warnings.is_empty(), "{}: prefix {n:#x}", e.name);
                }
            }
        }
    }
}

/// R2007 corpus files (AC1021).
fn r2007_files() -> Vec<(common::Entry, std::path::PathBuf)> {
    common::fetched_files_for("AC1021")
}

#[test]
fn r2007_corpus_sections() {
    for (e, path) in r2007_files() {
        let bytes = std::fs::read(&path).unwrap();
        let raw = container::read(&bytes).unwrap_or_else(|err| panic!("{}: {err}", e.name));
        assert_eq!(raw.version, Version::R2007, "{}", e.name);
        assert_eq!(raw.objects_base, 0);
        assert_eq!(raw.maintenance, bytes[0x0B]);
        assert_eq!(raw.codepage, u16::from_le_bytes([bytes[0x13], bytes[0x14]]));
        assert!(raw.warnings.is_empty(), "{}: {:?}", e.name, raw.warnings);
        let s = &raw.sections;
        assert!(
            s[&SectionName::Header].starts_with(&HEADER_START),
            "{}",
            e.name
        );
        assert!(
            s[&SectionName::Classes].starts_with(&CLASSES_START),
            "{}",
            e.name
        );
        assert!(!s[&SectionName::Handles].is_empty(), "{}", e.name);
        assert!(!s[&SectionName::Objects].is_empty(), "{}", e.name);
        // The preview address is the start of the (raw, encoding 1) preview page.
        let (off, p) = (raw.preview_offset.unwrap(), &s[&SectionName::Preview]);
        let off = off as usize;
        assert_eq!(&bytes[off..off + 16], &p[..16], "{}", e.name);
    }
}

#[test]
fn r2007_sample_details() {
    let Some(path) = common::corpus_file_or_skip("acadsharp-sample-ac1021") else {
        return;
    };
    let raw = container::read(&std::fs::read(path).unwrap()).unwrap();
    let s = &raw.sections;
    // Sizes from the section map (Data size field).
    assert_eq!(s[&SectionName::Header].len(), 0x350);
    assert_eq!(s[&SectionName::Classes].len(), 0x1895);
    assert_eq!(s[&SectionName::Handles].len(), 0xBAC);
    assert_eq!(s[&SectionName::Objects].len(), 0x14_D57C);
    assert_eq!(s[&SectionName::AppInfo].len(), 0x2CE);
    assert_eq!(s[&SectionName::Template].len(), 6);
    assert!(s.contains_key(&SectionName::AppInfoHistory));
    assert!(s.contains_key(&SectionName::Other(String::new())));
}

/// The R2007 file header at 0x80 (ODS 5.2): RS(255,239) with 3 interleaved blocks, then the
/// check sequence CRC, the compressed data CRC and the header data CRC.
#[test]
fn r2007_real_crcs() {
    for (e, path) in r2007_files() {
        let bytes = std::fs::read(&path).unwrap();
        let d = codec::rs2007::decode(&bytes[0x80..0x80 + 3 * 255], 3, 239, 3).unwrap();
        let q = |at: usize| u64::from_le_bytes(d[at..at + 8].try_into().unwrap());
        let key = q(8);
        let mut seq = key.to_le_bytes().to_vec();
        seq.extend(key.rotate_left((key & 0x1F) as u32).to_le_bytes());
        assert_eq!(
            codec::crc64(codec::crc64_seed1(0, 16), &seq),
            q(0),
            "{}: check sequence",
            e.name
        );
        let n = u32_le(&d, 0x18) as usize;
        let comp = &d[0x20..0x20 + n];
        assert_eq!(
            codec::crc64(codec::crc64_seed2(0, n), comp),
            q(0x10),
            "{}: compressed",
            e.name
        );
        let mut data = codec::lz77_2007::decompress(comp, 0x110).unwrap();
        let stored = u64::from_le_bytes(data[0x108..].try_into().unwrap());
        data[0x108..].fill(0);
        assert_eq!(
            codec::crc64(codec::crc64_seed2(0, 0x110), &data),
            stored,
            "{}: header data",
            e.name
        );
        assert_eq!(
            u64::from_le_bytes(data[8..16].try_into().unwrap()),
            bytes.len() as u64,
            "{}: file size",
            e.name
        );
    }
}

#[test]
fn r2007_truncated_errors() {
    for (e, path) in r2007_files() {
        let bytes = std::fs::read(&path).unwrap();
        let whole = container::read(&bytes).unwrap();
        for n in (0..bytes.len()).step_by(0x20).chain([bytes.len() - 1]) {
            match container::read(&bytes[..n]) {
                Err(Error::Truncated { .. }) => {}
                Err(err) => panic!("{}: prefix {n:#x}: {err}", e.name),
                Ok(raw) => {
                    assert_eq!(raw.sections, whole.sections, "{}: prefix {n:#x}", e.name);
                    assert!(!raw.warnings.is_empty(), "{}: prefix {n:#x}", e.name);
                }
            }
        }
    }
}

fn q64(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

/// Decode an R2007 system page (ODS 5.3) with the public codecs: `fields` are the file header
/// offsets of its compressed size, uncompressed size and repeat count.
fn r2007_system_page(bytes: &[u8], at: usize, h: &[u8], fields: [usize; 3]) -> Vec<u8> {
    let [c, u, rep] = fields.map(|f| q64(h, f) as usize);
    let blocks = (c.div_ceil(8) * 8 * rep).div_ceil(239);
    let d = codec::rs2007::decode(&bytes[at..], blocks, 239, blocks).unwrap();
    if c < u {
        codec::lz77_2007::decompress(&d[..c], u).unwrap()
    } else {
        d[..u].to_vec()
    }
}

/// File address and compressed size of the first data page of the R2007 section `name`, found
/// through the file header, pages map and section map.
fn r2007_first_page(bytes: &[u8], name: &str) -> (usize, usize) {
    let d = codec::rs2007::decode(&bytes[0x80..0x80 + 3 * 255], 3, 239, 3).unwrap();
    let n = u32_le(&d, 0x18) as usize;
    let h = codec::lz77_2007::decompress(&d[0x20..0x20 + n], 0x110).unwrap();
    let pm = r2007_system_page(
        bytes,
        0x480 + q64(&h, 0x38) as usize,
        &h,
        [0x50, 0x58, 0x18],
    );
    let mut pages = std::collections::BTreeMap::new();
    let mut addr = 0x480;
    for e in pm.chunks_exact(16) {
        pages.insert((q64(e, 8) as i64).unsigned_abs(), addr);
        addr += q64(e, 0) as usize;
    }
    let sm_at = pages[&q64(&h, 0xC0)];
    let sm = r2007_system_page(bytes, sm_at, &h, [0xB0, 0xC8, 0xD8]);
    let mut pos = 0;
    while pos < sm.len() {
        let name_len = q64(&sm, pos + 0x20) as usize;
        let count = q64(&sm, pos + 0x38) as usize;
        let units: Vec<u16> = sm[pos + 0x40..pos + 0x40 + name_len]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        pos += 0x40 + name_len;
        if String::from_utf16_lossy(&units).trim_end_matches('\0') == name && count > 0 {
            return (pages[&q64(&sm, pos + 0x10)], q64(&sm, pos + 0x20) as usize);
        }
        pos += count * 0x38;
    }
    panic!("no section {name}");
}

#[test]
fn r2007_corrupted_but_correctable_still_reads() {
    let Some(path) = common::corpus_file_or_skip("acadsharp-sample-ac1021") else {
        return;
    };
    let bytes = std::fs::read(path).unwrap();
    let good = container::read(&bytes).unwrap();
    // The AcDb:Header data page: `blocks` interleaved RS(255,251) codewords, byte `j` of
    // codeword `i` at `j * blocks + i`.
    let (header_page, comp) = r2007_first_page(&bytes, "AcDb:Header");
    let blocks = (comp.div_ceil(8) * 8).div_ceil(251);
    assert_eq!((header_page, blocks), (0xFF700, 4)); // this sample's layout
    let flipped = |positions: &[usize]| {
        let mut bad = bytes.clone();
        for &p in positions {
            bad[p] ^= 0x5A;
        }
        container::read(&bad)
    };
    let cases: [&[usize]; 5] = [
        // One byte in a data page codeword (the brief's case), in data and in parity.
        &[header_page],
        &[header_page + 250 * blocks + 3],
        // Two bytes in one data codeword: the most RS(255,251) corrects.
        &[header_page + 1, header_page + 1 + blocks * 77],
        // Eight bytes in one codeword of the page map (6 interleaved RS(255,239) blocks at 0x480).
        &[0x480, 0x486, 0x48C, 0x492, 0x498, 0x49E, 0x4A4, 0x4AA],
        // Eight bytes in one codeword of the file header (3 interleaved blocks at 0x80).
        &[0x80, 0x83, 0x86, 0x89, 0x8C, 0x8F, 0x92, 0x95],
    ];
    for c in cases {
        let raw = flipped(c).unwrap_or_else(|err| panic!("{c:x?}: {err}"));
        assert_eq!(
            raw.sections[&SectionName::Header],
            good.sections[&SectionName::Header],
            "{c:x?}"
        );
        assert_eq!(raw.sections, good.sections, "{c:x?}");
        assert!(raw.warnings.is_empty(), "{c:x?}: {:?}", raw.warnings);
    }
    // Three bytes in one data codeword exceed the code: RS reports it, or miscorrects and the
    // page CRC catches it. Either way the damage is never silent.
    match flipped(&[
        header_page,
        header_page + blocks * 9,
        header_page + blocks * 200,
    ]) {
        // The Header is required, so an uncorrectable codeword fails the read.
        Err(e) => assert!(e.to_string().contains("Reed-Solomon"), "{e}"),
        Ok(raw) => assert!(
            raw.warnings.iter().any(|w| w.contains("AcDb:Header")),
            "{:?}",
            raw.warnings
        ),
    }
}

#[test]
fn container_roundtrip_r2018() {
    for (e, path) in common::fetched_files_for("AC1032") {
        let bytes = std::fs::read(&path).unwrap();
        let raw = container::read(&bytes).unwrap();
        let out = container::write_r2018(&raw).unwrap_or_else(|err| panic!("{}: {err}", e.name));
        let back = container::read(&out).unwrap_or_else(|err| panic!("{}: {err}", e.name));
        assert!(back.warnings.is_empty(), "{}: {:?}", e.name, back.warnings);
        assert!(back.sections == raw.sections, "{}: sections differ", e.name);
        assert_eq!(back.version, Version::R2018, "{}", e.name);
        assert_eq!(back.maintenance, raw.maintenance, "{}", e.name);
        assert_eq!(back.codepage, raw.codepage, "{}", e.name);
        assert_eq!(back.preview_offset, raw.preview_offset, "{}", e.name);
        assert_eq!(back.section_meta, raw.section_meta, "{}", e.name);
        assert_eq!(back.file_header, raw.file_header, "{}", e.name);
        // Writing what was read back gives the same file.
        assert_eq!(container::write_r2018(&back).unwrap(), out, "{}", e.name);
    }
}

/// Deterministic bytes that compress partly: runs of a repeated pattern between noise.
fn mixed_bytes(n: usize) -> Vec<u8> {
    let mut s = 0x1234_5678u32;
    (0..n)
        .map(|i| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            if (i / 0x1000) % 3 == 0 {
                (i % 251) as u8
            } else {
                (s >> 8) as u8
            }
        })
        .collect()
}

#[test]
fn container_roundtrip_large_section() {
    use std::collections::BTreeMap;
    let objects = mixed_bytes(5 << 20);
    let sections = BTreeMap::from([
        (SectionName::Header, b"header".repeat(40)),
        (SectionName::Classes, b"classes".to_vec()),
        (SectionName::Handles, vec![0, 2]),
        (SectionName::Objects, objects.clone()),
        (SectionName::SummaryInfo, vec![7; 0x90]),
        (SectionName::Other("AcDb:Custom".into()), vec![3; 0x50]),
    ]);
    let raw = container::RawSections {
        version: Version::R2018,
        maintenance: 0,
        codepage: 30,
        sections,
        objects_base: 0,
        preview_offset: None,
        section_meta: BTreeMap::new(),
        file_header: None,
        warnings: vec!["ignored on write".into()],
    };
    let out = container::write_r2018(&raw).unwrap();
    let back = container::read(&out).unwrap();
    assert!(back.warnings.is_empty(), "{:?}", back.warnings);
    // The writer adds the empty first section ODS 4.5 requires.
    let mut want = raw.sections.clone();
    want.insert(SectionName::Other(String::new()), Vec::new());
    assert!(back.sections == want, "sections differ");
    // A compressed page holds at most 0x7400 bytes, so the section spans many pages.
    let meta = back.section_meta[&SectionName::Objects];
    assert!(meta.compressed);
    assert_eq!(meta.max_page_size, 0x7400);
    assert!(objects.len() > 100 * 0x7400);
}
