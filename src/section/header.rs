//! Header variables: the `AcDb:Header` section (ODS chapter 9).
//!
//! On disk:
//!
//! ```text
//! start sentinel (16) | RL size | [RL 0] | data (size bytes) | RS CRC | end sentinel (16)
//! ```
//!
//! The extra `RL` (zero, perhaps the high half of a 64-bit size) is present in
//! R2018 files and in R2010/R2013 files whose maintenance version is over 3.
//! [`decode`] has no maintenance version, so for R2010/R2013 it takes the
//! layout whose end sentinel sits where the size says. The CRC covers the
//! bytes from the size to the end of the data (ODS: "starting after the
//! sentinel"), seed 0xC0C1.
//!
//! The data are bit-coded variables in the order of ODS chapter 9. From R2007
//! the section is laid out like an object (ODS 5.9): the data start with an
//! `RL` size in bits, counted from the start of that `RL`, which ends the
//! data stream; the string stream sits at the end of the data stream and is
//! located from there exactly as an object's (see [`crate::object::stream`]);
//! the handle stream follows the data stream. `HANDSEED` stays in the data
//! stream. Before R2007 text and handles are inline in the one stream.
//!
//! The field order follows ODS, checked on every corpus version: the data
//! are consumed exactly (no bit left over in the data and string streams,
//! fewer than 8 padding bits at the end), which fails on any misaligned
//! field. A header with bits left over still decodes: other writers may pad
//! differently, and failing the header fails the whole read. The leftover
//! counts come back in [`UnreadBits`] with a warning diagnostic. Where
//! the corpus contradicts ODS, ACadSharp (MIT; `DwgHeaderReader.Read` and
//! `DwgHeaderWriter.Write`) was consulted and the corpus decides:
//!
//! - The R2007+ size in bits counts from the start of its own `RL` (ODS does
//!   not say), as in ACadSharp.
//! - `BLL` ($REQUIREDVERSIONS) has a plain 3-bit length, not ODS's 3B (see
//!   [`crate::bits::BitReader::read_bll`]).
//! - `MENUNAME` is stored in every version, not only up to R2004 as ODS says:
//!   R2007+ string streams have exactly one more string than ODS lists, and
//!   ACadSharp's writer emits `MENUNAME` for every version. A `TV` takes no
//!   data stream bits from R2007, so only its string stream position (after
//!   `CELTSCALE`, by ODS) is assumed; every later string decodes sensibly.
//! - ODS lists four trailing "type 5/6 only" `BS` for R14+ that "do not seem
//!   to be required"; every corpus file has them. R2004+ adds `BL`, `BL`, `B`
//!   after them, which ODS omits (ACadSharp's writer writes them). From R2007
//!   these seven fields follow the handle stream rather than sitting in the
//!   data stream, which ODS does not say either.
//! - ODS defaults 24 and "" for the first unknown `BL` and `TV`; every corpus
//!   file has 0 and "m".
//!
//! The fields ODS calls "unknown" are kept in [`UnknownHeaderFields`] so a
//! header round-trips exactly. The end padding is written as zero bits;
//! AutoCAD's R2007 and R2010 samples pad with one bits, and both R2018
//! samples end on a byte boundary, so they have no padding.

use crate::bits::{BitReader, BitWriter, Color};
use crate::diag::{Diagnostic, Severity};
use crate::error::{Error, Result};
use crate::handle::{Handle, HandleRef};
use crate::object::stream::{ObjReader, ObjWriter, locate_strings, resolve};
use crate::version::Version;

/// Sentinel before the header section (ODS chapter 9).
pub const START_SENTINEL: [u8; 16] = [
    0xCF, 0x7B, 0x1F, 0x23, 0xFD, 0xDE, 0x38, 0xA9, 0x5F, 0x7C, 0x68, 0xB8, 0x4E, 0x6D, 0x33, 0x5F,
];

/// Sentinel after the header section's CRC (ODS chapter 9).
pub const END_SENTINEL: [u8; 16] = [
    0x30, 0x84, 0xE0, 0xDC, 0x02, 0x21, 0xC7, 0x56, 0xA0, 0x83, 0x97, 0x47, 0xB1, 0x92, 0xCC, 0xA0,
];

/// Handle reference codes (ODS 2.13).
const HARD_OWNER: u8 = 3;
const HARD_POINTER: u8 = 5;

fn corrupt(msg: &str) -> Error {
    Error::Corrupt(format!("header: {msg}"))
}

/// A value that cannot be encoded (see [`Error::Encode`]).
fn encode_err(msg: &str) -> Error {
    Error::Encode(format!("header: {msg}"))
}

/// A date or a duration: a Julian day number (or a count of days) and the
/// milliseconds into the day, each a `BL`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct JulianDate {
    pub day: i32,
    pub ms: i32,
}

/// The header fields ODS calls "unknown", in ODS order, kept so a header
/// round-trips exactly. Slots that exist only from some version are `Option`.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct UnknownHeaderFields {
    /// ODS default 412148564080.0.
    pub unknown_bd1: f64,
    /// ODS default 1.0.
    pub unknown_bd2: f64,
    /// ODS default 1.0.
    pub unknown_bd3: f64,
    /// ODS default 1.0.
    pub unknown_bd4: f64,
    pub unknown_tv1: String,
    pub unknown_tv2: String,
    pub unknown_tv3: String,
    pub unknown_tv4: String,
    /// ODS default 24.
    pub unknown_bl1: i32,
    /// ODS default 0.
    pub unknown_bl2: i32,
    /// R2004+: a `B` after `LIMCHECK`.
    pub unknown_b1: Option<bool>,
    /// R2004+: three `BL` after `PDMODE`.
    pub unknown_bl3: Option<i32>,
    pub unknown_bl4: Option<i32>,
    pub unknown_bl5: Option<i32>,
    /// R2004+: three `BL` after `TDUPDATE`.
    pub unknown_bl6: Option<i32>,
    pub unknown_bl7: Option<i32>,
    pub unknown_bl8: Option<i32>,
    /// R2013+: a hard pointer after `DICTIONARY (VISUALSTYLE)`.
    pub unknown_h1: Option<Handle>,
    /// Bits of the R2000+ flags `BL` (the one holding `CELWEIGHT` ...
    /// `OLESTARTUP`) that ODS does not name: 0x1000 and 0x8000 upwards.
    pub flags_other_bits: i32,
    /// R2007+: two `BL` and a `BD` after `CAMERADISPLAY`.
    pub unknown_bl9: Option<i32>,
    pub unknown_bl10: Option<i32>,
    pub unknown_bd5: Option<f64>,
    /// R2007+: a `B` after `DGNFRAME`.
    pub unknown_b2: Option<bool>,
    /// R2007+: the last `BD`, after `CSHADOW` (ACadSharp names it
    /// `SHADOWPLANELOCATION`).
    pub unknown_bd6: Option<f64>,
    /// Four `BS` that ODS lists last ("type 5/6 only ... do not seem to be
    /// required"). Every corpus file has them; from R2007 they follow the
    /// handle stream.
    pub unknown_bs1: i16,
    pub unknown_bs2: i16,
    pub unknown_bs3: i16,
    pub unknown_bs4: i16,
    /// R2004+: `BL`, `BL`, `B` after the four `BS` (not in ODS; ACadSharp
    /// writes them, and every R2004+ corpus file has them).
    pub unknown_bl11: Option<i32>,
    pub unknown_bl12: Option<i32>,
    pub unknown_b3: Option<bool>,
}

