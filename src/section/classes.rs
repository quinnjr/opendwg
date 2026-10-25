//! Custom classes: the `AcDb:Classes` section (ODS chapter 10 and 5.8).
//!
//! On disk the section is framed like the header (see [`super::header`]):
//! sentinels, `RL` size, the R2010+ extra `RL`, data, `RS` CRC. The data:
//!
//! ```text
//! R2007+:  RL size in bits (counted from the start of this RL)
//! R2004+:  BS max class number | RC 0 | RC 0 | B true
//!          class records until the data stream is exhausted
//! R2007+:  string stream | string stream size | B flag  (as in an object)
//! ```
//!
//! A class record is `BS` class number, `BS` proxy flags, `TV` application
//! name, `TV` C++ class name, `TV` DXF name, `B` was-a-zombie, `BS` item
//! class id, then from R2004 `BL` instance count, `BL` DWG version, `BL`
//! maintenance version and two `BL` that are normally 0. Text is code page
//! bytes inline before R2007 and `TU` in the string stream from R2007.
//!
//! ODS gives the R2004+ part twice, differently. Chapter 10.2 has `BS` max
//! class number then two `RC` 0; chapter 5.8 has a `BL`. A `BL` with a 32-bit
//! payload and a `BS` followed by two zero bytes are the same bits, so they
//! agree. For the DWG and maintenance versions chapter 10.2 says `BS` and 5.8
//! says `BL`; the values are below 256 in every corpus file, where both encode
//! alike, and the corpus shows two further `BL` (chapter 10.2's "unknown")
//! on every class, so this reads `BL` (5.8). The unknowns are kept in
//! [`DxfClass::unknown`] and written back. The R2007+ size in bits counts from the
//! start of its own `RL`, as in the R2007+ header (checked on the corpus: the
//! data are consumed exactly).
//!
//! [`encode_r2018`] pads the last byte with one bits, as AutoCAD does; the
//! classes of the R2018 corpus files re-encode byte for byte.

use crate::bits::BitReader;
use crate::diag::{Diagnostic, Severity};
use crate::error::{Error, Result};
use crate::handle::Handle;
use crate::object::stream::{ObjReader, ObjWriter, locate_strings};
use crate::version::Version;

/// Sentinel before the classes section (ODS chapter 10).
pub const START_SENTINEL: [u8; 16] = [
    0x8D, 0xA1, 0xC4, 0xB8, 0xC4, 0xA9, 0xF8, 0xC5, 0xC0, 0xDC, 0xF4, 0x5F, 0xE7, 0xCF, 0xB6, 0x8A,
];

/// Sentinel after the classes section's CRC (ODS chapter 10).
pub const END_SENTINEL: [u8; 16] = [
    0x72, 0x5E, 0x3B, 0x47, 0x3B, 0x56, 0x07, 0x3A, 0x3F, 0x23, 0x0B, 0xA0, 0x18, 0x30, 0x49, 0x75,
];

const FRAME: super::Frame = super::Frame {
    start: &START_SENTINEL,
    end: &END_SENTINEL,
    start_name: "classes start",
    end_name: "classes end",
    context: "classes",
};

/// The first class number; object type codes from 500 up name a class.
pub const FIRST_CLASS_NUMBER: u16 = 500;

/// `item_class_id` of classes whose objects are entities.
pub const ITEM_CLASS_ENTITY: u16 = 0x1F2;
/// `item_class_id` of classes whose objects are not entities.
pub const ITEM_CLASS_OBJECT: u16 = 0x1F3;

