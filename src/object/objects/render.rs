//! Rendering and model documentation styles: MATERIAL, VISUALSTYLE,
//! ACDBSECTIONVIEWSTYLE and ACDBDETAILVIEWSTYLE. ODS does not describe them.
//!
//! - MATERIAL (R2007+) follows ACadSharp (MIT) `readMaterial`: the name and
//!   description, then for the ambient, diffuse and specular channels a
//!   color method (`RC`), factor (`BD`) and, for method 1 (override), an RGB
//!   `BL`; for the diffuse, specular, reflection, opacity, bump and
//!   refraction maps a blend factor, projection, tiling and auto-transform
//!   method, a 4x4 matrix and a source (`RC`), with a file name for source
//!   1 (an image file); and the R2007+ tail (translucence, self
//!   illumination, reflectivity, illumination model, channel flags, mode).
//!   Source 2 (procedural) is not described; such a material stays raw.
//! - VISUALSTYLE (R2013+; ACadSharp reads only its start): found on the
//!   corpus with the DXF exports beside it. The name, type (`BL`, DXF 70),
//!   a `BS` (DXF 177), the internal-use flag (`B`, DXF 291), then 58
//!   properties with no count stored (the DXF's second 70), each a value
//!   whose type is fixed by its position ([`VISUAL_STYLE_KINDS`], the DXF
//!   groups 90, 40, 62/420, 290 and 1 of every corpus style) and a `BS`
//!   (DXF 176). R2007 and R2010 styles have other layouts (R2010's has fewer
//!   properties) and stay raw.
//! - ACDBSECTIONVIEWSTYLE and ACDBDETAILVIEWSTYLE (R2007+): found on the
//!   corpus with the DXF exports beside it. The common model documentation
//!   style part (DXF 70, 3, 290, 300, 90), then the style's fields in their
//!   own order (each field documents its DXF group and the order the DXF
//!   lists it in); the DXF's 71 group markers are not stored; handles are
//!   hard pointers, in DXF order. Before R2018 the description (DXF 300)
//!   and one `BS` 70 are not stored; they are read as the name and 0, the
//!   values AutoCAD 2018 gives the corpus's same styles.
//! - A VISUALSTYLE whose extended data names `AcDbSavedByObjectVersion`
//!   (every pre-R2018 corpus style) is kept raw and copied as read.
//!
//! Every corpus object of these types decodes to its last bit and
//! re-encodes bit for bit (`tests/render_styles.rs`).

use crate::bits::Color;
use crate::error::Result;
use crate::error::encode_error as encode_err;
use crate::handle::Handle;
use crate::object::entities::dim::{check_fits, corrupt, count};
use crate::object::stream::{ObjReader, ObjWriter};
use crate::object::tables::{HARD_POINTER, encode_count, put};
use crate::version::Version;

// ---------------------------------------------------------------------------
// Materials.

/// A material channel's color.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MaterialColor {
    /// 0 use the current color, 1 override (DXF 70, 71, 76).
    pub method: u8,
    /// DXF 40, 41, 45.
    pub factor: f64,
    /// The override color as a `BL` (DXF 90, 91, 92): stored exactly when
    /// `method` is 1.
    pub rgb: Option<i32>,
}

/// A material map (texture) of a channel.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct MaterialMap {
    pub blend_factor: f64,
    /// 1 planar, 2 box, 3 cylinder, 4 sphere.
    pub projection: u8,
    /// 1 tile, 2 crop, 3 clamp.
    pub tiling: u8,
    /// Auto-transform flags.
    pub auto_transform: u8,
    /// The mapper transform, row by row.
    pub matrix: [f64; 16],
    /// 0 the current scene, 1 an image file, 2 procedural.
    pub source: u8,
    /// The image file name: stored exactly when `source` is 1.
    pub file_name: Option<String>,
}

/// A material (MATERIAL), R2007+.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct Material {
    /// DXF 1.
    pub name: String,
    /// DXF 2.
    pub description: String,
    pub ambient: MaterialColor,
    pub diffuse: MaterialColor,
    pub diffuse_map: MaterialMap,
    pub specular: MaterialColor,
    pub specular_map: MaterialMap,
    /// DXF 44.
    pub gloss: f64,
    pub reflection_map: MaterialMap,
    /// DXF 140.
    pub opacity: f64,
    pub opacity_map: MaterialMap,
    pub bump_map: MaterialMap,
    /// DXF 145.
    pub refraction_index: f64,
    pub refraction_map: MaterialMap,
    /// DXF 148, 149, 468.
    pub translucence: f64,
    pub self_illumination: f64,
    pub reflectivity: f64,
    /// DXF 93, 94, 282.
    pub illumination_model: i32,
    pub channel_flags: i32,
    pub mode: i32,
}

