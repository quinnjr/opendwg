//! Text entities (ODS 20.4.3 TEXT, 20.4.4 ATTRIB, 20.4.5 ATTDEF, 20.4.46
//! MTEXT).
//!
//! TEXT and the attributes use the R2000+ "data flags" form: a byte whose
//! bits say which fields are left out because they hold their default
//! (elevation 0, oblique 0, rotation 0, width factor 1, generation and
//! alignments 0) or, for the alignment point, are not stored at all. The
//! encoder leaves out exactly the default-valued fields, as AutoCAD does,
//! and stores the alignment point when [`Text::alignment`] is `Some`.
//!
//! From R2018 an ATTRIB or ATTDEF may be multi-line: it then embeds an
//! MTEXT, written from the entity mode onward (ODS 20.4.4), between its
//! text data and its tag.
//!
//! Deviations from ODS, found on the corpus and pinned by tests in
//! `tests/entities.rs`, are listed on the types they concern.

use super::super::common::{
    AsRead, EntityCommon, XDataApp, XDataItem, decode_entity_mode, encode_entity_mode,
};
use super::super::stream::{ObjReader, ObjWriter};
use super::super::tables::{HARD_POINTER, put};
use crate::bits::Color;
use crate::error::encode_error as encode_err;
use crate::error::{Error, Result};
use crate::handle::Handle;
use crate::version::Version;

fn corrupt(msg: &str) -> Error {
    Error::Corrupt(msg.to_owned())
}

fn same(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits()
}

/// Data flag bits (ODS 20.4.3): set when the field is not stored.
const NO_ELEVATION: u8 = 0x01;
const NO_ALIGNMENT: u8 = 0x02;
const NO_OBLIQUE: u8 = 0x04;
const NO_ROTATION: u8 = 0x08;
const NO_WIDTH: u8 = 0x10;
const NO_GENERATION: u8 = 0x20;
const NO_HORIZONTAL: u8 = 0x40;
const NO_VERTICAL: u8 = 0x80;

/// A single-line text (ODS 20.4.3); also the start of an ATTRIB and ATTDEF.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Text {
    /// The insertion point (DXF 10); Z is the elevation.
    pub insertion: [f64; 3],
    /// The alignment point's X and Y (DXF 11; Z is the elevation), when
    /// stored. AutoCAD stores it for text that is not left-aligned on the
    /// baseline.
    pub alignment: Option<[f64; 2]>,
    /// Normal (extrusion direction).
    pub extrusion: [f64; 3],
    /// Thickness.
    pub thickness: f64,
    /// Oblique angle, radians.
    pub oblique_angle: f64,
    /// Rotation, radians.
    pub rotation: f64,
    /// Height.
    pub height: f64,
    /// Width factor (1: as the style).
    pub width_factor: f64,
    /// The text.
    pub value: String,
    /// DXF 71: 2 mirrored in X, 4 mirrored in Y.
    pub generation: i16,
    /// DXF 72.
    pub horizontal_alignment: i16,
    /// DXF 73.
    pub vertical_alignment: i16,
    /// The text style (hard pointer).
    pub style: Handle,
}

impl Text {
    /// Single-line text `value` at `insertion` (left-aligned on the
    /// baseline), `height` high, unrotated, width factor 1, in the XY plane.
    /// Its style is left null: [`crate::Drawing::add_entity`] gives such
    /// text the current text style ($TEXTSTYLE).
    pub fn new(insertion: [f64; 3], height: f64, value: &str) -> Text {
        Text {
            insertion,
            alignment: None,
            extrusion: [0.0, 0.0, 1.0],
            thickness: 0.0,
            oblique_angle: 0.0,
            rotation: 0.0,
            height,
            width_factor: 1.0,
            value: value.to_owned(),
            generation: 0,
            horizontal_alignment: 0,
            vertical_alignment: 0,
            style: Handle(0),
        }
    }

    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let flags = r.data.read_rc()?;
        let d = &mut r.data;
        let absent = |bit: u8| flags & bit != 0;
        let elevation = if absent(NO_ELEVATION) {
            0.0
        } else {
            d.read_rd()?
        };
        let [x, y] = d.read_2rd()?;
        let alignment = if absent(NO_ALIGNMENT) {
            None
        } else {
            Some(d.read_2dd([x, y])?)
        };
        let extrusion = d.read_be()?;
        let thickness = d.read_bt()?;
        let oblique_angle = if absent(NO_OBLIQUE) {
            0.0
        } else {
            d.read_rd()?
        };
        let rotation = if absent(NO_ROTATION) {
            0.0
        } else {
            d.read_rd()?
        };
        let height = d.read_rd()?;
        let width_factor = if absent(NO_WIDTH) { 1.0 } else { d.read_rd()? };
        let value = r.text()?;
        let d = &mut r.data;
        let generation = if absent(NO_GENERATION) {
            0
        } else {
            d.read_bs()?
        };
        let horizontal_alignment = if absent(NO_HORIZONTAL) {
            0
        } else {
            d.read_bs()?
        };
        let vertical_alignment = if absent(NO_VERTICAL) { 0 } else { d.read_bs()? };
        Ok(Text {
            insertion: [x, y, elevation],
            alignment,
            extrusion,
            thickness,
            oblique_angle,
            rotation,
            height,
            width_factor,
            value,
            generation,
            horizontal_alignment,
            vertical_alignment,
            style: r.href_abs()?,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        let [x, y, elevation] = self.insertion;
        let flag = |absent: bool, bit: u8| if absent { bit } else { 0 };
        let flags = flag(same(elevation, 0.0), NO_ELEVATION)
            | flag(self.alignment.is_none(), NO_ALIGNMENT)
            | flag(same(self.oblique_angle, 0.0), NO_OBLIQUE)
            | flag(same(self.rotation, 0.0), NO_ROTATION)
            | flag(same(self.width_factor, 1.0), NO_WIDTH)
            | flag(self.generation == 0, NO_GENERATION)
            | flag(self.horizontal_alignment == 0, NO_HORIZONTAL)
            | flag(self.vertical_alignment == 0, NO_VERTICAL);
        let d = &mut w.data;
        d.write_rc(flags);
        if flags & NO_ELEVATION == 0 {
            d.write_rd(elevation);
        }
        d.write_2rd([x, y]);
        if let Some(a) = self.alignment {
            d.write_2dd(a, [x, y]);
        }
        d.write_be(self.extrusion);
        d.write_bt(self.thickness);
        for (bit, v) in [
            (NO_OBLIQUE, self.oblique_angle),
            (NO_ROTATION, self.rotation),
        ] {
            if flags & bit == 0 {
                d.write_rd(v);
            }
        }
        d.write_rd(self.height);
        if flags & NO_WIDTH == 0 {
            d.write_rd(self.width_factor);
        }
        w.text(&self.value)?;
        for (bit, v) in [
            (NO_GENERATION, self.generation),
            (NO_HORIZONTAL, self.horizontal_alignment),
            (NO_VERTICAL, self.vertical_alignment),
        ] {
            if flags & bit == 0 {
                w.data.write_bs(v);
            }
        }
        put(w, HARD_POINTER, self.style)
    }
}

/// R2018 attribute types (DXF 71 of the attribute).
pub const ATTRIB_SINGLE_LINE: u8 = 1;
/// A multi-line ATTRIB.
pub const ATTRIB_MULTI_LINE: u8 = 2;
/// A multi-line ATTDEF.
pub const ATTDEF_MULTI_LINE: u8 = 4;

fn is_multi_line(attribute_type: u8) -> bool {
    matches!(attribute_type, ATTRIB_MULTI_LINE | ATTDEF_MULTI_LINE)
}

/// Annotative data of an embedded MTEXT (ODS 20.4.4).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct AnnotativeData {
    pub bytes: Vec<u8>,
    /// The registered application (hard pointer).
    pub app: Handle,
    /// Undocumented `BS` (DXF 72?), 0 in AutoCAD files.
    pub unknown: i16,
}

