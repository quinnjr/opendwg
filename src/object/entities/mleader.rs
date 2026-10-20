//! Multileaders (ODS 20.4.48 MLEADER, 20.4.86 MLeaderAnnotContext), the
//! class type `MULTILEADER`.
//!
//! Where ACadSharp (MIT) differs from ODS, as the corpus confirms (pinned by
//! tests in `tests/dimensions.rs`): ODS's "-R2007" arrowhead list is stored
//! up to R2007 (AC1021) inclusive, and ODS's unnamed `BS 175` is the text
//! alignment type (DXF 175), as `readMultiLeader` reads them; a leader
//! line's break info repeats (ODS shows one), as `readLeaderLine` reads it.
//!
//! Unconfirmed by the corpus, which has none: block content, block labels
//! and R2007 arrowheads. They follow ODS.

use super::super::common::AsRead;
use super::super::stream::{ObjReader, ObjWriter};
use super::super::tables::{HARD_POINTER, SOFT_POINTER, put};
use super::dim::{
    MIN_3BD, MIN_BD, MIN_BL, MIN_BS, check_handles, read_count, read_pairs, read_points,
    write_count, write_pairs, write_points,
};
use crate::bits::Color;
use crate::error::Result;
use crate::error::encode_error as encode_err;
use crate::handle::Handle;
use crate::version::Version;

/// A leader line of a multileader's leader root (ODS 20.4.86).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct LeaderLine {
    pub points: Vec<[f64; 3]>,
    /// Breaks: a segment index and its start/end point pairs.
    pub breaks: Vec<LeaderBreak>,
    /// DXF 91.
    pub index: i32,
    /// R2010+ (the multileader's values before): 0 invisible, 1 straight,
    /// 2 spline (DXF 170).
    pub leader_type: i16,
    /// R2010+ (DXF 92).
    pub color: Color,
    /// R2010+ linetype (hard pointer).
    pub line_type: Handle,
    /// R2010+ (DXF 171).
    pub line_weight: i32,
    /// R2010+ (DXF 40).
    pub arrow_size: f64,
    /// R2010+ arrowhead block (hard pointer).
    pub arrow: Handle,
    /// R2010+ which of the values above override the multileader's (DXF
    /// 93): 1 type, 2 color, 4 linetype, 8 weight, 16 arrow size, 32 arrow.
    pub override_flags: i32,
}

/// One break of a leader line.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct LeaderBreak {
    /// DXF 90.
    pub segment: i32,
    /// Start and end points (DXF 11, 12).
    pub pairs: Vec<[[f64; 3]; 2]>,
}

/// A leader root: one or more leader lines attached on one side of a
/// multileader's content (ODS 20.4.86).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct LeaderRoot {
    /// DXF 290 ("is content valid").
    pub content_valid: bool,
    /// DXF 291.
    pub unknown: bool,
    /// DXF 10.
    pub connection: [f64; 3],
    /// DXF 11.
    pub direction: [f64; 3],
    /// Break start and end points (DXF 12, 13).
    pub breaks: Vec<[[f64; 3]; 2]>,
    /// DXF 90.
    pub index: i32,
    /// DXF 40.
    pub landing_distance: f64,
    pub lines: Vec<LeaderLine>,
    /// R2010+: 0 horizontal, 1 vertical (DXF 271); 0 before.
    pub attachment_direction: i16,
}

/// A multileader's text content (ODS 20.4.86).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct MLeaderText {
    /// DXF 304, with MTEXT format codes.
    pub label: String,
    /// DXF 11.
    pub normal: [f64; 3],
    /// The text style (hard pointer).
    pub style: Handle,
    /// DXF 12.
    pub location: [f64; 3],
    /// DXF 13.
    pub direction: [f64; 3],
    /// DXF 42 (radians).
    pub rotation: f64,
    /// DXF 43, 44.
    pub boundary_width: f64,
    pub boundary_height: f64,
    /// DXF 45.
    pub line_spacing_factor: f64,
    /// 1 at least, 2 exactly (DXF 170).
    pub line_spacing_style: i16,
    /// DXF 90.
    pub color: Color,
    /// DXF 171.
    pub alignment: i16,
    /// 1 horizontal, 3 vertical, 6 by style (DXF 172).
    pub flow_direction: i16,
    /// DXF 91.
    pub bg_color: Color,
    /// DXF 141.
    pub bg_scale: f64,
    /// DXF 92.
    pub bg_transparency: i32,
    /// DXF 291.
    pub bg_fill: bool,
    /// DXF 292.
    pub bg_mask_fill: bool,
    /// DXF 173.
    pub column_type: i16,
    /// DXF 293.
    pub auto_height: bool,
    /// DXF 142, 143.
    pub column_width: f64,
    pub column_gutter: f64,
    /// DXF 294.
    pub column_flow_reversed: bool,
    /// DXF 144.
    pub column_sizes: Vec<f64>,
    /// DXF 295.
    pub word_break: bool,
    pub unknown: bool,
}

/// A multileader's block content (ODS 20.4.86).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct MLeaderBlock {
    /// The block record (soft pointer).
    pub block: Handle,
    /// DXF 14, 15, 16.
    pub normal: [f64; 3],
    pub location: [f64; 3],
    pub scale: [f64; 3],
    /// DXF 46 (radians).
    pub rotation: f64,
    /// DXF 93.
    pub color: Color,
    /// The transformation matrix (DXF 47): rotation, OCS to WCS (by
    /// `normal`), scaling (by `scale`) and translation (by `location`). A
    /// cached copy of those fields, kept as read: it is not recomputed when
    /// they are edited, so it goes stale until AutoCAD regenerates the
    /// multileader.
    pub matrix: [f64; 16],
}