/// One custom class (DXF `CLASS`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DxfClass {
    /// The object type code of this class's objects (500 and up).
    pub class_number: u16,
    /// Proxy capability flags (DXF 90).
    pub proxy_flags: u16,
    /// The application that defines the class.
    pub app_name: String,
    /// The C++ class name.
    pub cpp_class_name: String,
    /// The class's identity: objects of this class are dispatched by it.
    pub dxf_name: String,
    /// The class's application was not loaded (a zombie).
    pub was_zombie: bool,
    /// [`ITEM_CLASS_ENTITY`] or [`ITEM_CLASS_OBJECT`]; any other value, as
    /// read from a damaged file, makes an object class (see
    /// [`DxfClass::is_entity`]), and is written as [`ITEM_CLASS_OBJECT`].
    pub item_class_id: u16,
    /// R2004+: objects of this class in the drawing (DXF 91); 0 before.
    pub instance_count: u32,
    /// R2004+; 0 before.
    pub dwg_version: u32,
    /// R2004+; 0 before.
    pub maintenance_version: u32,
    /// R2004+: the two `BL` ODS calls unknown (normally 0), kept as read.
    pub unknown: [u32; 2],
}

impl DxfClass {
    /// Whether objects of this class are entities: the item class id is
    /// [`ITEM_CLASS_ENTITY`] (any other id makes an object class).
    pub fn is_entity(&self) -> bool {
        self.item_class_id == ITEM_CLASS_ENTITY
    }
}

/// The class definitions of the class types the library models, for a
/// drawing that has no class of the type of an object added to it, and of
/// the other classes a new drawing has (`drawing::new`): DXF name, C++
/// class name, application name, proxy flags, item class, DWG version and
/// maintenance version.
///
/// Values from the R2018 corpus file `acadsharp-sample-ac1032` (checked by
/// `drawing::api`'s `known_classes_match_the_corpus`), but for three types
/// it has no class of: ARC_DIMENSION and LARGE_RADIAL_DIMENSION take ODS's
/// class properties (20.4.19, 20.4.30: "ObjectDBX Classes", 0x401, DWG
/// version R18, maintenance 0; R18 is written as 25, the DWG version of
/// the corpus's R18 classes TABLESTYLE and ACAD_TABLE); PLOTSETTINGS,
/// which neither has, takes those of LAYOUT, whose C++ class derives from
/// it. These three are for Task 26's oracle to confirm.
#[rustfmt::skip]
const KNOWN_CLASSES: [(&str, &str, &str, u16, u16, u32, u32); 29] = [
    ("ACDBDICTIONARYWDFLT", "AcDbDictionaryWithDefault", "ObjectDBX Classes", 0x0, ITEM_CLASS_OBJECT, 22, 42),
    ("ACDBPLACEHOLDER", "AcDbPlaceHolder", "ObjectDBX Classes", 0x0, ITEM_CLASS_OBJECT, 22, 42),
    ("LAYOUT", "AcDbLayout", "ObjectDBX Classes", 0x0, ITEM_CLASS_OBJECT, 22, 42),
    ("PLOTSETTINGS", "AcDbPlotSettings", "ObjectDBX Classes", 0x0, ITEM_CLASS_OBJECT, 22, 42),
    ("DICTIONARYVAR", "AcDbDictionaryVar", "ObjectDBX Classes", 0x0, ITEM_CLASS_OBJECT, 20, 0),
    ("TABLESTYLE", "AcDbTableStyle", "ObjectDBX Classes", 0xFFF, ITEM_CLASS_OBJECT, 25, 20),
    ("SCALE", "AcDbScale", "ObjectDBX Classes", 0x481, ITEM_CLASS_OBJECT, 27, 1),
    ("MLEADERSTYLE", "AcDbMLeaderStyle", "ACDB_MLEADERSTYLE_CLASS", 0xFFF, ITEM_CLASS_OBJECT, 33, 329),
    ("LAYER_INDEX", "AcDbLayerIndex", "ObjectDBX Classes", 0x0, ITEM_CLASS_OBJECT, 20, 0),
    ("SPATIAL_INDEX", "AcDbSpatialIndex", "ObjectDBX Classes", 0x0, ITEM_CLASS_OBJECT, 20, 0),
    ("SPATIAL_FILTER", "AcDbSpatialFilter", "ObjectDBX Classes", 0x0, ITEM_CLASS_OBJECT, 20, 0),
    ("SORTENTSTABLE", "AcDbSortentsTable", "ObjectDBX Classes", 0x0, ITEM_CLASS_OBJECT, 20, 0),
    ("RASTERVARIABLES", "AcDbRasterVariables", "ISM", 0x0, ITEM_CLASS_OBJECT, 20, 0),
    ("IMAGEDEF", "AcDbRasterImageDef", "ISM", 0x0, ITEM_CLASS_OBJECT, 20, 0),
    ("IMAGEDEF_REACTOR", "AcDbRasterImageDefReactor", "ISM", 0x1, ITEM_CLASS_OBJECT, 20, 0),
    ("IMAGE", "AcDbRasterImage", "ISM", 0x87F, ITEM_CLASS_ENTITY, 20, 0),
    ("WIPEOUTVARIABLES", "AcDbWipeoutVariables", "\"WipeOut|Product Desc:     WipeOut Dbx Application|Company:          Autodesk, Inc.|WEB Address:      www.autodesk.com\"", 0x0, ITEM_CLASS_OBJECT, 25, 0),
    ("WIPEOUT", "AcDbWipeout", "WipeOut|Product Desc: Object Enabler for WipeOut entity | Company: Autodesk, Inc. | WEB Address: www.autodesk.com", 0x87F, ITEM_CLASS_ENTITY, 33, 329),
    ("MULTILEADER", "AcDbMLeader", "ACDB_MLEADER_CLASS", 0x401, ITEM_CLASS_ENTITY, 33, 329),
    ("MATERIAL", "AcDbMaterial", "ObjectDBX Classes", 0x481, ITEM_CLASS_OBJECT, 23, 14),
    ("VISUALSTYLE", "AcDbVisualStyle", "ObjectDBX Classes", 0xFFF, ITEM_CLASS_OBJECT, 26, 0),
    ("CELLSTYLEMAP", "AcDbCellStyleMap", "ObjectDBX Classes", 0x480, ITEM_CLASS_OBJECT, 27, 1),
    ("EXACXREFPANELOBJECT", "ExAcXREFPanelObject", "EXAC_ESW", 0x401, ITEM_CLASS_OBJECT, 0, 0),
    ("NPOCOLLECTION", "AcDbImpNonPersistentObjectsCollection", "ObjectDBX Classes", 0x481, ITEM_CLASS_OBJECT, 26, 0),
    ("IDBUFFER", "AcDbIdBuffer", "ObjectDBX Classes", 0x0, ITEM_CLASS_OBJECT, 20, 0),
    ("ACDBSECTIONVIEWSTYLE", "AcDbSectionViewStyle", "ObjectDBX Classes", 0x401, ITEM_CLASS_OBJECT, 27, 50),
    ("ACDBDETAILVIEWSTYLE", "AcDbDetailViewStyle", "ObjectDBX Classes", 0x401, ITEM_CLASS_OBJECT, 33, 329),
    ("ARC_DIMENSION", "AcDbArcDimension", "ObjectDBX Classes", 0x401, ITEM_CLASS_ENTITY, 25, 0),
    ("LARGE_RADIAL_DIMENSION", "AcDbRadialDimensionLarge", "ObjectDBX Classes", 0x401, ITEM_CLASS_ENTITY, 25, 0),
];