/// The MTEXT a multi-line attribute embeds.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct EmbeddedMText {
    /// The common data from the entity mode on; the handle, extended data
    /// and proxy graphics are not stored and must stay empty.
    pub common: EntityCommon,
    pub mtext: MText,
    pub annotative_data: Option<AnnotativeData>,
}

impl EmbeddedMText {
    fn decode(r: &mut ObjReader) -> Result<Self> {
        let common = decode_entity_mode(r)?;
        let mtext = MText::decode(r)?;
        let size = r.data.read_bs()?;
        let n = usize::try_from(size)
            .map_err(|_| corrupt("negative annotative data size in an embedded MTEXT"))?;
        let annotative_data = if n > 0 {
            if n as u64 > r.data.remaining_bits() / 8 {
                return Err(Error::Truncated {
                    context: "annotative data",
                });
            }
            let bytes = r.data.read_bytes(n)?;
            Some(AnnotativeData {
                bytes,
                app: r.href_abs()?,
                unknown: r.data.read_bs()?,
            })
        } else {
            None
        };
        Ok(EmbeddedMText {
            common,
            mtext,
            annotative_data,
        })
    }

    fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        let c = &self.common;
        if c.obj.handle.0 != 0 || !c.obj.xdata.is_empty() || c.proxy_graphics.is_some() {
            return Err(encode_err(
                "an embedded MTEXT stores no handle, extended data or proxy graphics",
            ));
        }
        encode_entity_mode(w, c)?;
        self.mtext.encode(w)?;
        match &self.annotative_data {
            Some(a) => {
                let size = i16::try_from(a.bytes.len())
                    .ok()
                    .filter(|&n| n > 0)
                    .ok_or_else(|| encode_err("annotative data must be 1 to 32767 bytes"))?;
                w.data.write_bs(size);
                w.data.write_bytes(&a.bytes);
                put(w, HARD_POINTER, a.app)?;
                w.data.write_bs(a.unknown);
            }
            None => w.data.write_bs(0),
        }
        Ok(())
    }
}

/// An attribute (ODS 20.4.4, ATTRIB) of an insert.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct Attrib {
    pub text: Text,
    /// R2010+ version byte; 0 before.
    pub version: u8,
    /// R2018+: [`ATTRIB_SINGLE_LINE`], [`ATTRIB_MULTI_LINE`] or
    /// [`ATTDEF_MULTI_LINE`]; single line before.
    pub attribute_type: u8,
    /// The embedded MTEXT; `Some` exactly for the multi-line types.
    pub mtext: Option<Box<EmbeddedMText>>,
    pub tag: String,
    /// DXF 73, unused.
    pub field_length: i16,
    /// DXF 70: 1 invisible, 2 constant, 4 verify, 8 preset.
    pub flags: u8,
    /// R2007+ (DXF 280); `false` before.
    pub lock_position: bool,
}

impl Attrib {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let text = Text::decode(r)?;
        let version = if r.version >= Version::R2010 {
            r.data.read_rc()?
        } else {
            0
        };
        let attribute_type = if r.version >= Version::R2018 {
            r.data.read_rc()?
        } else {
            ATTRIB_SINGLE_LINE
        };
        let mtext = if is_multi_line(attribute_type) {
            Some(Box::new(EmbeddedMText::decode(r)?))
        } else {
            None
        };
        let tag = r.text()?;
        let field_length = r.data.read_bs()?;
        let flags = r.data.read_rc()?;
        let lock_position = r.version >= Version::R2007 && r.data.read_b()?;
        Ok(Attrib {
            text,
            version,
            attribute_type,
            mtext,
            tag,
            field_length,
            flags,
            lock_position,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        if is_multi_line(self.attribute_type) != self.mtext.is_some() {
            return Err(encode_err(
                "an attribute embeds an MTEXT exactly when it is multi-line",
            ));
        }
        self.text.encode(w)?;
        w.data.write_rc(self.version);
        w.data.write_rc(self.attribute_type);
        if let Some(m) = &self.mtext {
            m.encode(w)?;
        }
        w.text(&self.tag)?;
        w.data.write_bs(self.field_length);
        w.data.write_rc(self.flags);
        w.data.write_b(self.lock_position);
        Ok(())
    }
}

/// An attribute definition (ODS 20.4.5, ATTDEF) in a block.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct AttDef {
    pub attrib: Attrib,
    /// R2010+: a second version byte before the prompt; 0 before.
    pub prompt_version: u8,
    pub prompt: String,
}

impl AttDef {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let attrib = Attrib::decode(r)?;
        let prompt_version = if r.version >= Version::R2010 {
            r.data.read_rc()?
        } else {
            0
        };
        Ok(AttDef {
            attrib,
            prompt_version,
            prompt: r.text()?,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        self.attrib.encode(w)?;
        w.data.write_rc(self.prompt_version);
        w.text(&self.prompt)
    }
}

/// An MTEXT background fill (R2004+, present when the background flags ask
/// for it).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct MTextBackground {
    /// DXF 45; AutoCAD's default is 1.5. ODS gives a `BL`; it is a `BD`,
    /// as ACadSharp (MIT, `DwgObjectReader.readMText`) reads it.
    pub scale: f64,
    pub color: Color,
    /// Raw transparency long (DXF 441).
    pub transparency: i32,
}

/// The column layout of an R2018 MTEXT (DXF 71-74, 44-46).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct MTextColumns {
    /// 1 static, 2 dynamic (0, no columns, is `None` in [`MTextContext`]).
    pub column_type: i16,
    /// The stored column count (DXF 72). For dynamic columns without auto
    /// height it is `heights.len()`, and is not used on write.
    pub count: i32,
    pub width: f64,
    pub gutter: f64,
    pub auto_height: bool,
    pub flow_reversed: bool,
    /// The column heights, stored for dynamic columns without auto height.
    pub heights: Vec<f64>,
}

const DYNAMIC_COLUMNS: i16 = 2;

