//! Viewports (ODS 20.4.38), images and wipeouts (20.4.80), the ACIS
//! (modeler) entities REGION, 3DSOLID and BODY (20.4.41), OLE frames
//! (20.4.88) and proxy entities (20.4.90).
//!
//! ACIS entities keep their modeler data untouched (see [`AcisEntity`]); OLE
//! frames and proxies keep their payloads as stored.
//!
//! Where the corpus or ACadSharp (MIT) differ from ODS, each pinned by a test
//! in `tests/hatch_misc.rs`:
//! - From R2013 an ACIS entity stores no "ACIS empty" bit, unknown bit,
//!   version or modeler data (ODS: all versions): it starts with the
//!   wireframe, and its SAB is in the `AcDb:AcDsPrototype_1b` section, as
//!   ACadSharp `readModelerGeometry` reads it.
//! - From R2013 an ACIS entity ends, after the R2007 unknown `BL`, with
//!   fields ODS does not describe: a `B` (DXF 290), the entity's GUID (DXF
//!   2: `BL`, `BS`, `BS`, 8 `RC`) and a 0 read as a `BS`, checked against
//!   the DXF export.
//! - From R2007 a REGION stores no history handle (ODS: all ACIS entities);
//!   3DSOLID does, as ACadSharp `readSolid3D` reads it. BODY (not in the
//!   corpus) is taken to store one, as ODS says.
//! - A VIEWPORT's clip boundary is a hard pointer (ODS: soft), as every
//!   corpus viewport stores it and ACadSharp `writeViewport` writes it.
//! - A WIPEOUT, which ODS does not describe, is laid out as an IMAGE, as
//!   ACadSharp `readCadImage` reads both.
//!
//! Unconfirmed by the corpus, which has none: BODY, OLE2FRAME,
//! ACAD_PROXY_ENTITY, ACIS silhouettes and an image clip type other than 1.
//! They follow ODS.

use super::super::raw::{HandleTail, take_bits, take_rest, trailing_refs};
use super::super::stream::{ObjReader, ObjWriter};
use super::super::tables::{HARD_OWNER, HARD_POINTER, SOFT_POINTER, put};
use super::dim::{MIN_3BD, check_handles, corrupt, count, read_count, write_count};
use crate::bits::{BitReader, Color};
use crate::error::Result;
use crate::error::encode_error as encode_err;
use crate::handle::{Handle, HandleRef};
use crate::object::type_code;
use crate::version::Version;

// ---------------------------------------------------------------------------
// Viewports.

/// A paper space viewport (ODS 20.4.38). Fields ODS gives from R2004 or
/// R2007 take, in viewports read from earlier versions, the values AutoCAD
/// writes when it saves such a drawing in the later version.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct Viewport {
    /// DXF 10.
    pub center: [f64; 3],
    /// DXF 40, 41.
    pub width: f64,
    pub height: f64,
    /// DXF 17, 16.
    pub view_target: [f64; 3],
    pub view_direction: [f64; 3],
    /// DXF 51.
    pub twist_angle: f64,
    /// DXF 45.
    pub view_height: f64,
    /// DXF 42.
    pub lens_length: f64,
    /// DXF 43, 44.
    pub front_clip: f64,
    pub back_clip: f64,
    /// DXF 50.
    pub snap_angle: f64,
    /// DXF 12, 13, 14, 15.
    pub view_center: [f64; 2],
    pub snap_base: [f64; 2],
    pub snap_spacing: [f64; 2],
    pub grid_spacing: [f64; 2],
    /// DXF 72.
    pub circle_zoom: i16,
    /// R2007+ major grid line frequency (DXF 61).
    pub grid_major: i16,
    /// DXF 90.
    pub status_flags: i32,
    /// The plot style sheet (DXF 1).
    pub style_sheet: String,
    /// DXF 281.
    pub render_mode: u8,
    /// DXF 74, 71.
    pub ucs_at_origin: bool,
    pub ucs_per_viewport: bool,
    /// DXF 110, 111, 112.
    pub ucs_origin: [f64; 3],
    pub ucs_x_axis: [f64; 3],
    pub ucs_y_axis: [f64; 3],
    /// DXF 146.
    pub ucs_elevation: f64,
    /// DXF 79.
    pub ucs_ortho_type: i16,
    /// R2004+ (DXF 170).
    pub shade_plot_mode: i16,
    /// R2007+ (DXF 292, 282, 141, 142, 63).
    pub use_default_lights: bool,
    pub default_lighting_type: u8,
    pub brightness: f64,
    pub contrast: f64,
    pub ambient_color: Color,
    /// Layers frozen in this viewport (DXF 341; soft pointers from R2004).
    pub frozen_layers: Vec<Handle>,
    /// The clip boundary entity (DXF 340); null when none. Written as a hard
    /// pointer, as the corpus stores it and ACadSharp writes it (ODS: soft).
    pub clip_boundary: Handle,
    /// The VP_ENT_HDR, stored in R2000 only (hard pointer). R2004+ has no
    /// place for it: it is not written.
    pub vp_ent_header: Option<Handle>,
    /// The named and base UCS (hard pointers, DXF 345, 346).
    pub named_ucs: Handle,
    pub base_ucs: Handle,
    /// R2007+ background (soft pointer, DXF 332), visual style (hard
    /// pointer, DXF 348), shade plot (soft pointer, DXF 333) and sun (hard
    /// owner, DXF 361); null before. (AutoCAD gives an R2007 copy of an
    /// older viewport the drawing's default visual style, an object an
    /// older drawing does not have; it stays null here.)
    pub background: Handle,
    pub visual_style: Handle,
    pub shade_plot: Handle,
    pub sun: Handle,
}

/// The R2007+ viewport values before R2007: grid major, use default lights,
/// lighting type, brightness, contrast and ambient color, as AutoCAD writes
/// for viewports that had none (see `corpus_viewport_values`).
const VIEWPORT_R2007_DEFAULTS: (i16, bool, u8, f64, f64) = (5, true, 1, 0.0, 0.0);

/// The R2007+ ambient light color before R2007, as AutoCAD writes it.
const VIEWPORT_AMBIENT_DEFAULT: Color = Color::Rgb {
    r: 51,
    g: 51,
    b: 51,
};

/// The smallest encoding of a 2RD.
const MIN_2RD: u64 = 128;