/// What a multileader's leaders point from.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub enum MLeaderContent {
    None,
    Text(Box<MLeaderText>),
    Block(Box<MLeaderBlock>),
}

/// The annotation context embedded in a multileader (ODS 20.4.86, without
/// the AcDbAnnotScaleObjectContextData part): its content and leaders.
///
/// These are the values AutoCAD displays. The like-named fields of
/// [`MultiLeader`] (arrowhead size, attachments, landing distance, ...) are
/// the multileader's own copies of its MLEADERSTYLE properties, flagged by
/// `override_flags` where they differ from the style; they are inputs AutoCAD
/// lays the context out from, not copies of the context.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct MLeaderContext {
    pub roots: Vec<LeaderRoot>,
    /// DXF 40.
    pub scale: f64,
    /// DXF 10.
    pub content_base: [f64; 3],
    /// DXF 41.
    pub text_height: f64,
    /// DXF 140.
    pub arrowhead_size: f64,
    /// DXF 145.
    pub landing_gap: f64,
    /// DXF 174, 175.
    pub text_left_attachment: i16,
    pub text_right_attachment: i16,
    /// 0 left, 1 center, 2 right (DXF 176).
    pub text_align: i16,
    /// 0 content extents, 1 insertion point (DXF 177).
    pub block_connection: i16,
    pub content: MLeaderContent,
    /// DXF 110, 111, 112.
    pub base_point: [f64; 3],
    pub base_direction: [f64; 3],
    pub base_vertical: [f64; 3],
    /// DXF 297.
    pub normal_reversed: bool,
    /// R2010+ (DXF 273, 272); 9 before R2010, AutoCAD's default.
    pub text_top_attachment: i16,
    pub text_bottom_attachment: i16,
}

/// A label of a multileader's block content: an attribute's text.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MLeaderBlockLabel {
    /// The ATTDEF (DXF 330), written as a soft pointer as ODS gives it
    /// (ACadSharp writes a hard pointer; the corpus has no block labels).
    pub attdef: Handle,
    /// DXF 302.
    pub text: String,
    /// DXF 177.
    pub ui_index: i16,
    /// DXF 44.
    pub width: f64,
}

/// A multileader (ODS 20.4.48), a class type (`MULTILEADER`).
///
/// Besides its content and leaders it stores the properties of its
/// MLEADERSTYLE, with `override_flags` saying which differ from the style.
/// These are values of the multileader, not copies kept in step with the
/// style, and are written as they are.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct MultiLeader {
    /// R2010+ version (DXF 270); 2 before R2010.
    pub version: i16,
    pub context: MLeaderContext,
    /// The MLEADERSTYLE (hard pointer, DXF 340).
    pub style: Handle,
    /// DXF 90: which properties override the style's (ODS 20.4.48).
    pub override_flags: i32,
    /// 0 invisible, 1 straight, 2 spline (DXF 170).
    pub leader_type: i16,
    /// DXF 91.
    pub line_color: Color,
    /// The leader linetype (hard pointer, DXF 341).
    pub line_type: Handle,
    /// DXF 171.
    pub line_weight: i32,
    /// DXF 290, 291.
    pub landing_enabled: bool,
    pub dogleg_enabled: bool,
    /// DXF 41.
    pub landing_distance: f64,
    /// The arrowhead block (hard pointer, DXF 342).
    pub arrowhead: Handle,
    /// DXF 42.
    pub arrowhead_size: f64,
    /// 0 none, 1 block, 2 MTEXT, 3 tolerance (DXF 172). It describes the
    /// context's content: while the kind of content (text, block, none) is
    /// the one read, it is written as it is, even if it disagrees; once the
    /// kind changed (or for a multileader not read from a file) text
    /// content is written with 2 and block content with 1.
    pub content_type: i16,
    /// The text style (hard pointer, DXF 343).
    pub text_style: Handle,
    /// DXF 173, 95.
    pub text_left_attachment: i16,
    pub text_right_attachment: i16,
    /// DXF 174.
    pub text_angle_type: i16,
    /// DXF 175 (ODS: unknown).
    pub text_alignment: i16,
    /// DXF 92.
    pub text_color: Color,
    /// DXF 292.
    pub text_frame: bool,
    /// The content block (hard pointer, DXF 344).
    pub block: Handle,
    /// DXF 93.
    pub block_color: Color,
    /// DXF 10.
    pub block_scale: [f64; 3],
    /// DXF 43.
    pub block_rotation: f64,
    /// DXF 176.
    pub block_connection: i16,
    /// DXF 293.
    pub annotative: bool,
    /// Up to R2007 only: arrowheads, each with an "is default" flag (DXF
    /// 94, 345). R2018 has no place for them: a multileader with some cannot
    /// be written.
    pub arrowheads: Vec<(bool, Handle)>,
    pub block_labels: Vec<MLeaderBlockLabel>,
    /// DXF 294.
    pub text_direction_negative: bool,
    /// DXF 178.
    pub ipe_align: i16,
    /// 1 left, 2 center, 3 right (DXF 179).
    pub justification: i16,
    /// DXF 45.
    pub scale: f64,
    /// R2010+: 0 horizontal, 1 vertical (DXF 271); 0 before.
    pub attachment_direction: i16,
    /// R2010+ (DXF 272, 273, in that order); 9 before.
    pub text_bottom_attachment: i16,
    pub text_top_attachment: i16,
    /// R2013+ (DXF 295); `false` before.
    pub extended_to_text: bool,
    /// The kind of content read, for [`MultiLeader::written_content_type`].
    #[cfg_attr(feature = "serde", serde(skip))]
    pub read_content: AsRead<ContentKind>,
}