/// The header (system) variables of a drawing, in ODS order. Fields are named
/// after their `$VAR` (lowercased); paper space copies use the DXF `$P...`
/// names. Fields stored only from some version are `Option` (`None` when the
/// file is older); `viewport_entity_header` and `vpentity_control` exist only
/// in R2000 and are dropped when writing R2018.
///
/// Handles are absolute. Defaults for a new drawing come from
/// [`defaults_r2018`].
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct HeaderVars {
    /// `$DWGCODEPAGE`: stored in the file header, not in this section;
    /// [`decode`] sets it from its `codepage` argument.
    pub dwgcodepage: u16,
    /// `$MEASUREMENT`: stored in the `AcDb:Template` section, not in this
    /// section; [`decode`] leaves it `None`, and `Drawing::from_bytes` sets
    /// it from the Template section (`None` without a readable one).
    pub measurement: Option<i16>,

    /// R2013+.
    pub requiredversions: Option<u64>,
    pub unknown: UnknownHeaderFields,
    /// R2000 only: the current viewport entity header (hard pointer).
    pub viewport_entity_header: Option<Handle>,
    pub dimaso: bool,
    pub dimsho: bool,
    pub plinegen: bool,
    pub orthomode: bool,
    pub regenmode: bool,
    pub fillmode: bool,
    pub qtextmode: bool,
    pub psltscale: bool,
    pub limcheck: bool,
    pub usrtimer: bool,
    pub skpoly: bool,
    pub angdir: bool,
    pub splframe: bool,
    pub mirrtext: bool,
    pub worldview: bool,
    pub tilemode: bool,
    pub plimcheck: bool,
    pub visretain: bool,
    pub dispsilh: bool,
    /// Not in DXF.
    pub pellipse: bool,
    pub proxygraphics: i16,
    pub treedepth: i16,
    pub lunits: i16,
    pub luprec: i16,
    pub aunits: i16,
    pub auprec: i16,
    pub attmode: i16,
    pub pdmode: i16,
    pub useri1: i16,
    pub useri2: i16,
    pub useri3: i16,
    pub useri4: i16,
    pub useri5: i16,
    pub splinesegs: i16,
    pub surfu: i16,
    pub surfv: i16,
    pub surftype: i16,
    pub surftab1: i16,
    pub surftab2: i16,
    pub splinetype: i16,
    pub shadedge: i16,
    pub shadedif: i16,
    pub unitmode: i16,
    pub maxactvp: i16,
    pub isolines: i16,
    pub cmljust: i16,
    pub textqlty: i16,
    pub ltscale: f64,
    pub textsize: f64,
    pub tracewid: f64,
    pub sketchinc: f64,
    pub filletrad: f64,
    pub thickness: f64,
    pub angbase: f64,
    pub pdsize: f64,
    pub plinewid: f64,
    pub userr1: f64,
    pub userr2: f64,
    pub userr3: f64,
    pub userr4: f64,
    pub userr5: f64,
    pub chamfera: f64,
    pub chamferb: f64,
    pub chamferc: f64,
    pub chamferd: f64,
    pub facetres: f64,
    pub cmlscale: f64,
    pub celtscale: f64,
    /// ODS lists it for R13-R2004 only, but R2007+ string streams hold it
    /// too (see the module docs).
    pub menuname: String,
    pub tdcreate: JulianDate,
    pub tdupdate: JulianDate,
    /// A duration: days and milliseconds.
    pub tdindwg: JulianDate,
    /// A duration: days and milliseconds.
    pub tdusrtimer: JulianDate,
    pub cecolor: Color,
    /// The next handle to allocate. Stored in the data stream in every
    /// version.
    pub handseed: Handle,
    pub clayer: Handle,
    pub textstyle: Handle,
    pub celtype: Handle,
    /// R2007+.
    pub cmaterial: Option<Handle>,
    pub dimstyle: Handle,
    pub cmlstyle: Handle,
    pub psvpscale: f64,
    pub pinsbase: [f64; 3],
    pub pextmin: [f64; 3],
    pub pextmax: [f64; 3],
    pub plimmin: [f64; 2],
    pub plimmax: [f64; 2],
    pub pelevation: f64,
    pub pucsorg: [f64; 3],
    pub pucsxdir: [f64; 3],
    pub pucsydir: [f64; 3],
    pub pucsname: Handle,
    pub pucsorthoref: Handle,
    pub pucsorthoview: i16,
    pub pucsbase: Handle,
    pub pucsorgtop: [f64; 3],
    pub pucsorgbottom: [f64; 3],
    pub pucsorgleft: [f64; 3],
    pub pucsorgright: [f64; 3],
    pub pucsorgfront: [f64; 3],
    pub pucsorgback: [f64; 3],
    pub insbase: [f64; 3],
    pub extmin: [f64; 3],
    pub extmax: [f64; 3],
    pub limmin: [f64; 2],
    pub limmax: [f64; 2],
    pub elevation: f64,
    pub ucsorg: [f64; 3],
    pub ucsxdir: [f64; 3],
    pub ucsydir: [f64; 3],
    pub ucsname: Handle,
    pub ucsorthoref: Handle,
    pub ucsorthoview: i16,
    pub ucsbase: Handle,
    pub ucsorgtop: [f64; 3],
    pub ucsorgbottom: [f64; 3],
    pub ucsorgleft: [f64; 3],
    pub ucsorgright: [f64; 3],
    pub ucsorgfront: [f64; 3],
    pub ucsorgback: [f64; 3],
    pub dimpost: String,
    pub dimapost: String,
    pub dimscale: f64,
    pub dimasz: f64,
    pub dimexo: f64,
    pub dimdli: f64,
    pub dimexe: f64,
    pub dimrnd: f64,
    pub dimdle: f64,
    pub dimtp: f64,
    pub dimtm: f64,
    /// R2007+.
    pub dimfxl: Option<f64>,
    /// R2007+.
    pub dimjogang: Option<f64>,
    /// R2007+.
    pub dimtfill: Option<i16>,
    /// R2007+.
    pub dimtfillclr: Option<Color>,
    pub dimtol: bool,
    pub dimlim: bool,
    pub dimtih: bool,
    pub dimtoh: bool,
    pub dimse1: bool,
    pub dimse2: bool,
    pub dimtad: i16,
    pub dimzin: i16,
    pub dimazin: i16,
    /// R2007+.
    pub dimarcsym: Option<i16>,
    pub dimtxt: f64,
    pub dimcen: f64,
    pub dimtsz: f64,
    pub dimaltf: f64,
    pub dimlfac: f64,
    pub dimtvp: f64,
    pub dimtfac: f64,
    pub dimgap: f64,
    pub dimaltrnd: f64,
    pub dimalt: bool,
    pub dimaltd: i16,
    pub dimtofl: bool,
    pub dimsah: bool,
    pub dimtix: bool,
    pub dimsoxd: bool,
    pub dimclrd: Color,
    pub dimclre: Color,
    pub dimclrt: Color,
    pub dimadec: i16,
    pub dimdec: i16,
    pub dimtdec: i16,
    pub dimaltu: i16,
    pub dimalttd: i16,
    pub dimaunit: i16,
    pub dimfrac: i16,
    pub dimlunit: i16,
    pub dimdsep: i16,
    pub dimtmove: i16,
    pub dimjust: i16,
    pub dimsd1: bool,
    pub dimsd2: bool,
    pub dimtolj: i16,
    pub dimtzin: i16,
    pub dimaltz: i16,
    pub dimalttz: i16,
    pub dimupt: bool,
    pub dimatfit: i16,
    /// R2007+.
    pub dimfxlon: Option<bool>,
    /// R2010+.
    pub dimtxtdirection: Option<bool>,
    /// R2010+.
    pub dimaltmzf: Option<f64>,
    /// R2010+.
    pub dimaltmzs: Option<String>,
    /// R2010+.
    pub dimmzf: Option<f64>,
    /// R2010+.
    pub dimmzs: Option<String>,
    pub dimtxsty: Handle,
    pub dimldrblk: Handle,
    pub dimblk: Handle,
    pub dimblk1: Handle,
    pub dimblk2: Handle,
    /// R2007+.
    pub dimltype: Option<Handle>,
    /// R2007+.
    pub dimltex1: Option<Handle>,
    /// R2007+.
    pub dimltex2: Option<Handle>,
    pub dimlwd: i16,
    pub dimlwe: i16,
    pub block_control: Handle,
    pub layer_control: Handle,
    pub style_control: Handle,
    pub linetype_control: Handle,
    pub view_control: Handle,
    pub ucs_control: Handle,
    pub vport_control: Handle,
    pub appid_control: Handle,
    pub dimstyle_control: Handle,
    /// R2000 only: the VIEWPORT ENTITY HEADER control object.
    pub vpentity_control: Option<Handle>,
    pub dict_acad_group: Handle,
    pub dict_acad_mlinestyle: Handle,
    pub dict_named_objects: Handle,
    /// Not in DXF; default 1.
    pub tstackalign: i16,
    /// Not in DXF; default 70.
    pub tstacksize: i16,
    pub hyperlinkbase: String,
    pub stylesheet: String,
    pub dict_layouts: Handle,
    pub dict_plotsettings: Handle,
    pub dict_plotstyles: Handle,
    /// R2004+.
    pub dict_materials: Option<Handle>,
    /// R2004+.
    pub dict_colors: Option<Handle>,
    /// R2007+.
    pub dict_visualstyle: Option<Handle>,
    /// Flags `BL` & 0x1F (a lineweight index).
    pub celweight: u8,
    /// Flags `BL` & 0x60, shifted down.
    pub endcaps: u8,
    /// Flags `BL` & 0x180, shifted down.
    pub joinstyle: u8,
    /// Stored inverted: flags `BL` bit 0x200 clear.
    pub lwdisplay: bool,
    /// Stored inverted: flags `BL` bit 0x400 clear.
    pub xedit: bool,
    /// Flags `BL` bit 0x800.
    pub extnames: bool,
    /// Flags `BL` bit 0x2000.
    pub pstylemode: bool,
    /// Flags `BL` bit 0x4000.
    pub olestartup: bool,
    pub insunits: i16,
    pub cepsntype: i16,
    /// Stored only when `cepsntype == 3`; `Handle(0)` otherwise.
    pub cpsnid: Handle,
    pub fingerprintguid: String,
    pub versionguid: String,
    /// R2004+.
    pub sortents: Option<u8>,
    /// R2004+.
    pub indexctl: Option<u8>,
    /// R2004+.
    pub hidetext: Option<u8>,
    /// R2004+ (0 or 1 before R2010).
    pub xclipframe: Option<u8>,
    /// R2004+.
    pub dimassoc: Option<u8>,
    /// R2004+.
    pub halogap: Option<u8>,
    /// R2004+.
    pub obscuredcolor: Option<i16>,
    /// R2004+.
    pub intersectioncolor: Option<i16>,
    /// R2004+.
    pub obscuredltype: Option<u8>,
    /// R2004+.
    pub intersectiondisplay: Option<u8>,
    /// R2004+.
    pub projectname: Option<String>,
    pub block_record_paper_space: Handle,
    pub block_record_model_space: Handle,
    pub ltype_bylayer: Handle,
    pub ltype_byblock: Handle,
    pub ltype_continuous: Handle,
    /// R2007+ (this and every field down to `cshadow`).
    pub cameradisplay: Option<bool>,
    pub stepspersec: Option<f64>,
    pub stepsize: Option<f64>,
    /// `$3DDWFPREC`.
    pub dwfprec3d: Option<f64>,
    pub lenslength: Option<f64>,
    pub cameraheight: Option<f64>,
    pub solidhist: Option<u8>,
    pub showhist: Option<u8>,
    pub psolwidth: Option<f64>,
    pub psolheight: Option<f64>,
    pub loftang1: Option<f64>,
    pub loftang2: Option<f64>,
    pub loftmag1: Option<f64>,
    pub loftmag2: Option<f64>,
    pub loftparam: Option<i16>,
    pub loftnormals: Option<u8>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub northdirection: Option<f64>,
    pub timezone: Option<i32>,
    pub lightglyphdisplay: Option<u8>,
    pub tilemodelightsynch: Option<u8>,
    pub dwfframe: Option<u8>,
    pub dgnframe: Option<u8>,
    pub interferecolor: Option<Color>,
    pub interfereobjvs: Option<Handle>,
    pub interferevpvs: Option<Handle>,
    pub dragvs: Option<Handle>,
    pub cshadow: Option<u8>,
}

impl HeaderVars {
    /// The header as [`encode_r2018`] writes it: fields R2018 stores but this
    /// header lacks take their [`defaults_r2018`] value, and fields R2018 does
    /// not store (`viewport_entity_header`, `vpentity_control`) become `None`.
    pub fn to_r2018(&self) -> HeaderVars {
        let mut h = self.clone();
        // Walking with an I/O that moves no data fills each R2018 `Option`
        // from the defaults; every `NoIo` step returns `Ok`.
        let filled = walk(&mut NoIo, &mut h, &defaults_r2018());
        debug_assert!(filled.is_ok());
        h.viewport_entity_header = None;
        h.vpentity_control = None;
        h
    }

    /// The R2000+ flags `BL` (ODS: `CELWEIGHT` ... `OLESTARTUP`).
    fn flags(&self) -> i32 {
        i32::from(self.celweight & 0x1F)
            | i32::from(self.endcaps & 3) << 5
            | i32::from(self.joinstyle & 3) << 7
            | i32::from(!self.lwdisplay) << 9
            | i32::from(!self.xedit) << 10
            | i32::from(self.extnames) << 11
            | i32::from(self.pstylemode) << 13
            | i32::from(self.olestartup) << 14
            | self.unknown.flags_other_bits & !FLAGS_NAMED
    }