impl Viewport {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let v = r.version;
        let d = &mut r.data;
        let center = d.read_3bd()?;
        let width = d.read_bd()?;
        let height = d.read_bd()?;
        let view_target = d.read_3bd()?;
        let view_direction = d.read_3bd()?;
        let twist_angle = d.read_bd()?;
        let view_height = d.read_bd()?;
        let lens_length = d.read_bd()?;
        let front_clip = d.read_bd()?;
        let back_clip = d.read_bd()?;
        let snap_angle = d.read_bd()?;
        let view_center = d.read_2rd()?;
        let snap_base = d.read_2rd()?;
        let snap_spacing = d.read_2rd()?;
        let grid_spacing = d.read_2rd()?;
        let circle_zoom = d.read_bs()?;
        let (major, lights, lighting, brightness, contrast) = VIEWPORT_R2007_DEFAULTS;
        let grid_major = if v >= Version::R2007 {
            d.read_bs()?
        } else {
            major
        };
        let frozen = count(d.read_bl()?, "viewport frozen layers")?;
        check_handles(r, frozen, "viewport frozen layers")?;
        let status_flags = r.data.read_bl()?;
        let style_sheet = r.text()?;
        let d = &mut r.data;
        let render_mode = d.read_rc()?;
        let ucs_at_origin = d.read_b()?;
        let ucs_per_viewport = d.read_b()?;
        let ucs_origin = d.read_3bd()?;
        let ucs_x_axis = d.read_3bd()?;
        let ucs_y_axis = d.read_3bd()?;
        let ucs_elevation = d.read_bd()?;
        let ucs_ortho_type = d.read_bs()?;
        let shade_plot_mode = if v >= Version::R2004 { d.read_bs()? } else { 0 };
        let (use_default_lights, default_lighting_type, brightness, contrast, ambient_color) =
            if v >= Version::R2007 {
                let d = &mut r.data;
                let lights = d.read_b()?;
                let lighting = d.read_rc()?;
                let brightness = d.read_bd()?;
                let contrast = d.read_bd()?;
                (lights, lighting, brightness, contrast, r.read_cmc()?)
            } else {
                (
                    lights,
                    lighting,
                    brightness,
                    contrast,
                    VIEWPORT_AMBIENT_DEFAULT,
                )
            };
        let frozen_layers = (0..frozen).map(|_| r.href_abs()).collect::<Result<_>>()?;
        let clip_boundary = r.href_abs()?;
        let vp_ent_header = if v == Version::R2000 {
            Some(r.href_abs()?)
        } else {
            None
        };
        let named_ucs = r.href_abs()?;
        let base_ucs = r.href_abs()?;
        let (background, visual_style, shade_plot, sun) = if v >= Version::R2007 {
            (r.href_abs()?, r.href_abs()?, r.href_abs()?, r.href_abs()?)
        } else {
            (Handle(0), Handle(0), Handle(0), Handle(0))
        };
        Ok(Viewport {
            center,
            width,
            height,
            view_target,
            view_direction,
            twist_angle,
            view_height,
            lens_length,
            front_clip,
            back_clip,
            snap_angle,
            view_center,
            snap_base,
            snap_spacing,
            grid_spacing,
            circle_zoom,
            grid_major,
            status_flags,
            style_sheet,
            render_mode,
            ucs_at_origin,
            ucs_per_viewport,
            ucs_origin,
            ucs_x_axis,
            ucs_y_axis,
            ucs_elevation,
            ucs_ortho_type,
            shade_plot_mode,
            use_default_lights,
            default_lighting_type,
            brightness,
            contrast,
            ambient_color,
            frozen_layers,
            clip_boundary,
            vp_ent_header,
            named_ucs,
            base_ucs,
            background,
            visual_style,
            shade_plot,
            sun,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        let d = &mut w.data;
        d.write_3bd(self.center);
        d.write_bd(self.width);
        d.write_bd(self.height);
        d.write_3bd(self.view_target);
        d.write_3bd(self.view_direction);
        for v in [
            self.twist_angle,
            self.view_height,
            self.lens_length,
            self.front_clip,
            self.back_clip,
            self.snap_angle,
        ] {
            d.write_bd(v);
        }
        for p in [
            self.view_center,
            self.snap_base,
            self.snap_spacing,
            self.grid_spacing,
        ] {
            d.write_2rd(p);
        }
        d.write_bs(self.circle_zoom);
        d.write_bs(self.grid_major);
        write_count(w, self.frozen_layers.len(), "viewport frozen layers")?;
        w.data.write_bl(self.status_flags);
        w.text(&self.style_sheet)?;
        let d = &mut w.data;
        d.write_rc(self.render_mode);
        d.write_b(self.ucs_at_origin);
        d.write_b(self.ucs_per_viewport);
        d.write_3bd(self.ucs_origin);
        d.write_3bd(self.ucs_x_axis);
        d.write_3bd(self.ucs_y_axis);
        d.write_bd(self.ucs_elevation);
        d.write_bs(self.ucs_ortho_type);
        d.write_bs(self.shade_plot_mode);
        d.write_b(self.use_default_lights);
        d.write_rc(self.default_lighting_type);
        d.write_bd(self.brightness);
        d.write_bd(self.contrast);
        w.write_cmc(&self.ambient_color)?;
        for &h in &self.frozen_layers {
            put(w, SOFT_POINTER, h)?;
        }
        put(w, HARD_POINTER, self.clip_boundary)?;
        put(w, HARD_POINTER, self.named_ucs)?;
        put(w, HARD_POINTER, self.base_ucs)?;
        put(w, SOFT_POINTER, self.background)?;
        put(w, HARD_POINTER, self.visual_style)?;
        put(w, SOFT_POINTER, self.shade_plot)?;
        put(w, HARD_OWNER, self.sun)
    }
}

// ---------------------------------------------------------------------------
// Images and wipeouts.

/// An image (class type `IMAGE`) or wipeout (class type `WIPEOUT`; ODS
/// 20.4.80 IMAGE, which a wipeout shares).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct Image {
    /// DXF 90.
    pub class_version: i32,
    /// DXF 10, 11, 12.
    pub insertion: [f64; 3],
    pub u_vector: [f64; 3],
    pub v_vector: [f64; 3],
    /// The image size in pixels (DXF 13).
    pub size: [f64; 2],
    /// DXF 70: 1 show, 2 show when not aligned, 4 use the clip boundary, 8
    /// transparency.
    pub display_flags: i16,
    /// DXF 280.
    pub clipping: bool,
    /// DXF 281, 282, 283.
    pub brightness: u8,
    pub contrast: u8,
    pub fade: u8,
    /// R2010+: clip inside the boundary (DXF 290); `false` (outside) before.
    pub clip_inside: bool,
    /// 1 rectangle, 2 polygon (DXF 71).
    pub clip_type: i16,
    /// The clip boundary (DXF 14), in pixels: for a rectangle (type 1) its
    /// two corners, exactly; otherwise the polygon's vertices.
    pub clip_boundary: Vec<[f64; 2]>,
    /// The IMAGEDEF (hard pointer, DXF 340).
    pub image_def: Handle,
    /// The IMAGEDEF_REACTOR (hard owner, DXF 360).
    pub reactor: Handle,
}

/// [`Image::clip_type`] for a rectangle.
pub const CLIP_RECTANGLE: i16 = 1;

impl Image {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let v = r.version;
        let d = &mut r.data;
        let class_version = d.read_bl()?;
        let insertion = d.read_3bd()?;
        let u_vector = d.read_3bd()?;
        let v_vector = d.read_3bd()?;
        let size = d.read_2rd()?;
        let display_flags = d.read_bs()?;
        let clipping = d.read_b()?;
        let brightness = d.read_rc()?;
        let contrast = d.read_rc()?;
        let fade = d.read_rc()?;
        let clip_inside = if v >= Version::R2010 {
            d.read_b()?
        } else {
            false
        };
        let clip_type = d.read_bs()?;
        let clip_boundary = if clip_type == CLIP_RECTANGLE {
            vec![d.read_2rd()?, d.read_2rd()?]
        } else {
            let n = read_count(r, MIN_2RD, "image clip vertices")?;
            (0..n).map(|_| r.data.read_2rd()).collect::<Result<_>>()?
        };
        Ok(Image {
            class_version,
            insertion,
            u_vector,
            v_vector,
            size,
            display_flags,
            clipping,
            brightness,
            contrast,
            fade,
            clip_inside,
            clip_type,
            clip_boundary,
            image_def: r.href_abs()?,
            reactor: r.href_abs()?,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        if self.clip_type == CLIP_RECTANGLE && self.clip_boundary.len() != 2 {
            return Err(encode_err(format!(
                "a rectangular clip boundary needs 2 corners, not {}",
                self.clip_boundary.len()
            )));
        }
        let d = &mut w.data;
        d.write_bl(self.class_version);
        d.write_3bd(self.insertion);
        d.write_3bd(self.u_vector);
        d.write_3bd(self.v_vector);
        d.write_2rd(self.size);
        d.write_bs(self.display_flags);
        d.write_b(self.clipping);
        d.write_rc(self.brightness);
        d.write_rc(self.contrast);
        d.write_rc(self.fade);
        d.write_b(self.clip_inside);
        d.write_bs(self.clip_type);
        if self.clip_type != CLIP_RECTANGLE {
            write_count(w, self.clip_boundary.len(), "image clip vertices")?;
        }
        for &p in &self.clip_boundary {
            w.data.write_2rd(p);
        }
        put(w, HARD_POINTER, self.image_def)?;
        put(w, HARD_OWNER, self.reactor)
    }
}

// ---------------------------------------------------------------------------
// ACIS entities.

/// Which ACIS entity type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub enum AcisKind {
    Region,
    Solid3d,
    Body,
}