/// The kind of a multileader's content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub enum ContentKind {
    None,
    Text,
    Block,
}

impl MLeaderContent {
    /// Which kind of content this is.
    pub fn kind(&self) -> ContentKind {
        match self {
            MLeaderContent::None => ContentKind::None,
            MLeaderContent::Text(_) => ContentKind::Text,
            MLeaderContent::Block(_) => ContentKind::Block,
        }
    }
}

/// [`MultiLeader::content_type`] values for block and MTEXT content.
const CONTENT_BLOCK: i16 = 1;
const CONTENT_MTEXT: i16 = 2;

/// The smallest leader root: two B, two 3BD, BL, BL, BD and BL.
const MIN_LEADER_ROOT: u64 = 2 + 2 * MIN_3BD + 2 * MIN_BL + MIN_BD + MIN_BL;
/// The smallest leader line: three BL.
const MIN_LEADER_LINE: u64 = 3 * MIN_BL;
/// The smallest break info: two BL.
const MIN_BREAK: u64 = 2 * MIN_BL;
/// The smallest block label: a BS and a BD (and its text and handle).
const MIN_BLOCK_LABEL: u64 = MIN_BS + MIN_BD;

/// The R2010+ values a leader line takes before R2010: straight, by block,
/// no linetype or arrowhead, by-block weight, no overrides.
const LEADER_LINE_DEFAULTS: (i16, Color, i32, f64) = (1, Color::ByBlock, -2, 0.0);

impl LeaderLine {
    fn decode(r: &mut ObjReader) -> Result<Self> {
        let points = read_points(r, "leader line points")?;
        let nb = read_count(r, MIN_BREAK, "leader line breaks")?;
        let breaks = (0..nb)
            .map(|_| {
                Ok(LeaderBreak {
                    segment: r.data.read_bl()?,
                    pairs: read_pairs(r, "leader line break points")?,
                })
            })
            .collect::<Result<_>>()?;
        let index = r.data.read_bl()?;
        let (leader_type, color, line_weight, arrow_size) = LEADER_LINE_DEFAULTS;
        let mut line = LeaderLine {
            points,
            breaks,
            index,
            leader_type,
            color,
            line_type: Handle(0),
            line_weight,
            arrow_size,
            arrow: Handle(0),
            override_flags: 0,
        };
        if r.version >= Version::R2010 {
            line.leader_type = r.data.read_bs()?;
            line.color = r.read_cmc()?;
            line.line_type = r.href_abs()?;
            line.line_weight = r.data.read_bl()?;
            line.arrow_size = r.data.read_bd()?;
            line.arrow = r.href_abs()?;
            line.override_flags = r.data.read_bl()?;
        }
        Ok(line)
    }

    fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        write_points(w, &self.points, "leader line points")?;
        write_count(w, self.breaks.len(), "leader line breaks")?;
        for b in &self.breaks {
            w.data.write_bl(b.segment);
            write_pairs(w, &b.pairs, "leader line break points")?;
        }
        w.data.write_bl(self.index);
        w.data.write_bs(self.leader_type);
        w.write_cmc(&self.color)?;
        put(w, HARD_POINTER, self.line_type)?;
        w.data.write_bl(self.line_weight);
        w.data.write_bd(self.arrow_size);
        put(w, HARD_POINTER, self.arrow)?;
        w.data.write_bl(self.override_flags);
        Ok(())
    }
}

impl LeaderRoot {
    fn decode(r: &mut ObjReader) -> Result<Self> {
        let d = &mut r.data;
        let content_valid = d.read_b()?;
        let unknown = d.read_b()?;
        let connection = d.read_3bd()?;
        let direction = d.read_3bd()?;
        let breaks = read_pairs(r, "leader root breaks")?;
        let index = r.data.read_bl()?;
        let landing_distance = r.data.read_bd()?;
        let nl = read_count(r, MIN_LEADER_LINE, "leader lines")?;
        let lines = (0..nl)
            .map(|_| LeaderLine::decode(r))
            .collect::<Result<_>>()?;
        let attachment_direction = if r.version >= Version::R2010 {
            r.data.read_bs()?
        } else {
            0
        };
        Ok(LeaderRoot {
            content_valid,
            unknown,
            connection,
            direction,
            breaks,
            index,
            landing_distance,
            lines,
            attachment_direction,
        })
    }

    fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        w.data.write_b(self.content_valid);
        w.data.write_b(self.unknown);
        w.data.write_3bd(self.connection);
        w.data.write_3bd(self.direction);
        write_pairs(w, &self.breaks, "leader root breaks")?;
        w.data.write_bl(self.index);
        w.data.write_bd(self.landing_distance);
        write_count(w, self.lines.len(), "leader lines")?;
        for l in &self.lines {
            l.encode(w)?;
        }
        w.data.write_bs(self.attachment_direction);
        Ok(())
    }
}