    fn set_flags(&mut self, f: i32) {
        self.celweight = (f & 0x1F) as u8;
        self.endcaps = (f >> 5 & 3) as u8;
        self.joinstyle = (f >> 7 & 3) as u8;
        self.lwdisplay = f & 0x200 == 0;
        self.xedit = f & 0x400 == 0;
        self.extnames = f & 0x800 != 0;
        self.pstylemode = f & 0x2000 != 0;
        self.olestartup = f & 0x4000 != 0;
        self.unknown.flags_other_bits = f & !FLAGS_NAMED;
    }
}

/// Bits of the flags `BL` that have a named field.
const FLAGS_NAMED: i32 = 0x0FFF | 0x2000 | 0x4000;

// ---- the field walk ----

/// One step of reading or writing a field. [`walk`] states the field order
/// once; the reader fills the fields, the writer emits them.
trait Io {
    fn version(&self) -> Version;
    fn b(&mut self, v: &mut bool) -> Result<()>;
    fn bs(&mut self, v: &mut i16) -> Result<()>;
    fn bl(&mut self, v: &mut i32) -> Result<()>;
    fn bll(&mut self, v: &mut u64) -> Result<()>;
    fn bd(&mut self, v: &mut f64) -> Result<()>;
    fn rc(&mut self, v: &mut u8) -> Result<()>;
    fn rd2(&mut self, v: &mut [f64; 2]) -> Result<()>;
    fn bd3(&mut self, v: &mut [f64; 3]) -> Result<()>;
    fn tv(&mut self, v: &mut String) -> Result<()>;
    fn cmc(&mut self, v: &mut Color) -> Result<()>;
    /// A handle reference with its expected code: in the handle stream from
    /// R2007, inline before.
    fn h(&mut self, v: &mut Handle, code: u8) -> Result<()>;
    /// `HANDSEED`: always in the data stream, code 0.
    fn seed(&mut self, v: &mut Handle) -> Result<()>;

    /// The fields after the last handle: from R2007 they follow the handle
    /// stream instead of being in the data stream.
    fn start_tail(&mut self) {}

    fn date(&mut self, v: &mut JulianDate) -> Result<()> {
        self.bl(&mut v.day)?;
        self.bl(&mut v.ms)
    }
}

/// The value of a version-dependent field, set first from `default` (and
/// then from the file, when reading) if absent.
fn opt<'f, T: Clone + Default>(field: &'f mut Option<T>, default: &Option<T>) -> &'f mut T {
    field.get_or_insert_with(|| default.clone().unwrap_or_default())
}

