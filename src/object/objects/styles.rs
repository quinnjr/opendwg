//! Multiline styles (ODS 20.4.73) and multileader styles (20.4.87).

use crate::bits::Color;
use crate::error::Result;
use crate::error::encode_error as encode_err;
use crate::handle::Handle;
use crate::object::entities::dim::{MIN_BD, check_fits};
use crate::object::stream::{ObjReader, ObjWriter};
use crate::object::tables::{HARD_POINTER, put};
use crate::version::Version;

/// The linetype of a multiline style element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum MLineLinetype {
    /// R2018+: the linetype (hard pointer).
    Handle(Handle),
    /// Before R2018: ODS "Ltindex" ("Linetype index (yes, index)"). 32767
    /// is BYLAYER and 32766 BYBLOCK, which the drawing resolves to handles
    /// on read. Other values are probably the index into the LTYPE control's
    /// entries (ACadSharp (MIT) resolves them as the linetype table's
    /// `ElementAt(index)`), unconfirmed by the corpus (to be checked by Task
    /// 26's oracle); they are kept, and the style has no R2018 form (see
    /// [`MLineStyle::is_convertible`]).
    Index(i16),
}

/// One line of a multiline style.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct MLineStyleElement {
    pub offset: f64,
    pub color: Color,
    pub linetype: MLineLinetype,
}

/// A multiline style (ODS 20.4.73, MLINESTYLE).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct MLineStyle {
    pub name: String,
    pub description: String,
    /// The flags as DWG stores them; ODS 20.4.73 maps them to DXF 70 (DWG
    /// 1 is DXF 2, 2 is 1, 16 is 16, 32 is 64, 64 is 32, 256 is 256, 512 is
    /// 1024, 1024 is 512).
    pub flags: i16,
    pub fill_color: Color,
    pub start_angle: f64,
    pub end_angle: f64,
    /// At most 255 (an `RC` count).
    pub elements: Vec<MLineStyleElement>,
}