/// The [`MTextContext::version`] AutoCAD 2018 writes (ODS: "default 0"; 4 in
/// every corpus MTEXT).
pub const MTEXT_CONTEXT_VERSION: i16 = 4;

impl MTextColumns {
    fn has_heights(column_type: i16, auto_height: bool) -> bool {
        column_type == DYNAMIC_COLUMNS && !auto_height
    }
}

/// The main MTEXT fields an R2018 context repeats (ODS 20.4.46, "redundant
/// fields").
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct MTextCopies {
    pub attachment: i32,
    pub x_axis_dir: [f64; 3],
    pub insertion: [f64; 3],
    pub rect_width: f64,
    pub rect_height: f64,
    pub extents_width: f64,
    pub extents_height: f64,
}

impl MTextCopies {
    fn of(m: &MText) -> Self {
        MTextCopies {
            attachment: i32::from(m.attachment),
            x_axis_dir: m.x_axis_dir,
            insertion: m.insertion,
            rect_width: m.rect_width,
            rect_height: m.rect_height,
            extents_width: m.extents_width,
            extents_height: m.extents_height,
        }
    }

    /// Field by field: the stored copy while the main field equals its
    /// value as read (bit for bit), else the main field's current value.
    fn merge(now: &Self, read: &Self, stored: &Self) -> Self {
        fn pick<T: Copy + PartialEq>(now: T, read: T, stored: T) -> T {
            if now == read { stored } else { now }
        }
        let b3 = |v: [f64; 3]| v.map(f64::to_bits);
        let pick3 = |now: [f64; 3], read: [f64; 3], stored: [f64; 3]| {
            if b3(now) == b3(read) { stored } else { now }
        };
        let pick1 = |now: f64, read: f64, stored: f64| {
            if now.to_bits() == read.to_bits() {
                stored
            } else {
                now
            }
        };
        MTextCopies {
            attachment: pick(now.attachment, read.attachment, stored.attachment),
            x_axis_dir: pick3(now.x_axis_dir, read.x_axis_dir, stored.x_axis_dir),
            insertion: pick3(now.insertion, read.insertion, stored.insertion),
            rect_width: pick1(now.rect_width, read.rect_width, stored.rect_width),
            rect_height: pick1(now.rect_height, read.rect_height, stored.rect_height),
            extents_width: pick1(now.extents_width, read.extents_width, stored.extents_width),
            extents_height: pick1(
                now.extents_height,
                read.extents_height,
                stored.extents_height,
            ),
        }
    }
}

/// The R2018 data of an MTEXT that is not annotative: a version, a flag, a
/// registered application, the columns, and copies of the main fields
/// (ODS 20.4.46). The copies are not separate values: they are written from
/// the main fields, except that while a main field is unchanged since it
/// was read, its copy is written as read (see [`MText::context_copies`]).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct MTextContext {
    pub version: i16,
    pub default_flag: bool,
    /// The registered application (hard pointer).
    pub app: Handle,
    pub columns: Option<MTextColumns>,
    /// The main fields and their copies, as read.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub as_read: AsRead<(MTextCopies, MTextCopies)>,
}

/// A multi-line text (ODS 20.4.46). `contents` keeps the format codes; see
/// [`MText::plain_text`].
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MText {
    /// Insertion point (the attachment point).
    pub insertion: [f64; 3],
    /// Normal (extrusion direction).
    pub extrusion: [f64; 3],
    /// The text direction (DXF 11).
    pub x_axis_dir: [f64; 3],
    /// The reference rectangle width (DXF 41).
    pub rect_width: f64,
    /// R2007+ reference rectangle height (DXF 46); 0 before.
    pub rect_height: f64,
    /// Text height.
    pub text_height: f64,
    /// DXF 71: 1-9, top left to bottom right.
    pub attachment: i16,
    /// DXF 72.
    pub drawing_dir: i16,
    /// Height of the text drawn (as AutoCAD last computed it).
    pub extents_height: f64,
    /// Width of the text drawn (as AutoCAD last computed it).
    pub extents_width: f64,
    /// The text, with MTEXT format codes.
    pub contents: String,
    /// The text style (hard pointer).
    pub style: Handle,
    /// DXF 73.
    pub line_spacing_style: i16,
    /// DXF 44.
    pub line_spacing_factor: f64,
    /// Undocumented bit after the line spacing.
    pub unknown_bit: bool,
    /// R2004+ DXF 90: 1 background fill, 2 fill with the drawing background
    /// color, 0x10 text frame (R2018+); 0 before.
    pub background_flags: i32,
    /// The background fill, stored when `background_flags` has bit 0x01 (or,
    /// from R2018, 0x10).
    pub background: Option<MTextBackground>,
    /// `Some` for an MTEXT that is not annotative (R2018+ files store the
    /// difference; from older files every MTEXT gets
    /// [`MText::context_from_fields`]).
    pub context: Option<MTextContext>,
}

impl MText {
    /// Multiline text `contents` (MTEXT format codes allowed, `\P` a new
    /// paragraph) with its top left corner at `insertion`, `text_height`
    /// high, wrapped at `width` (0: no wrapping), left to right in the XY
    /// plane, with single line spacing and no background. Not annotative:
    /// its R2018 context is the one these fields give
    /// ([`MText::context_from_fields`]). The extents are estimated (AutoCAD
    /// recomputes them). Its style is left null:
    /// [`crate::Drawing::add_entity`] gives such text the current text
    /// style ($TEXTSTYLE).
    pub fn new(insertion: [f64; 3], text_height: f64, width: f64, contents: &str) -> MText {
        let lines = contents.matches("\\P").count() + 1;
        let mut m = MText {
            insertion,
            extrusion: [0.0, 0.0, 1.0],
            x_axis_dir: [1.0, 0.0, 0.0],
            rect_width: width,
            rect_height: 0.0,
            text_height,
            attachment: 1,
            drawing_dir: 1,
            extents_height: text_height * lines as f64,
            extents_width: width,
            contents: contents.to_owned(),
            style: Handle(0),
            line_spacing_style: 1,
            line_spacing_factor: 1.0,
            unknown_bit: false,
            background_flags: 0,
            background: None,
            context: None,
        };
        m.context = Some(m.context_from_fields());
        m
    }