/// The identity matrix, as maps store it by default.
pub const IDENTITY_4X4: [f64; 16] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

const COLOR_OVERRIDE: u8 = 1;
const SOURCE_FILE: u8 = 1;
const SOURCE_PROCEDURAL: u8 = 2;

impl MaterialColor {
    fn decode(r: &mut ObjReader) -> Result<Self> {
        let method = r.data.read_rc()?;
        let factor = r.data.read_bd()?;
        let rgb = if method == COLOR_OVERRIDE {
            Some(r.data.read_bl()?)
        } else {
            None
        };
        Ok(MaterialColor {
            method,
            factor,
            rgb,
        })
    }

    fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        if (self.method == COLOR_OVERRIDE) != self.rgb.is_some() {
            return Err(encode_err(
                "a material color has an RGB value exactly when its method is 1 (override)",
            ));
        }
        w.data.write_rc(self.method);
        w.data.write_bd(self.factor);
        if let Some(rgb) = self.rgb {
            w.data.write_bl(rgb);
        }
        Ok(())
    }
}

impl MaterialMap {
    fn decode(r: &mut ObjReader) -> Result<Self> {
        let blend_factor = r.data.read_bd()?;
        let projection = r.data.read_rc()?;
        let tiling = r.data.read_rc()?;
        let auto_transform = r.data.read_rc()?;
        let mut matrix = [0.0; 16];
        for m in &mut matrix {
            *m = r.data.read_bd()?;
        }
        let source = r.data.read_rc()?;
        let file_name = match source {
            SOURCE_FILE => Some(r.text()?),
            SOURCE_PROCEDURAL => {
                return Err(corrupt(
                    "procedural material maps are not described; the material is kept raw",
                ));
            }
            _ => None,
        };
        Ok(MaterialMap {
            blend_factor,
            projection,
            tiling,
            auto_transform,
            matrix,
            source,
            file_name,
        })
    }

    fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        if self.source == SOURCE_PROCEDURAL
            || (self.source == SOURCE_FILE) != self.file_name.is_some()
        {
            return Err(encode_err(
                "a material map has a file name exactly when its source is 1 (image file), and \
                 procedural maps cannot be written",
            ));
        }
        w.data.write_bd(self.blend_factor);
        w.data.write_rc(self.projection);
        w.data.write_rc(self.tiling);
        w.data.write_rc(self.auto_transform);
        for &m in &self.matrix {
            w.data.write_bd(m);
        }
        w.data.write_rc(self.source);
        if let Some(f) = &self.file_name {
            w.text(f)?;
        }
        Ok(())
    }
}