impl MLineStyle {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let name = r.text()?;
        let description = r.text()?;
        let flags = r.data.read_bs()?;
        let fill_color = r.read_cmc()?;
        let start_angle = r.data.read_bd()?;
        let end_angle = r.data.read_bd()?;
        let n = usize::from(r.data.read_rc()?);
        check_fits(r, n, MIN_BD + 2, "multiline style lines")?;
        let mut elements = Vec::with_capacity(n);
        for _ in 0..n {
            let offset = r.data.read_bd()?;
            let color = r.read_cmc()?;
            let linetype = if r.version >= Version::R2018 {
                MLineLinetype::Handle(r.href_abs()?)
            } else {
                MLineLinetype::Index(r.data.read_bs()?)
            };
            elements.push(MLineStyleElement {
                offset,
                color,
                linetype,
            });
        }
        Ok(MLineStyle {
            name,
            description,
            flags,
            fill_color,
            start_angle,
            end_angle,
            elements,
        })
    }

    /// Whether R2018 can store every element's linetype: not while one is
    /// still a pre-R2018 index.
    pub fn is_convertible(&self) -> bool {
        self.elements
            .iter()
            .all(|e| matches!(e.linetype, MLineLinetype::Handle(_)))
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        w.text(&self.name)?;
        w.text(&self.description)?;
        w.data.write_bs(self.flags);
        w.write_cmc(&self.fill_color)?;
        w.data.write_bd(self.start_angle);
        w.data.write_bd(self.end_angle);
        let n = u8::try_from(self.elements.len())
            .map_err(|_| encode_err("a multiline style has at most 255 lines"))?;
        w.data.write_rc(n);
        for e in &self.elements {
            w.data.write_bd(e.offset);
            w.write_cmc(&e.color)?;
            match e.linetype {
                MLineLinetype::Handle(h) => put(w, HARD_POINTER, h)?,
                MLineLinetype::Index(_) => {
                    return Err(encode_err(
                        "a multiline style linetype index must be resolved to a handle",
                    ));
                }
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Multileader styles.

/// A multileader style (ODS 20.4.87, MLEADERSTYLE).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct MLeaderStyle {
    /// R2010+ DXF 179 (expected 2); 2 before.
    pub version: i16,
    /// DXF 170.
    pub content_type: i16,
    /// DXF 171.
    pub draw_mleader_order: i16,
    /// DXF 172.
    pub draw_leader_order: i16,
    /// DXF 90.
    pub max_leader_points: i32,
    /// DXF 40, 41.
    pub first_segment_angle: f64,
    pub second_segment_angle: f64,
    /// DXF 173.
    pub leader_type: i16,
    /// DXF 91.
    pub line_color: Color,
    /// DXF 340 (hard pointer).
    pub line_type: Handle,
    /// DXF 92.
    pub line_weight: i32,
    /// DXF 290.
    pub landing_enabled: bool,
    /// DXF 42.
    pub landing_gap: f64,
    /// DXF 291.
    pub dogleg_enabled: bool,
    /// DXF 43.
    pub landing_distance: f64,
    /// DXF 3.
    pub description: String,
    /// DXF 341 (hard pointer).
    pub arrow_head: Handle,
    /// DXF 44.
    pub arrow_head_size: f64,
    /// DXF 300.
    pub default_text: String,
    /// DXF 342 (hard pointer).
    pub text_style: Handle,
    /// DXF 174, 178.
    pub left_attachment: i16,
    pub right_attachment: i16,
    /// DXF 175. Not stored by a pre-R2010 style without the ACAD_MLEADERVER
    /// mark; 1 (as every corpus style has it) for one.
    pub text_angle_type: i16,
    /// DXF 176.
    pub text_alignment: i16,
    /// DXF 93.
    pub text_color: Color,
    /// DXF 45.
    pub text_height: f64,
    /// DXF 292.
    pub text_frame: bool,
    /// DXF 297. Like [`MLeaderStyle::text_angle_type`]; `false` for an
    /// unmarked pre-R2010 style.
    pub align_text_left: bool,
    /// DXF 46.
    pub align_space: f64,
    /// DXF 343 (hard pointer).
    pub block: Handle,
    /// DXF 94.
    pub block_color: Color,
    /// DXF 47, 49, 140.
    pub block_scale: [f64; 3],
    /// DXF 293.
    pub block_scale_enabled: bool,
    /// DXF 141.
    pub block_rotation: f64,
    /// DXF 294.
    pub block_rotation_enabled: bool,
    /// DXF 177.
    pub block_connection: i16,
    /// DXF 142.
    pub scale: f64,
    /// DXF 295. Stored after [`MLeaderStyle::annotative`] (ODS: before; the
    /// corpus's annotative style has DXF 296 = 1, 295 = 0).
    pub property_changed: bool,
    /// DXF 296.
    pub annotative: bool,
    /// DXF 143.
    pub break_size: f64,
    /// R2010+ DXF 271, 273, 272; 0, 9 and 9 before (what AutoCAD writes
    /// for the corpus's older styles saved as R2010).
    pub attachment_direction: i16,
    pub top_attachment: i16,
    pub bottom_attachment: i16,
    /// R2013+ DXF 298 (not in ODS; as ACadSharp (MIT) reads it); `false`
    /// before.
    pub unknown_298: bool,
}

impl MLeaderStyle {
    /// Before R2010 the text angle type and align-left fields are present
    /// only in a style marked with ACAD_MLEADERVER extended data (ODS's
    /// "IsNewFormat"), which [`super::super::decode_object`] finds from the
    /// already decoded extended data.
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let new_format = r.version >= Version::R2010 || r.mleader_new_format;
        Self::decode_format(r, new_format)
    }

    fn decode_format(r: &mut ObjReader, new_format: bool) -> Result<Self> {
        let v = r.version;
        let version = if v >= Version::R2010 {
            r.data.read_bs()?
        } else {
            2
        };
        let content_type = r.data.read_bs()?;
        let draw_mleader_order = r.data.read_bs()?;
        let draw_leader_order = r.data.read_bs()?;
        let max_leader_points = r.data.read_bl()?;
        let first_segment_angle = r.data.read_bd()?;
        let second_segment_angle = r.data.read_bd()?;
        let leader_type = r.data.read_bs()?;
        let line_color = r.read_cmc()?;
        let line_type = r.href_abs()?;
        let line_weight = r.data.read_bl()?;
        let landing_enabled = r.data.read_b()?;
        let landing_gap = r.data.read_bd()?;
        let dogleg_enabled = r.data.read_b()?;
        let landing_distance = r.data.read_bd()?;
        let description = r.text()?;
        let arrow_head = r.href_abs()?;
        let arrow_head_size = r.data.read_bd()?;
        let default_text = r.text()?;
        let text_style = r.href_abs()?;
        let left_attachment = r.data.read_bs()?;
        let right_attachment = r.data.read_bs()?;
        let text_angle_type = if new_format { r.data.read_bs()? } else { 1 };
        let text_alignment = r.data.read_bs()?;
        let text_color = r.read_cmc()?;
        let text_height = r.data.read_bd()?;
        let text_frame = r.data.read_b()?;
        let align_text_left = if new_format { r.data.read_b()? } else { false };
        let align_space = r.data.read_bd()?;
        let block = r.href_abs()?;
        let block_color = r.read_cmc()?;
        let block_scale = r.data.read_3bd()?;
        let block_scale_enabled = r.data.read_b()?;
        let block_rotation = r.data.read_bd()?;
        let block_rotation_enabled = r.data.read_b()?;
        let block_connection = r.data.read_bs()?;
        let scale = r.data.read_bd()?;
        let annotative = r.data.read_b()?;
        let property_changed = r.data.read_b()?;
        let break_size = r.data.read_bd()?;
        let (attachment_direction, top_attachment, bottom_attachment) = if v >= Version::R2010 {
            (r.data.read_bs()?, r.data.read_bs()?, r.data.read_bs()?)
        } else {
            (0, 9, 9)
        };
        let unknown_298 = if v >= Version::R2013 {
            r.data.read_b()?
        } else {
            false
        };
        Ok(MLeaderStyle {
            version,
            content_type,
            draw_mleader_order,
            draw_leader_order,
            max_leader_points,
            first_segment_angle,
            second_segment_angle,
            leader_type,
            line_color,
            line_type,
            line_weight,
            landing_enabled,
            landing_gap,
            dogleg_enabled,
            landing_distance,
            description,
            arrow_head,
            arrow_head_size,
            default_text,
            text_style,
            left_attachment,
            right_attachment,
            text_angle_type,
            text_alignment,
            text_color,
            text_height,
            text_frame,
            align_text_left,
            align_space,
            block,
            block_color,
            block_scale,
            block_scale_enabled,
            block_rotation,
            block_rotation_enabled,
            block_connection,
            scale,
            property_changed,
            annotative,
            break_size,
            attachment_direction,
            top_attachment,
            bottom_attachment,
            unknown_298,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        let d = &mut w.data;
        d.write_bs(self.version);
        d.write_bs(self.content_type);
        d.write_bs(self.draw_mleader_order);
        d.write_bs(self.draw_leader_order);
        d.write_bl(self.max_leader_points);
        d.write_bd(self.first_segment_angle);
        d.write_bd(self.second_segment_angle);
        d.write_bs(self.leader_type);
        w.write_cmc(&self.line_color)?;
        put(w, HARD_POINTER, self.line_type)?;
        let d = &mut w.data;
        d.write_bl(self.line_weight);
        d.write_b(self.landing_enabled);
        d.write_bd(self.landing_gap);
        d.write_b(self.dogleg_enabled);
        d.write_bd(self.landing_distance);
        w.text(&self.description)?;
        put(w, HARD_POINTER, self.arrow_head)?;
        w.data.write_bd(self.arrow_head_size);
        w.text(&self.default_text)?;
        put(w, HARD_POINTER, self.text_style)?;
        let d = &mut w.data;
        d.write_bs(self.left_attachment);
        d.write_bs(self.right_attachment);
        d.write_bs(self.text_angle_type);
        d.write_bs(self.text_alignment);
        w.write_cmc(&self.text_color)?;
        let d = &mut w.data;
        d.write_bd(self.text_height);
        d.write_b(self.text_frame);
        d.write_b(self.align_text_left);
        d.write_bd(self.align_space);
        put(w, HARD_POINTER, self.block)?;
        w.write_cmc(&self.block_color)?;
        let d = &mut w.data;
        d.write_3bd(self.block_scale);
        d.write_b(self.block_scale_enabled);
        d.write_bd(self.block_rotation);
        d.write_b(self.block_rotation_enabled);
        d.write_bs(self.block_connection);
        d.write_bd(self.scale);
        d.write_b(self.annotative);
        d.write_b(self.property_changed);
        d.write_bd(self.break_size);
        d.write_bs(self.attachment_direction);
        d.write_bs(self.top_attachment);
        d.write_bs(self.bottom_attachment);
        d.write_b(self.unknown_298);
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::bits::BitWriter;
    use crate::bits::Color;
    use crate::error::Error;
    use crate::handle::{Handle, HandleRef};
    use crate::object::entities::tests::roundtrip;
    use crate::object::objects::test_util::{H, book};
    use crate::object::objects::*;
    use crate::object::stream::tests::assemble;
    use crate::object::stream::{ObjReader, ObjWriter};

    use crate::version::Version;

    #[test]
    fn roundtrip_group_and_mline_style() {
        let g = Group {
            name: "Doors".into(),
            unnamed: 0,
            selectable: 1,
            entities: vec![Handle(0x69D), Handle(0x69C)],
        };
        roundtrip(&g, Group::encode, Group::decode, 72);
        let m = MLineStyle {
            name: "STANDARD".into(),
            description: "Desc ü".into(),
            flags: 0x112,
            fill_color: book(),
            start_angle: 1.5,
            end_angle: 1.25,
            elements: vec![
                MLineStyleElement {
                    offset: 0.5,
                    color: Color::Index(3),
                    linetype: MLineLinetype::Handle(Handle(0x15)),
                },
                MLineStyleElement {
                    offset: -0.5,
                    color: Color::Rgb { r: 1, g: 2, b: 3 },
                    linetype: MLineLinetype::Handle(Handle(0x16)),
                },
            ],
        };
        roundtrip(&m, MLineStyle::encode, MLineStyle::decode, 73);
    }

    /// Before R2018 a multiline style element stores a linetype index, which
    /// has no R2018 form (the drawing resolves BYLAYER and BYBLOCK).
    #[test]
    fn r2004_mline_style_index() {
        let mut d = BitWriter::new();
        d.write_tv_raw(b"STANDARD").unwrap();
        d.write_tv_raw(b"").unwrap();
        d.write_bs(0);
        d.write_cmc(&Color::ByLayer, Version::R2004).unwrap();
        d.write_bd(1.5);
        d.write_bd(1.5);
        d.write_rc(1);
        d.write_bd(0.5);
        d.write_cmc(&Color::ByLayer, Version::R2004).unwrap();
        d.write_bs(32767);
        let bytes = assemble(Version::R2004, 73, &d, None, &BitWriter::new());
        let (mut r, _) = ObjReader::split(&bytes, Version::R2004, 30).unwrap();
        let m = MLineStyle::decode(&mut r).unwrap();
        assert_eq!(m.elements[0].linetype, MLineLinetype::Index(32767));
        assert!(!m.is_convertible());
        assert!(m.encode(&mut ObjWriter::new(H)).is_err());
        // An element count beyond the data left fails before allocating.
        let mut d = BitWriter::new();
        d.write_tv_raw(b"S").unwrap();
        d.write_tv_raw(b"").unwrap();
        d.write_bs(0);
        d.write_cmc(&Color::ByLayer, Version::R2004).unwrap();
        d.write_bd(1.5);
        d.write_bd(1.5);
        d.write_rc(255);
        let bytes = assemble(Version::R2004, 73, &d, None, &BitWriter::new());
        let (mut r, _) = ObjReader::split(&bytes, Version::R2004, 30).unwrap();
        assert!(matches!(
            MLineStyle::decode(&mut r),
            Err(Error::Truncated { .. })
        ));
    }

    pub(crate) fn mleader_style() -> MLeaderStyle {
        MLeaderStyle {
            version: 2,
            content_type: 1,
            draw_mleader_order: 1,
            draw_leader_order: 1,
            max_leader_points: 3,
            first_segment_angle: 0.25,
            second_segment_angle: 0.5,
            leader_type: 2,
            line_color: Color::Index(5),
            line_type: Handle(0x14),
            line_weight: -2,
            landing_enabled: true,
            landing_gap: 0.09,
            dogleg_enabled: false,
            landing_distance: 0.36,
            description: "Style ü".into(),
            arrow_head: Handle(0x99),
            arrow_head_size: 0.18,
            default_text: "default".into(),
            text_style: Handle(0x11),
            left_attachment: 1,
            right_attachment: 3,
            text_angle_type: 2,
            text_alignment: 1,
            text_color: book(),
            text_height: 0.2,
            text_frame: true,
            align_text_left: true,
            align_space: 4.0,
            block: Handle(0x55),
            block_color: Color::ByBlock,
            block_scale: [1.0, 2.0, 3.0],
            block_scale_enabled: true,
            block_rotation: 0.75,
            block_rotation_enabled: false,
            block_connection: 1,
            scale: 2.0,
            property_changed: true,
            annotative: true,
            break_size: 0.125,
            attachment_direction: 1,
            top_attachment: 8,
            bottom_attachment: 7,
            unknown_298: true,
        }
    }

    #[test]
    fn roundtrip_mleader_style() {
        roundtrip(
            &mleader_style(),
            MLeaderStyle::encode,
            MLeaderStyle::decode,
            500,
        );
    }

    /// Write `s` in the R2004 layout, with or without the fields of the new
    /// format.
    fn r2004_mleader_style(s: &MLeaderStyle, new_format: bool) -> Vec<u8> {
        let v = Version::R2004;
        let mut d = BitWriter::new();
        let mut h = BitWriter::new();
        let href = |h: &mut BitWriter, x: Handle| {
            h.write_handle_ref(HandleRef {
                code: 5,
                value: x.0,
            })
            .unwrap()
        };
        d.write_bs(s.content_type);
        d.write_bs(s.draw_mleader_order);
        d.write_bs(s.draw_leader_order);
        d.write_bl(s.max_leader_points);
        d.write_bd(s.first_segment_angle);
        d.write_bd(s.second_segment_angle);
        d.write_bs(s.leader_type);
        d.write_cmc(&s.line_color, v).unwrap();
        href(&mut h, s.line_type);
        d.write_bl(s.line_weight);
        d.write_b(s.landing_enabled);
        d.write_bd(s.landing_gap);
        d.write_b(s.dogleg_enabled);
        d.write_bd(s.landing_distance);
        d.write_tv_raw(s.description.as_bytes()).unwrap();
        href(&mut h, s.arrow_head);
        d.write_bd(s.arrow_head_size);
        d.write_tv_raw(s.default_text.as_bytes()).unwrap();
        href(&mut h, s.text_style);
        d.write_bs(s.left_attachment);
        d.write_bs(s.right_attachment);
        if new_format {
            d.write_bs(s.text_angle_type);
        }
        d.write_bs(s.text_alignment);
        d.write_cmc(&s.text_color, v).unwrap();
        d.write_bd(s.text_height);
        d.write_b(s.text_frame);
        if new_format {
            d.write_b(s.align_text_left);
        }
        d.write_bd(s.align_space);
        href(&mut h, s.block);
        d.write_cmc(&s.block_color, v).unwrap();
        d.write_3bd(s.block_scale);
        d.write_b(s.block_scale_enabled);
        d.write_bd(s.block_rotation);
        d.write_b(s.block_rotation_enabled);
        d.write_bs(s.block_connection);
        d.write_bd(s.scale);
        d.write_b(s.annotative);
        d.write_b(s.property_changed);
        d.write_bd(s.break_size);
        assemble(v, 500, &d, None, &h)
    }

    /// Before R2010 a style without the ACAD_MLEADERVER mark has no text
    /// angle type or align-left flag; R2010 fields take their defaults.
    #[test]
    fn r2004_mleader_style_formats() {
        let mut want = mleader_style();
        want.description = "Style".into();
        want.text_color = Color::Rgb { r: 1, g: 2, b: 3 };
        (
            want.attachment_direction,
            want.top_attachment,
            want.bottom_attachment,
        ) = (0, 9, 9);
        want.unknown_298 = false;
        for new_format in [true, false] {
            let bytes = r2004_mleader_style(&want, new_format);
            let (mut r, _) = ObjReader::split(&bytes, Version::R2004, 30).unwrap();
            r.mleader_new_format = new_format;
            let got = MLeaderStyle::decode(&mut r).unwrap();
            assert_eq!(r.data.remaining_bits(), 0, "{new_format}");
            let mut expect = want.clone();
            if !new_format {
                expect.text_angle_type = 1;
                expect.align_text_left = false;
            }
            assert_eq!(got, expect, "{new_format}");
        }
    }
}