/// The definition of the class named `dxf_name` among the class types the
/// library models (see [`KNOWN_CLASSES`]), numbered `number`, with no
/// instances.
pub(crate) fn known_class(dxf_name: &str, number: u16) -> Option<DxfClass> {
    let &(dxf, cpp, app, proxy, item, ver, maint) =
        KNOWN_CLASSES.iter().find(|c| c.0 == dxf_name)?;
    Some(DxfClass {
        class_number: number,
        proxy_flags: proxy,
        app_name: app.into(),
        cpp_class_name: cpp.into(),
        dxf_name: dxf.into(),
        was_zombie: false,
        item_class_id: item,
        instance_count: 0,
        dwg_version: ver,
        maintenance_version: maint,
        unknown: [0, 0],
    })
}

fn corrupt(msg: &str) -> Error {
    Error::Corrupt(format!("classes: {msg}"))
}

/// A value that cannot be encoded (see [`Error::Encode`]).
fn encode_err(msg: &str) -> Error {
    Error::Encode(format!("classes: {msg}"))
}

/// Decode the `AcDb:Classes` section (sentinels through end sentinel).
/// `codepage` decodes R2000/R2004 text.
pub fn decode(bytes: &[u8], version: Version, codepage: u16) -> Result<Vec<DxfClass>> {
    decode_with_diagnostics(bytes, version, codepage).map(|(c, _)| c)
}