impl AcisKind {
    pub(crate) fn type_code(self) -> u16 {
        match self {
            AcisKind::Region => type_code::REGION,
            AcisKind::Solid3d => type_code::SOLID3D,
            AcisKind::Body => type_code::BODY,
        }
    }
}

/// A GUID as an R2013+ ACIS entity stores it: `BL` data 1, `BS` data 2
/// and 3, then 8 `RC`. [`std::fmt::Display`] gives the DXF form
/// (`{1a113328-eb6d-d44d-824d-78b33668f9e7}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct AcisGuid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

impl AcisGuid {
    /// The GUID the drawing writer gives an entity converted from an older
    /// version: derived from the drawing's `$FINGERPRINTGUID` and the
    /// entity's handle, so the same drawing always gets the same GUIDs, in
    /// the form of AutoCAD's (version 4: the corpus GUIDs have 4 as the
    /// high digit of `data3`'s low byte, and the RFC 4122 variant in
    /// `data4[0]`). AutoCAD's are random; nothing is known to depend on
    /// their value beyond uniqueness.
    pub fn derived(fingerprint: &str, handle: Handle) -> AcisGuid {
        // FNV-1a over the fingerprint and the handle, then two SplitMix64
        // steps for the 128 bits.
        let mut x: u64 = 0xCBF2_9CE4_8422_2325;
        for b in fingerprint.bytes().chain([0]).chain(handle.0.to_le_bytes()) {
            x = (x ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01B3);
        }
        let mut next = || {
            x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = x;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        };
        let (a, b) = (next(), next());
        let mut data4 = b.to_le_bytes();
        data4[0] = (data4[0] & 0x3F) | 0x80;
        AcisGuid {
            data1: a as u32,
            data2: (a >> 32) as u16,
            data3: ((a >> 48) as u16 & 0xFF0F) | 0x0040,
            data4,
        }
    }
}

impl std::fmt::Display for AcisGuid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let d = &self.data4;
        write!(
            f,
            "{{{:08x}-{:04x}-{:04x}-{:02x}{:02x}-",
            self.data1, self.data2, self.data3, d[0], d[1]
        )?;
        d[2..].iter().try_for_each(|b| write!(f, "{b:02x}"))?;
        write!(f, "}}")
    }
}

/// An ACIS entity's modeler data, kept as stored.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub enum AcisPayload {
    /// R2013+: the entity stores no modeler data. Its SAB is a record of
    /// the `AcDb:AcDsPrototype_1b` section ([`crate::Drawing::data_storage`])
    /// keyed by the entity's handle. The entity ends (after
    /// the R2007 unknown `BL`) with these fields, which ODS does not
    /// describe; the corpus pins them against the DXF export.
    DataStorage {
        /// DXF 290 (1 on every corpus entity).
        flag_290: bool,
        /// DXF 2: the entity's GUID.
        guid: AcisGuid,
        /// A last field, 0 on every corpus entity; read as a `BS` (a `BL`
        /// 0 would be the same two bits).
        unknown: i16,
    },
    /// Before R2013 with the "ACIS empty" bit set: no data.
    Empty,
    /// Before R2013, format version 1: SAT text in blocks, each stored
    /// encrypted (see [`decrypt_sat`]), kept verbatim.
    SatBlocks {
        /// ODS's unknown bit.
        unknown: bool,
        /// The blocks' bytes, one after another.
        data: Vec<u8>,
        /// The size of each block.
        block_sizes: Vec<u32>,
    },
    /// Before R2013, format version 2: a SAB file (or SAT text) through its
    /// end-of-data marker, kept verbatim.
    Inline {
        /// ODS's unknown bit.
        unknown: bool,
        data: Vec<u8>,
    },
}

impl AcisPayload {
    /// Whether the entity carries modeler data in itself (before R2013).
    pub fn is_inline(&self) -> bool {
        matches!(
            self,
            AcisPayload::SatBlocks { .. } | AcisPayload::Inline { .. }
        )
    }
}

/// A REGION, 3DSOLID or BODY (ODS 20.4.41).
///
/// Its ACIS (modeler) data is kept untouched: see [`AcisPayload`]. R2018
/// keeps that data in the `AcDb:AcDsPrototype_1b` section
/// ([`crate::section::DataStorage`]), not in the entity, and stores a GUID
/// per entity that older versions do not have. The drawing writer converts
/// an entity read from R2004-R2010 ([`AcisEntity::to_r2013`]): its SAB goes
/// to an AcDs record keyed by its handle, and it gets a GUID derived from
/// the drawing's fingerprint and its handle ([`AcisGuid::derived`]). R2000
/// SAT text cannot be converted: AcDs holds SAB (ODS 24.2.2.3), and no
/// source this library may use specifies the SAT to SAB mapping, so such
/// an entity stays [`crate::Error::UnconvertibleObjects`].
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct AcisEntity {
    pub kind: AcisKind,
    pub payload: AcisPayload,
    /// The wireframe (isolines) and silhouette data, from its "wireframe
    /// present" bit through the trailing "ACIS empty" bit, as stored bits
    /// (MSB-first, zero-padded). The layout is checked on read but the bits
    /// are not interpreted.
    pub wireframe: Vec<u8>,
    pub wireframe_bits: u64,
    /// R2007+ unknown `BL`; 0 before.
    pub unknown: i32,
    /// R2007+ history (hard owner, DXF 350), stored by 3DSOLIDs and BODYs
    /// (not REGIONs); `None` before R2007 and for REGIONs.
    pub history: Option<Handle>,
}

/// The SAB and SAT end-of-data markers that end a version 2 payload: SAT
/// text spells them out, SAB stores them as tagged strings (ODS 20.4.41,
/// ACadSharp `AcisTextCodec`).
const ACIS_END_MARKERS: [&[u8]; 4] = [
    b"End-of-ACIS-data",
    b"End-of-ASM-data",
    b"\x0E\x03End\x0E\x02of\x0E\x04ACIS\x0D\x04data",
    b"\x0E\x03End\x0E\x02of\x0E\x03ASM\x0D\x04data",
];

/// Decrypt a version 1 SAT block: each printable byte but the space (0x21
/// to 0x7E) is stored as 0x9F minus it; other bytes (space, CR, LF) are
/// stored as they are. ODS gives the range as 0x20-0x7E, which would turn a
/// space into 0x7F; the corpus SAT decrypts to text with plain spaces, as
/// ACadSharp (MIT) `AcisTextCodec` decodes it. The mapping is its own
/// inverse, so it also encrypts.
pub fn decrypt_sat(block: &[u8]) -> Vec<u8> {
    block
        .iter()
        .map(|&b| {
            if (0x21..=0x7E).contains(&b) {
                0x9F - b
            } else {
                b
            }
        })
        .collect()
}

/// Whether `data` is a SAB file (ODS 24.2.2.3: AcDs records hold SAB,
/// which starts with "ACIS BinaryFile", or from ASM 2013 "ASM BinaryFile"
/// as AutoCAD 2018 writes it).
fn is_sab(data: &[u8]) -> bool {
    data.starts_with(b"ACIS BinaryFile") || data.starts_with(b"ASM BinaryFile")
}

/// The end of the first end-of-data marker in `data` (the earliest end of
/// any marker), in one pass: at each end position, whether a marker ends
/// there.
fn acis_end(data: &[u8]) -> Option<usize> {
    (1..=data.len()).find(|&end| {
        ACIS_END_MARKERS
            .iter()
            .any(|m| end >= m.len() && data[end - m.len()..end] == **m)
    })
}

fn skip_3bd(d: &mut BitReader, n: usize) -> Result<()> {
    for _ in 0..n {
        d.read_3bd()?;
    }
    Ok(())
}

/// A `BL` count of items of at least `bits` bits, checked against the data
/// left.
fn walk_count(d: &mut BitReader, bits: u64, what: &str) -> Result<usize> {
    let n = count(d.read_bl()?, what)?;
    if (n as u64).saturating_mul(bits) > d.remaining_bits() {
        return Err(corrupt(format!("{what} count {n} exceeds the data")));
    }
    Ok(n)
}