impl Material {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let name = r.text()?;
        let description = r.text()?;
        let ambient = MaterialColor::decode(r)?;
        let diffuse = MaterialColor::decode(r)?;
        let diffuse_map = MaterialMap::decode(r)?;
        let specular = MaterialColor::decode(r)?;
        let specular_map = MaterialMap::decode(r)?;
        let gloss = r.data.read_bd()?;
        let reflection_map = MaterialMap::decode(r)?;
        let opacity = r.data.read_bd()?;
        let opacity_map = MaterialMap::decode(r)?;
        let bump_map = MaterialMap::decode(r)?;
        let refraction_index = r.data.read_bd()?;
        let refraction_map = MaterialMap::decode(r)?;
        Ok(Material {
            name,
            description,
            ambient,
            diffuse,
            diffuse_map,
            specular,
            specular_map,
            gloss,
            reflection_map,
            opacity,
            opacity_map,
            bump_map,
            refraction_index,
            refraction_map,
            translucence: r.data.read_bd()?,
            self_illumination: r.data.read_bd()?,
            reflectivity: r.data.read_bd()?,
            illumination_model: r.data.read_bl()?,
            channel_flags: r.data.read_bl()?,
            mode: r.data.read_bl()?,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        w.text(&self.name)?;
        w.text(&self.description)?;
        self.ambient.encode(w)?;
        self.diffuse.encode(w)?;
        self.diffuse_map.encode(w)?;
        self.specular.encode(w)?;
        self.specular_map.encode(w)?;
        w.data.write_bd(self.gloss);
        self.reflection_map.encode(w)?;
        w.data.write_bd(self.opacity);
        self.opacity_map.encode(w)?;
        self.bump_map.encode(w)?;
        w.data.write_bd(self.refraction_index);
        self.refraction_map.encode(w)?;
        w.data.write_bd(self.translucence);
        w.data.write_bd(self.self_illumination);
        w.data.write_bd(self.reflectivity);
        w.data.write_bl(self.illumination_model);
        w.data.write_bl(self.channel_flags);
        w.data.write_bl(self.mode);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Visual styles.

/// The type of each of a visual style's 58 properties, by position: `L` a
/// `BL` (DXF 90), `D` a `BD` (40), `C` a `CMC` (62 and 420), `B` a `B`
/// (290), `T` text (1). The same in every R2013+ corpus style.
pub const VISUAL_STYLE_KINDS: &str = "LLLLDDCLLCCLLDLCDLLLCLLLBLDLBBBBBBBBBLDDLCLLCBLLLBLCDLTBDD";

/// A visual style property's value.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub enum VisualStyleValue {
    Int(i32),
    Double(f64),
    Color(Color),
    Bool(bool),
    Text(String),
}

impl VisualStyleValue {
    fn kind(&self) -> u8 {
        match self {
            VisualStyleValue::Int(_) => b'L',
            VisualStyleValue::Double(_) => b'D',
            VisualStyleValue::Color(_) => b'C',
            VisualStyleValue::Bool(_) => b'B',
            VisualStyleValue::Text(_) => b'T',
        }
    }
}

/// A visual style (VISUALSTYLE), R2013+.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct VisualStyle {
    /// DXF 2.
    pub name: String,
    /// DXF 70.
    pub style_type: i32,
    /// DXF 177 (the DXF exports show 3 where the files store 2).
    pub unknown_bs: i16,
    /// DXF 291: for AutoCAD's internal use.
    pub internal: bool,
    /// The properties, each with its DXF 176 value, of the types of
    /// [`VISUAL_STYLE_KINDS`].
    pub properties: Vec<(VisualStyleValue, i16)>,
}

impl VisualStyle {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let name = r.text()?;
        let style_type = r.data.read_bl()?;
        let unknown_bs = r.data.read_bs()?;
        let internal = r.data.read_b()?;
        let mut properties = Vec::with_capacity(VISUAL_STYLE_KINDS.len());
        for k in VISUAL_STYLE_KINDS.bytes() {
            let v = match k {
                b'L' => VisualStyleValue::Int(r.data.read_bl()?),
                b'D' => VisualStyleValue::Double(r.data.read_bd()?),
                b'C' => VisualStyleValue::Color(r.read_cmc()?),
                b'B' => VisualStyleValue::Bool(r.data.read_b()?),
                _ => VisualStyleValue::Text(r.text()?),
            };
            properties.push((v, r.data.read_bs()?));
        }
        Ok(VisualStyle {
            name,
            style_type,
            unknown_bs,
            internal,
            properties,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        let kinds: Vec<u8> = self.properties.iter().map(|(v, _)| v.kind()).collect();
        if kinds != VISUAL_STYLE_KINDS.as_bytes() {
            return Err(encode_err(
                "a visual style's properties must have the types of VISUAL_STYLE_KINDS",
            ));
        }
        w.text(&self.name)?;
        w.data.write_bl(self.style_type);
        w.data.write_bs(self.unknown_bs);
        w.data.write_b(self.internal);
        for (v, flags) in &self.properties {
            match v {
                VisualStyleValue::Int(x) => w.data.write_bl(*x),
                VisualStyleValue::Double(x) => w.data.write_bd(*x),
                VisualStyleValue::Color(c) => w.write_cmc(c)?,
                VisualStyleValue::Bool(b) => w.data.write_b(*b),
                VisualStyleValue::Text(t) => w.text(t)?,
            }
            w.data.write_bs(*flags);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Section and detail view styles.

/// A `BS` R2018 stores and earlier versions do not (0 then, as AutoCAD 2018
/// writes the corpus's styles). Before R2018 the files have one `BS` fewer
/// before the flags; which of the two DXF 70s it is cannot be told (both
/// are 0 in every corpus file): it is taken to be the style's own.
fn read_r2018_bs(r: &mut ObjReader) -> Result<i16> {
    if r.version >= Version::R2018 {
        r.data.read_bs()
    } else {
        Ok(0)
    }
}

/// The model documentation view style part common to section and detail
/// view styles (DXF subclass AcDbModelDocViewStyle).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ModelDocViewStyle {
    /// DXF 70.
    pub unknown_70: i16,
    /// DXF 3.
    pub name: String,
    /// DXF 290.
    pub unknown_290: bool,
    /// DXF 300.
    pub description: String,
    /// DXF 90.
    pub unknown_90: i32,
}

impl ModelDocViewStyle {
    fn decode(r: &mut ObjReader) -> Result<Self> {
        let unknown_70 = r.data.read_bs()?;
        let name = r.text()?;
        let unknown_290 = r.data.read_b()?;
        // Not stored before R2018: AutoCAD 2018 gives the corpus's styles
        // their name.
        let description = if r.version >= Version::R2018 {
            r.text()?
        } else {
            name.clone()
        };
        Ok(ModelDocViewStyle {
            unknown_70,
            name,
            unknown_290,
            description,
            unknown_90: r.data.read_bl()?,
        })
    }

    fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        w.data.write_bs(self.unknown_70);
        w.text(&self.name)?;
        w.data.write_b(self.unknown_290);
        w.text(&self.description)?;
        w.data.write_bl(self.unknown_90);
        Ok(())
    }
}

/// A section view style (ACDBSECTIONVIEWSTYLE), R2007+. The DXF lists its
/// fields in four groups (71 = 1 identifier and arrows, 2 cutting plane, 3
/// view label, 4 hatch); the file stores them in the order of the fields
/// here.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct SectionViewStyle {
    pub base: ModelDocViewStyle,
    /// DXF 70.
    pub unknown_70: i16,
    /// DXF 90: flags.
    pub flags: i32,
    /// Group 1: the identifier's text style (340), color (62), height (40).
    pub identifier_style: Handle,
    pub identifier_color: Color,
    pub identifier_height: f64,
    /// Group 1: the arrows' start and end blocks (340, 340), color (62) and
    /// size (40).
    pub arrow_start: Handle,
    pub arrow_end: Handle,
    pub arrow_color: Color,
    pub arrow_size: f64,
    /// Group 1: the characters identifiers exclude (300), and the 40 after
    /// them.
    pub identifier_exclude: String,
    pub identifier_offset: f64,
    /// Group 2: the plane line's linetype (340), lineweight (90), color (62).
    pub plane_linetype: Handle,
    pub plane_lineweight: i32,
    pub plane_color: Color,
    /// Group 2: the bend line's linetype (340), lineweight (90), color (62).
    pub bend_linetype: Handle,
    pub bend_lineweight: i32,
    pub bend_color: Color,
    /// Group 2: the first and last of its three 40s.
    pub bend_length: f64,
    pub end_length: f64,
    /// Group 3: the label's text style (340), color (62), height (40), the
    /// 90, 40, 90 that follow, and its text (300).
    pub label_style: Handle,
    pub label_color: Color,
    pub label_height: f64,
    pub label_attachment: i32,
    pub label_offset: f64,
    pub label_alignment: i32,
    pub label_text: String,
    /// Group 4: the hatch color (62), background color (62), pattern (300),
    /// scale (40), transparency (90) and two flags (290, 290).
    pub hatch_color: Color,
    pub hatch_background: Color,
    pub hatch_pattern: String,
    pub hatch_scale: f64,
    pub hatch_transparency: i32,
    pub hatch_flags: [bool; 2],
    /// Group 1: the 90, 40 and 90 after the identifier offset, stored here.
    pub identifier_position: i32,
    pub identifier_extension: f64,
    pub identifier_unknown_90: i32,
    /// Group 2: the middle of its three 40s, stored here.
    pub end_overshoot: f64,
    /// Group 4: the hatch angles (90 count, then 40s).
    pub hatch_angles: Vec<f64>,
}

impl SectionViewStyle {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let base = ModelDocViewStyle::decode(r)?;
        let unknown_70 = read_r2018_bs(r)?;
        let flags = r.data.read_bl()?;
        let identifier_color = r.read_cmc()?;
        let identifier_height = r.data.read_bd()?;
        let arrow_color = r.read_cmc()?;
        let arrow_size = r.data.read_bd()?;
        let identifier_offset = r.data.read_bd()?;
        let plane_lineweight = r.data.read_bl()?;
        let plane_color = r.read_cmc()?;
        let bend_lineweight = r.data.read_bl()?;
        let bend_color = r.read_cmc()?;
        let bend_length = r.data.read_bd()?;
        let end_length = r.data.read_bd()?;
        let label_color = r.read_cmc()?;
        let label_height = r.data.read_bd()?;
        let label_attachment = r.data.read_bl()?;
        let label_offset = r.data.read_bd()?;
        let label_alignment = r.data.read_bl()?;
        let hatch_color = r.read_cmc()?;
        let hatch_background = r.read_cmc()?;
        let hatch_scale = r.data.read_bd()?;
        let hatch_transparency = r.data.read_bl()?;
        let hatch_flags = [r.data.read_b()?, r.data.read_b()?];
        let identifier_position = r.data.read_bl()?;
        let identifier_extension = r.data.read_bd()?;
        let identifier_unknown_90 = r.data.read_bl()?;
        let end_overshoot = r.data.read_bd()?;
        let n = count(r.data.read_bl()?, "hatch angle")?;
        // A BD takes at least 2 bits.
        check_fits(r, n, 2, "hatch angles")?;
        let hatch_angles = (0..n)
            .map(|_| r.data.read_bd())
            .collect::<Result<Vec<_>>>()?;
        let identifier_exclude = r.text()?;
        let label_text = r.text()?;
        let hatch_pattern = r.text()?;
        Ok(SectionViewStyle {
            base,
            unknown_70,
            flags,
            identifier_style: r.href_abs()?,
            identifier_color,
            identifier_height,
            arrow_start: r.href_abs()?,
            arrow_end: r.href_abs()?,
            arrow_color,
            arrow_size,
            identifier_exclude,
            identifier_offset,
            plane_linetype: r.href_abs()?,
            plane_lineweight,
            plane_color,
            bend_linetype: r.href_abs()?,
            bend_lineweight,
            bend_color,
            bend_length,
            end_length,
            label_style: r.href_abs()?,
            label_color,
            label_height,
            label_attachment,
            label_offset,
            label_alignment,
            label_text,
            hatch_color,
            hatch_background,
            hatch_pattern,
            hatch_scale,
            hatch_transparency,
            hatch_flags,
            identifier_position,
            identifier_extension,
            identifier_unknown_90,
            end_overshoot,
            hatch_angles,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        self.base.encode(w)?;
        w.data.write_bs(self.unknown_70);
        w.data.write_bl(self.flags);
        w.write_cmc(&self.identifier_color)?;
        w.data.write_bd(self.identifier_height);
        w.write_cmc(&self.arrow_color)?;
        w.data.write_bd(self.arrow_size);
        w.data.write_bd(self.identifier_offset);
        w.data.write_bl(self.plane_lineweight);
        w.write_cmc(&self.plane_color)?;
        w.data.write_bl(self.bend_lineweight);
        w.write_cmc(&self.bend_color)?;
        w.data.write_bd(self.bend_length);
        w.data.write_bd(self.end_length);
        w.write_cmc(&self.label_color)?;
        w.data.write_bd(self.label_height);
        w.data.write_bl(self.label_attachment);
        w.data.write_bd(self.label_offset);
        w.data.write_bl(self.label_alignment);
        w.write_cmc(&self.hatch_color)?;
        w.write_cmc(&self.hatch_background)?;
        w.data.write_bd(self.hatch_scale);
        w.data.write_bl(self.hatch_transparency);
        w.data.write_b(self.hatch_flags[0]);
        w.data.write_b(self.hatch_flags[1]);
        w.data.write_bl(self.identifier_position);
        w.data.write_bd(self.identifier_extension);
        w.data.write_bl(self.identifier_unknown_90);
        w.data.write_bd(self.end_overshoot);
        w.data
            .write_bl(encode_count(self.hatch_angles.len(), "hatch angles")?);
        for &a in &self.hatch_angles {
            w.data.write_bd(a);
        }
        w.text(&self.identifier_exclude)?;
        w.text(&self.label_text)?;
        w.text(&self.hatch_pattern)?;
        for h in [
            self.identifier_style,
            self.arrow_start,
            self.arrow_end,
            self.plane_linetype,
            self.bend_linetype,
            self.label_style,
        ] {
            put(w, HARD_POINTER, h)?;
        }
        Ok(())
    }
}

/// A detail view style (ACDBDETAILVIEWSTYLE), R2007+. The DXF lists its
/// fields in four groups (71 = 1 identifier and arrows, 2 boundary, 3 view
/// label, 4 connection line and border); the file stores them in the order
/// of the fields here.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct DetailViewStyle {
    pub base: ModelDocViewStyle,
    /// DXF 70.
    pub unknown_70: i16,
    /// DXF 90: flags.
    pub flags: i32,
    /// Group 1: the identifier's text style (340), color (62), height (40),
    /// the 40 and 280 after its excluded characters, and those characters
    /// (300).
    pub identifier_style: Handle,
    pub identifier_color: Color,
    pub identifier_height: f64,
    pub identifier_offset: f64,
    pub identifier_placement: u8,
    pub identifier_exclude: String,
    /// Group 1: the arrow block (340), color (62), size (40).
    pub arrow: Handle,
    pub arrow_color: Color,
    pub arrow_size: f64,
    /// Group 2: the boundary linetype (340), lineweight (90), color (62).
    pub boundary_linetype: Handle,
    pub boundary_lineweight: i32,
    pub boundary_color: Color,
    /// Group 3: the label's text style (340), color (62), height (40), the
    /// 90, 40, 90 that follow, and its text (300).
    pub label_style: Handle,
    pub label_color: Color,
    pub label_height: f64,
    pub label_attachment: i32,
    pub label_offset: f64,
    pub label_alignment: i32,
    pub label_text: String,
    /// Group 4: the connection line's linetype (340), lineweight (90),
    /// color (62); the border's likewise; and the closing 280.
    pub connection_linetype: Handle,
    pub connection_lineweight: i32,
    pub connection_color: Color,
    pub border_linetype: Handle,
    pub border_lineweight: i32,
    pub border_color: Color,
    pub model_edge: u8,
}

impl DetailViewStyle {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let base = ModelDocViewStyle::decode(r)?;
        let unknown_70 = read_r2018_bs(r)?;
        let flags = r.data.read_bl()?;
        let identifier_color = r.read_cmc()?;
        let identifier_height = r.data.read_bd()?;
        let identifier_offset = r.data.read_bd()?;
        let identifier_placement = r.data.read_rc()?;
        let arrow_color = r.read_cmc()?;
        let arrow_size = r.data.read_bd()?;
        let boundary_lineweight = r.data.read_bl()?;
        let boundary_color = r.read_cmc()?;
        let label_color = r.read_cmc()?;
        let label_height = r.data.read_bd()?;
        let label_attachment = r.data.read_bl()?;
        let label_offset = r.data.read_bd()?;
        let label_alignment = r.data.read_bl()?;
        let connection_lineweight = r.data.read_bl()?;
        let connection_color = r.read_cmc()?;
        let border_lineweight = r.data.read_bl()?;
        let border_color = r.read_cmc()?;
        let model_edge = r.data.read_rc()?;
        let identifier_exclude = r.text()?;
        let label_text = r.text()?;
        Ok(DetailViewStyle {
            base,
            unknown_70,
            flags,
            identifier_style: r.href_abs()?,
            identifier_color,
            identifier_height,
            identifier_offset,
            identifier_placement,
            identifier_exclude,
            arrow: r.href_abs()?,
            arrow_color,
            arrow_size,
            boundary_linetype: r.href_abs()?,
            boundary_lineweight,
            boundary_color,
            label_style: r.href_abs()?,
            label_color,
            label_height,
            label_attachment,
            label_offset,
            label_alignment,
            label_text,
            connection_linetype: r.href_abs()?,
            connection_lineweight,
            connection_color,
            border_linetype: r.href_abs()?,
            border_lineweight,
            border_color,
            model_edge,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        self.base.encode(w)?;
        w.data.write_bs(self.unknown_70);
        w.data.write_bl(self.flags);
        w.write_cmc(&self.identifier_color)?;
        w.data.write_bd(self.identifier_height);
        w.data.write_bd(self.identifier_offset);
        w.data.write_rc(self.identifier_placement);
        w.write_cmc(&self.arrow_color)?;
        w.data.write_bd(self.arrow_size);
        w.data.write_bl(self.boundary_lineweight);
        w.write_cmc(&self.boundary_color)?;
        w.write_cmc(&self.label_color)?;
        w.data.write_bd(self.label_height);
        w.data.write_bl(self.label_attachment);
        w.data.write_bd(self.label_offset);
        w.data.write_bl(self.label_alignment);
        w.data.write_bl(self.connection_lineweight);
        w.write_cmc(&self.connection_color)?;
        w.data.write_bl(self.border_lineweight);
        w.write_cmc(&self.border_color)?;
        w.data.write_rc(self.model_edge);
        w.text(&self.identifier_exclude)?;
        w.text(&self.label_text)?;
        for h in [
            self.identifier_style,
            self.arrow,
            self.boundary_linetype,
            self.label_style,
            self.connection_linetype,
            self.border_linetype,
        ] {
            put(w, HARD_POINTER, h)?;
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::object::entities::tests::roundtrip;
    use crate::object::objects::test_util::book;

    pub(crate) fn material() -> Material {
        let map = |source, file_name: Option<&str>| MaterialMap {
            blend_factor: 0.5,
            projection: 2,
            tiling: 1,
            auto_transform: 1,
            matrix: IDENTITY_4X4,
            source,
            file_name: file_name.map(Into::into),
        };
        let color = |method, rgb| MaterialColor {
            method,
            factor: 0.75,
            rgb,
        };
        Material {
            name: "Brick ü".into(),
            description: "red".into(),
            ambient: color(0, None),
            diffuse: color(1, Some(0x00C0_2010)),
            diffuse_map: map(1, Some("brick.png")),
            specular: color(0, None),
            specular_map: map(0, None),
            gloss: 0.5,
            reflection_map: map(1, Some("")),
            opacity: 1.0,
            opacity_map: map(0, None),
            bump_map: map(1, Some("bump.png")),
            refraction_index: 1.25,
            refraction_map: map(0, None),
            translucence: 0.1,
            self_illumination: 0.2,
            reflectivity: 0.3,
            illumination_model: 1,
            channel_flags: 63,
            mode: 0,
        }
    }

    pub(crate) fn visual_style() -> VisualStyle {
        let properties = VISUAL_STYLE_KINDS
            .bytes()
            .enumerate()
            .map(|(i, k)| {
                let v = match k {
                    b'L' => VisualStyleValue::Int(i as i32),
                    b'D' => VisualStyleValue::Double(i as f64 / 4.0),
                    b'C' if i % 2 == 0 => VisualStyleValue::Color(book()),
                    b'C' => VisualStyleValue::Color(Color::Index(7)),
                    b'B' => VisualStyleValue::Bool(i % 2 == 0),
                    _ => VisualStyleValue::Text("strokes_ogs.tif".into()),
                };
                (v, (i % 2) as i16)
            })
            .collect();
        VisualStyle {
            name: "Shaded".into(),
            style_type: 27,
            unknown_bs: 2,
            internal: false,
            properties,
        }
    }

    fn base() -> ModelDocViewStyle {
        ModelDocViewStyle {
            unknown_70: 0,
            name: "Imperial24".into(),
            unknown_290: true,
            description: "Imperial".into(),
            unknown_90: 7,
        }
    }

    pub(crate) fn section_view_style() -> SectionViewStyle {
        SectionViewStyle {
            base: base(),
            unknown_70: 1,
            flags: 78,
            identifier_style: Handle(0x11),
            identifier_color: book(),
            identifier_height: 0.24,
            arrow_start: Handle(0x40),
            arrow_end: Handle(0),
            arrow_color: Color::ByLayer,
            arrow_size: 0.25,
            identifier_exclude: "I, O".into(),
            identifier_offset: 0.48,
            plane_linetype: Handle(0x16),
            plane_lineweight: 25,
            plane_color: Color::Index(1),
            bend_linetype: Handle(0x16),
            bend_lineweight: 50,
            bend_color: Color::ByBlock,
            bend_length: 0.3,
            end_length: 0.4,
            label_style: Handle(0x11),
            label_color: Color::ByLayer,
            label_height: 0.24,
            label_attachment: 1,
            label_offset: 0.75,
            label_alignment: 1,
            label_text: "%<\\AcVar ViewScale>%".into(),
            hatch_color: Color::ByLayer,
            hatch_background: Color::None,
            hatch_pattern: "ANSI31".into(),
            hatch_scale: 1.0,
            hatch_transparency: 0,
            hatch_flags: [true, false],
            identifier_position: 3,
            identifier_extension: 0.18,
            identifier_unknown_90: 1,
            end_overshoot: 0.1,
            hatch_angles: vec![0.0, 1.5],
        }
    }

    pub(crate) fn detail_view_style() -> DetailViewStyle {
        DetailViewStyle {
            base: base(),
            unknown_70: 1,
            flags: 3,
            identifier_style: Handle(0x11),
            identifier_color: Color::ByLayer,
            identifier_height: 0.24,
            identifier_offset: 0.36,
            identifier_placement: 3,
            identifier_exclude: String::new(),
            arrow: Handle(0x41),
            arrow_color: book(),
            arrow_size: 0.24,
            boundary_linetype: Handle(0x16),
            boundary_lineweight: 25,
            boundary_color: Color::ByLayer,
            label_style: Handle(0x11),
            label_color: Color::Index(2),
            label_height: 0.24,
            label_attachment: 1,
            label_offset: 0.75,
            label_alignment: 1,
            label_text: "%<\\AcVar ViewDetailId>%".into(),
            connection_linetype: Handle(0x16),
            connection_lineweight: 25,
            connection_color: Color::ByLayer,
            border_linetype: Handle(0x16),
            border_lineweight: 30,
            border_color: Color::ByLayer,
            model_edge: 1,
        }
    }

    /// Each type encodes and decodes to an equal value, and fails cleanly
    /// on every truncation of every stream.
    #[test]
    fn render_styles_roundtrip() {
        roundtrip(&material(), Material::encode, Material::decode, 505);
        roundtrip(
            &visual_style(),
            VisualStyle::encode,
            VisualStyle::decode,
            506,
        );
        roundtrip(
            &section_view_style(),
            SectionViewStyle::encode,
            SectionViewStyle::decode,
            515,
        );
        roundtrip(
            &detail_view_style(),
            DetailViewStyle::encode,
            DetailViewStyle::decode,
            516,
        );
    }

    fn encode_error<T>(v: &T, enc: fn(&T, &mut ObjWriter) -> Result<()>) -> String {
        let mut w = ObjWriter::new(Handle(0x80));
        match enc(v, &mut w) {
            Err(crate::error::Error::Encode(m)) => m,
            other => panic!("{other:?}"),
        }
    }

    /// Values the layouts cannot hold are refused on write.
    #[test]
    fn inconsistent_values_are_refused() {
        // A color with an RGB value but no override method, and one with
        // the method but no value.
        let mut m = material();
        m.ambient.rgb = Some(1);
        assert!(encode_error(&m, Material::encode).contains("RGB"));
        let mut m = material();
        m.diffuse.rgb = None;
        assert!(encode_error(&m, Material::encode).contains("RGB"));
        // A procedural map, a file name without an image source, an image
        // source without a file name.
        for change in [
            |m: &mut MaterialMap| m.source = 2,
            |m: &mut MaterialMap| m.source = 0,
            |m: &mut MaterialMap| m.file_name = None,
        ] {
            let mut m = material();
            change(&mut m.diffuse_map);
            assert!(encode_error(&m, Material::encode).contains("file name"));
        }
        // Visual style properties of the wrong types, or too few.
        let mut v = visual_style();
        v.properties[0].0 = VisualStyleValue::Bool(true);
        assert!(encode_error(&v, VisualStyle::encode).contains("VISUAL_STYLE_KINDS"));
        let mut v = visual_style();
        v.properties.pop();
        assert!(encode_error(&v, VisualStyle::encode).contains("VISUAL_STYLE_KINDS"));
    }

    /// Before R2018 a view style stores no description and one `BS` fewer;
    /// they read as the name and 0.
    #[test]
    fn older_view_styles_read_their_layout() {
        use crate::version::Version;
        let mut w = ObjWriter::new(Handle(0x80));
        // Base: 70, name, 290, (no description), 90; then (no 70) flags.
        w.data.write_bs(0);
        w.text("Imperial24").unwrap();
        w.data.write_b(false);
        w.data.write_bl(0);
        w.data.write_bl(3);
        let mut d = detail_view_style();
        d.base = base();
        d.base.description = d.base.name.clone();
        d.base.unknown_290 = false;
        d.base.unknown_90 = 0;
        d.unknown_70 = 0;
        // No color names in the string stream.
        d.arrow_color = Color::Index(5);
        // The rest as R2018 writes it.
        let mut rest = ObjWriter::new(Handle(0x80));
        d.encode(&mut rest).unwrap();
        let full = rest.finish(516).unwrap();
        let (mut r18, _) = ObjReader::split(&full, Version::R2018, 30).unwrap();
        // Skip the R2018 head (BS, name, B, description, BL, BS, BL) and
        // copy the remaining data bits after the old head.
        r18.data.read_bs().unwrap();
        r18.text().unwrap();
        r18.data.read_b().unwrap();
        r18.text().unwrap();
        r18.data.read_bl().unwrap();
        r18.data.read_bs().unwrap();
        r18.data.read_bl().unwrap();
        while r18.data.remaining_bits() > 0 {
            w.data.write_b(r18.data.read_b().unwrap());
        }
        w.text(&d.identifier_exclude).unwrap();
        w.text(&d.label_text).unwrap();
        for h in [
            d.identifier_style,
            d.arrow,
            d.boundary_linetype,
            d.label_style,
            d.connection_linetype,
            d.border_linetype,
        ] {
            put(&mut w, HARD_POINTER, h).unwrap();
        }
        let old = w.finish(516).unwrap();
        let (mut r, _) = ObjReader::split(&old, Version::R2013, 30).unwrap();
        r.handle = Handle(0x80);
        assert_eq!(DetailViewStyle::decode(&mut r).unwrap(), d);
        assert_eq!(r.data.remaining_bits(), 0);
    }
}