    fn has_background(flags: i32, version: Version) -> bool {
        flags & 0x01 != 0 || (version >= Version::R2018 && flags & 0x10 != 0)
    }

    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let v = r.version;
        let d = &mut r.data;
        let insertion = d.read_3bd()?;
        let extrusion = d.read_3bd()?;
        let x_axis_dir = d.read_3bd()?;
        let rect_width = d.read_bd()?;
        let rect_height = if v >= Version::R2007 {
            d.read_bd()?
        } else {
            0.0
        };
        let text_height = d.read_bd()?;
        let attachment = d.read_bs()?;
        let drawing_dir = d.read_bs()?;
        let extents_height = d.read_bd()?;
        let extents_width = d.read_bd()?;
        let contents = r.text()?;
        let style = r.href_abs()?;
        let line_spacing_style = r.data.read_bs()?;
        let line_spacing_factor = r.data.read_bd()?;
        let unknown_bit = r.data.read_b()?;
        let background_flags = if v >= Version::R2004 {
            r.data.read_bl()?
        } else {
            0
        };
        let background = if MText::has_background(background_flags, v) {
            Some(MTextBackground {
                scale: r.data.read_bd()?,
                color: r.read_cmc()?,
                transparency: r.data.read_bl()?,
            })
        } else {
            None
        };
        let (context, stored_copies) = if v >= Version::R2018 && r.data.read_b()? {
            let (c, stored) = MText::decode_context(r)?;
            (Some(c), Some(stored))
        } else {
            // Before R2018 (filled in below), or annotative.
            (None, None)
        };
        let mut m = MText {
            insertion,
            extrusion,
            x_axis_dir,
            rect_width,
            rect_height,
            text_height,
            attachment,
            drawing_dir,
            extents_height,
            extents_width,
            contents,
            style,
            line_spacing_style,
            line_spacing_factor,
            unknown_bit,
            background_flags,
            background,
            context,
        };
        if v < Version::R2018 {
            m.context = Some(m.context_from_fields());
        } else if let Some(stored) = stored_copies {
            let main = MTextCopies::of(&m);
            if let Some(c) = m.context.as_mut() {
                c.as_read = AsRead::new((main, stored));
            }
        }
        Ok(m)
    }

    /// The copies of the main fields an R2018 context is written with: the
    /// main fields, except that one unchanged since it was read keeps its
    /// copy as read.
    pub fn context_copies(&self) -> MTextCopies {
        let now = MTextCopies::of(self);
        match self.context.as_ref().and_then(|c| c.as_read.get()) {
            Some((read, stored)) => MTextCopies::merge(&now, read, stored),
            None => now,
        }
    }

    /// The column heights to write. AutoCAD stores the height of a dynamic
    /// column as the negated extents height (3 of 4 corpus cases) or the
    /// negated extents width (1 case): derived copies. A height equal to
    /// either, as read, follows that extent when it changes (the height
    /// relation first when both hold); other heights are written as they
    /// are.
    fn column_heights(&self, columns: &MTextColumns) -> Vec<f64> {
        let Some((read, _)) = self.context.as_ref().and_then(|c| c.as_read.get()) else {
            return columns.heights.clone();
        };
        let same = |a: f64, b: f64| a.to_bits() == b.to_bits();
        columns
            .heights
            .iter()
            .map(|&h| {
                for (was, now) in [
                    (read.extents_height, self.extents_height),
                    (read.extents_width, self.extents_width),
                ] {
                    if same(h, -was) {
                        return if same(was, now) { h } else { -now };
                    }
                }
                h
            })
            .collect()
    }

    /// The R2018 context of a non-annotative MTEXT with these fields: what
    /// AutoCAD 2018 writes (version 4, default flag set, no registered
    /// application, the fields copied, no columns). An MTEXT read from an
    /// older file gets this context. The fields are recorded as read, so
    /// copies derived from them (column heights) follow later edits.
    pub fn context_from_fields(&self) -> MTextContext {
        let now = MTextCopies::of(self);
        MTextContext {
            version: MTEXT_CONTEXT_VERSION,
            default_flag: true,
            app: Handle(0),
            columns: None,
            as_read: AsRead::new((now, now)),
        }
    }

    /// The context and the copies of the main fields it stores.
    fn decode_context(r: &mut ObjReader) -> Result<(MTextContext, MTextCopies)> {
        let version = r.data.read_bs()?;
        let default_flag = r.data.read_b()?;
        let app = r.href_abs()?;
        let d = &mut r.data;
        let attachment = d.read_bl()?;
        let x_axis_dir = d.read_3bd()?;
        let insertion = d.read_3bd()?;
        let rect_width = d.read_bd()?;
        let rect_height = d.read_bd()?;
        let extents_width = d.read_bd()?;
        let extents_height = d.read_bd()?;
        let column_type = d.read_bs()?;
        let columns = if column_type != 0 {
            let count = d.read_bl()?;
            let width = d.read_bd()?;
            let gutter = d.read_bd()?;
            let auto_height = d.read_b()?;
            let flow_reversed = d.read_b()?;
            let heights = if MTextColumns::has_heights(column_type, auto_height) {
                // Each height takes at least two bits.
                let n = usize::try_from(count)
                    .ok()
                    .filter(|&n| n as u64 <= d.remaining_bits() / 2)
                    .ok_or_else(|| corrupt("MTEXT column count exceeds the data"))?;
                (0..n).map(|_| d.read_bd()).collect::<Result<_>>()?
            } else {
                Vec::new()
            };
            Some(MTextColumns {
                column_type,
                count,
                width,
                gutter,
                auto_height,
                flow_reversed,
                heights,
            })
        } else {
            None
        };
        let context = MTextContext {
            version,
            default_flag,
            app,
            columns,
            as_read: AsRead::default(),
        };
        let stored = MTextCopies {
            attachment,
            x_axis_dir,
            insertion,
            rect_width,
            rect_height,
            extents_width,
            extents_height,
        };
        Ok((context, stored))
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        let v = ObjWriter::VERSION;
        if MText::has_background(self.background_flags, v) != self.background.is_some() {
            return Err(encode_err(
                "MTEXT background data must be present exactly when its flags ask for it",
            ));
        }
        let d = &mut w.data;
        d.write_3bd(self.insertion);
        d.write_3bd(self.extrusion);
        d.write_3bd(self.x_axis_dir);
        d.write_bd(self.rect_width);
        d.write_bd(self.rect_height);
        d.write_bd(self.text_height);
        d.write_bs(self.attachment);
        d.write_bs(self.drawing_dir);
        d.write_bd(self.extents_height);
        d.write_bd(self.extents_width);
        w.text(&self.contents)?;
        put(w, HARD_POINTER, self.style)?;
        w.data.write_bs(self.line_spacing_style);
        w.data.write_bd(self.line_spacing_factor);
        w.data.write_b(self.unknown_bit);
        w.data.write_bl(self.background_flags);
        if let Some(b) = &self.background {
            w.data.write_bd(b.scale);
            w.write_cmc(&b.color)?;
            w.data.write_bl(b.transparency);
        }
        w.data.write_b(self.context.is_some());
        if let Some(c) = &self.context {
            self.encode_context(c, w)?;
        }
        Ok(())
    }

    fn encode_context(&self, c: &MTextContext, w: &mut ObjWriter) -> Result<()> {
        w.data.write_bs(c.version);
        w.data.write_b(c.default_flag);
        put(w, HARD_POINTER, c.app)?;
        let copies = self.context_copies();
        let d = &mut w.data;
        d.write_bl(copies.attachment);
        d.write_3bd(copies.x_axis_dir);
        d.write_3bd(copies.insertion);
        d.write_bd(copies.rect_width);
        d.write_bd(copies.rect_height);
        d.write_bd(copies.extents_width);
        d.write_bd(copies.extents_height);
        match &c.columns {
            None => d.write_bs(0),
            Some(col) => {
                if col.column_type == 0 {
                    return Err(encode_err("MTEXT columns need a column type"));
                }
                let has_heights = MTextColumns::has_heights(col.column_type, col.auto_height);
                if !has_heights && !col.heights.is_empty() {
                    return Err(encode_err(
                        "MTEXT column heights are stored for dynamic columns without auto \
                         height only",
                    ));
                }
                d.write_bs(col.column_type);
                let count = if has_heights {
                    i32::try_from(col.heights.len())
                        .map_err(|_| encode_err("too many MTEXT columns to encode"))?
                } else {
                    col.count
                };
                d.write_bl(count);
                d.write_bd(col.width);
                d.write_bd(col.gutter);
                d.write_b(col.auto_height);
                d.write_b(col.flow_reversed);
                for h in self.column_heights(col) {
                    d.write_bd(h);
                }
            }
        }
        Ok(())
    }

    /// The text without format codes: `\P` (and `\N`, a column break)
    /// become line breaks, `\~` a no-break space, `\\`, `\{` and `\}` the
    /// character, `\U+XXXX` the Unicode character, `\M+nXXXX` the character
    /// of double-byte code page `n` (`?` when it cannot be decoded), and a
    /// stack `\Sa^b;` (or `a/b`, `a#b`) `a/b`. Groups `{...}`, the toggles
    /// `\L \l \O \o \K \k`, and codes running to a `;` (`\f \F \H \C \c \T
    /// \Q \W \A \p`) are removed.
    pub fn plain_text(&self) -> String {
        plain_text(&self.contents)
    }
}