/// Like [`decode`], also returning problems that did not stop decoding
/// (lossy text, unread bits, unexpected values). They carry no handle.
pub fn decode_with_diagnostics(
    bytes: &[u8],
    version: Version,
    codepage: u16,
) -> Result<(Vec<DxfClass>, Vec<Diagnostic>)> {
    decode_counted(bytes, version, codepage, &mut ClassNumbers::new())
}

/// The class numbers seen so far: a bitmap of all 65536 numbers, so checking
/// one for a duplicate is a single word access however many classes there
/// are. `probes` counts the checks.
struct ClassNumbers {
    bits: Vec<u64>,
    probes: usize,
}

impl ClassNumbers {
    fn new() -> Self {
        ClassNumbers {
            bits: vec![0; 1 << 10],
            probes: 0,
        }
    }

    /// Record `n`; `false` if it was already recorded.
    fn insert(&mut self, n: u16) -> bool {
        self.probes += 1;
        let bit = 1u64 << (n & 63);
        // `n >> 6` is below 1024, the bitmap's length.
        let Some(word) = self.bits.get_mut(usize::from(n >> 6)) else {
            return false;
        };
        let new = *word & bit == 0;
        *word |= bit;
        new
    }
}

fn decode_counted(
    bytes: &[u8],
    version: Version,
    codepage: u16,
    numbers: &mut ClassNumbers,
) -> Result<(Vec<DxfClass>, Vec<Diagnostic>)> {
    let body = super::unframe(bytes, version, &FRAME)?;
    let mut r = if version >= Version::R2007 {
        let mut head = BitReader::new(body);
        let data_end = u64::from(head.read_rl()? as u32);
        let header_end = head.bit_pos();
        if data_end > (body.len() as u64) * 8 {
            return Err(corrupt("size in bits exceeds the section"));
        }
        let (data_limit, strings) = locate_strings(body, header_end, data_end)?;
        let mut data = BitReader::with_bit_limit(body, data_limit);
        data.seek_bit(header_end)?;
        ObjReader::from_streams(data, Some(strings), BitReader::new(&[]), version, codepage)
    } else {
        let empty = BitReader::new(&[]);
        ObjReader::from_streams(BitReader::new(body), None, empty, version, codepage)
    };
    let mut extra = Vec::new();
    if version >= Version::R2004 {
        let _max_class_number = r.data.read_bs()?;
        let zeros = [r.data.read_rc()?, r.data.read_rc()?];
        // AutoCAD 2007 and later set this bit; AutoCAD 2004 left it clear
        // (`arx-05`, `arx-09`, `arx-10` of the extended corpus).
        let _flag = r.data.read_b()?;
        if zeros != [0, 0] {
            extra.push(format!("classes: preamble {zeros:02X?} differs from 00 00"));
        }
    }
    let mut classes = Vec::new();
    // Before R2007 the data end on a byte boundary; fewer than 8 bits left are
    // padding. From R2007 the class data end where the string stream starts.
    let padding = if version >= Version::R2007 { 0 } else { 7 };
    while r.data.remaining_bits() > padding {
        let class_number = r.data.read_bs()? as u16;
        let proxy_flags = r.data.read_bs()? as u16;
        let app_name = r.text()?;
        let cpp_class_name = r.text()?;
        let dxf_name = r.text()?;
        let was_zombie = r.data.read_b()?;
        let item_class_id = r.data.read_bs()? as u16;
        let (mut instance_count, mut dwg_version, mut maintenance_version) = (0, 0, 0);
        let mut unknown = [0; 2];
        if version >= Version::R2004 {
            instance_count = r.data.read_bl()? as u32;
            dwg_version = r.data.read_bl()? as u32;
            maintenance_version = r.data.read_bl()? as u32;
            unknown = [r.data.read_bl()? as u32, r.data.read_bl()? as u32];
        }
        if class_number < FIRST_CLASS_NUMBER {
            return Err(corrupt(&format!("class number {class_number} below 500")));
        }
        if !numbers.insert(class_number) {
            extra.push(format!(
                "classes: duplicate class number {class_number} ({dxf_name}) dropped; the \
                 first one is kept"
            ));
            continue;
        }
        classes.push(DxfClass {
            class_number,
            proxy_flags,
            app_name,
            cpp_class_name,
            dxf_name,
            was_zombie,
            item_class_id,
            instance_count,
            dwg_version,
            maintenance_version,
            unknown,
        });
    }
    let left_strings = r.strings.as_ref().map_or(0, BitReader::remaining_bits);
    let mut diagnostics = r.diagnostics;
    for d in &mut diagnostics {
        d.handle = None;
    }
    if left_strings > 0 {
        extra.push(format!("classes: {left_strings} string stream bits unread"));
    }
    diagnostics.extend(extra.into_iter().map(|message| Diagnostic {
        severity: Severity::Warning,
        handle: None,
        message,
    }));
    Ok((classes, diagnostics))
}