/// Read or write every field in ODS order. `d` supplies values for absent
/// version-dependent fields (see [`opt`]).
fn walk(io: &mut impl Io, h: &mut HeaderVars, d: &HeaderVars) -> Result<()> {
    use Version::*;
    let v = io.version();
    let u = &mut h.unknown;
    let du = &d.unknown;

    if v >= R2013 {
        io.bll(opt(&mut h.requiredversions, &d.requiredversions))?;
    }
    io.bd(&mut u.unknown_bd1)?;
    io.bd(&mut u.unknown_bd2)?;
    io.bd(&mut u.unknown_bd3)?;
    io.bd(&mut u.unknown_bd4)?;
    io.tv(&mut u.unknown_tv1)?;
    io.tv(&mut u.unknown_tv2)?;
    io.tv(&mut u.unknown_tv3)?;
    io.tv(&mut u.unknown_tv4)?;
    io.bl(&mut u.unknown_bl1)?;
    io.bl(&mut u.unknown_bl2)?;
    if v < R2004 {
        let f = opt(&mut h.viewport_entity_header, &d.viewport_entity_header);
        io.h(f, HARD_POINTER)?;
    }
    io.b(&mut h.dimaso)?;
    io.b(&mut h.dimsho)?;
    io.b(&mut h.plinegen)?;
    io.b(&mut h.orthomode)?;
    io.b(&mut h.regenmode)?;
    io.b(&mut h.fillmode)?;
    io.b(&mut h.qtextmode)?;
    io.b(&mut h.psltscale)?;
    io.b(&mut h.limcheck)?;
    if v >= R2004 {
        io.b(opt(&mut u.unknown_b1, &du.unknown_b1))?;
    }
    io.b(&mut h.usrtimer)?;
    io.b(&mut h.skpoly)?;
    io.b(&mut h.angdir)?;
    io.b(&mut h.splframe)?;
    io.b(&mut h.mirrtext)?;
    io.b(&mut h.worldview)?;
    io.b(&mut h.tilemode)?;
    io.b(&mut h.plimcheck)?;
    io.b(&mut h.visretain)?;
    io.b(&mut h.dispsilh)?;
    io.b(&mut h.pellipse)?;
    io.bs(&mut h.proxygraphics)?;
    io.bs(&mut h.treedepth)?;
    io.bs(&mut h.lunits)?;
    io.bs(&mut h.luprec)?;
    io.bs(&mut h.aunits)?;
    io.bs(&mut h.auprec)?;
    io.bs(&mut h.attmode)?;
    io.bs(&mut h.pdmode)?;
    if v >= R2004 {
        io.bl(opt(&mut u.unknown_bl3, &du.unknown_bl3))?;
        io.bl(opt(&mut u.unknown_bl4, &du.unknown_bl4))?;
        io.bl(opt(&mut u.unknown_bl5, &du.unknown_bl5))?;
    }
    for f in [
        &mut h.useri1,
        &mut h.useri2,
        &mut h.useri3,
        &mut h.useri4,
        &mut h.useri5,
        &mut h.splinesegs,
        &mut h.surfu,
        &mut h.surfv,
        &mut h.surftype,
        &mut h.surftab1,
        &mut h.surftab2,
        &mut h.splinetype,
        &mut h.shadedge,
        &mut h.shadedif,
        &mut h.unitmode,
        &mut h.maxactvp,
        &mut h.isolines,
        &mut h.cmljust,
        &mut h.textqlty,
    ] {
        io.bs(f)?;
    }
    for f in [
        &mut h.ltscale,
        &mut h.textsize,
        &mut h.tracewid,
        &mut h.sketchinc,
        &mut h.filletrad,
        &mut h.thickness,
        &mut h.angbase,
        &mut h.pdsize,
        &mut h.plinewid,
        &mut h.userr1,
        &mut h.userr2,
        &mut h.userr3,
        &mut h.userr4,
        &mut h.userr5,
        &mut h.chamfera,
        &mut h.chamferb,
        &mut h.chamferc,
        &mut h.chamferd,
        &mut h.facetres,
        &mut h.cmlscale,
        &mut h.celtscale,
    ] {
        io.bd(f)?;
    }
    io.tv(&mut h.menuname)?;
    io.date(&mut h.tdcreate)?;
    io.date(&mut h.tdupdate)?;
    if v >= R2004 {
        io.bl(opt(&mut u.unknown_bl6, &du.unknown_bl6))?;
        io.bl(opt(&mut u.unknown_bl7, &du.unknown_bl7))?;
        io.bl(opt(&mut u.unknown_bl8, &du.unknown_bl8))?;
    }
    io.date(&mut h.tdindwg)?;
    io.date(&mut h.tdusrtimer)?;
    io.cmc(&mut h.cecolor)?;
    io.seed(&mut h.handseed)?;
    io.h(&mut h.clayer, HARD_POINTER)?;
    io.h(&mut h.textstyle, HARD_POINTER)?;
    io.h(&mut h.celtype, HARD_POINTER)?;
    if v >= R2007 {
        io.h(opt(&mut h.cmaterial, &d.cmaterial), HARD_POINTER)?;
    }
    io.h(&mut h.dimstyle, HARD_POINTER)?;
    io.h(&mut h.cmlstyle, HARD_POINTER)?;
    io.bd(&mut h.psvpscale)?;

    io.bd3(&mut h.pinsbase)?;
    io.bd3(&mut h.pextmin)?;
    io.bd3(&mut h.pextmax)?;
    io.rd2(&mut h.plimmin)?;
    io.rd2(&mut h.plimmax)?;
    io.bd(&mut h.pelevation)?;
    io.bd3(&mut h.pucsorg)?;
    io.bd3(&mut h.pucsxdir)?;
    io.bd3(&mut h.pucsydir)?;
    io.h(&mut h.pucsname, HARD_POINTER)?;
    io.h(&mut h.pucsorthoref, HARD_POINTER)?;
    io.bs(&mut h.pucsorthoview)?;
    io.h(&mut h.pucsbase, HARD_POINTER)?;
    for f in [
        &mut h.pucsorgtop,
        &mut h.pucsorgbottom,
        &mut h.pucsorgleft,
        &mut h.pucsorgright,
        &mut h.pucsorgfront,
        &mut h.pucsorgback,
    ] {
        io.bd3(f)?;
    }

    io.bd3(&mut h.insbase)?;
    io.bd3(&mut h.extmin)?;
    io.bd3(&mut h.extmax)?;
    io.rd2(&mut h.limmin)?;
    io.rd2(&mut h.limmax)?;
    io.bd(&mut h.elevation)?;
    io.bd3(&mut h.ucsorg)?;
    io.bd3(&mut h.ucsxdir)?;
    io.bd3(&mut h.ucsydir)?;
    io.h(&mut h.ucsname, HARD_POINTER)?;
    io.h(&mut h.ucsorthoref, HARD_POINTER)?;
    io.bs(&mut h.ucsorthoview)?;
    io.h(&mut h.ucsbase, HARD_POINTER)?;
    for f in [
        &mut h.ucsorgtop,
        &mut h.ucsorgbottom,
        &mut h.ucsorgleft,
        &mut h.ucsorgright,
        &mut h.ucsorgfront,
        &mut h.ucsorgback,
    ] {
        io.bd3(f)?;
    }
    io.tv(&mut h.dimpost)?;
    io.tv(&mut h.dimapost)?;

    for f in [
        &mut h.dimscale,
        &mut h.dimasz,
        &mut h.dimexo,
        &mut h.dimdli,
        &mut h.dimexe,
        &mut h.dimrnd,
        &mut h.dimdle,
        &mut h.dimtp,
        &mut h.dimtm,
    ] {
        io.bd(f)?;
    }
    if v >= R2007 {
        io.bd(opt(&mut h.dimfxl, &d.dimfxl))?;
        io.bd(opt(&mut h.dimjogang, &d.dimjogang))?;
        io.bs(opt(&mut h.dimtfill, &d.dimtfill))?;
        io.cmc(opt(&mut h.dimtfillclr, &d.dimtfillclr))?;
    }
    io.b(&mut h.dimtol)?;
    io.b(&mut h.dimlim)?;
    io.b(&mut h.dimtih)?;
    io.b(&mut h.dimtoh)?;
    io.b(&mut h.dimse1)?;
    io.b(&mut h.dimse2)?;
    io.bs(&mut h.dimtad)?;
    io.bs(&mut h.dimzin)?;
    io.bs(&mut h.dimazin)?;
    if v >= R2007 {
        io.bs(opt(&mut h.dimarcsym, &d.dimarcsym))?;
    }
    for f in [
        &mut h.dimtxt,
        &mut h.dimcen,
        &mut h.dimtsz,
        &mut h.dimaltf,
        &mut h.dimlfac,
        &mut h.dimtvp,
        &mut h.dimtfac,
        &mut h.dimgap,
        &mut h.dimaltrnd,
    ] {
        io.bd(f)?;
    }
    io.b(&mut h.dimalt)?;
    io.bs(&mut h.dimaltd)?;
    io.b(&mut h.dimtofl)?;
    io.b(&mut h.dimsah)?;
    io.b(&mut h.dimtix)?;
    io.b(&mut h.dimsoxd)?;
    io.cmc(&mut h.dimclrd)?;
    io.cmc(&mut h.dimclre)?;
    io.cmc(&mut h.dimclrt)?;
    for f in [
        &mut h.dimadec,
        &mut h.dimdec,
        &mut h.dimtdec,
        &mut h.dimaltu,
        &mut h.dimalttd,
        &mut h.dimaunit,
        &mut h.dimfrac,
        &mut h.dimlunit,
        &mut h.dimdsep,
        &mut h.dimtmove,
        &mut h.dimjust,
    ] {
        io.bs(f)?;
    }
    io.b(&mut h.dimsd1)?;
    io.b(&mut h.dimsd2)?;
    io.bs(&mut h.dimtolj)?;
    io.bs(&mut h.dimtzin)?;
    io.bs(&mut h.dimaltz)?;
    io.bs(&mut h.dimalttz)?;
    io.b(&mut h.dimupt)?;
    io.bs(&mut h.dimatfit)?;
    if v >= R2007 {
        io.b(opt(&mut h.dimfxlon, &d.dimfxlon))?;
    }
    if v >= R2010 {
        io.b(opt(&mut h.dimtxtdirection, &d.dimtxtdirection))?;
        io.bd(opt(&mut h.dimaltmzf, &d.dimaltmzf))?;
        io.tv(opt(&mut h.dimaltmzs, &d.dimaltmzs))?;
        io.bd(opt(&mut h.dimmzf, &d.dimmzf))?;
        io.tv(opt(&mut h.dimmzs, &d.dimmzs))?;
    }
    for f in [
        &mut h.dimtxsty,
        &mut h.dimldrblk,
        &mut h.dimblk,
        &mut h.dimblk1,
        &mut h.dimblk2,
    ] {
        io.h(f, HARD_POINTER)?;
    }
    if v >= R2007 {
        io.h(opt(&mut h.dimltype, &d.dimltype), HARD_POINTER)?;
        io.h(opt(&mut h.dimltex1, &d.dimltex1), HARD_POINTER)?;
        io.h(opt(&mut h.dimltex2, &d.dimltex2), HARD_POINTER)?;
    }
    io.bs(&mut h.dimlwd)?;
    io.bs(&mut h.dimlwe)?;

    for f in [
        &mut h.block_control,
        &mut h.layer_control,
        &mut h.style_control,
        &mut h.linetype_control,
        &mut h.view_control,
        &mut h.ucs_control,
        &mut h.vport_control,
        &mut h.appid_control,
        &mut h.dimstyle_control,
    ] {
        io.h(f, HARD_OWNER)?;
    }
    if v < R2004 {
        io.h(
            opt(&mut h.vpentity_control, &d.vpentity_control),
            HARD_OWNER,
        )?;
    }
    io.h(&mut h.dict_acad_group, HARD_POINTER)?;
    io.h(&mut h.dict_acad_mlinestyle, HARD_POINTER)?;
    io.h(&mut h.dict_named_objects, HARD_OWNER)?;
    io.bs(&mut h.tstackalign)?;
    io.bs(&mut h.tstacksize)?;
    io.tv(&mut h.hyperlinkbase)?;
    io.tv(&mut h.stylesheet)?;
    io.h(&mut h.dict_layouts, HARD_POINTER)?;
    io.h(&mut h.dict_plotsettings, HARD_POINTER)?;
    io.h(&mut h.dict_plotstyles, HARD_POINTER)?;
    if v >= R2004 {
        io.h(opt(&mut h.dict_materials, &d.dict_materials), HARD_POINTER)?;
        io.h(opt(&mut h.dict_colors, &d.dict_colors), HARD_POINTER)?;
    }
    if v >= R2007 {
        let f = opt(&mut h.dict_visualstyle, &d.dict_visualstyle);
        io.h(f, HARD_POINTER)?;
    }
    if v >= R2013 {
        let f = opt(&mut h.unknown.unknown_h1, &d.unknown.unknown_h1);
        io.h(f, HARD_POINTER)?;
    }
    let mut flags = h.flags();
    io.bl(&mut flags)?;
    h.set_flags(flags);
    io.bs(&mut h.insunits)?;
    io.bs(&mut h.cepsntype)?;
    if h.cepsntype == 3 {
        io.h(&mut h.cpsnid, HARD_POINTER)?;
    }
    io.tv(&mut h.fingerprintguid)?;
    io.tv(&mut h.versionguid)?;
    if v >= R2004 {
        io.rc(opt(&mut h.sortents, &d.sortents))?;
        io.rc(opt(&mut h.indexctl, &d.indexctl))?;
        io.rc(opt(&mut h.hidetext, &d.hidetext))?;
        io.rc(opt(&mut h.xclipframe, &d.xclipframe))?;
        io.rc(opt(&mut h.dimassoc, &d.dimassoc))?;
        io.rc(opt(&mut h.halogap, &d.halogap))?;
        io.bs(opt(&mut h.obscuredcolor, &d.obscuredcolor))?;
        io.bs(opt(&mut h.intersectioncolor, &d.intersectioncolor))?;
        io.rc(opt(&mut h.obscuredltype, &d.obscuredltype))?;
        io.rc(opt(&mut h.intersectiondisplay, &d.intersectiondisplay))?;
        io.tv(opt(&mut h.projectname, &d.projectname))?;
    }
    io.h(&mut h.block_record_paper_space, HARD_POINTER)?;
    io.h(&mut h.block_record_model_space, HARD_POINTER)?;
    io.h(&mut h.ltype_bylayer, HARD_POINTER)?;
    io.h(&mut h.ltype_byblock, HARD_POINTER)?;
    io.h(&mut h.ltype_continuous, HARD_POINTER)?;
    if v >= R2007 {
        let u = &mut h.unknown;
        io.b(opt(&mut h.cameradisplay, &d.cameradisplay))?;
        io.bl(opt(&mut u.unknown_bl9, &du.unknown_bl9))?;
        io.bl(opt(&mut u.unknown_bl10, &du.unknown_bl10))?;
        io.bd(opt(&mut u.unknown_bd5, &du.unknown_bd5))?;
        io.bd(opt(&mut h.stepspersec, &d.stepspersec))?;
        io.bd(opt(&mut h.stepsize, &d.stepsize))?;
        io.bd(opt(&mut h.dwfprec3d, &d.dwfprec3d))?;
        io.bd(opt(&mut h.lenslength, &d.lenslength))?;
        io.bd(opt(&mut h.cameraheight, &d.cameraheight))?;
        io.rc(opt(&mut h.solidhist, &d.solidhist))?;
        io.rc(opt(&mut h.showhist, &d.showhist))?;
        io.bd(opt(&mut h.psolwidth, &d.psolwidth))?;
        io.bd(opt(&mut h.psolheight, &d.psolheight))?;
        io.bd(opt(&mut h.loftang1, &d.loftang1))?;
        io.bd(opt(&mut h.loftang2, &d.loftang2))?;
        io.bd(opt(&mut h.loftmag1, &d.loftmag1))?;
        io.bd(opt(&mut h.loftmag2, &d.loftmag2))?;
        io.bs(opt(&mut h.loftparam, &d.loftparam))?;
        io.rc(opt(&mut h.loftnormals, &d.loftnormals))?;
        io.bd(opt(&mut h.latitude, &d.latitude))?;
        io.bd(opt(&mut h.longitude, &d.longitude))?;
        io.bd(opt(&mut h.northdirection, &d.northdirection))?;
        io.bl(opt(&mut h.timezone, &d.timezone))?;
        io.rc(opt(&mut h.lightglyphdisplay, &d.lightglyphdisplay))?;
        io.rc(opt(&mut h.tilemodelightsynch, &d.tilemodelightsynch))?;
        io.rc(opt(&mut h.dwfframe, &d.dwfframe))?;
        io.rc(opt(&mut h.dgnframe, &d.dgnframe))?;
        io.b(opt(&mut u.unknown_b2, &du.unknown_b2))?;
        io.cmc(opt(&mut h.interferecolor, &d.interferecolor))?;
        io.h(opt(&mut h.interfereobjvs, &d.interfereobjvs), HARD_POINTER)?;
        io.h(opt(&mut h.interferevpvs, &d.interferevpvs), HARD_POINTER)?;
        io.h(opt(&mut h.dragvs, &d.dragvs), HARD_POINTER)?;
        io.rc(opt(&mut h.cshadow, &d.cshadow))?;
        io.bd(opt(&mut u.unknown_bd6, &du.unknown_bd6))?;
    }
    io.start_tail();
    let u = &mut h.unknown;
    io.bs(&mut u.unknown_bs1)?;
    io.bs(&mut u.unknown_bs2)?;
    io.bs(&mut u.unknown_bs3)?;
    io.bs(&mut u.unknown_bs4)?;
    if v >= R2004 {
        io.bl(opt(&mut u.unknown_bl11, &du.unknown_bl11))?;
        io.bl(opt(&mut u.unknown_bl12, &du.unknown_bl12))?;
        io.b(opt(&mut u.unknown_b3, &du.unknown_b3))?;
    }
    Ok(())
}

// ---- reading ----

struct Reader<'a> {
    r: ObjReader<'a>,
    tail: bool,
}

impl<'a> Reader<'a> {
    /// Where plain values come from: the data stream, or after the handles
    /// for the R2007+ tail.
    fn data(&mut self) -> &mut BitReader<'a> {
        if self.tail && self.r.version >= Version::R2007 {
            &mut self.r.handles
        } else {
            &mut self.r.data
        }
    }
}