/// The smallest wire: RC, BL, BS, BL, BL and a B.
const MIN_WIRE: u64 = 8 + 4 * 2 + 1;
/// The smallest silhouette: BL, three 3BD, B, BL.
const MIN_SILHOUETTE: u64 = 2 + 3 * MIN_3BD + 1 + 2;

/// Read past a wire (ODS 20.4.41).
fn skip_wire(d: &mut BitReader) -> Result<()> {
    d.read_rc()?;
    d.read_bl()?;
    d.read_bs()?;
    d.read_bl()?;
    let n = walk_count(d, MIN_3BD, "wire points")?;
    skip_3bd(d, n)?;
    if d.read_b()? {
        skip_3bd(d, 4)?;
        d.read_bd()?;
        for _ in 0..3 {
            d.read_b()?;
        }
    }
    Ok(())
}

fn skip_wires(d: &mut BitReader) -> Result<()> {
    let n = walk_count(d, MIN_WIRE, "wires")?;
    (0..n).try_for_each(|_| skip_wire(d))
}

/// Read past the wireframe and silhouette data (ODS 20.4.41). A trailing
/// "ACIS empty" bit of 0 would mean more modeler data follows, which is not
/// supported (the entity is then kept raw).
fn skip_wireframe(d: &mut BitReader) -> Result<()> {
    if !d.read_b()? {
        return Ok(());
    }
    if d.read_b()? {
        d.read_3bd()?;
    }
    d.read_bl()?; // the isoline count
    if d.read_b()? {
        skip_wires(d)?;
    }
    let n = walk_count(d, MIN_SILHOUETTE, "silhouettes")?;
    for _ in 0..n {
        d.read_bl()?;
        skip_3bd(d, 3)?;
        d.read_b()?;
        skip_wires(d)?;
    }
    if !d.read_b()? {
        return Err(corrupt("ACIS data after the wireframe is not supported"));
    }
    Ok(())
}

impl AcisEntity {
    /// The modeler data before the wireframe, stored before R2013 only.
    fn decode_payload(r: &mut ObjReader) -> Result<AcisPayload> {
        let d = &mut r.data;
        if d.read_b()? {
            return Ok(AcisPayload::Empty);
        }
        let unknown = d.read_b()?;
        Ok(match d.read_bs()? {
            1 => {
                let mut data = Vec::new();
                let mut block_sizes = Vec::new();
                loop {
                    let n = count(d.read_bl()?, "SAT block size")?;
                    if n == 0 {
                        break;
                    }
                    data.extend_from_slice(&d.read_bytes(n)?);
                    block_sizes.push(n as u32);
                }
                AcisPayload::SatBlocks {
                    unknown,
                    data,
                    block_sizes,
                }
            }
            2 => {
                // No length is stored: the data runs to its end marker.
                let start = d.bit_pos();
                let mut rest = d.clone();
                let mut data = rest.read_bytes((rest.remaining_bits() / 8) as usize)?;
                let end = acis_end(&data).ok_or_else(|| corrupt("no end of ACIS data"))?;
                d.seek_bit(start + 8 * end as u64)?;
                data.truncate(end);
                AcisPayload::Inline { unknown, data }
            }
            v => return Err(corrupt(format!("unknown ACIS data version {v}"))),
        })
    }

    pub(crate) fn decode(r: &mut ObjReader, kind: AcisKind) -> Result<Self> {
        let v = r.version;
        let old_payload = if v < Version::R2013 {
            Some(Self::decode_payload(r)?)
        } else {
            None
        };
        let start = r.data.bit_pos();
        let mut walk = r.data.clone();
        skip_wireframe(&mut walk)?;
        let wireframe_bits = walk.bit_pos() - start;
        let wireframe = take_bits(&mut r.data, wireframe_bits)?;
        let unknown = if v >= Version::R2007 {
            r.data.read_bl()?
        } else {
            0
        };
        let payload = if let Some(p) = old_payload {
            p
        } else {
            let d = &mut r.data;
            let flag_290 = d.read_b()?;
            let data1 = d.read_bl()? as u32;
            let data2 = d.read_bs()? as u16;
            let data3 = d.read_bs()? as u16;
            let mut data4 = [0; 8];
            for b in &mut data4 {
                *b = d.read_rc()?;
            }
            AcisPayload::DataStorage {
                flag_290,
                guid: AcisGuid {
                    data1,
                    data2,
                    data3,
                    data4,
                },
                unknown: d.read_bs()?,
            }
        };
        let history = if v >= Version::R2007 && kind != AcisKind::Region {
            Some(r.href_abs()?)
        } else {
            None
        };
        Ok(AcisEntity {
            kind,
            payload,
            wireframe,
            wireframe_bits,
            unknown,
            history,
        })
    }

    /// Whether the drawing writer can write the entity as R2018: read from
    /// R2013+, or convertible by [`AcisEntity::to_r2013`] (see the type
    /// documentation).
    pub fn is_convertible(&self) -> bool {
        match &self.payload {
            AcisPayload::DataStorage { .. } | AcisPayload::Empty => true,
            AcisPayload::Inline { data, .. } => is_sab(data),
            AcisPayload::SatBlocks { .. } => false,
        }
    }