/// Encode the classes section in R2018 form, including the 8 zero bytes
/// R2004+ files have after the end sentinel.
pub fn encode_r2018(classes: &[DxfClass]) -> Result<Vec<u8>> {
    let mut w = ObjWriter::new(Handle(0));
    let max = classes
        .iter()
        .map(|c| c.class_number)
        .max()
        .unwrap_or(FIRST_CLASS_NUMBER - 1);
    w.data.write_bs(max as i16);
    w.data.write_rc(0);
    w.data.write_rc(0);
    w.data.write_b(true);
    for c in classes {
        w.data.write_bs(c.class_number as i16);
        w.data.write_bs(c.proxy_flags as i16);
        w.text(&c.app_name)?;
        w.text(&c.cpp_class_name)?;
        w.text(&c.dxf_name)?;
        w.data.write_b(c.was_zombie);
        // The kind the reader gives the class: an id other than entity's
        // is read as an object class, and written as one.
        let kind = if c.is_entity() {
            ITEM_CLASS_ENTITY
        } else {
            ITEM_CLASS_OBJECT
        };
        w.data.write_bs(kind as i16);
        w.data.write_bl(c.instance_count as i32);
        w.data.write_bl(c.dwg_version as i32);
        w.data.write_bl(c.maintenance_version as i32);
        w.data.write_bl(c.unknown[0] as i32);
        w.data.write_bl(c.unknown[1] as i32);
    }
    let mut main = crate::bits::BitWriter::new();
    main.write_rl(0); // size in bits, patched below
    let data_end = w.write_streams(&mut main)?;
    let bits = i32::try_from(data_end).map_err(|_| encode_err("data too large"))?;
    main.patch_rl_at(0, bits)?;
    // AutoCAD pads the last byte with one bits (every corpus file).
    while main.bit_len() % 8 != 0 {
        main.write_b(true);
    }
    let mut out = super::frame_r2018(&main.into_bytes(), &FRAME)?;
    out.extend_from_slice(&[0; 8]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::BitWriter;
    use crate::codec::crc16;
    use crate::section::CRC_SEED;

    fn class(n: u16, name: &str, entity: bool) -> DxfClass {
        DxfClass {
            class_number: n,
            proxy_flags: 0x481,
            app_name: "ObjectDBX Classes".into(),
            cpp_class_name: format!("AcDb{name}"),
            dxf_name: name.into(),
            was_zombie: n % 2 == 1,
            item_class_id: if entity {
                ITEM_CLASS_ENTITY
            } else {
                ITEM_CLASS_OBJECT
            },
            instance_count: 3,
            dwg_version: 33,
            maintenance_version: 329,
            unknown: [0, u32::from(n)],
        }
    }

    fn sample() -> Vec<DxfClass> {
        vec![
            class(500, "ACDBDICTIONARYWDFLT", false),
            class(501, "WIPEOUT", true),
            class(502, "名前", false),
        ]
    }

    #[test]
    fn roundtrip_r2018() {
        let bytes = encode_r2018(&sample()).unwrap();
        assert!(bytes.ends_with(&[0; 8]));
        let (back, diags) = decode_with_diagnostics(&bytes, Version::R2018, 30).unwrap();
        assert_eq!(back, sample());
        assert!(diags.is_empty(), "{diags:?}");
        assert!(back[1].is_entity() && !back[0].is_entity());
    }

    #[test]
    fn roundtrip_empty() {
        let bytes = encode_r2018(&[]).unwrap();
        assert_eq!(decode(&bytes, Version::R2018, 30).unwrap(), []);
    }

    /// An R2000 section: sentinels, RL size, inline class records, CRC.
    fn r2000_section(records: &[(u16, &[u8])]) -> Vec<u8> {
        let mut w = BitWriter::new();
        for &(n, name) in records {
            w.write_bs(n as i16);
            w.write_bs(0);
            w.write_tv_raw(b"App").unwrap();
            w.write_tv_raw(b"AcDbThing").unwrap();
            w.write_tv_raw(name).unwrap();
            w.write_b(false);
            w.write_bs(ITEM_CLASS_OBJECT as i16);
        }
        frame_r2000(&w.into_bytes())
    }

    fn frame_r2000(body: &[u8]) -> Vec<u8> {
        let mut out = START_SENTINEL.to_vec();
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(body);
        let crc = crc16(CRC_SEED, &out[16..]);
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&END_SENTINEL);
        out
    }

    /// An R2004 section's preamble: AutoCAD 2004 writes the bit after the
    /// two zero bytes clear (`arx-05`, `arx-09`, `arx-10` of the extended
    /// corpus), AutoCAD 2007 and later set; either reads without a warning.
    /// Non-zero bytes there are reported.
    #[test]
    fn r2004_preamble_flag_either_way() {
        for (zeros, flag, warned) in [
            ([0, 0], true, false),
            ([0, 0], false, false),
            ([0, 1], true, true),
        ] {
            let mut w = BitWriter::new();
            w.write_bs(500);
            w.write_rc(zeros[0]);
            w.write_rc(zeros[1]);
            w.write_b(flag);
            w.write_bs(500);
            w.write_bs(0);
            w.write_tv_raw(b"App").unwrap();
            w.write_tv_raw(b"AcDbThing").unwrap();
            w.write_tv_raw(b"THING").unwrap();
            w.write_b(false);
            w.write_bs(ITEM_CLASS_OBJECT as i16);
            for v in [1, 0, 0, 0, 0] {
                w.write_bl(v);
            }
            let bytes = frame_r2000(&w.into_bytes());
            let (c, diags) = decode_with_diagnostics(&bytes, Version::R2004, 30).unwrap();
            assert_eq!(c.len(), 1);
            assert_eq!(c[0].dxf_name, "THING");
            assert_eq!(!diags.is_empty(), warned, "{zeros:?} {flag}: {diags:?}");
        }
    }

    #[test]
    fn r2000_inline_text_uses_codepage() {
        // 0x82 0xA0 is "あ" in Shift-JIS (code page 38).
        let bytes = r2000_section(&[(500, b"A"), (501, &[0x82, 0xA0])]);
        let c = decode(&bytes, Version::R2000, 38).unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(c[1].dxf_name, "あ");
        assert_eq!(c[1].app_name, "App");
        assert_eq!((c[1].instance_count, c[1].dwg_version), (0, 0));
    }

    #[test]
    fn hostile_truncated() {
        let bytes = encode_r2018(&sample()).unwrap();
        // Without the 8 trailing bytes the section is still whole.
        for n in 0..bytes.len() - 8 {
            assert!(
                decode(&bytes[..n], Version::R2018, 30).is_err(),
                "prefix {n}"
            );
        }
        let bytes = r2000_section(&[(500, b"A")]);
        for n in 0..bytes.len() {
            assert!(
                decode(&bytes[..n], Version::R2000, 30).is_err(),
                "prefix {n}"
            );
        }
    }

    #[test]
    fn hostile_bad_crc_and_sentinels() {
        let good = encode_r2018(&sample()).unwrap();
        let mut b = good.clone();
        b[30] ^= 0x10;
        assert!(matches!(
            decode(&b, Version::R2018, 30),
            Err(Error::BadCrc { .. })
        ));
        let mut b = good.clone();
        b[0] ^= 1;
        assert!(matches!(
            decode(&b, Version::R2018, 30),
            Err(Error::BadSentinel("classes start"))
        ));
        let mut b = good;
        let at = b.len() - 9;
        b[at] ^= 1;
        assert!(matches!(
            decode(&b, Version::R2018, 30),
            Err(Error::BadSentinel("classes end"))
        ));
    }

    /// Re-frame an R2018 section after editing its body.
    fn reframe(body: &[u8]) -> Vec<u8> {
        crate::section::frame_r2018(body, &FRAME).unwrap()
    }

    #[test]
    fn hostile_absurd_sizes() {
        // Size in bits far beyond the section.
        let bytes = encode_r2018(&sample()).unwrap();
        let mut body = bytes[24..bytes.len() - 26].to_vec();
        body[..4].copy_from_slice(&0x7FFF_FFF0u32.to_le_bytes());
        assert!(matches!(
            decode(&reframe(&body), Version::R2018, 30),
            Err(Error::Corrupt(_))
        ));
        // Section size far beyond the data.
        let mut b = bytes.clone();
        b[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode(&b, Version::R2018, 30).is_err());
        // A huge max class number is only a number: nothing is sized by it.
        let mut w = ObjWriter::new(Handle(0));
        w.data.write_bs(0x7FFF);
        w.data.write_rc(0);
        w.data.write_rc(0);
        w.data.write_b(true);
        let mut main = BitWriter::new();
        main.write_rl(0);
        let end = w.write_streams(&mut main).unwrap();
        main.patch_rl_at(0, end as i32).unwrap();
        assert_eq!(
            decode(&reframe(&main.into_bytes()), Version::R2018, 30).unwrap(),
            []
        );
    }

    #[test]
    fn hostile_class_number_below_500() {
        let bytes = r2000_section(&[(499, b"A")]);
        assert!(matches!(
            decode(&bytes, Version::R2000, 30),
            Err(Error::Corrupt(_))
        ));
    }

    #[test]
    fn duplicate_class_numbers_keep_the_first() {
        let bytes = r2000_section(&[(500, b"A"), (500, b"B"), (501, b"C")]);
        let (c, diags) = decode_with_diagnostics(&bytes, Version::R2000, 30).unwrap();
        let names: Vec<_> = c.iter().map(|c| c.dxf_name.as_str()).collect();
        assert_eq!(names, ["A", "C"]);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("duplicate class number 500"));
    }

    #[test]
    fn many_duplicates_stay_linear() {
        let records: Vec<(u16, &[u8])> = (0..200_000).map(|_| (500, &b""[..])).collect();
        let bytes = r2000_section(&records);
        let mut numbers = ClassNumbers::new();
        let (c, diags) = decode_counted(&bytes, Version::R2000, 30, &mut numbers).unwrap();
        // Work count: one constant-time duplicate check per record (a
        // search of the kept classes would be 200k x 200k).
        assert_eq!(numbers.probes, 200_000);
        assert_eq!(c.len(), 1);
        assert_eq!(diags.len(), 199_999);
    }

    #[test]
    fn corrupted_bytes_never_panic() {
        let good = encode_r2018(&sample()).unwrap();
        for i in 16..good.len() - 8 {
            for bit in [0x01u8, 0x80] {
                let mut b = good.clone();
                b[i] ^= bit;
                // Re-frame so the CRC passes and the data are decoded.
                let size = u32::from_le_bytes([b[16], b[17], b[18], b[19]]) as usize;
                if let Some(body) = b.get(24..24 + size) {
                    let _ = decode(&reframe(body), Version::R2018, 30);
                }
                let _ = decode(&b, Version::R2018, 30);
            }
        }
    }
}