impl Io for Reader<'_> {
    fn version(&self) -> Version {
        self.r.version
    }
    fn start_tail(&mut self) {
        self.tail = true;
    }
    fn b(&mut self, v: &mut bool) -> Result<()> {
        *v = self.data().read_b()?;
        Ok(())
    }
    fn bs(&mut self, v: &mut i16) -> Result<()> {
        *v = self.data().read_bs()?;
        Ok(())
    }
    fn bl(&mut self, v: &mut i32) -> Result<()> {
        *v = self.data().read_bl()?;
        Ok(())
    }
    fn bll(&mut self, v: &mut u64) -> Result<()> {
        *v = self.data().read_bll()?;
        Ok(())
    }
    fn bd(&mut self, v: &mut f64) -> Result<()> {
        *v = self.data().read_bd()?;
        Ok(())
    }
    fn rc(&mut self, v: &mut u8) -> Result<()> {
        *v = self.data().read_rc()?;
        Ok(())
    }
    fn rd2(&mut self, v: &mut [f64; 2]) -> Result<()> {
        *v = self.data().read_2rd()?;
        Ok(())
    }
    fn bd3(&mut self, v: &mut [f64; 3]) -> Result<()> {
        *v = self.data().read_3bd()?;
        Ok(())
    }
    fn tv(&mut self, v: &mut String) -> Result<()> {
        *v = self.r.text()?;
        Ok(())
    }
    fn cmc(&mut self, v: &mut Color) -> Result<()> {
        *v = self.r.read_cmc()?;
        Ok(())
    }
    fn h(&mut self, v: &mut Handle, code: u8) -> Result<()> {
        let stream = if self.r.version >= Version::R2007 {
            &mut self.r.handles
        } else {
            &mut self.r.data
        };
        let r = stream.read_handle_ref()?;
        // The header has no handle of its own, so relative codes (6 and up)
        // have no base. Every corpus header uses exactly the ODS codes; other
        // absolute codes are accepted, and writing uses the ODS code.
        let _ = code;
        if r.code > 5 {
            return Err(corrupt(&format!(
                "relative handle reference (code {})",
                r.code
            )));
        }
        *v = resolve(r, Handle(0))?;
        Ok(())
    }
    fn seed(&mut self, v: &mut Handle) -> Result<()> {
        *v = Handle(self.r.data.read_handle_ref()?.value);
        Ok(())
    }
}

const FRAME: super::Frame = super::Frame {
    start: &START_SENTINEL,
    end: &END_SENTINEL,
    start_name: "header start",
    end_name: "header end",
    context: "header",
};

/// The section's data bytes (after the size fields, before the CRC) once the
/// sentinels, size and CRC have been checked.
fn unframe(bytes: &[u8], version: Version) -> Result<&[u8]> {
    super::unframe(bytes, version, &FRAME)
}

/// Bits a decode left unread. An exactly consumed header has none in the
/// data and string streams and fewer than 8 (padding) at the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct UnreadBits {
    /// Data stream bits left before the string stream (R2007+; always 0
    /// before, where everything is one stream counted as `trailing`).
    pub data: u64,
    /// String stream bits left (R2007+).
    pub strings: u64,
    /// Bits left after the last field, to the end of the data.
    pub trailing: u64,
}

impl UnreadBits {
    /// Whether nothing but byte padding was left.
    pub fn is_exact(&self) -> bool {
        self.data == 0 && self.strings == 0 && self.trailing < 8
    }
}

/// The result of [`decode_with_diagnostics`].
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct DecodedHeader {
    pub vars: HeaderVars,
    /// Problems that did not stop decoding: text that did not decode
    /// cleanly, bits left unread. Header diagnostics have no handle.
    pub diagnostics: Vec<Diagnostic>,
    pub unread: UnreadBits,
}

/// Decode the `AcDb:Header` section (sentinels through end sentinel).
/// `codepage` is the drawing code page from the file header; it decodes
/// R2000/R2004 text and becomes [`HeaderVars::dwgcodepage`].
pub fn decode(bytes: &[u8], version: Version, codepage: u16) -> Result<HeaderVars> {
    decode_with_diagnostics(bytes, version, codepage).map(|d| d.vars)
}

/// Like [`decode`], also returning the diagnostics and the unread bit counts.
pub fn decode_with_diagnostics(
    bytes: &[u8],
    version: Version,
    codepage: u16,
) -> Result<DecodedHeader> {
    let body = unframe(bytes, version)?;
    let r = if version >= Version::R2007 {
        let mut head = BitReader::new(body);
        let data_end = u64::from(head.read_rl()? as u32);
        let header_end = head.bit_pos();
        if data_end > (body.len() as u64) * 8 {
            return Err(corrupt("size in bits exceeds the section"));
        }
        let (data_limit, strings) = locate_strings(body, header_end, data_end)?;
        let mut data = BitReader::with_bit_limit(body, data_limit);
        data.seek_bit(header_end)?;
        let mut handles = BitReader::new(body);
        handles.seek_bit(data_end)?;
        ObjReader::from_streams(data, Some(strings), handles, version, codepage)
    } else {
        let empty = BitReader::new(&[]);
        ObjReader::from_streams(BitReader::new(body), None, empty, version, codepage)
    };
    let mut io = Reader { r, tail: false };
    let mut vars = HeaderVars::default();
    walk(&mut io, &mut vars, &HeaderVars::default())?;
    let r = io.r;
    let unread = if version >= Version::R2007 {
        UnreadBits {
            data: r.data.remaining_bits(),
            strings: r.strings.as_ref().map_or(0, BitReader::remaining_bits),
            trailing: r.handles.remaining_bits(),
        }
    } else {
        UnreadBits {
            data: 0,
            strings: 0,
            trailing: r.data.remaining_bits(),
        }
    };
    let mut diagnostics = r.diagnostics;
    // The object reader tags its diagnostics with an object handle; the
    // header has none.
    for d in &mut diagnostics {
        d.handle = None;
    }
    if !unread.is_exact() {
        diagnostics.push(Diagnostic {
            severity: Severity::Warning,
            handle: None,
            message: format!(
                "header: {} data, {} string and {} trailing bits unread",
                unread.data, unread.strings, unread.trailing
            ),
        });
    }
    vars.dwgcodepage = codepage;
    Ok(DecodedHeader {
        vars,
        diagnostics,
        unread,
    })
}

// ---- writing ----

struct Writer {
    w: ObjWriter,
    tail: bool,
}

impl Writer {
    /// Where plain values go: the data stream, or the handle stream for the
    /// tail (see [`Io::start_tail`]).
    fn data(&mut self) -> &mut BitWriter {
        if self.tail {
            self.w.handle_stream()
        } else {
            &mut self.w.data
        }
    }
}

impl Io for Writer {
    fn version(&self) -> Version {
        ObjWriter::VERSION
    }
    fn start_tail(&mut self) {
        self.tail = true;
    }
    fn b(&mut self, v: &mut bool) -> Result<()> {
        self.data().write_b(*v);
        Ok(())
    }
    fn bs(&mut self, v: &mut i16) -> Result<()> {
        self.data().write_bs(*v);
        Ok(())
    }
    fn bl(&mut self, v: &mut i32) -> Result<()> {
        self.data().write_bl(*v);
        Ok(())
    }
    fn bll(&mut self, v: &mut u64) -> Result<()> {
        self.data().write_bll(*v)
    }
    fn bd(&mut self, v: &mut f64) -> Result<()> {
        self.data().write_bd(*v);
        Ok(())
    }
    fn rc(&mut self, v: &mut u8) -> Result<()> {
        self.data().write_rc(*v);
        Ok(())
    }
    fn rd2(&mut self, v: &mut [f64; 2]) -> Result<()> {
        self.data().write_2rd(*v);
        Ok(())
    }
    fn bd3(&mut self, v: &mut [f64; 3]) -> Result<()> {
        self.data().write_3bd(*v);
        Ok(())
    }
    fn tv(&mut self, v: &mut String) -> Result<()> {
        self.w.text(v)
    }
    fn cmc(&mut self, v: &mut Color) -> Result<()> {
        self.w.write_cmc(v)
    }
    fn h(&mut self, v: &mut Handle, code: u8) -> Result<()> {
        self.w.href(HandleRef { code, value: v.0 })
    }
    fn seed(&mut self, v: &mut Handle) -> Result<()> {
        self.w.data.write_handle_ref(HandleRef {
            code: 0,
            value: v.0,
        })
    }
}

/// Moves no data: [`walk`] with it only fills absent fields (see
/// [`HeaderVars::to_r2018`]).
struct NoIo;

impl Io for NoIo {
    fn version(&self) -> Version {
        Version::R2018
    }
    fn b(&mut self, _: &mut bool) -> Result<()> {
        Ok(())
    }
    fn bs(&mut self, _: &mut i16) -> Result<()> {
        Ok(())
    }
    fn bl(&mut self, _: &mut i32) -> Result<()> {
        Ok(())
    }
    fn bll(&mut self, _: &mut u64) -> Result<()> {
        Ok(())
    }
    fn bd(&mut self, _: &mut f64) -> Result<()> {
        Ok(())
    }
    fn rc(&mut self, _: &mut u8) -> Result<()> {
        Ok(())
    }
    fn rd2(&mut self, _: &mut [f64; 2]) -> Result<()> {
        Ok(())
    }
    fn bd3(&mut self, _: &mut [f64; 3]) -> Result<()> {
        Ok(())
    }
    fn tv(&mut self, _: &mut String) -> Result<()> {
        Ok(())
    }
    fn cmc(&mut self, _: &mut Color) -> Result<()> {
        Ok(())
    }
    fn h(&mut self, _: &mut Handle, _: u8) -> Result<()> {
        Ok(())
    }
    fn seed(&mut self, _: &mut Handle) -> Result<()> {
        Ok(())
    }
}