/// The extended data that holds an MTEXT's columns before R2018.
const COLUMNS_BEGIN: &str = "ACAD_MTEXT_COLUMN_INFO_BEGIN";
const COLUMNS_END: &str = "ACAD_MTEXT_COLUMN_INFO_END";

/// Before R2018 an MTEXT's columns are extended data of the ACAD application
/// (`acad`, its APPID): `ACAD_MTEXT_COLUMN_INFO_BEGIN`, group code and value pairs
/// (75 type, 79 auto height, 76 count, 78 flow reversed, 48 width, 49
/// gutter, 50 a height count and that many heights), then
/// `ACAD_MTEXT_COLUMN_INFO_END`. AutoCAD 2018 moves them into the context
/// and drops the extended data; this does the same, so the R2018 form of an
/// older MTEXT is the one AutoCAD writes. The extended data stores 0 for
/// the height of one dynamic column; AutoCAD 2018 stores the negated extents
/// height for three of the four such corpus MTEXTs (the fourth has the
/// negated extents width, which nothing in the older file gives), so a
/// stored 0 becomes the negated extents height, a derived copy that follows
/// the extents height (see `column_heights`). Other heights are kept
/// (unconfirmed by the corpus). Data that does not parse is left alone.
pub(crate) fn take_column_xdata(xdata: &mut Vec<XDataApp>, acad: Handle, m: &mut MText) {
    for (i, app) in xdata.iter_mut().enumerate() {
        if app.app != acad {
            continue;
        }
        let start = app
            .items
            .iter()
            .position(|x| matches!(x, XDataItem::String(s) if s == COLUMNS_BEGIN));
        let end = app
            .items
            .iter()
            .position(|x| matches!(x, XDataItem::String(s) if s == COLUMNS_END));
        let (Some(start), Some(end)) = (start, end) else {
            continue;
        };
        let Some(body) = app.items.get(start + 1..end) else {
            continue;
        };
        let Some(columns) = parse_column_xdata(body, m.extents_height) else {
            return;
        };
        if let Some(c) = m.context.as_mut() {
            c.columns = columns;
        }
        app.items.drain(start..=end);
        if app.items.is_empty() {
            xdata.remove(i);
        }
        return;
    }
}

/// The columns of [`take_column_xdata`]'s pairs; `Some(None)` for type 0.
fn parse_column_xdata(items: &[XDataItem], extents_height: f64) -> Option<Option<MTextColumns>> {
    let (mut kind, mut auto, mut count, mut flow) = (None, None, None, None);
    let (mut width, mut gutter, mut heights) = (None, None, None);
    let mut it = items.iter();
    while let Some(item) = it.next() {
        let XDataItem::Short(code) = item else {
            return None;
        };
        match (code, it.next()?) {
            (75, XDataItem::Short(v)) => kind = Some(*v),
            (79, XDataItem::Short(v)) => auto = Some(*v != 0),
            (76, XDataItem::Short(v)) => count = Some(i32::from(*v)),
            (78, XDataItem::Short(v)) => flow = Some(*v != 0),
            (48, XDataItem::Real(v)) => width = Some(*v),
            (49, XDataItem::Real(v)) => gutter = Some(*v),
            (50, XDataItem::Short(n)) => {
                let n = usize::try_from(*n).ok()?;
                let mut hs = Vec::new();
                for _ in 0..n {
                    let XDataItem::Real(h) = it.next()? else {
                        return None;
                    };
                    hs.push(*h);
                }
                heights = Some(hs);
            }
            _ => return None,
        }
    }
    let column_type = kind?;
    if column_type == 0 {
        return Some(None);
    }
    let auto_height = auto.unwrap_or(false);
    let mut heights = if MTextColumns::has_heights(column_type, auto_height) {
        heights.unwrap_or_default()
    } else {
        Vec::new()
    };
    if let [h] = heights.as_mut_slice()
        && h.to_bits() == 0
    {
        *h = -extents_height;
    }
    Some(Some(MTextColumns {
        column_type,
        count: count.unwrap_or(0),
        width: width?,
        gutter: gutter?,
        auto_height,
        flow_reversed: flow.unwrap_or(false),
        heights,
    }))
}

/// The double-byte code page of `\M+n` (1 Shift-JIS, 2 Big5, 3 Korean
/// Wansung, 4 Johab, 5 GB2312), as a DWG code page index.
fn mbcs_codepage(n: u8) -> Option<u16> {
    Some(match n {
        b'1' => 38, // ANSI_932
        b'2' => 41, // ANSI_950
        b'3' => 40, // ANSI_949
        b'4' => 42, // ANSI_1361 (Johab)
        b'5' => 39, // ANSI_936
        _ => return None,
    })
}

fn hex_value(digits: &[char]) -> Option<u32> {
    if digits.len() != 4 {
        return None;
    }
    digits
        .iter()
        .try_fold(0u32, |acc, c| Some(acc * 16 + c.to_digit(16)?))
}