impl MLeaderText {
    fn decode(r: &mut ObjReader) -> Result<Self> {
        let label = r.text()?;
        let normal = r.data.read_3bd()?;
        let style = r.href_abs()?;
        let d = &mut r.data;
        let location = d.read_3bd()?;
        let direction = d.read_3bd()?;
        let rotation = d.read_bd()?;
        let boundary_width = d.read_bd()?;
        let boundary_height = d.read_bd()?;
        let line_spacing_factor = d.read_bd()?;
        let line_spacing_style = d.read_bs()?;
        let color = r.read_cmc()?;
        let alignment = r.data.read_bs()?;
        let flow_direction = r.data.read_bs()?;
        let bg_color = r.read_cmc()?;
        let d = &mut r.data;
        let bg_scale = d.read_bd()?;
        let bg_transparency = d.read_bl()?;
        let bg_fill = d.read_b()?;
        let bg_mask_fill = d.read_b()?;
        let column_type = d.read_bs()?;
        let auto_height = d.read_b()?;
        let column_width = d.read_bd()?;
        let column_gutter = d.read_bd()?;
        let column_flow_reversed = d.read_b()?;
        let n = read_count(r, MIN_BD, "column sizes")?;
        let column_sizes = (0..n).map(|_| r.data.read_bd()).collect::<Result<_>>()?;
        Ok(MLeaderText {
            label,
            normal,
            style,
            location,
            direction,
            rotation,
            boundary_width,
            boundary_height,
            line_spacing_factor,
            line_spacing_style,
            color,
            alignment,
            flow_direction,
            bg_color,
            bg_scale,
            bg_transparency,
            bg_fill,
            bg_mask_fill,
            column_type,
            auto_height,
            column_width,
            column_gutter,
            column_flow_reversed,
            column_sizes,
            word_break: r.data.read_b()?,
            unknown: r.data.read_b()?,
        })
    }

    fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        w.text(&self.label)?;
        w.data.write_3bd(self.normal);
        put(w, HARD_POINTER, self.style)?;
        let d = &mut w.data;
        d.write_3bd(self.location);
        d.write_3bd(self.direction);
        d.write_bd(self.rotation);
        d.write_bd(self.boundary_width);
        d.write_bd(self.boundary_height);
        d.write_bd(self.line_spacing_factor);
        d.write_bs(self.line_spacing_style);
        w.write_cmc(&self.color)?;
        w.data.write_bs(self.alignment);
        w.data.write_bs(self.flow_direction);
        w.write_cmc(&self.bg_color)?;
        let d = &mut w.data;
        d.write_bd(self.bg_scale);
        d.write_bl(self.bg_transparency);
        d.write_b(self.bg_fill);
        d.write_b(self.bg_mask_fill);
        d.write_bs(self.column_type);
        d.write_b(self.auto_height);
        d.write_bd(self.column_width);
        d.write_bd(self.column_gutter);
        d.write_b(self.column_flow_reversed);
        write_count(w, self.column_sizes.len(), "column sizes")?;
        for &s in &self.column_sizes {
            w.data.write_bd(s);
        }
        w.data.write_b(self.word_break);
        w.data.write_b(self.unknown);
        Ok(())
    }
}

impl MLeaderBlock {
    fn decode(r: &mut ObjReader) -> Result<Self> {
        let block = r.href_abs()?;
        let d = &mut r.data;
        let normal = d.read_3bd()?;
        let location = d.read_3bd()?;
        let scale = d.read_3bd()?;
        let rotation = d.read_bd()?;
        let color = r.read_cmc()?;
        let mut matrix = [0.0; 16];
        for m in &mut matrix {
            *m = r.data.read_bd()?;
        }
        Ok(MLeaderBlock {
            block,
            normal,
            location,
            scale,
            rotation,
            color,
            matrix,
        })
    }

    fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        put(w, SOFT_POINTER, self.block)?;
        w.data.write_3bd(self.normal);
        w.data.write_3bd(self.location);
        w.data.write_3bd(self.scale);
        w.data.write_bd(self.rotation);
        w.write_cmc(&self.color)?;
        for &m in &self.matrix {
            w.data.write_bd(m);
        }
        Ok(())
    }
}

impl MLeaderContext {
    fn decode(r: &mut ObjReader) -> Result<Self> {
        let n = read_count(r, MIN_LEADER_ROOT, "leader roots")?;
        let roots = (0..n)
            .map(|_| LeaderRoot::decode(r))
            .collect::<Result<_>>()?;
        let d = &mut r.data;
        let scale = d.read_bd()?;
        let content_base = d.read_3bd()?;
        let text_height = d.read_bd()?;
        let arrowhead_size = d.read_bd()?;
        let landing_gap = d.read_bd()?;
        let text_left_attachment = d.read_bs()?;
        let text_right_attachment = d.read_bs()?;
        let text_align = d.read_bs()?;
        let block_connection = d.read_bs()?;
        let content = if r.data.read_b()? {
            MLeaderContent::Text(Box::new(MLeaderText::decode(r)?))
        } else if r.data.read_b()? {
            MLeaderContent::Block(Box::new(MLeaderBlock::decode(r)?))
        } else {
            MLeaderContent::None
        };
        let d = &mut r.data;
        let base_point = d.read_3bd()?;
        let base_direction = d.read_3bd()?;
        let base_vertical = d.read_3bd()?;
        let normal_reversed = d.read_b()?;
        let (text_top_attachment, text_bottom_attachment) = if r.version >= Version::R2010 {
            (r.data.read_bs()?, r.data.read_bs()?)
        } else {
            (9, 9)
        };
        Ok(MLeaderContext {
            roots,
            scale,
            content_base,
            text_height,
            arrowhead_size,
            landing_gap,
            text_left_attachment,
            text_right_attachment,
            text_align,
            block_connection,
            content,
            base_point,
            base_direction,
            base_vertical,
            normal_reversed,
            text_top_attachment,
            text_bottom_attachment,
        })
    }

    fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        write_count(w, self.roots.len(), "leader roots")?;
        for root in &self.roots {
            root.encode(w)?;
        }
        let d = &mut w.data;
        d.write_bd(self.scale);
        d.write_3bd(self.content_base);
        d.write_bd(self.text_height);
        d.write_bd(self.arrowhead_size);
        d.write_bd(self.landing_gap);
        d.write_bs(self.text_left_attachment);
        d.write_bs(self.text_right_attachment);
        d.write_bs(self.text_align);
        d.write_bs(self.block_connection);
        match &self.content {
            MLeaderContent::Text(t) => {
                w.data.write_b(true);
                t.encode(w)?;
            }
            MLeaderContent::Block(b) => {
                w.data.write_b(false);
                w.data.write_b(true);
                b.encode(w)?;
            }
            MLeaderContent::None => {
                w.data.write_b(false);
                w.data.write_b(false);
            }
        }
        let d = &mut w.data;
        d.write_3bd(self.base_point);
        d.write_3bd(self.base_direction);
        d.write_3bd(self.base_vertical);
        d.write_b(self.normal_reversed);
        d.write_bs(self.text_top_attachment);
        d.write_bs(self.text_bottom_attachment);
        Ok(())
    }
}

impl MultiLeader {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let v = r.version;
        let version = if v >= Version::R2010 {
            r.data.read_bs()?
        } else {
            2
        };
        let context = MLeaderContext::decode(r)?;
        let context_kind = context.content.kind();
        let style = r.href_abs()?;
        let override_flags = r.data.read_bl()?;
        let leader_type = r.data.read_bs()?;
        let line_color = r.read_cmc()?;
        let line_type = r.href_abs()?;
        let d = &mut r.data;
        let line_weight = d.read_bl()?;
        let landing_enabled = d.read_b()?;
        let dogleg_enabled = d.read_b()?;
        let landing_distance = d.read_bd()?;
        let arrowhead = r.href_abs()?;
        let arrowhead_size = r.data.read_bd()?;
        let content_type = r.data.read_bs()?;
        let text_style = r.href_abs()?;
        let d = &mut r.data;
        let text_left_attachment = d.read_bs()?;
        let text_right_attachment = d.read_bs()?;
        let text_angle_type = d.read_bs()?;
        let text_alignment = d.read_bs()?;
        let text_color = r.read_cmc()?;
        let text_frame = r.data.read_b()?;
        let block = r.href_abs()?;
        let block_color = r.read_cmc()?;
        let d = &mut r.data;
        let block_scale = d.read_3bd()?;
        let block_rotation = d.read_bd()?;
        let block_connection = d.read_bs()?;
        let annotative = d.read_b()?;
        let arrowheads = if v <= Version::R2007 {
            let n = read_count(r, 1, "multileader arrowheads")?;
            check_handles(r, n, "multileader arrowheads")?;
            (0..n)
                .map(|_| Ok((r.data.read_b()?, r.href_abs()?)))
                .collect::<Result<_>>()?
        } else {
            Vec::new()
        };
        let n = read_count(r, MIN_BLOCK_LABEL, "multileader block labels")?;
        check_handles(r, n, "multileader block labels")?;
        let block_labels = (0..n)
            .map(|_| {
                Ok(MLeaderBlockLabel {
                    attdef: r.href_abs()?,
                    text: r.text()?,
                    ui_index: r.data.read_bs()?,
                    width: r.data.read_bd()?,
                })
            })
            .collect::<Result<_>>()?;
        let d = &mut r.data;
        let text_direction_negative = d.read_b()?;
        let ipe_align = d.read_bs()?;
        let justification = d.read_bs()?;
        let scale = d.read_bd()?;
        let (attachment_direction, text_bottom_attachment, text_top_attachment) =
            if v >= Version::R2010 {
                (d.read_bs()?, d.read_bs()?, d.read_bs()?)
            } else {
                (0, 9, 9)
            };
        let extended_to_text = if v >= Version::R2013 {
            d.read_b()?
        } else {
            false
        };
        Ok(MultiLeader {
            version,
            context,
            style,
            override_flags,
            leader_type,
            line_color,
            line_type,
            line_weight,
            landing_enabled,
            dogleg_enabled,
            landing_distance,
            arrowhead,
            arrowhead_size,
            content_type,
            text_style,
            text_left_attachment,
            text_right_attachment,
            text_angle_type,
            text_alignment,
            text_color,
            text_frame,
            block,
            block_color,
            block_scale,
            block_rotation,
            block_connection,
            annotative,
            arrowheads,
            block_labels,
            text_direction_negative,
            ipe_align,
            justification,
            scale,
            attachment_direction,
            text_bottom_attachment,
            text_top_attachment,
            extended_to_text,
            read_content: AsRead::new(context_kind),
        })
    }

    /// The content type written: [`MultiLeader::content_type`] while the
    /// kind of content is the one read; after the content kind changed,
    /// the type matching text (2) or block (1) content.
    pub fn written_content_type(&self) -> i16 {
        let kind = self.context.content.kind();
        if self.read_content.get() == Some(&kind) {
            return self.content_type;
        }
        match kind {
            ContentKind::Text => CONTENT_MTEXT,
            ContentKind::Block => CONTENT_BLOCK,
            ContentKind::None => self.content_type,
        }
    }

    /// Whether the multileader can be written as R2018: R2018 has no place
    /// for the arrowheads stored up to R2007.
    pub fn is_convertible(&self) -> bool {
        self.arrowheads.is_empty()
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        if !self.is_convertible() {
            return Err(encode_err(
                "a multileader's R2007 arrowhead list cannot be written as R2018",
            ));
        }
        w.data.write_bs(self.version);
        self.context.encode(w)?;
        put(w, HARD_POINTER, self.style)?;
        w.data.write_bl(self.override_flags);
        w.data.write_bs(self.leader_type);
        w.write_cmc(&self.line_color)?;
        put(w, HARD_POINTER, self.line_type)?;
        let d = &mut w.data;
        d.write_bl(self.line_weight);
        d.write_b(self.landing_enabled);
        d.write_b(self.dogleg_enabled);
        d.write_bd(self.landing_distance);
        put(w, HARD_POINTER, self.arrowhead)?;
        w.data.write_bd(self.arrowhead_size);
        w.data.write_bs(self.written_content_type());
        put(w, HARD_POINTER, self.text_style)?;
        let d = &mut w.data;
        d.write_bs(self.text_left_attachment);
        d.write_bs(self.text_right_attachment);
        d.write_bs(self.text_angle_type);
        d.write_bs(self.text_alignment);
        w.write_cmc(&self.text_color)?;
        w.data.write_b(self.text_frame);
        put(w, HARD_POINTER, self.block)?;
        w.write_cmc(&self.block_color)?;
        let d = &mut w.data;
        d.write_3bd(self.block_scale);
        d.write_bd(self.block_rotation);
        d.write_bs(self.block_connection);
        d.write_b(self.annotative);
        write_count(w, self.block_labels.len(), "multileader block labels")?;
        for l in &self.block_labels {
            put(w, SOFT_POINTER, l.attdef)?;
            w.text(&l.text)?;
            w.data.write_bs(l.ui_index);
            w.data.write_bd(l.width);
        }
        let d = &mut w.data;
        d.write_b(self.text_direction_negative);
        d.write_bs(self.ipe_align);
        d.write_bs(self.justification);
        d.write_bd(self.scale);
        d.write_bs(self.attachment_direction);
        d.write_bs(self.text_bottom_attachment);
        d.write_bs(self.text_top_attachment);
        d.write_b(self.extended_to_text);
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::tests::{reader, roundtrip};
    use super::*;
    use crate::bits::BitWriter;
    use crate::error::Error;
    use crate::handle::HandleRef;
    use crate::object::stream::tests::assemble;

    const P: [f64; 3] = [1.0, 2.0, 0.0];
    const Q: [f64; 3] = [4.0, 6.0, 0.0];
    const R: [f64; 3] = [-3.0, 0.5, 0.0];

    fn leader_line(index: i32) -> LeaderLine {
        LeaderLine {
            points: vec![P, Q],
            breaks: vec![LeaderBreak {
                segment: 1,
                pairs: vec![[P, Q], [Q, R]],
            }],
            index,
            leader_type: 2,
            color: Color::Rgb { r: 1, g: 2, b: 3 },
            line_type: Handle(0x14),
            line_weight: 25,
            arrow_size: 0.5,
            arrow: Handle(0x99),
            override_flags: 0x3F,
        }
    }

    fn text_content() -> MLeaderText {
        MLeaderText {
            label: "Note\\Ptwo".into(),
            normal: [0.0, 0.0, 1.0],
            style: Handle(0x11),
            location: P,
            direction: [1.0, 0.0, 0.0],
            rotation: 0.5,
            boundary_width: 10.0,
            boundary_height: 2.0,
            line_spacing_factor: 1.0,
            line_spacing_style: 1,
            color: Color::Index(3),
            alignment: 1,
            flow_direction: 5,
            bg_color: Color::Rgb {
                r: 200,
                g: 200,
                b: 200,
            },
            bg_scale: 1.5,
            bg_transparency: 7,
            bg_fill: true,
            bg_mask_fill: true,
            column_type: 2,
            auto_height: true,
            column_width: 4.0,
            column_gutter: 0.5,
            column_flow_reversed: true,
            column_sizes: vec![1.0, 2.0],
            word_break: true,
            unknown: false,
        }
    }

    pub(crate) fn multileader() -> MultiLeader {
        MultiLeader {
            version: 2,
            context: MLeaderContext {
                roots: vec![LeaderRoot {
                    content_valid: true,
                    unknown: true,
                    connection: Q,
                    direction: [1.0, 0.0, 0.0],
                    breaks: vec![[P, R]],
                    index: 0,
                    landing_distance: 0.36,
                    lines: vec![leader_line(0), leader_line(1)],
                    attachment_direction: 1,
                }],
                scale: 1.0,
                content_base: Q,
                text_height: 0.18,
                arrowhead_size: 0.18,
                landing_gap: 0.09,
                text_left_attachment: 1,
                text_right_attachment: 8,
                text_align: 0,
                block_connection: 0,
                content: MLeaderContent::Text(Box::new(text_content())),
                base_point: P,
                base_direction: [1.0, 0.0, 0.0],
                base_vertical: [0.0, 1.0, 0.0],
                normal_reversed: false,
                text_top_attachment: 9,
                text_bottom_attachment: 10,
            },
            style: Handle(0xD8),
            override_flags: 279552,
            leader_type: 1,
            line_color: Color::ByBlock,
            line_type: Handle(0x14),
            line_weight: -2,
            landing_enabled: true,
            dogleg_enabled: true,
            landing_distance: 0.36,
            arrowhead: Handle(0),
            arrowhead_size: 0.18,
            content_type: 2,
            text_style: Handle(0x11),
            text_left_attachment: 1,
            text_right_attachment: 1,
            text_angle_type: 1,
            text_alignment: 0,
            text_color: Color::ByBlock,
            text_frame: true,
            block: Handle(0),
            block_color: Color::ByLayer,
            block_scale: [1.0; 3],
            block_rotation: 0.0,
            block_connection: 0,
            annotative: true,
            arrowheads: vec![],
            block_labels: vec![MLeaderBlockLabel {
                attdef: Handle(0x300),
                text: "label".into(),
                ui_index: 0,
                width: 2.5,
            }],
            text_direction_negative: true,
            ipe_align: 0,
            justification: 1,
            scale: 1.0,
            attachment_direction: 1,
            text_bottom_attachment: 9,
            text_top_attachment: 10,
            extended_to_text: true,
            read_content: AsRead::default(),
        }
    }

    fn block_content() -> MLeaderBlock {
        MLeaderBlock {
            block: Handle(0x1F0),
            normal: [0.0, 0.0, 1.0],
            location: Q,
            scale: [2.0; 3],
            rotation: 0.25,
            color: Color::Index(5),
            matrix: std::array::from_fn(|i| i as f64),
        }
    }

    #[test]
    fn roundtrip_multileader() {
        let code = 500;
        let m = multileader();
        roundtrip(&m, MultiLeader::encode, MultiLeader::decode, code);
        let mut b = m.clone();
        b.context.content = MLeaderContent::Block(Box::new(block_content()));
        b.content_type = 1;
        roundtrip(&b, MultiLeader::encode, MultiLeader::decode, code);
        let mut n = m;
        n.context.content = MLeaderContent::None;
        n.context.roots.clear();
        n.block_labels.clear();
        roundtrip(&n, MultiLeader::encode, MultiLeader::decode, code);
    }

    /// The block content's block record is a soft pointer (ODS, ACadSharp),
    /// as is a block label's ATTDEF (ODS; ACadSharp writes a hard pointer).
    #[test]
    fn multileader_reference_codes() {
        let mut m = multileader();
        m.context.content = MLeaderContent::Block(Box::new(block_content()));
        m.content_type = 1;
        let mut w = ObjWriter::new(Handle(0x80));
        m.encode(&mut w).unwrap();
        let bytes = w.finish(500).unwrap();
        let mut r = reader(&bytes);
        let codes: Vec<_> = std::iter::from_fn(|| r.href().ok())
            .take_while(|h| h.code != 0xF)
            .map(|h| (h.code, h.value))
            .collect();
        assert_eq!(
            codes,
            [
                (5, 0x14), // line 1 linetype
                (5, 0x99), // line 1 arrow
                (5, 0x14),
                (5, 0x99),
                (4, 0x1F0), // the block content
                (5, 0xD8),  // style
                (5, 0x14),  // linetype
                (5, 0),     // arrowhead
                (5, 0x11),  // text style
                (5, 0),     // block
                (4, 0x300), // the label's ATTDEF
            ]
        );
    }

    /// Up to R2007 a multileader stores arrowheads, which R2018 cannot hold;
    /// a multileader with some cannot be written. Older files also lack the
    /// R2010+ fields, which read as AutoCAD's defaults.
    #[test]
    fn r2004_multileader_arrowheads() {
        let mut d = BitWriter::new();
        let mut h = BitWriter::new();
        let mut href = |value| {
            h.write_handle_ref(HandleRef { code: 5, value }).unwrap();
        };
        // Context: one root with one line of one point, no content.
        d.write_bl(1);
        d.write_b(true);
        d.write_b(true);
        d.write_3bd(Q);
        d.write_3bd([1.0, 0.0, 0.0]);
        d.write_bl(0); // root breaks
        d.write_bl(0); // index
        d.write_bd(0.36);
        d.write_bl(1); // lines
        d.write_bl(1);
        d.write_3bd(P);
        d.write_bl(0); // line breaks
        d.write_bl(0); // line index
        d.write_bd(1.0);
        d.write_3bd(Q);
        for v in [0.18, 0.18, 0.09] {
            d.write_bd(v);
        }
        for v in [1, 1, 0, 0] {
            d.write_bs(v);
        }
        d.write_b(false); // no text
        d.write_b(false); // no block
        for p in [P, [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
            d.write_3bd(p);
        }
        d.write_b(false);
        // The multileader.
        href(0xD8);
        d.write_bl(0);
        d.write_bs(1);
        d.write_cmc(&Color::ByBlock, Version::R2004).unwrap();
        href(0x14);
        d.write_bl(-2);
        d.write_b(true);
        d.write_b(true);
        d.write_bd(0.36);
        href(0);
        d.write_bd(0.18);
        d.write_bs(2);
        href(0x11);
        for v in [1, 1, 1, 0] {
            d.write_bs(v);
        }
        d.write_cmc(&Color::ByBlock, Version::R2004).unwrap();
        d.write_b(false);
        href(0);
        d.write_cmc(&Color::ByBlock, Version::R2004).unwrap();
        d.write_3bd([1.0; 3]);
        d.write_bd(0.0);
        d.write_bs(0);
        d.write_b(false);
        d.write_bl(2); // arrowheads
        d.write_b(true);
        href(0x40);
        d.write_b(false);
        href(0x41);
        d.write_bl(0); // labels
        d.write_b(false);
        d.write_bs(0);
        d.write_bs(1);
        d.write_bd(1.0);
        let bytes = assemble(Version::R2004, 500, &d, None, &h);
        let (mut r, _) = ObjReader::split(&bytes, Version::R2004, 30).unwrap();
        let m = MultiLeader::decode(&mut r).unwrap();
        assert_eq!(r.data.remaining_bits(), 0);
        assert_eq!(m.arrowheads, [(true, Handle(0x40)), (false, Handle(0x41))]);
        assert_eq!((m.version, m.attachment_direction), (2, 0));
        assert_eq!((m.text_top_attachment, m.text_bottom_attachment), (9, 9));
        assert!(!m.extended_to_text);
        let c = &m.context;
        assert_eq!((c.text_top_attachment, c.text_bottom_attachment), (9, 9));
        let line = &c.roots[0].lines[0];
        assert_eq!(line.points, [P]);
        assert_eq!(
            (
                line.leader_type,
                &line.color,
                line.line_weight,
                line.override_flags
            ),
            (1, &Color::ByBlock, -2, 0)
        );
        assert!(!m.is_convertible());
        assert!(m.encode(&mut ObjWriter::new(Handle(1))).is_err());
        // Without the arrowheads it converts.
        let m = MultiLeader {
            arrowheads: vec![],
            ..m
        };
        let mut w = ObjWriter::new(Handle(0x80));
        m.encode(&mut w).unwrap();
        let out = w.finish(500).unwrap();
        assert_eq!(MultiLeader::decode(&mut reader(&out)).unwrap(), m);
    }

    /// Counts read from a file are checked against the stream left before
    /// anything is allocated for them.
    #[test]
    fn multileader_counts_are_bounded() {
        // Leader roots, and a huge line count inside a root.
        for (roots, lines) in [(i32::MAX, 0), (1, i32::MAX)] {
            let mut d = BitWriter::new();
            d.write_bs(2);
            d.write_bl(roots);
            d.write_b(true);
            d.write_b(true);
            d.write_3bd(P);
            d.write_3bd(P);
            d.write_bl(0);
            d.write_bl(0);
            d.write_bd(0.0);
            d.write_bl(lines);
            let bytes = assemble(Version::R2018, 500, &d, None, &BitWriter::new());
            assert!(matches!(
                MultiLeader::decode(&mut reader(&bytes)),
                Err(Error::Truncated { .. })
            ));
        }
        // Block labels need their handles: a count that fits the data
        // stream but not the handle stream is refused before allocating.
        let mut m = multileader();
        m.block_labels = vec![
            MLeaderBlockLabel {
                attdef: Handle(1),
                text: String::new(),
                ui_index: 0,
                width: 0.0,
            };
            40
        ];
        let mut w = ObjWriter::new(Handle(0x80));
        m.encode(&mut w).unwrap();
        let bytes = w.finish(500).unwrap();
        let mut r = reader(&bytes);
        // Each label reference takes 2 bytes; 50 bytes fewer leave room for
        // fewer than 40 of the 1-byte minimum the check assumes.
        r.handles
            .limit_remaining(r.handles.remaining_bits() - 50 * 8);
        let e = MultiLeader::decode(&mut r);
        assert!(
            matches!(
                e,
                Err(Error::Truncated {
                    context: "multileader block labels"
                })
            ),
            "{e:?}"
        );
    }

    /// A multileader with R2007 arrowheads cannot be written as R2018: the
    /// whole object is reported unconvertible.
    #[test]
    fn r2007_arrowheads_are_unconvertible() {
        use crate::object::{Entity, Object, encode_object};
        use crate::section::classes::{DxfClass, ITEM_CLASS_ENTITY};
        let mut m = multileader();
        m.arrowheads = vec![(true, Handle(0x40))];
        let obj = Object::Entity {
            common: super::super::dim::tests::entity_common(),
            entity: Entity::MultiLeader(Box::new(m)),
        };
        let classes = [DxfClass {
            class_number: 500,
            proxy_flags: 0x401,
            app_name: "ObjectDBX Classes".into(),
            cpp_class_name: "AcDbMLeader".into(),
            dxf_name: "MULTILEADER".into(),
            was_zombie: false,
            item_class_id: ITEM_CLASS_ENTITY,
            instance_count: 0,
            dwg_version: 0,
            maintenance_version: 0,
            unknown: [0; 2],
        }];
        assert!(matches!(
            encode_object(&obj, &classes),
            Err(Error::UnconvertibleObjects(v)) if v == [(Handle(0x80), "MULTILEADER".to_owned())]
        ));
    }

    fn written_type(m: &MultiLeader) -> i16 {
        let mut w = ObjWriter::new(Handle(0x80));
        m.encode(&mut w).unwrap();
        let bytes = w.finish(500).unwrap();
        MultiLeader::decode(&mut reader(&bytes))
            .unwrap()
            .content_type
    }

    /// A multileader read with a content type that disagrees with its
    /// content is written as read while the kind of content is unchanged;
    /// once the content kind changes the type follows it. A multileader not
    /// read from a file gets the type of its content.
    #[test]
    fn content_type_follows_changed_content() {
        let mut m = multileader();
        m.content_type = 1;
        assert_eq!(written_type(&m), 2); // not read: derived from the text
        m.context.content = MLeaderContent::Block(Box::new(block_content()));
        m.content_type = 2;
        assert_eq!(written_type(&m), 1);
        m.context.content = MLeaderContent::None;
        for t in [0, 1, 2, 3] {
            m.content_type = t;
            assert_eq!(written_type(&m), t);
        }
        // Read with text content but type 3: written back as read.
        let mut m = multileader();
        m.content_type = 3;
        m.read_content = AsRead::new(ContentKind::Text);
        assert_eq!(written_type(&m), 3);
        // Text edited: still text, still as read.
        if let MLeaderContent::Text(t) = &mut m.context.content {
            t.label = "edited".into();
        }
        assert_eq!(written_type(&m), 3);
        // Changed to a block: the type follows.
        m.context.content = MLeaderContent::Block(Box::new(block_content()));
        assert_eq!(written_type(&m), 1);
        // A read-then-write cycle keeps a mismatched type.
        let mut m = multileader();
        m.content_type = 0;
        m.read_content = AsRead::new(ContentKind::Text);
        let mut w = ObjWriter::new(Handle(0x80));
        m.encode(&mut w).unwrap();
        let bytes = w.finish(500).unwrap();
        let back = MultiLeader::decode(&mut reader(&bytes)).unwrap();
        assert_eq!(back.content_type, 0);
        assert_eq!(written_type(&back), 0);
    }
}