/// The handle variables, named, that point at an object: every handle
/// [`walk`] reads in any version (the R2000-only viewport entity header and
/// its control included; `$HANDSEED` is the next free handle, not a
/// reference), null ones left out. Names are the DXF variable names, or
/// ODS's description for variables DXF does not have.
pub(crate) fn handle_vars(h: &HeaderVars) -> Vec<(&'static str, Handle)> {
    let o = Option::unwrap_or_default;
    [
        (
            "VIEWPORT ENTITY HEADER (R2000)",
            o(h.viewport_entity_header),
        ),
        ("$CLAYER", h.clayer),
        ("$TEXTSTYLE", h.textstyle),
        ("$CELTYPE", h.celtype),
        ("$CMATERIAL", o(h.cmaterial)),
        ("$DIMSTYLE", h.dimstyle),
        ("$CMLSTYLE", h.cmlstyle),
        ("$PUCSNAME", h.pucsname),
        ("$PUCSORTHOREF", h.pucsorthoref),
        ("$PUCSBASE", h.pucsbase),
        ("$UCSNAME", h.ucsname),
        ("$UCSORTHOREF", h.ucsorthoref),
        ("$UCSBASE", h.ucsbase),
        ("$DIMTXSTY", h.dimtxsty),
        ("$DIMLDRBLK", h.dimldrblk),
        ("$DIMBLK", h.dimblk),
        ("$DIMBLK1", h.dimblk1),
        ("$DIMBLK2", h.dimblk2),
        ("$DIMLTYPE", o(h.dimltype)),
        ("$DIMLTEX1", o(h.dimltex1)),
        ("$DIMLTEX2", o(h.dimltex2)),
        ("BLOCK CONTROL OBJECT", h.block_control),
        ("LAYER CONTROL OBJECT", h.layer_control),
        ("STYLE CONTROL OBJECT", h.style_control),
        ("LINETYPE CONTROL OBJECT", h.linetype_control),
        ("VIEW CONTROL OBJECT", h.view_control),
        ("UCS CONTROL OBJECT", h.ucs_control),
        ("VPORT CONTROL OBJECT", h.vport_control),
        ("APPID CONTROL OBJECT", h.appid_control),
        ("DIMSTYLE CONTROL OBJECT", h.dimstyle_control),
        (
            "VIEWPORT ENTITY HEADER CONTROL OBJECT (R2000)",
            o(h.vpentity_control),
        ),
        ("DICTIONARY (ACAD_GROUP)", h.dict_acad_group),
        ("DICTIONARY (ACAD_MLINESTYLE)", h.dict_acad_mlinestyle),
        ("DICTIONARY (NAMED OBJECTS)", h.dict_named_objects),
        ("DICTIONARY (LAYOUTS)", h.dict_layouts),
        ("DICTIONARY (PLOTSETTINGS)", h.dict_plotsettings),
        ("DICTIONARY (PLOTSTYLES)", h.dict_plotstyles),
        ("DICTIONARY (MATERIALS)", o(h.dict_materials)),
        ("DICTIONARY (COLORS)", o(h.dict_colors)),
        ("DICTIONARY (VISUALSTYLE)", o(h.dict_visualstyle)),
        ("UNKNOWN HANDLE (R2013+)", o(h.unknown.unknown_h1)),
        ("$CPSNID", h.cpsnid),
        ("BLOCK_RECORD (*PAPER_SPACE)", h.block_record_paper_space),
        ("BLOCK_RECORD (*MODEL_SPACE)", h.block_record_model_space),
        ("LTYPE (BYLAYER)", h.ltype_bylayer),
        ("LTYPE (BYBLOCK)", h.ltype_byblock),
        ("LTYPE (CONTINUOUS)", h.ltype_continuous),
        ("$INTERFEREOBJVS", o(h.interfereobjvs)),
        ("$INTERFEREVPVS", o(h.interferevpvs)),
        ("$DRAGVS", o(h.dragvs)),
    ]
    .into_iter()
    .filter(|(_, v)| v.0 != 0)
    .collect()
}

impl HeaderVars {
    /// Pass every handle variable (every handle [`handle_vars`] lists, in
    /// any version) through `f`, for a handle renumbering. Null handles stay
    /// null (`f` is not called for them); `$HANDSEED`, the next free handle
    /// rather than a reference, is left to the caller.
    pub(crate) fn map_handles(&mut self, f: &mut dyn FnMut(Handle) -> Handle) {
        let mut map = |h: &mut Handle| {
            if h.0 != 0 {
                *h = f(*h);
            }
        };
        for h in [
            &mut self.viewport_entity_header,
            &mut self.cmaterial,
            &mut self.dimltype,
            &mut self.dimltex1,
            &mut self.dimltex2,
            &mut self.vpentity_control,
            &mut self.dict_materials,
            &mut self.dict_colors,
            &mut self.dict_visualstyle,
            &mut self.unknown.unknown_h1,
            &mut self.interfereobjvs,
            &mut self.interferevpvs,
            &mut self.dragvs,
        ]
        .into_iter()
        .flatten()
        {
            map(h);
        }
        for h in [
            &mut self.clayer,
            &mut self.textstyle,
            &mut self.celtype,
            &mut self.dimstyle,
            &mut self.cmlstyle,
            &mut self.pucsname,
            &mut self.pucsorthoref,
            &mut self.pucsbase,
            &mut self.ucsname,
            &mut self.ucsorthoref,
            &mut self.ucsbase,
            &mut self.dimtxsty,
            &mut self.dimldrblk,
            &mut self.dimblk,
            &mut self.dimblk1,
            &mut self.dimblk2,
            &mut self.block_control,
            &mut self.layer_control,
            &mut self.style_control,
            &mut self.linetype_control,
            &mut self.view_control,
            &mut self.ucs_control,
            &mut self.vport_control,
            &mut self.appid_control,
            &mut self.dimstyle_control,
            &mut self.dict_acad_group,
            &mut self.dict_acad_mlinestyle,
            &mut self.dict_named_objects,
            &mut self.dict_layouts,
            &mut self.dict_plotsettings,
            &mut self.dict_plotstyles,
            &mut self.cpsnid,
            &mut self.block_record_paper_space,
            &mut self.block_record_model_space,
            &mut self.ltype_bylayer,
            &mut self.ltype_byblock,
            &mut self.ltype_continuous,
        ] {
            map(h);
        }
    }
}

/// Encode the header section in R2018 form: sentinels, size, data, CRC.
/// Fields R2018 stores but `h` lacks take their [`defaults_r2018`] value (see
/// [`HeaderVars::to_r2018`]).
pub fn encode_r2018(h: &HeaderVars) -> Result<Vec<u8>> {
    let mut h = h.clone();
    let mut io = Writer {
        w: ObjWriter::new(Handle(0)),
        tail: false,
    };
    walk(&mut io, &mut h, &defaults_r2018())?;
    let mut main = BitWriter::new();
    main.write_rl(0); // size in bits, patched below
    let data_end = io.w.write_streams(&mut main)?;
    let bits = i32::try_from(data_end).map_err(|_| encode_err("data too large"))?;
    main.patch_rl_at(0, bits)?;
    super::frame_r2018(&main.into_bytes(), &FRAME)
}