/// See [`MText::plain_text`].
fn plain_text(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    // A UTF-16 high surrogate from `\U+D800`-`\U+DBFF`, awaiting its pair.
    let mut high: Option<u32> = None;
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        let low_follows =
            c == '\\' && chars.get(i + 1) == Some(&'U') && chars.get(i + 2) == Some(&'+');
        if high.is_some() && !low_follows {
            high = None;
            out.push('\u{FFFD}');
        }
        i += 1;
        if c == '{' || c == '}' {
            continue;
        }
        if c != '\\' {
            out.push(c);
            continue;
        }
        let Some(&code) = chars.get(i) else {
            out.push('\\');
            break;
        };
        i += 1;
        match code {
            'P' | 'N' => out.push('\n'),
            '~' => out.push('\u{A0}'),
            '\\' | '{' | '}' => out.push(code),
            'L' | 'l' | 'O' | 'o' | 'K' | 'k' => {}
            'U' if chars.get(i) == Some(&'+') => {
                let Some(u) = chars.get(i + 1..i + 5).and_then(hex_value) else {
                    if high.take().is_some() {
                        out.push('\u{FFFD}');
                    }
                    out.push_str("\\U");
                    continue;
                };
                i += 5;
                match (high.take(), u) {
                    (Some(h), 0xDC00..=0xDFFF) => {
                        let c = 0x10000 + ((h - 0xD800) << 10) + (u - 0xDC00);
                        out.push(char::from_u32(c).unwrap_or('\u{FFFD}'));
                    }
                    (pending, u) => {
                        if pending.is_some() {
                            out.push('\u{FFFD}');
                        }
                        if (0xD800..=0xDBFF).contains(&u) {
                            high = Some(u);
                        } else {
                            out.push(char::from_u32(u).unwrap_or('\u{FFFD}'));
                        }
                    }
                }
            }
            'M' if chars.get(i) == Some(&'+') => {
                let n = chars.get(i + 1).copied().unwrap_or(' ');
                let Some(code) = chars.get(i + 2..i + 6).and_then(hex_value) else {
                    out.push_str("\\M");
                    continue;
                };
                i += 6;
                let bytes = [(code >> 8) as u8, code as u8];
                let decoded = u8::try_from(n)
                    .ok()
                    .and_then(mbcs_codepage)
                    .filter(|&cp| crate::strings::is_supported(cp))
                    .map(|cp| crate::strings::decode(cp, &bytes))
                    .filter(|t| t.chars().count() == 1 && !t.contains('\u{FFFD}'));
                out.push_str(decoded.as_deref().unwrap_or("?"));
            }
            'S' => {
                let rest = chars.get(i..).unwrap_or_default();
                let end = rest.iter().position(|&c| c == ';');
                let body = rest.get(..end.unwrap_or(rest.len())).unwrap_or_default();
                i += body.len() + usize::from(end.is_some());
                for &c in body {
                    out.push(if matches!(c, '^' | '#') { '/' } else { c });
                }
            }
            'f' | 'F' | 'H' | 'C' | 'c' | 'T' | 'Q' | 'W' | 'A' | 'p' => {
                let rest = chars.get(i..).unwrap_or_default();
                i += rest
                    .iter()
                    .position(|&c| c == ';')
                    .map_or(rest.len(), |e| e + 1);
            }
            other => {
                out.push('\\');
                out.push(other);
            }
        }
    }
    if high.is_some() {
        out.push('\u{FFFD}');
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::tests::roundtrip;
    use super::*;
    use crate::bits::Color;
    use crate::object::common::{LinetypeRef, MaterialRef, ObjectCommon, PlotStyleRef};
    use crate::object::type_code;

    pub(crate) fn text() -> Text {
        Text {
            insertion: [1.0, 2.0, 0.5],
            alignment: Some([3.0, 2.0]),
            extrusion: [0.0, 0.0, 1.0],
            thickness: 0.25,
            oblique_angle: 0.1,
            rotation: 1.5,
            height: 2.5,
            width_factor: 0.8,
            value: "Hello ✓".into(),
            generation: 2,
            horizontal_alignment: 1,
            vertical_alignment: 3,
            style: Handle(0x11),
        }
    }

    #[test]
    fn roundtrip_text() {
        let mut t = text();
        roundtrip(&t, Text::encode, Text::decode, type_code::TEXT);
        // Every field at its default: only the flags, the point, BE, BT,
        // the height and the value are stored.
        t.insertion[2] = 0.0;
        t.alignment = None;
        t.thickness = 0.0;
        t.oblique_angle = 0.0;
        t.rotation = 0.0;
        t.width_factor = 1.0;
        t.generation = 0;
        t.horizontal_alignment = 0;
        t.vertical_alignment = 0;
        roundtrip(&t, Text::encode, Text::decode, type_code::TEXT);
        let mut w = ObjWriter::new(Handle(1));
        t.encode(&mut w).unwrap();
        assert_eq!(w.data.bit_len(), 8 + 128 + 1 + 1 + 64);
        // An alignment point equal to the insertion point is still stored.
        t.alignment = Some([1.0, 2.0]);
        roundtrip(&t, Text::encode, Text::decode, type_code::TEXT);
    }

    pub(crate) fn attrib() -> Attrib {
        Attrib {
            text: text(),
            version: 0,
            attribute_type: ATTRIB_SINGLE_LINE,
            mtext: None,
            tag: "TAG".into(),
            field_length: 0,
            flags: 8,
            lock_position: true,
        }
    }

    pub(crate) fn mtext() -> MText {
        MText {
            insertion: [1.0, 2.0, 0.0],
            extrusion: [0.0, 0.0, 1.0],
            x_axis_dir: [1.0, 0.0, 0.0],
            rect_width: 20.0,
            rect_height: 5.0,
            text_height: 2.5,
            attachment: 1,
            drawing_dir: 5,
            extents_height: 4.0,
            extents_width: 18.0,
            contents: "{\\fArial|b1;Hi}\\PThere".into(),
            style: Handle(0x11),
            line_spacing_style: 1,
            line_spacing_factor: 1.0,
            unknown_bit: false,
            background_flags: 0,
            background: None,
            context: Some(MTextContext {
                version: 0,
                default_flag: true,
                app: Handle(0x12),
                columns: None,
                as_read: AsRead::default(),
            }),
        }
    }

    fn embedded() -> EmbeddedMText {
        EmbeddedMText {
            common: EntityCommon {
                obj: ObjectCommon {
                    handle: Handle(0),
                    owner: Handle(0),
                    owner_relative: false,
                    reactors: vec![],
                    xdict: None,
                    xdata: vec![],
                    has_ds_binary_data: false,
                },
                entmode: 0,
                layer: Handle(0x10),
                linetype: LinetypeRef::ByLayer,
                plotstyle: PlotStyleRef::ByLayer,
                material: MaterialRef::ByLayer,
                color: Color::ByLayer,
                transparency: None,
                read_color: Default::default(),
                color_book: None,
                ltype_scale: 1.0,
                lineweight: 29,
                invisible: false,
                shadow_flags: 0,
                full_visual_style: None,
                face_visual_style: None,
                edge_visual_style: None,
                proxy_graphics: None,
                prev_next: None,
            },
            mtext: mtext(),
            annotative_data: None,
        }
    }

    #[test]
    fn roundtrip_attrib() {
        let mut a = attrib();
        roundtrip(&a, Attrib::encode, Attrib::decode, type_code::ATTRIB);
        a.version = 1;
        a.attribute_type = ATTRIB_MULTI_LINE;
        a.mtext = Some(Box::new(embedded()));
        roundtrip(&a, Attrib::encode, Attrib::decode, type_code::ATTRIB);
        // The embedded MTEXT's owner as an offset from the attribute.
        let m = a.mtext.as_mut().unwrap();
        m.common.obj.owner = Handle(0x7E);
        m.common.obj.owner_relative = true;
        roundtrip(&a, Attrib::encode, Attrib::decode, type_code::ATTRIB);
        a.mtext.as_mut().unwrap().annotative_data = Some(AnnotativeData {
            bytes: vec![1, 2, 3],
            app: Handle(0x13),
            unknown: 0,
        });
        roundtrip(&a, Attrib::encode, Attrib::decode, type_code::ATTRIB);
    }

    #[test]
    fn attrib_multi_line_needs_its_mtext() {
        let mut a = attrib();
        a.attribute_type = ATTRIB_MULTI_LINE;
        assert!(a.encode(&mut ObjWriter::new(Handle(1))).is_err());
        let mut a = attrib();
        a.mtext = Some(Box::new(embedded()));
        assert!(a.encode(&mut ObjWriter::new(Handle(1))).is_err());
        // The embedded MTEXT has no handle of its own.
        let mut a = attrib();
        a.attribute_type = ATTRIB_MULTI_LINE;
        let mut e = embedded();
        e.common.obj.handle = Handle(5);
        a.mtext = Some(Box::new(e));
        assert!(a.encode(&mut ObjWriter::new(Handle(1))).is_err());
    }

    #[test]
    fn roundtrip_attdef() {
        let mut d = AttDef {
            attrib: attrib(),
            prompt_version: 0,
            prompt: "Enter number:".into(),
        };
        roundtrip(&d, AttDef::encode, AttDef::decode, type_code::ATTDEF);
        d.attrib.attribute_type = ATTDEF_MULTI_LINE;
        d.attrib.mtext = Some(Box::new(embedded()));
        roundtrip(&d, AttDef::encode, AttDef::decode, type_code::ATTDEF);
    }

    #[test]
    fn roundtrip_mtext() {
        let mut m = mtext();
        roundtrip(&m, MText::encode, MText::decode, type_code::MTEXT);
        // Background fill, dynamic columns with heights; then annotative.
        m.background_flags = 0x11;
        m.background = Some(MTextBackground {
            scale: 1.5,
            color: Color::Index(3),
            transparency: 0,
        });
        let c = m.context.as_mut().unwrap();
        c.columns = Some(MTextColumns {
            column_type: DYNAMIC_COLUMNS,
            count: 2,
            width: 10.0,
            gutter: 1.0,
            auto_height: false,
            flow_reversed: true,
            heights: vec![3.0, 4.0],
        });
        roundtrip(&m, MText::encode, MText::decode, type_code::MTEXT);
        let c = m.context.as_mut().unwrap();
        c.columns = Some(MTextColumns {
            column_type: 1,
            count: 3,
            width: 10.0,
            gutter: 1.0,
            auto_height: false,
            flow_reversed: false,
            heights: vec![],
        });
        roundtrip(&m, MText::encode, MText::decode, type_code::MTEXT);
        m.context = None;
        roundtrip(&m, MText::encode, MText::decode, type_code::MTEXT);
    }

    /// Encode as R2018 and decode again.
    fn reencode(m: &MText) -> MText {
        let mut w = ObjWriter::new(Handle(0x80));
        m.encode(&mut w).unwrap();
        let bytes = w.finish(type_code::MTEXT).unwrap();
        MText::decode(&mut super::super::tests::reader(&bytes)).unwrap()
    }

    /// The context copies of the main fields are written as read while the
    /// main fields are unchanged (here they differ from the main fields, as
    /// a third-party file might store them), and follow the main fields
    /// once edited. So does a column height stored as the negated extents
    /// height.
    #[test]
    fn mtext_context_copies_follow_edits() {
        let mut m = mtext();
        let mut stored = MTextCopies::of(&m);
        stored.insertion = [9.0, 9.0, 9.0];
        stored.rect_width = 99.0;
        stored.extents_width = 77.0;
        let main = MTextCopies::of(&m);
        let c = m.context.as_mut().unwrap();
        c.columns = Some(MTextColumns {
            column_type: DYNAMIC_COLUMNS,
            count: 2,
            width: 10.0,
            gutter: 1.0,
            auto_height: false,
            flow_reversed: false,
            heights: vec![-4.0, 3.0],
        });
        c.as_read = AsRead::new((main, stored));
        // Unchanged: the copies as read, and the same bytes again.
        let back = reencode(&m);
        assert_eq!(back.context_copies(), stored);
        let encode = |m: &MText| {
            let mut w = ObjWriter::new(Handle(0x80));
            m.encode(&mut w).unwrap();
            w.finish(type_code::MTEXT).unwrap()
        };
        assert_eq!(encode(&back), encode(&m));
        // Moved, widened and taller: those copies follow; extents width,
        // unchanged, keeps its copy; the height -4.0 (= -extents height)
        // follows, 3.0 does not.
        let mut edited = m.clone();
        edited.insertion = [5.0, 6.0, 0.0];
        edited.rect_width = 30.0;
        edited.extents_height = 6.5;
        let back = reencode(&edited);
        let copies = back.context_copies();
        assert_eq!(copies.insertion, [5.0, 6.0, 0.0]);
        assert_eq!(copies.rect_width, 30.0);
        assert_eq!(copies.extents_height, 6.5);
        assert_eq!(copies.extents_width, 77.0);
        let cols = back.context.as_ref().unwrap().columns.as_ref().unwrap();
        assert_eq!(cols.heights, [-6.5, 3.0]);
        // A context made from the fields copies them.
        let mut fresh = mtext();
        fresh.insertion = [1.5, 2.5, 0.0];
        assert_eq!(reencode(&fresh).context_copies(), MTextCopies::of(&fresh));
    }

    fn column_xdata(app: u64, height: f64) -> XDataApp {
        use XDataItem::*;
        XDataApp {
            app: crate::handle::Handle(app),
            items: vec![
                String(COLUMNS_BEGIN.into()),
                Short(75),
                Short(2),
                Short(79),
                Short(0),
                Short(76),
                Short(1),
                Short(78),
                Short(0),
                Short(48),
                Real(15.5),
                Short(49),
                Real(2.0),
                Short(50),
                Short(1),
                Real(height),
                String(COLUMNS_END.into()),
            ],
        }
    }

    /// Pre-R2018 columns in the ACAD application's extended data move into
    /// the context; the stored 0 height becomes the negated extents height,
    /// which then follows edits of the extents height.
    #[test]
    fn column_xdata_converts_and_its_height_follows_edits() {
        let mut m = mtext();
        m.context = Some(m.context_from_fields());
        // Another application's identical data is not the ACAD columns.
        let mut xdata = vec![column_xdata(0x99, 0.0), column_xdata(0x12, 0.0)];
        take_column_xdata(&mut xdata, Handle(0x12), &mut m);
        assert_eq!(xdata, [column_xdata(0x99, 0.0)]);
        let cols = m.context.as_ref().unwrap().columns.clone().unwrap();
        assert_eq!(
            (cols.column_type, cols.count, cols.width, cols.gutter),
            (2, 1, 15.5, 2.0)
        );
        assert_eq!(cols.heights, [-4.0]);
        assert_eq!(
            reencode(&m).context.unwrap().columns.unwrap().heights,
            [-4.0]
        );
        m.extents_height = 9.0;
        assert_eq!(
            reencode(&m).context.unwrap().columns.unwrap().heights,
            [-9.0]
        );
        // A stored height other than 0 is kept, and is not a derived copy.
        let mut m = mtext();
        m.context = Some(m.context_from_fields());
        let mut xdata = vec![column_xdata(0x12, 3.5)];
        take_column_xdata(&mut xdata, Handle(0x12), &mut m);
        assert!(xdata.is_empty());
        m.extents_height = 9.0;
        assert_eq!(
            reencode(&m).context.unwrap().columns.unwrap().heights,
            [3.5]
        );
    }

    /// A height stored as the negated extents width (MTEXT 3F5 in the R2018
    /// sample) follows the extents width.
    #[test]
    fn column_height_follows_extents_width() {
        let mut m = mtext();
        let mut c = m.context_from_fields();
        c.columns = Some(MTextColumns {
            column_type: DYNAMIC_COLUMNS,
            count: 1,
            width: 1.0,
            gutter: 1.0,
            auto_height: false,
            flow_reversed: false,
            heights: vec![-m.extents_width],
        });
        m.context = Some(c);
        let mut back = reencode(&m);
        assert_eq!(
            back.context
                .as_ref()
                .unwrap()
                .columns
                .as_ref()
                .unwrap()
                .heights,
            [-18.0]
        );
        back.extents_width = 21.0;
        back.extents_height = 7.0;
        assert_eq!(
            reencode(&back).context.unwrap().columns.unwrap().heights,
            [-21.0]
        );
    }

    #[test]
    fn negative_annotative_size_is_corrupt() {
        let mut a = attrib();
        a.version = 0;
        a.attribute_type = ATTRIB_MULTI_LINE;
        a.mtext = Some(Box::new(embedded()));
        // The attribute with the annotative size BS written as -1.
        let mut w = ObjWriter::new(Handle(0x80));
        a.text.encode(&mut w).unwrap();
        w.data.write_rc(a.version);
        w.data.write_rc(a.attribute_type);
        let e = a.mtext.as_ref().unwrap();
        encode_entity_mode(&mut w, &e.common).unwrap();
        e.mtext.encode(&mut w).unwrap();
        w.data.write_bs(-1);
        w.text(&a.tag).unwrap();
        w.data.write_bs(0);
        w.data.write_rc(0);
        w.data.write_b(false);
        let bytes = w.finish(type_code::ATTRIB).unwrap();
        let err = Attrib::decode(&mut super::super::tests::reader(&bytes)).unwrap_err();
        assert!(err.to_string().contains("negative annotative"), "{err}");
    }

    #[test]
    fn mtext_background_must_match_its_flags() {
        let mut m = mtext();
        m.background_flags = 1;
        assert!(m.encode(&mut ObjWriter::new(Handle(1))).is_err());
        m.background_flags = 0;
        m.background = Some(MTextBackground {
            scale: 1.5,
            color: Color::Index(3),
            transparency: 0,
        });
        assert!(m.encode(&mut ObjWriter::new(Handle(1))).is_err());
    }

    #[test]
    fn mtext_column_count_is_bounded() {
        // The R2018 context of an MTEXT with dynamic columns claiming
        // i32::MAX heights: rejected before allocating.
        let mut d = crate::bits::BitWriter::new();
        d.write_bs(0);
        d.write_b(true);
        d.write_bl(0);
        for _ in 0..2 {
            d.write_3bd([0.0; 3]);
        }
        for _ in 0..4 {
            d.write_bd(0.0);
        }
        d.write_bs(DYNAMIC_COLUMNS);
        d.write_bl(i32::MAX);
        d.write_bd(0.0);
        d.write_bd(0.0);
        d.write_b(false);
        d.write_b(false);
        let mut h = crate::bits::BitWriter::new();
        h.write_handle_ref(crate::handle::HandleRef { code: 5, value: 1 })
            .unwrap();
        let obj = crate::object::stream::tests::assemble(
            Version::R2018,
            type_code::MTEXT,
            &d,
            Some(&crate::bits::BitWriter::new()),
            &h,
        );
        let (mut r, _) = ObjReader::split(&obj, Version::R2018, 30).unwrap();
        let err = MText::decode_context(&mut r).unwrap_err();
        assert!(err.to_string().contains("column count"), "{err}");
    }

    #[test]
    fn mtext_plain_text() {
        let mut m = mtext();
        m.contents = "{\\fArial|b1;Hi}\\PThere \\U+00E9".into();
        assert_eq!(m.plain_text(), "Hi\nThere é");
    }

    #[test]
    fn plain_text_codes() {
        let cases = [
            ("a\\Pb\\Nc", "a\nb\nc"),
            ("a\\~b", "a\u{A0}b"),
            ("\\\\ \\{x\\}", "\\ {x}"),
            ("{a{b}c}", "abc"),
            ("\\U+00E9\\U+4E2D", "é中"),
            // A surrogate pair, and an unpaired surrogate.
            ("\\U+D83D\\U+DE00", "😀"),
            ("\\U+D83Dx", "\u{FFFD}x"),
            ("\\U+D83D\\U+zz", "\u{FFFD}\\U+zz"),
            // Bad hex is kept as text.
            ("\\U+00G9", "\\U+00G9"),
            // Shift-JIS 0x82A0 is あ; Johab (4) is not supported.
            ("\\M+182A0", "あ"),
            ("\\M+48861", "?"),
            ("\\M+982A0", "?"),
            ("\\fArial|b1|i0|c0|p34;x", "x"),
            (
                "\\Ftxt.shx;\\H2.5;\\H0.7x;\\C1;\\c255;\\T1.1;\\Q15;\\W0.8;\\A1;y",
                "y",
            ),
            ("\\pxi-3,l3,t4;z", "z"),
            ("\\Lu\\l\\Oo\\o\\Kk\\k", "uok"),
            ("\\S1/2;\\S3^4;\\S5#6;", "1/23/45/6"),
            ("\\S1/2", "1/2"),
            ("x\\", "x\\"),
            ("\\X", "\\X"),
        ];
        for (src, want) in cases {
            assert_eq!(plain_text(src), want, "{src:?}");
        }
    }
}