    /// The entity in its R2013+ form, with `guid`, and the SAB data that
    /// belongs in its AcDs record (`None` for an entity without data, or
    /// one already in that form, returned unchanged). `None` when it cannot
    /// be converted (SAT text).
    ///
    /// The R2013+ fields are set as on every corpus entity: DXF 290 set and
    /// a trailing 0 (an entity without data gets the same; the corpus has
    /// none to compare). A 3DSOLID or BODY read before R2007 has no
    /// history, and gets a null one.
    pub fn to_r2013(&self, guid: AcisGuid) -> Option<(AcisEntity, Option<Vec<u8>>)> {
        let data = match &self.payload {
            AcisPayload::DataStorage { .. } => return Some((self.clone(), None)),
            AcisPayload::Empty => None,
            AcisPayload::Inline { data, .. } if is_sab(data) => Some(data.clone()),
            AcisPayload::Inline { .. } | AcisPayload::SatBlocks { .. } => return None,
        };
        let history = match self.kind {
            AcisKind::Region => None,
            AcisKind::Solid3d | AcisKind::Body => Some(self.history.unwrap_or_default()),
        };
        let entity = AcisEntity {
            payload: AcisPayload::DataStorage {
                flag_290: true,
                guid,
                unknown: 0,
            },
            history,
            ..self.clone()
        };
        Some((entity, data))
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        let AcisPayload::DataStorage {
            flag_290,
            guid,
            unknown,
        } = &self.payload
        else {
            return Err(encode_err(
                "an ACIS entity read before R2013 is written as R2018 by the drawing \
                 writer, which moves its data to AcDs and gives it a GUID",
            ));
        };
        let has_history = self.kind != AcisKind::Region;
        if self.history.is_some() != has_history {
            return Err(encode_err(format!(
                "a {:?} needs {} history handle",
                self.kind,
                if has_history { "a" } else { "no" }
            )));
        }
        let d = &mut w.data;
        d.write_bits_from(&self.wireframe, self.wireframe_bits)?;
        d.write_bl(self.unknown);
        d.write_b(*flag_290);
        d.write_bl(guid.data1 as i32);
        d.write_bs(guid.data2 as i16);
        d.write_bs(guid.data3 as i16);
        for &b in &guid.data4 {
            d.write_rc(b);
        }
        d.write_bs(*unknown);
        if let Some(h) = self.history {
            put(w, HARD_OWNER, h)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// OLE frames.

/// An OLE2FRAME (ODS 20.4.88): an embedded OLE object, kept as stored.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Ole2Frame {
    /// DXF 70.
    pub flags: i16,
    /// ODS "Mode".
    pub mode: i16,
    /// The OLE2 data, verbatim.
    pub data: Vec<u8>,
    /// ODS's unknown byte.
    pub unknown: u8,
}

impl Ole2Frame {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let d = &mut r.data;
        let flags = d.read_bs()?;
        let mode = d.read_bs()?;
        let n = count(d.read_bl()?, "OLE data length")?;
        let data = d.read_bytes(n)?;
        Ok(Ole2Frame {
            flags,
            mode,
            data,
            unknown: d.read_rc()?,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        w.data.write_bs(self.flags);
        w.data.write_bs(self.mode);
        write_count(w, self.data.len(), "OLE data bytes")?;
        w.data.write_bytes(&self.data);
        w.data.write_rc(self.unknown);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Proxy entities.

/// An ACAD_PROXY_ENTITY (ODS 20.4.90): an entity of a class whose
/// application was not present, its data kept as stored.
///
/// The proxy's data is in the object format named by its own version
/// fields, not the drawing's, so it is written back unchanged into R2018
/// even when read from R2000 or R2004: proxies are always convertible.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct ProxyEntity {
    /// DXF 91.
    pub class_id: i32,
    /// The AutoCAD version (DXF 71) and maintenance version (DXF 97) of the
    /// data. Before R2018 they are stored as one `BL` "drawing format"
    /// (DXF 95), the version in the low 16 bits and the maintenance version
    /// above (as ACadSharp (MIT) splits it).
    pub version: u32,
    pub maintenance_version: u32,
    /// DXF 70: the original data was DXF (else DWG).
    pub original_data_is_dxf: bool,
    /// The data bits, MSB-first and zero-padded.
    pub data: Vec<u8>,
    pub data_bits: u64,
    /// R2007+: the string stream bits, as stored; `None` when empty.
    pub strings: Option<(Vec<u8>, u64)>,
    /// The object ids: every reference left in the handle stream, kept as
    /// stored.
    pub object_ids: Vec<HandleRef>,
    /// Handle stream bits after the last reference that do not parse as
    /// one, kept as stored (see [`crate::object::HandleTail`]).
    #[cfg_attr(feature = "serde", serde(default))]
    pub handle_tail: HandleTail,
}

impl ProxyEntity {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let d = &mut r.data;
        let class_id = d.read_bl()?;
        let (version, maintenance_version) = if r.version >= Version::R2018 {
            (d.read_bl()? as u32, d.read_bl()? as u32)
        } else {
            let format = d.read_bl()? as u32;
            (format & 0xFFFF, format >> 16)
        };
        let original_data_is_dxf = r.data.read_b()?;
        let (data, data_bits) = take_rest(&mut r.data)?;
        let strings = match r.strings.as_mut() {
            Some(s) => Some(take_rest(s)?).filter(|(_, bits)| *bits > 0),
            None => None,
        };
        let (object_ids, handle_tail) = trailing_refs(r)?;
        Ok(ProxyEntity {
            class_id,
            version,
            maintenance_version,
            original_data_is_dxf,
            data,
            data_bits,
            strings,
            object_ids,
            handle_tail,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        w.data.write_bl(self.class_id);
        w.data.write_bl(self.version as i32);
        w.data.write_bl(self.maintenance_version as i32);
        w.data.write_b(self.original_data_is_dxf);
        w.data.write_bits_from(&self.data, self.data_bits)?;
        if let Some((bytes, bits)) = &self.strings {
            w.write_raw_strings(bytes, *bits)?;
        }
        for &h in &self.object_ids {
            w.href(h)?;
        }
        self.handle_tail.write(w)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::tests::{reader, roundtrip};
    use super::*;
    use crate::bits::BitWriter;
    use crate::error::Error;
    use crate::object::stream::tests::assemble;

    /// The one-pass marker search finds the earliest end, as searching for
    /// each marker and taking the first end does.
    #[test]
    fn acis_end_is_the_earliest_marker_end() {
        let each = |data: &[u8]| {
            ACIS_END_MARKERS
                .iter()
                .filter_map(|m| {
                    data.windows(m.len())
                        .position(|w| w == *m)
                        .map(|at| at + m.len())
                })
                .min()
        };
        let sab_acis = ACIS_END_MARKERS[2];
        let sab_asm = ACIS_END_MARKERS[3];
        let cases: Vec<Vec<u8>> = vec![
            b"".to_vec(),
            b"no marker".to_vec(),
            b"x End-of-ACIS-data y".to_vec(),
            b"End-of-ASM-dataEnd-of-ACIS-data".to_vec(),
            b"End-of-ACIS-dataEnd-of-ASM-data".to_vec(),
            [b"head".as_slice(), sab_asm, sab_acis].concat(),
            [b"head".as_slice(), sab_acis, b"tail"].concat(),
            b"End-of-ACIS-dat".to_vec(),
        ];
        for c in cases {
            assert_eq!(acis_end(&c), each(&c), "{c:?}");
        }
    }

    pub(crate) fn viewport() -> Viewport {
        Viewport {
            center: [1.0, 2.0, 0.0],
            width: 30.0,
            height: 20.0,
            view_target: [0.5, 0.5, 0.0],
            view_direction: [0.0, 0.0, 1.0],
            twist_angle: 0.25,
            view_height: 12.0,
            lens_length: 50.0,
            front_clip: 1.0,
            back_clip: -1.0,
            snap_angle: 0.0,
            view_center: [3.0, 4.0],
            snap_base: [0.0, 0.0],
            snap_spacing: [0.5, 0.5],
            grid_spacing: [10.0, 10.0],
            circle_zoom: 1000,
            grid_major: 5,
            status_flags: 819232,
            style_sheet: "monochrome.ctb ü".into(),
            render_mode: 2,
            ucs_at_origin: true,
            ucs_per_viewport: true,
            ucs_origin: [0.0; 3],
            ucs_x_axis: [1.0, 0.0, 0.0],
            ucs_y_axis: [0.0, 1.0, 0.0],
            ucs_elevation: 2.5,
            ucs_ortho_type: 1,
            shade_plot_mode: 3,
            use_default_lights: false,
            default_lighting_type: 2,
            brightness: 10.0,
            contrast: -5.0,
            ambient_color: VIEWPORT_AMBIENT_DEFAULT,
            frozen_layers: vec![Handle(0x10), Handle(0x11)],
            clip_boundary: Handle(0x95),
            vp_ent_header: None,
            named_ucs: Handle(0x96),
            base_ucs: Handle(0),
            background: Handle(0x97),
            visual_style: Handle(0x9F),
            shade_plot: Handle(0),
            sun: Handle(0x98),
        }
    }

    pub(crate) fn image() -> Image {
        Image {
            class_version: 0,
            insertion: [0.0, 15.0, 0.0],
            u_vector: [0.01, 0.0, 0.0],
            v_vector: [0.0, 0.01, 0.0],
            size: [994.0, 965.0],
            display_flags: 7,
            clipping: true,
            brightness: 50,
            contrast: 60,
            fade: 10,
            clip_inside: true,
            clip_type: 2,
            clip_boundary: vec![[-0.5, 0.5], [0.3, 0.5], [0.25, -0.2]],
            image_def: Handle(0x8C4),
            reactor: Handle(0x8C5),
        }
    }

    pub(crate) fn acis() -> AcisEntity {
        // A wireframe with a point, two wires (one transformed) and a
        // silhouette with one wire.
        let mut d = BitWriter::new();
        d.write_b(true);
        d.write_b(true);
        d.write_3bd([1.0, 2.0, 3.0]);
        d.write_bl(4);
        d.write_b(true);
        d.write_bl(2);
        let wire = |d: &mut BitWriter, transform: bool| {
            d.write_rc(1);
            d.write_bl(7);
            d.write_bs(256);
            d.write_bl(3);
            d.write_bl(2);
            d.write_3bd([0.0; 3]);
            d.write_3bd([1.0; 3]);
            d.write_b(transform);
            if transform {
                for _ in 0..4 {
                    d.write_3bd([0.0, 0.0, 1.0]);
                }
                d.write_bd(2.0);
                d.write_b(true);
                d.write_b(false);
                d.write_b(false);
            }
        };
        wire(&mut d, false);
        wire(&mut d, true);
        d.write_bl(1);
        d.write_bl(9);
        for _ in 0..3 {
            d.write_3bd([0.0, 0.0, 1.0]);
        }
        d.write_b(false);
        d.write_bl(1);
        wire(&mut d, false);
        d.write_b(true); // ACIS empty
        let wireframe_bits = d.bit_len();
        AcisEntity {
            kind: AcisKind::Solid3d,
            payload: AcisPayload::DataStorage {
                flag_290: true,
                guid: AcisGuid {
                    data1: 0x1A11_3328,
                    data2: 0x00EB,
                    data3: 0xD44D,
                    data4: [0x82, 0x4D, 0x78, 0xB3, 0x36, 0x68, 0xF9, 0xE7],
                },
                unknown: 7,
            },
            wireframe: d.into_bytes(),
            wireframe_bits,
            unknown: 3,
            history: Some(Handle(0xD66)),
        }
    }

    pub(crate) fn proxy() -> ProxyEntity {
        ProxyEntity {
            class_id: 499,
            version: 27,
            maintenance_version: 105,
            original_data_is_dxf: false,
            data: vec![0xDE, 0xAD, 0xBE, 0xE0],
            data_bits: 29,
            strings: Some((vec![0x12, 0x30], 13)),
            object_ids: vec![
                HandleRef {
                    code: 3,
                    value: 0x91,
                },
                HandleRef { code: 8, value: 0 },
                HandleRef {
                    code: 5,
                    value: 0x1234,
                },
            ],
            handle_tail: HandleTail::default(),
        }
    }

    fn viewport_decode(r: &mut ObjReader) -> Result<Viewport> {
        Viewport::decode(r)
    }

    #[test]
    fn roundtrip_viewport_image_and_payloads() {
        roundtrip(
            &viewport(),
            Viewport::encode,
            viewport_decode,
            type_code::VIEWPORT,
        );
        roundtrip(&image(), Image::encode, Image::decode, 500);
        let mut rect = image();
        rect.clip_type = CLIP_RECTANGLE;
        rect.clip_boundary = vec![[-0.5, -0.5], [993.5, 964.5]];
        roundtrip(&rect, Image::encode, Image::decode, 500);
        let solid = |r: &mut ObjReader| AcisEntity::decode(r, AcisKind::Solid3d);
        roundtrip(&acis(), AcisEntity::encode, solid, type_code::SOLID3D);
        let mut region = acis();
        region.kind = AcisKind::Region;
        region.history = None;
        region.wireframe = vec![0]; // no wireframe
        region.wireframe_bits = 1;
        let region_decode = |r: &mut ObjReader| AcisEntity::decode(r, AcisKind::Region);
        roundtrip(
            &region,
            AcisEntity::encode,
            region_decode,
            type_code::REGION,
        );
        let ole = Ole2Frame {
            flags: 2,
            mode: 1,
            data: (0..=255).collect(),
            unknown: 3,
        };
        roundtrip(
            &ole,
            Ole2Frame::encode,
            Ole2Frame::decode,
            type_code::OLE2FRAME,
        );
        // A proxy's data, strings and references are kept as stored, so a
        // cut stream still decodes (to fewer bits): truncation is checked
        // in its own test.
        let p = proxy();
        let mut w = ObjWriter::new(Handle(0x80));
        p.encode(&mut w).unwrap();
        let bytes = w.finish(type_code::ACAD_PROXY_ENTITY).unwrap();
        assert_eq!(ProxyEntity::decode(&mut reader(&bytes)).unwrap(), p);
    }

    /// A rectangular clip boundary must have its two corners.
    #[test]
    fn rectangle_clip_needs_two_corners() {
        let mut i = image();
        i.clip_type = CLIP_RECTANGLE;
        let e = i.encode(&mut ObjWriter::new(Handle(0x80))).unwrap_err();
        assert!(e.to_string().contains("2 corners, not 3"), "{e}");
    }

    /// An R2000 viewport: no R2004/R2007 fields (they take AutoCAD's
    /// values), frozen layers and the VP_ENT_HDR, which R2018 cannot hold.
    #[test]
    fn r2000_viewport() {
        let v = viewport();
        let mut d = BitWriter::new();
        d.write_3bd(v.center);
        d.write_bd(v.width);
        d.write_bd(v.height);
        d.write_3bd(v.view_target);
        d.write_3bd(v.view_direction);
        for x in [0.25, 12.0, 50.0, 1.0, -1.0, 0.0] {
            d.write_bd(x);
        }
        for p in [v.view_center, v.snap_base, v.snap_spacing, v.grid_spacing] {
            d.write_2rd(p);
        }
        d.write_bs(v.circle_zoom);
        d.write_bl(2);
        d.write_bl(v.status_flags);
        d.write_tv_raw(b"plot.ctb").unwrap();
        d.write_rc(2);
        d.write_b(true);
        d.write_b(true);
        d.write_3bd(v.ucs_origin);
        d.write_3bd(v.ucs_x_axis);
        d.write_3bd(v.ucs_y_axis);
        d.write_bd(2.5);
        d.write_bs(1);
        let mut h = BitWriter::new();
        for (code, value) in [
            (5, 0x10),
            (5, 0x11),
            (4, 0x95),
            (5, 0x30),
            (5, 0x96),
            (5, 0),
        ] {
            h.write_handle_ref(HandleRef { code, value }).unwrap();
        }
        let bytes = assemble(Version::R2000, type_code::VIEWPORT, &d, None, &h);
        let (mut r, _) = ObjReader::split(&bytes, Version::R2000, 30).unwrap();
        let x = Viewport::decode(&mut r).unwrap();
        assert_eq!(r.data.remaining_bits(), 0);
        assert_eq!(x.frozen_layers, [Handle(0x10), Handle(0x11)]);
        assert_eq!(x.vp_ent_header, Some(Handle(0x30)));
        assert_eq!((x.named_ucs, x.base_ucs), (Handle(0x96), Handle(0)));
        assert_eq!(x.style_sheet, "plot.ctb");
        assert_eq!(
            (
                x.grid_major,
                x.use_default_lights,
                x.default_lighting_type,
                x.shade_plot_mode
            ),
            (5, true, 1, 0)
        );
        assert_eq!(
            x.ambient_color,
            Color::Rgb {
                r: 51,
                g: 51,
                b: 51
            }
        );
        assert_eq!(x.sun, Handle(0));
        // As R2018: no VP_ENT_HDR, everything else equal.
        let mut w = ObjWriter::new(Handle(0x80));
        x.encode(&mut w).unwrap();
        let out = w.finish(type_code::VIEWPORT).unwrap();
        let back = Viewport::decode(&mut reader(&out)).unwrap();
        assert_eq!(
            back,
            Viewport {
                vp_ent_header: None,
                ..x
            }
        );
    }

    /// The handles of a viewport are written with the reference codes
    /// AutoCAD uses: the clip boundary as a hard pointer (ODS: soft).
    #[test]
    fn viewport_reference_codes() {
        let mut w = ObjWriter::new(Handle(0x80));
        viewport().encode(&mut w).unwrap();
        let bytes = w.finish(type_code::VIEWPORT).unwrap();
        let mut r = reader(&bytes);
        Viewport::decode(&mut r.clone()).unwrap();
        let mut codes = vec![];
        let mut hs = r.handles.clone();
        while hs.remaining_bits() >= 8 {
            codes.push(hs.read_handle_ref().unwrap().code);
        }
        let _ = &mut r;
        assert_eq!(codes, [4, 4, 5, 5, 5, 4, 5, 4, 3]);
    }

    /// An R2000 3DSOLID with SAT blocks; an R2004 REGION with an inline SAB
    /// and no wireframe. Both decode; the SAT cannot be converted to R2018,
    /// the SAB can (its data moving to AcDs).
    #[test]
    fn inline_acis_is_read_and_converted() {
        let sat = decrypt_sat(b"400 26 1 0 \r\nbody $-1 #\r\n");
        let mut d = BitWriter::new();
        d.write_b(false); // data follows
        d.write_b(true);
        d.write_bs(1);
        for block in [&sat[..10], &sat[10..]] {
            d.write_bl(block.len() as i32);
            d.write_bytes(block);
        }
        d.write_bl(0);
        d.write_b(false); // no wireframe
        let bytes = assemble(
            Version::R2000,
            type_code::SOLID3D,
            &d,
            None,
            &BitWriter::new(),
        );
        let (mut r, _) = ObjReader::split(&bytes, Version::R2000, 30).unwrap();
        let x = AcisEntity::decode(&mut r, AcisKind::Solid3d).unwrap();
        assert_eq!(r.data.remaining_bits(), 0);
        let AcisPayload::SatBlocks {
            unknown,
            data,
            block_sizes,
        } = &x.payload
        else {
            panic!("{:?}", x.payload);
        };
        assert!(*unknown);
        assert_eq!(data, &sat);
        assert_eq!(block_sizes, &[10, sat.len() as u32 - 10]);
        assert_eq!(decrypt_sat(data), b"400 26 1 0 \r\nbody $-1 #\r\n");
        assert!(x.payload.is_inline() && !x.is_convertible());
        assert!(x.encode(&mut ObjWriter::new(Handle(0x80))).is_err());
        assert!(x.to_r2013(AcisGuid::default()).is_none());

        let mut sab = b"ACIS BinaryFile\x01\x02".to_vec();
        sab.extend_from_slice(ACIS_END_MARKERS[2]);
        let mut d = BitWriter::new();
        d.write_b(false);
        d.write_b(false);
        d.write_bs(2);
        d.write_bytes(&sab);
        d.write_b(false); // no wireframe
        let bytes = assemble(
            Version::R2004,
            type_code::REGION,
            &d,
            None,
            &BitWriter::new(),
        );
        let (mut r, _) = ObjReader::split(&bytes, Version::R2004, 30).unwrap();
        let x = AcisEntity::decode(&mut r, AcisKind::Region).unwrap();
        assert_eq!(r.data.remaining_bits(), 0);
        assert_eq!(
            x.payload,
            AcisPayload::Inline {
                unknown: false,
                data: sab.clone()
            }
        );
        assert_eq!((x.wireframe_bits, x.unknown, x.history), (1, 0, None));
        assert!(x.is_convertible());
        let e = x.encode(&mut ObjWriter::new(Handle(0x80))).unwrap_err();
        assert!(e.to_string().contains("drawing writer"), "{e}");
        let guid = AcisGuid::derived("{fp}", Handle(0x80));
        let (y, data) = x.to_r2013(guid).unwrap();
        assert_eq!(data.as_deref(), Some(&sab[..]));
        assert_eq!(
            y.payload,
            AcisPayload::DataStorage {
                flag_290: true,
                guid,
                unknown: 0
            }
        );
        assert_eq!((y.wireframe_bits, y.history), (1, None));
        let decode = |r: &mut ObjReader| AcisEntity::decode(r, AcisKind::Region);
        roundtrip(&y, AcisEntity::encode, decode, type_code::REGION);
        // Already converted: returned unchanged, with no data.
        assert_eq!(y.to_r2013(AcisGuid::default()), Some((y.clone(), None)));
        // SAT text in a version 2 payload is not SAB.
        let mut sat = x.clone();
        sat.payload = AcisPayload::Inline {
            unknown: false,
            data: b"700 0 1 0 End-of-ACIS-data".to_vec(),
        };
        assert!(!sat.is_convertible() && sat.to_r2013(guid).is_none());
    }

    /// An R2004 entity with the "ACIS empty" bit set carries no data; it
    /// converts with the R2013 fields set, a BODY getting a null history.
    #[test]
    fn empty_acis_converts() {
        let mut d = BitWriter::new();
        d.write_b(true); // empty
        d.write_b(true); // wireframe
        d.write_b(false);
        d.write_bl(0);
        d.write_b(false);
        d.write_bl(0);
        d.write_b(true);
        let bytes = assemble(Version::R2004, type_code::BODY, &d, None, &BitWriter::new());
        let (mut r, _) = ObjReader::split(&bytes, Version::R2004, 30).unwrap();
        let x = AcisEntity::decode(&mut r, AcisKind::Body).unwrap();
        assert_eq!(r.data.remaining_bits(), 0);
        assert_eq!(x.payload, AcisPayload::Empty);
        assert!(x.is_convertible());
        assert!(x.encode(&mut ObjWriter::new(Handle(0x80))).is_err());
        let guid = AcisGuid::derived("{fp}", Handle(0x80));
        let (y, data) = x.to_r2013(guid).unwrap();
        assert_eq!(data, None);
        assert_eq!(y.history, Some(Handle(0)));
        assert!(matches!(y.payload, AcisPayload::DataStorage { guid: g, .. } if g == guid));
        let decode = |r: &mut ObjReader| AcisEntity::decode(r, AcisKind::Body);
        roundtrip(&y, AcisEntity::encode, decode, type_code::BODY);
    }

    /// Derived GUIDs: the same for the same fingerprint and handle,
    /// different otherwise, in AutoCAD's version 4 form.
    #[test]
    fn derived_guids() {
        let fp = "{A1B2C3D4-0000-4000-8000-000000000001}";
        let g = AcisGuid::derived(fp, Handle(0xD65));
        assert_eq!(g, AcisGuid::derived(fp, Handle(0xD65)));
        assert_ne!(g, AcisGuid::derived(fp, Handle(0xD66)));
        assert_ne!(g, AcisGuid::derived("{other}", Handle(0xD65)));
        let mut seen = std::collections::HashSet::new();
        for h in 0..10_000 {
            let g = AcisGuid::derived(fp, Handle(h));
            // As the corpus GUIDs: {1a113328-eb6d-d44d-824d-...}.
            assert_eq!(g.data3 & 0x00F0, 0x0040, "{g}");
            assert_eq!(g.data4[0] & 0xC0, 0x80, "{g}");
            assert!(seen.insert(g), "{g}");
        }
        assert_eq!(g.to_string().len(), 38);
    }

    /// R2007+ REGIONs store no history; 3DSOLIDs and BODYs one. A history
    /// that disagrees with the kind is refused on write, and a reference
    /// left after a REGION is not read (so the entity stays raw).
    #[test]
    fn acis_history_by_kind() {
        let mut region = acis();
        region.kind = AcisKind::Region;
        assert!(region.encode(&mut ObjWriter::new(Handle(0x80))).is_err());
        let mut body = acis();
        body.kind = AcisKind::Body;
        body.history = None;
        assert!(body.encode(&mut ObjWriter::new(Handle(0x80))).is_err());
        // A solid's bytes read as a region leave the history unread.
        let mut w = ObjWriter::new(Handle(0x80));
        acis().encode(&mut w).unwrap();
        let bytes = w.finish(type_code::SOLID3D).unwrap();
        let mut r = reader(&bytes);
        let x = AcisEntity::decode(&mut r, AcisKind::Region).unwrap();
        assert_eq!(x.history, None);
        assert!(r.handles.remaining_bits() >= 8);
    }

    /// The GUID prints in the DXF form.
    #[test]
    fn guid_display() {
        let AcisPayload::DataStorage { guid, .. } = acis().payload else {
            unreachable!()
        };
        assert_eq!(guid.to_string(), "{1a113328-00eb-d44d-824d-78b33668f9e7}");
    }

    /// Bad modeler data fails: no end marker, an unknown version, more data
    /// after the wireframe, counts beyond the data.
    #[test]
    fn acis_errors() {
        let decode = |v: Version, d: &BitWriter| {
            let bytes = assemble(v, type_code::REGION, d, None, &BitWriter::new());
            let (mut r, _) = ObjReader::split(&bytes, v, 30).unwrap();
            AcisEntity::decode(&mut r, AcisKind::Region)
        };
        let mut d = BitWriter::new();
        d.write_b(false);
        d.write_b(false);
        d.write_bs(2);
        d.write_bytes(b"ACIS BinaryFile no end");
        assert!(
            decode(Version::R2004, &d)
                .unwrap_err()
                .to_string()
                .contains("end of ACIS")
        );
        let mut d = BitWriter::new();
        d.write_b(false);
        d.write_b(false);
        d.write_bs(3);
        assert!(
            decode(Version::R2004, &d)
                .unwrap_err()
                .to_string()
                .contains("version 3")
        );
        // SAT blocks: a block longer than the data, a negative size.
        for size in [i32::MAX, -1] {
            let mut d = BitWriter::new();
            d.write_b(false);
            d.write_b(false);
            d.write_bs(1);
            d.write_bl(size);
            d.write_bytes(&[0; 16]);
            assert!(decode(Version::R2000, &d).is_err(), "{size}");
        }
        // The trailing "ACIS empty" bit clear: more data would follow.
        let mut d = BitWriter::new();
        d.write_b(true);
        d.write_b(false);
        d.write_bl(0);
        d.write_b(false);
        d.write_bl(0);
        d.write_b(false);
        let e = decode(Version::R2018, &d).unwrap_err();
        assert!(e.to_string().contains("after the wireframe"), "{e}");
        // Wire, point and silhouette counts beyond the data.
        let wires = |n: i32| {
            let mut d = BitWriter::new();
            d.write_b(true);
            d.write_b(false);
            d.write_bl(0);
            d.write_b(true);
            d.write_bl(n);
            d
        };
        assert!(decode(Version::R2018, &wires(i32::MAX)).is_err());
        assert!(decode(Version::R2018, &wires(-1)).is_err());
        let mut d = wires(1);
        d.write_rc(1);
        d.write_bl(0);
        d.write_bs(0);
        d.write_bl(0);
        d.write_bl(i32::MAX);
        assert!(decode(Version::R2018, &d).is_err());
        let mut d = BitWriter::new();
        d.write_b(true);
        d.write_b(false);
        d.write_bl(0);
        d.write_b(false);
        d.write_bl(i32::MAX);
        assert!(decode(Version::R2018, &d).is_err());
    }

    /// The decrypt helper maps 0x21-0x7E to 0x9F minus the byte and is its
    /// own inverse.
    #[test]
    fn sat_decryption() {
        assert_eq!(decrypt_sat(b"\x6B\x6F\x6F"), b"400");
        assert_eq!(decrypt_sat(b" \r\n\x7F\x21"), b" \r\n\x7F\x7E");
        let all: Vec<u8> = (0..=255).collect();
        assert_eq!(decrypt_sat(&decrypt_sat(&all)), all);
    }

    /// A proxy read from R2000 is written into R2018 with its data and
    /// references copied verbatim; its drawing format word is split into
    /// the version and maintenance version.
    #[test]
    fn proxy_payload_is_copied_verbatim() {
        let mut d = BitWriter::new();
        d.write_bl(499);
        d.write_bl(27 | 105 << 16);
        d.write_b(true);
        // The proxy's own data: an R2000 text field and odd bits.
        d.write_tv_raw(b"custom").unwrap();
        d.write_bs(-3);
        d.write_b(true);
        let mut h = BitWriter::new();
        let ids = [
            HandleRef {
                code: 3,
                value: 0x91,
            },
            HandleRef { code: 4, value: 0 },
        ];
        for id in ids {
            h.write_handle_ref(id).unwrap();
        }
        for v in [Version::R2000, Version::R2004] {
            let bytes = assemble(v, type_code::ACAD_PROXY_ENTITY, &d, None, &h);
            let (mut r, _) = ObjReader::split(&bytes, v, 30).unwrap();
            let x = ProxyEntity::decode(&mut r).unwrap();
            assert_eq!(
                (x.class_id, x.version, x.maintenance_version),
                (499, 27, 105)
            );
            assert!(x.original_data_is_dxf);
            assert_eq!(x.object_ids, ids);
            assert_eq!(x.strings, None);
            // The data after the header, bit for bit.
            let mut payload = BitWriter::new();
            payload.write_tv_raw(b"custom").unwrap();
            payload.write_bs(-3);
            payload.write_b(true);
            assert_eq!(x.data_bits, payload.bit_len());
            assert_eq!(x.data, payload.into_bytes());
            let mut w = ObjWriter::new(Handle(0x80));
            x.encode(&mut w).unwrap();
            let out = w.finish(type_code::ACAD_PROXY_ENTITY).unwrap();
            let back = ProxyEntity::decode(&mut reader(&out)).unwrap();
            assert_eq!(back, x, "{v:?}");
        }
    }

    /// A proxy cut short fails cleanly; one whose last reference runs past
    /// the handle stream keeps its bits.
    #[test]
    fn proxy_truncated() {
        let p = proxy();
        let mut w = ObjWriter::new(Handle(0x80));
        p.encode(&mut w).unwrap();
        let bytes = w.finish(type_code::ACAD_PROXY_ENTITY).unwrap();
        // Header fields cut short.
        for cut in 0..40 {
            let mut r = reader(&bytes);
            r.data.limit_remaining(cut.min(r.data.remaining_bits()));
            let _ = ProxyEntity::decode(&mut r);
        }
        let mut r = reader(&bytes);
        r.data.limit_remaining(3);
        assert!(ProxyEntity::decode(&mut r).is_err());
        // A reference cut in half ends the list; its bits are kept.
        let mut r = reader(&bytes);
        let n = r.handles.remaining_bits();
        r.handles.limit_remaining(n - 12);
        let back = ProxyEntity::decode(&mut r).unwrap();
        assert_eq!(back.object_ids, p.object_ids[..p.object_ids.len() - 1]);
        assert!(back.handle_tail.bits >= 8, "{:?}", back.handle_tail);
        assert_eq!(r.handles.remaining_bits(), 0);
    }

    /// The OLE data length is checked against the data left.
    #[test]
    fn ole_length_is_bounded() {
        for n in [i32::MAX, -1, 5] {
            let mut d = BitWriter::new();
            d.write_bs(0);
            d.write_bs(0);
            d.write_bl(n);
            d.write_bytes(&[1, 2, 3]);
            let bytes = assemble(
                Version::R2018,
                type_code::OLE2FRAME,
                &d,
                None,
                &BitWriter::new(),
            );
            assert!(Ole2Frame::decode(&mut reader(&bytes)).is_err(), "{n}");
        }
    }

    /// A frozen layer or clip vertex count beyond the data fails before
    /// allocating.
    #[test]
    fn viewport_and_image_counts_are_bounded() {
        let mut w = ObjWriter::new(Handle(0x80));
        let mut v = viewport();
        v.frozen_layers = vec![Handle(1); 3];
        v.encode(&mut w).unwrap();
        let bytes = w.finish(type_code::VIEWPORT).unwrap();
        let mut r = reader(&bytes);
        let n = r.handles.remaining_bits();
        // Fewer bytes of handles than the frozen layer count needs.
        r.handles.limit_remaining(n.min(16));
        assert!(Viewport::decode(&mut r).is_err());
        let mut i = image();
        i.clip_boundary = vec![[0.0; 2]; 3];
        let mut w = ObjWriter::new(Handle(0x80));
        i.encode(&mut w).unwrap();
        let bytes = w.finish(500).unwrap();
        let mut r = reader(&bytes);
        r.data.limit_remaining(r.data.remaining_bits() - 129);
        assert!(Image::decode(&mut r).is_err());
        let mut d = BitWriter::new();
        d.write_bl(0);
        for _ in 0..3 {
            d.write_3bd([0.0; 3]);
        }
        d.write_2rd([1.0; 2]);
        d.write_bs(7);
        d.write_b(false);
        d.write_rc(0);
        d.write_rc(0);
        d.write_rc(0);
        d.write_b(false);
        d.write_bs(2);
        d.write_bl(i32::MAX);
        let bytes = assemble(Version::R2018, 500, &d, None, &BitWriter::new());
        let e = Image::decode(&mut reader(&bytes)).unwrap_err();
        assert!(matches!(e, Error::Truncated { .. }), "{e}");
    }
}