/// The header of a new R2018 drawing: AutoCAD's imperial `acad.dwt` values
/// (`$INSUNITS` 1 inches, `$MEASUREMENT` 0 imperial).
///
/// Where neither ODS nor the AutoCAD system variable reference gives a value,
/// the value is copied from the R2018 corpus sample
/// (`acadsharp-sample-ac1032`, an `acad.dwt`-based drawing): the unknown
/// fields, `menuname`, the flags `BL` fields and the R2007+ camera, loft
/// and geographic defaults. Drawing-specific values of that sample (extents,
/// text size, point style, current color, dimension colors) are replaced by
/// the template values.
///
/// Placeholders that `Drawing::new()` fills once it has created the objects
/// and knows the time: every handle (`Handle(0)`, including `handseed`),
/// `tdcreate` and `tdupdate` (zero), and the two GUIDs (empty).
pub fn defaults_r2018() -> HeaderVars {
    const BIG: f64 = 1.0e20;
    let none = Handle(0);
    let some_none = Some(Handle(0));
    HeaderVars {
        dwgcodepage: crate::strings::DEFAULT_CODEPAGE,
        measurement: Some(0),
        requiredversions: Some(0),
        unknown: UnknownHeaderFields {
            // ODS defaults; ODS says "" and 24 for `unknown_tv1` and
            // `unknown_bl1`, but every corpus file has "m" and 0.
            unknown_bd1: 412_148_564_080.0,
            unknown_bd2: 1.0,
            unknown_bd3: 1.0,
            unknown_bd4: 1.0,
            unknown_tv1: "m".into(),
            unknown_tv2: String::new(),
            unknown_tv3: String::new(),
            unknown_tv4: String::new(),
            unknown_bl1: 0,
            unknown_bl2: 0,
            // From the corpus sample.
            unknown_b1: Some(false),
            unknown_bl3: Some(808_464_432),
            unknown_bl4: Some(486_869_248),
            unknown_bl5: Some(1_295_333_680),
            unknown_bl6: Some(1_295_003_696),
            unknown_bl7: Some(486_539_264),
            unknown_bl8: Some(808_464_432),
            unknown_h1: some_none,
            flags_other_bits: 0,
            unknown_bl9: Some(0),
            unknown_bl10: Some(10),
            unknown_bd5: Some(1.0),
            unknown_b2: Some(true),
            unknown_bd6: Some(0.0),
            unknown_bs1: -22556,
            unknown_bs2: -25059,
            unknown_bs3: -17538,
            unknown_bs4: -13464,
            unknown_bl11: Some(0),
            unknown_bl12: Some(0),
            unknown_b3: Some(true),
        },
        viewport_entity_header: None,
        dimaso: true,
        dimsho: true,
        plinegen: false,
        orthomode: false,
        regenmode: true,
        fillmode: true,
        qtextmode: false,
        psltscale: true,
        limcheck: false,
        usrtimer: true,
        skpoly: false,
        angdir: false,
        splframe: false,
        mirrtext: false,
        worldview: true,
        tilemode: true,
        plimcheck: false,
        visretain: true,
        dispsilh: false,
        pellipse: false,
        proxygraphics: 1,
        treedepth: 3020,
        lunits: 2,
        luprec: 4,
        aunits: 0,
        auprec: 0,
        attmode: 1,
        pdmode: 0,
        useri1: 0,
        useri2: 0,
        useri3: 0,
        useri4: 0,
        useri5: 0,
        splinesegs: 8,
        surfu: 6,
        surfv: 6,
        surftype: 6,
        surftab1: 6,
        surftab2: 6,
        splinetype: 6,
        shadedge: 3,
        shadedif: 70,
        unitmode: 0,
        maxactvp: 64,
        isolines: 4,
        cmljust: 0,
        textqlty: 50,
        ltscale: 1.0,
        textsize: 0.2,
        tracewid: 0.05,
        sketchinc: 0.1,
        filletrad: 0.0,
        thickness: 0.0,
        angbase: 0.0,
        pdsize: 0.0,
        plinewid: 0.0,
        userr1: 0.0,
        userr2: 0.0,
        userr3: 0.0,
        userr4: 0.0,
        userr5: 0.0,
        chamfera: 0.0,
        chamferb: 0.0,
        chamferc: 0.0,
        chamferd: 0.0,
        facetres: 0.5,
        cmlscale: 1.0,
        celtscale: 1.0,
        menuname: ".".into(),
        tdcreate: JulianDate::default(),
        tdupdate: JulianDate::default(),
        tdindwg: JulianDate::default(),
        tdusrtimer: JulianDate::default(),
        cecolor: Color::ByLayer,
        handseed: none,
        clayer: none,
        textstyle: none,
        celtype: none,
        cmaterial: some_none,
        dimstyle: none,
        cmlstyle: none,
        psvpscale: 0.0,
        pinsbase: [0.0; 3],
        pextmin: [BIG; 3],
        pextmax: [-BIG; 3],
        plimmin: [0.0; 2],
        plimmax: [12.0, 9.0],
        pelevation: 0.0,
        pucsorg: [0.0; 3],
        pucsxdir: [1.0, 0.0, 0.0],
        pucsydir: [0.0, 1.0, 0.0],
        pucsname: none,
        pucsorthoref: none,
        pucsorthoview: 0,
        pucsbase: none,
        pucsorgtop: [0.0; 3],
        pucsorgbottom: [0.0; 3],
        pucsorgleft: [0.0; 3],
        pucsorgright: [0.0; 3],
        pucsorgfront: [0.0; 3],
        pucsorgback: [0.0; 3],
        insbase: [0.0; 3],
        extmin: [BIG; 3],
        extmax: [-BIG; 3],
        limmin: [0.0; 2],
        limmax: [12.0, 9.0],
        elevation: 0.0,
        ucsorg: [0.0; 3],
        ucsxdir: [1.0, 0.0, 0.0],
        ucsydir: [0.0, 1.0, 0.0],
        ucsname: none,
        ucsorthoref: none,
        ucsorthoview: 0,
        ucsbase: none,
        ucsorgtop: [0.0; 3],
        ucsorgbottom: [0.0; 3],
        ucsorgleft: [0.0; 3],
        ucsorgright: [0.0; 3],
        ucsorgfront: [0.0; 3],
        ucsorgback: [0.0; 3],
        dimpost: String::new(),
        dimapost: String::new(),
        dimscale: 1.0,
        dimasz: 0.18,
        dimexo: 0.0625,
        dimdli: 0.38,
        dimexe: 0.18,
        dimrnd: 0.0,
        dimdle: 0.0,
        dimtp: 0.0,
        dimtm: 0.0,
        dimfxl: Some(1.0),
        dimjogang: Some(std::f64::consts::FRAC_PI_4),
        dimtfill: Some(0),
        dimtfillclr: Some(Color::ByBlock),
        dimtol: false,
        dimlim: false,
        dimtih: true,
        dimtoh: true,
        dimse1: false,
        dimse2: false,
        dimtad: 0,
        dimzin: 0,
        dimazin: 0,
        dimarcsym: Some(0),
        dimtxt: 0.18,
        dimcen: 0.09,
        dimtsz: 0.0,
        dimaltf: 25.4,
        dimlfac: 1.0,
        dimtvp: 0.0,
        dimtfac: 1.0,
        dimgap: 0.09,
        dimaltrnd: 0.0,
        dimalt: false,
        dimaltd: 2,
        dimtofl: false,
        dimsah: false,
        dimtix: false,
        dimsoxd: false,
        dimclrd: Color::ByBlock,
        dimclre: Color::ByBlock,
        dimclrt: Color::ByBlock,
        dimadec: 0,
        dimdec: 4,
        dimtdec: 4,
        dimaltu: 2,
        dimalttd: 2,
        dimaunit: 0,
        dimfrac: 0,
        dimlunit: 2,
        dimdsep: i16::from(b'.'),
        dimtmove: 0,
        dimjust: 0,
        dimsd1: false,
        dimsd2: false,
        dimtolj: 1,
        dimtzin: 0,
        dimaltz: 0,
        dimalttz: 0,
        dimupt: false,
        dimatfit: 3,
        dimfxlon: Some(false),
        dimtxtdirection: Some(false),
        dimaltmzf: Some(100.0),
        dimaltmzs: Some(String::new()),
        dimmzf: Some(100.0),
        dimmzs: Some(String::new()),
        dimtxsty: none,
        dimldrblk: none,
        dimblk: none,
        dimblk1: none,
        dimblk2: none,
        dimltype: some_none,
        dimltex1: some_none,
        dimltex2: some_none,
        // -2: by block.
        dimlwd: -2,
        dimlwe: -2,
        block_control: none,
        layer_control: none,
        style_control: none,
        linetype_control: none,
        view_control: none,
        ucs_control: none,
        vport_control: none,
        appid_control: none,
        dimstyle_control: none,
        vpentity_control: None,
        dict_acad_group: none,
        dict_acad_mlinestyle: none,
        dict_named_objects: none,
        tstackalign: 1,
        tstacksize: 70,
        hyperlinkbase: String::new(),
        stylesheet: String::new(),
        dict_layouts: none,
        dict_plotsettings: none,
        dict_plotstyles: none,
        dict_materials: some_none,
        dict_colors: some_none,
        dict_visualstyle: some_none,
        // 29: by layer (from the corpus sample, as are the other flags).
        celweight: 29,
        endcaps: 0,
        joinstyle: 0,
        lwdisplay: false,
        xedit: true,
        extnames: true,
        pstylemode: true,
        olestartup: false,
        insunits: 1,
        cepsntype: 0,
        cpsnid: none,
        fingerprintguid: String::new(),
        versionguid: String::new(),
        sortents: Some(127),
        indexctl: Some(0),
        hidetext: Some(1),
        xclipframe: Some(2),
        dimassoc: Some(2),
        halogap: Some(0),
        obscuredcolor: Some(257),
        intersectioncolor: Some(257),
        obscuredltype: Some(0),
        intersectiondisplay: Some(0),
        projectname: Some(String::new()),
        block_record_paper_space: none,
        block_record_model_space: none,
        ltype_bylayer: none,
        ltype_byblock: none,
        ltype_continuous: none,
        cameradisplay: Some(false),
        stepspersec: Some(2.0),
        stepsize: Some(6.0),
        dwfprec3d: Some(2.0),
        lenslength: Some(50.0),
        cameraheight: Some(0.0),
        solidhist: Some(0),
        showhist: Some(1),
        psolwidth: Some(0.25),
        psolheight: Some(4.0),
        loftang1: Some(std::f64::consts::FRAC_PI_2),
        loftang2: Some(std::f64::consts::FRAC_PI_2),
        loftmag1: Some(0.0),
        loftmag2: Some(0.0),
        loftparam: Some(7),
        loftnormals: Some(1),
        latitude: Some(37.795),
        longitude: Some(-122.394),
        northdirection: Some(0.0),
        timezone: Some(-8000),
        lightglyphdisplay: Some(1),
        tilemodelightsynch: Some(1),
        dwfframe: Some(2),
        dgnframe: Some(0),
        interferecolor: Some(Color::Index(1)),
        interfereobjvs: some_none,
        interferevpvs: some_none,
        dragvs: some_none,
        cshadow: Some(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::crc16;
    use crate::section::CRC_SEED;

    /// Gives every handle the walk visits a distinct value.
    struct Stamp(Version, u64);

    impl Io for Stamp {
        fn version(&self) -> Version {
            self.0
        }
        fn b(&mut self, _: &mut bool) -> Result<()> {
            Ok(())
        }
        fn bs(&mut self, _: &mut i16) -> Result<()> {
            Ok(())
        }
        fn bl(&mut self, _: &mut i32) -> Result<()> {
            Ok(())
        }
        fn bll(&mut self, _: &mut u64) -> Result<()> {
            Ok(())
        }
        fn bd(&mut self, _: &mut f64) -> Result<()> {
            Ok(())
        }
        fn rc(&mut self, _: &mut u8) -> Result<()> {
            Ok(())
        }
        fn rd2(&mut self, _: &mut [f64; 2]) -> Result<()> {
            Ok(())
        }
        fn bd3(&mut self, _: &mut [f64; 3]) -> Result<()> {
            Ok(())
        }
        fn tv(&mut self, _: &mut String) -> Result<()> {
            Ok(())
        }
        fn cmc(&mut self, _: &mut Color) -> Result<()> {
            Ok(())
        }
        fn h(&mut self, v: &mut Handle, _: u8) -> Result<()> {
            self.1 += 1;
            *v = Handle(self.1);
            Ok(())
        }
        fn seed(&mut self, v: &mut Handle) -> Result<()> {
            *v = Handle(0xDEAD);
            Ok(())
        }
    }

    /// `handle_vars` names every handle the header stores, in every version
    /// (the R2000-only ones included), and not `$HANDSEED`.
    #[test]
    fn handle_vars_lists_every_header_handle() {
        use Version::*;
        for v in [R2000, R2004, R2007, R2010, R2013, R2018] {
            let mut h = HeaderVars {
                cepsntype: 3,
                ..HeaderVars::default()
            };
            let mut io = Stamp(v, 0);
            walk(&mut io, &mut h, &HeaderVars::default()).unwrap();
            let mut listed: Vec<u64> = handle_vars(&h).iter().map(|(_, x)| x.0).collect();
            listed.sort_unstable();
            let stamped: Vec<u64> = (1..=io.1).collect();
            assert_eq!(listed, stamped, "{v:?}");
            let names: std::collections::HashSet<_> =
                handle_vars(&h).iter().map(|(n, _)| *n).collect();
            assert_eq!(names.len(), stamped.len(), "{v:?}: names are distinct");
        }
    }

    /// `map_handles` maps every handle the header stores, in every version,
    /// and leaves `$HANDSEED` (the next free handle, not a reference) and
    /// null handles alone.
    #[test]
    fn map_handles_maps_every_header_handle() {
        use Version::*;
        for v in [R2000, R2004, R2007, R2010, R2013, R2018] {
            let mut h = HeaderVars {
                cepsntype: 3,
                ..HeaderVars::default()
            };
            walk(&mut Stamp(v, 0), &mut h, &HeaderVars::default()).unwrap();
            let before = handle_vars(&h);
            let mut mapped = h.clone();
            mapped.map_handles(&mut |x| Handle(x.0 + 0x1000));
            let expect: Vec<_> = before
                .iter()
                .map(|&(n, x)| (n, Handle(x.0 + 0x1000)))
                .collect();
            assert_eq!(handle_vars(&mapped), expect, "{v:?}");
            assert_eq!(mapped.handseed, h.handseed, "{v:?}");
        }
        let mut h = HeaderVars::default();
        h.map_handles(&mut |_| panic!("a null handle is not mapped"));
        assert_eq!(h, HeaderVars::default());
    }

    fn sample() -> Vec<u8> {
        let mut h = defaults_r2018();
        h.handseed = Handle(0x2A);
        h.clayer = Handle(0x10);
        h.layer_control = Handle(2);
        h.menuname = "acad".into();
        h.fingerprintguid = "{00000000-0000-0000-0000-000000000001}".into();
        h.cecolor = Color::Book {
            rgb: 0x01_0203,
            color_name: "あ".into(),
            book_name: "BOOK".into(),
        };
        h.cepsntype = 3;
        h.cpsnid = Handle(0x77);
        h.requiredversions = Some(0x0102_0304_0506);
        encode_r2018(&h).unwrap()
    }

    /// Recompute the CRC after editing the data.
    fn fix_crc(b: &mut [u8]) {
        let end = b.len() - 18;
        let crc = crc16(CRC_SEED, &b[16..end]);
        b[end..end + 2].copy_from_slice(&crc.to_le_bytes());
    }

    #[test]
    fn roundtrip_non_default_values() {
        let bytes = sample();
        let h = decode(&bytes, Version::R2018, 30).unwrap();
        assert_eq!(h.handseed, Handle(0x2A));
        assert_eq!(h.clayer, Handle(0x10));
        assert_eq!(h.menuname, "acad");
        assert_eq!(h.cpsnid, Handle(0x77));
        assert_eq!(h.requiredversions, Some(0x0102_0304_0506));
        assert!(matches!(h.cecolor, Color::Book { ref color_name, .. } if color_name == "あ"));
        assert_eq!(encode_r2018(&h).unwrap(), bytes);
    }

    #[test]
    fn flags_bl_roundtrip() {
        let mut h = defaults_r2018();
        for f in [0, 0x1F, 0x7FFF, 0x1000 | 0x8000 | 0x0200, -1] {
            h.set_flags(f);
            assert_eq!(h.flags(), f, "{f:X}");
        }
        h.set_flags(0x0200 | 0x0400);
        assert!(!h.lwdisplay && !h.xedit);
    }

    #[test]
    fn r2010_layout_without_the_extra_long() {
        // R2010/R2013 with maintenance version <= 3 have no zero RL after the
        // size; the end sentinel position tells the layouts apart.
        let long = sample();
        let mut short = long.clone();
        short.drain(20..24);
        fix_crc(&mut short);
        // R2013 stores the same fields as R2018.
        let a = decode(&long, Version::R2013, 30).unwrap();
        let b = decode(&short, Version::R2013, 30).unwrap();
        assert_eq!(a, b);
        assert!(decode(&short, Version::R2018, 30).is_err());
    }

    #[test]
    fn hostile_truncated() {
        let bytes = sample();
        for n in 0..bytes.len() {
            assert!(decode(&bytes[..n], Version::R2018, 30).is_err(), "{n}");
        }
        assert!(matches!(
            decode(&bytes[..10], Version::R2018, 30),
            Err(Error::Truncated { .. })
        ));
    }

    #[test]
    fn hostile_bad_sentinels() {
        let mut b = sample();
        b[3] ^= 1;
        assert!(matches!(
            decode(&b, Version::R2018, 30),
            Err(Error::BadSentinel("header start"))
        ));
        let mut b = sample();
        let n = b.len();
        b[n - 1] ^= 1;
        assert!(matches!(
            decode(&b, Version::R2018, 30),
            Err(Error::BadSentinel("header end"))
        ));
    }

    #[test]
    fn hostile_bad_crc() {
        let mut b = sample();
        b[40] ^= 0x10;
        assert!(matches!(
            decode(&b, Version::R2018, 30),
            Err(Error::BadCrc { .. })
        ));
    }

    #[test]
    fn hostile_size_fields() {
        // Section size beyond the buffer, up to u32::MAX.
        for size in [0x7FFF_FFFFu32, u32::MAX, 1 << 20] {
            let mut b = sample();
            b[16..20].copy_from_slice(&size.to_le_bytes());
            assert!(matches!(
                decode(&b, Version::R2018, 30),
                Err(Error::Truncated { .. })
            ));
        }
        // A non-zero high size long.
        let mut b = sample();
        b[20] = 1;
        fix_crc(&mut b);
        assert!(matches!(
            decode(&b, Version::R2018, 30),
            Err(Error::Corrupt(_))
        ));
        // Size in bits beyond the data, or misaligned by a few bits.
        for bits in [u32::MAX, 0x7FFF_FFFF, 0, 31, 33] {
            let mut b = sample();
            b[24..28].copy_from_slice(&bits.to_le_bytes());
            fix_crc(&mut b);
            assert!(decode(&b, Version::R2018, 30).is_err(), "{bits}");
        }
    }

    /// Insert `extra` bytes at the end of the data (before the CRC) and fix
    /// the size and CRC.
    fn with_extra_data(mut b: Vec<u8>, extra: &[u8]) -> Vec<u8> {
        let end = b.len() - 18;
        b.splice(end..end, extra.iter().copied());
        let size = u32::from_le_bytes(b[16..20].try_into().unwrap()) + extra.len() as u32;
        b[16..20].copy_from_slice(&size.to_le_bytes());
        fix_crc(&mut b);
        b
    }

    #[test]
    fn leftover_bits_are_a_warning() {
        #[allow(unused_mut)]
        let mut cases = vec![(sample(), Version::R2018)];
        #[cfg(opendwg_fixtures)]
        cases.extend([
            (embedded::R2000.to_vec(), Version::R2000),
            (embedded::R2004.to_vec(), Version::R2004),
            (embedded::R2007.to_vec(), Version::R2007),
        ]);
        for (clean, v) in cases {
            let want = decode(&clean, v, 30).unwrap();
            let b = with_extra_data(clean, &[0xAA, 0x55]);
            let d = decode_with_diagnostics(&b, v, 30).unwrap();
            assert_eq!(d.vars, want, "{v:?}");
            assert_eq!((d.unread.data, d.unread.strings), (0, 0), "{v:?}");
            assert!(d.unread.trailing >= 16 && !d.unread.is_exact(), "{v:?}");
            assert_eq!(d.diagnostics.len(), 1, "{v:?}");
            assert_eq!(d.diagnostics[0].severity, Severity::Warning);
            assert_eq!(d.diagnostics[0].handle, None);
        }
    }

    #[test]
    fn misaligned_bit_size_never_passes_silently() {
        // A size in bits 8 short moves the string stream and the handles:
        // decoding fails or reports unread bits.
        let mut b = sample();
        let bits = u32::from_le_bytes(b[24..28].try_into().unwrap());
        b[24..28].copy_from_slice(&(bits - 8).to_le_bytes());
        fix_crc(&mut b);
        if let Ok(d) = decode_with_diagnostics(&b, Version::R2018, 30) {
            assert!(!d.unread.is_exact() && !d.diagnostics.is_empty());
        }
    }

    /// The tests of the embedded corpus headers: test-only fixtures, not
    /// published with the crate (`build.rs` sets `opendwg_fixtures` when
    /// they are present).
    #[cfg(opendwg_fixtures)]
    mod embedded {
        use super::*;

        // ---- embedded corpus headers (pre-R2007 inline layout, R2007 tail) ----
        //
        // `AcDb:Header` sections of the ACadSharp sample drawings
        // `sample_AC1015.dwg`, `sample_AC1018.dwg` and `sample_AC1021.dwg`
        // (commit ed5ff1cf4731c0296bb8931a387bd9d7c634122f), Copyright (c) 2021
        // Albert Domenech, MIT licence; see THIRD_PARTY.md.
        pub(super) const R2000: &[u8] = include_bytes!("testdata/header_ac1015.bin");
        pub(super) const R2004: &[u8] = include_bytes!("testdata/header_ac1018.bin");
        pub(super) const R2007: &[u8] = include_bytes!("testdata/header_ac1021.bin");

        #[test]
        fn embedded_headers_decode_exactly() {
            for (bytes, v, seed, bs1) in [
                (R2000, Version::R2000, 4343, -8322),
                (R2004, Version::R2004, 4127, 26924),
                (R2007, Version::R2007, 3933, 28173),
            ] {
                let d = decode_with_diagnostics(bytes, v, 30).unwrap();
                assert!(d.unread.is_exact(), "{v:?}: {:?}", d.unread);
                assert!(d.diagnostics.is_empty(), "{v:?}");
                let h = d.vars;
                assert_eq!(h.handseed, Handle(seed), "{v:?}");
                assert_eq!(h.clayer, Handle(0x10), "{v:?}");
                assert_eq!(h.layer_control, Handle(2), "{v:?}");
                assert_eq!(h.menuname, ".", "{v:?}");
                assert_eq!(h.unknown.unknown_tv1, "m", "{v:?}");
                assert_eq!(h.unknown.unknown_bs1, bs1, "{v:?}");
                assert!(h.fingerprintguid.starts_with('{'), "{v:?}");
                assert_eq!(h.viewport_entity_header.is_some(), v == Version::R2000);
                if v >= Version::R2004 {
                    let u = &h.unknown;
                    assert_eq!(
                        (u.unknown_bl11, u.unknown_bl12, u.unknown_b3),
                        (Some(0), Some(0), Some(true)),
                        "{v:?}"
                    );
                } else {
                    assert!(h.unknown.unknown_bl11.is_none());
                }
                assert_eq!(h.cmaterial.is_some(), v >= Version::R2007);
            }
            // The R2007 tail follows the handle stream and is padded with ones.
            let d = decode_with_diagnostics(R2007, Version::R2007, 30).unwrap();
            assert_eq!(d.unread.trailing, 4);
        }

        #[test]
        fn embedded_headers_truncated_and_corrupted() {
            for (bytes, v) in [
                (R2000, Version::R2000),
                (R2004, Version::R2004),
                (R2007, Version::R2007),
            ] {
                for n in 0..bytes.len() {
                    assert!(decode(&bytes[..n], v, 30).is_err(), "{v:?} {n}");
                }
                // Every data byte flipped: the CRC catches it.
                for i in 20..bytes.len() - 18 {
                    let mut b = bytes.to_vec();
                    b[i] ^= 0x40;
                    assert!(
                        matches!(decode(&b, v, 30), Err(Error::BadCrc { .. })),
                        "{v:?} {i}"
                    );
                }
            }
        }

        #[test]
        fn embedded_headers_convert_to_r2018() {
            for (bytes, v) in [
                (R2000, Version::R2000),
                (R2004, Version::R2004),
                (R2007, Version::R2007),
            ] {
                let old = decode(bytes, v, 30).unwrap();
                let out = encode_r2018(&old).unwrap();
                let d = decode_with_diagnostics(&out, Version::R2018, 30).unwrap();
                assert!(d.unread.is_exact() && d.diagnostics.is_empty());
                assert_eq!(d.vars, old.to_r2018(), "{v:?}");
            }
        }
    }

    #[test]
    fn to_r2018_fills_and_drops() {
        let mut h = HeaderVars {
            vpentity_control: Some(Handle(0x30)),
            viewport_entity_header: Some(Handle(0x31)),
            ..HeaderVars::default()
        };
        h.ltscale = 2.5;
        let d = defaults_r2018();
        let up = h.to_r2018();
        assert_eq!(up.ltscale, 2.5);
        assert_eq!(up.vpentity_control, None);
        assert_eq!(up.viewport_entity_header, None);
        assert_eq!(up.dimfxl, d.dimfxl);
        assert_eq!(up.latitude, d.latitude);
        assert_eq!(up.unknown.unknown_bl11, d.unknown.unknown_bl11);
        // An R2018 header is unchanged.
        assert_eq!(d.to_r2018(), d);
    }

    #[test]
    fn types_are_send_sync() {
        fn a<T: Send + Sync>() {}
        a::<HeaderVars>();
    }
}
