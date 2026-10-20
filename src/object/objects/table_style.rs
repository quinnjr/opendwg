//! Table styles (ODS 20.4.101): the pre-R2010 and R2010+ formats.

use crate::bits::Color;
use crate::error::Result;
use crate::error::encode_error as encode_err;
use crate::handle::Handle;
use crate::object::entities::dim::{MIN_BL, corrupt, read_count, write_count};
use crate::object::stream::{ObjReader, ObjWriter};
use crate::object::tables::{HARD_OWNER, HARD_POINTER, put};
use crate::version::Version;

fn read_bool_bs(r: &mut ObjReader, what: &str) -> Result<bool> {
    match r.data.read_bs()? {
        0 => Ok(false),
        1 => Ok(true),
        v => Err(corrupt(format!("{what} flag {v} is not 0 or 1"))),
    }
}

/// One border of a cell style (ODS 20.4.101.4).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct CellBorder {
    /// DXF 95: which edge (1 top, 2 right, 4 bottom, 8 left, 0x10 inside
    /// vertical, 0x20 inside horizontal). An edge of 0 stores no border
    /// data.
    pub edge: i32,
    pub data: Option<CellBorderData>,
}

/// The properties of a cell border.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CellBorderData {
    /// DXF 90.
    pub overrides: i32,
    /// DXF 91: 1 single, 2 double.
    pub border_type: i32,
    /// DXF 62.
    pub color: Color,
    /// DXF 92.
    pub line_weight: i32,
    /// DXF 340 (hard pointer).
    pub line_type: Handle,
    /// DXF 93: 1 invisible.
    pub invisible: i32,
    /// DXF 40.
    pub double_line_spacing: f64,
}

/// A content format (ODS 20.4.101.3).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct ContentFormat {
    /// DXF 90.
    pub property_overrides: i32,
    /// DXF 91.
    pub property_flags: i32,
    /// DXF 92, 93.
    pub value_data_type: i32,
    pub value_unit_type: i32,
    /// DXF 300.
    pub value_format: String,
    /// DXF 40.
    pub rotation: f64,
    /// DXF 140.
    pub block_scale: f64,
    /// DXF 94.
    pub alignment: i32,
    /// DXF 62.
    pub color: Color,
    /// DXF 340 (hard pointer).
    pub text_style: Handle,
    /// DXF 144.
    pub text_height: f64,
}

/// The data of a cell style that has data.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct CellStyleData {
    /// DXF 91.
    pub property_overrides: i32,
    /// DXF 92.
    pub merge_flags: i32,
    /// DXF 62.
    pub background_color: Color,
    /// DXF 93.
    pub content_layout: i32,
    pub content: ContentFormat,
    /// DXF 171 without bit 1, which is written as `margins.is_some()`.
    pub margin_flags: i16,
    /// Vertical, horizontal, bottom and right margins, then horizontal and
    /// vertical spacing.
    pub margins: Option<[f64; 6]>,
    pub borders: Vec<CellBorder>,
}

/// A cell style (ODS 20.4.101.4).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CellStyle {
    /// DXF 90: 1 cell, 2 row, 3 column, 4 formatted table data, 5 table.
    pub style_type: i32,
    /// `None` when the style stores no data (DXF 170 = 0).
    pub data: Option<CellStyleData>,
}

/// A cell style with its identification.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct NamedCellStyle {
    pub style: CellStyle,
    /// DXF 90: 1 title, 2 header, 3 data, 4 table, 101 up custom.
    pub id: i32,
    /// DXF 91: 1 data, 2 label.
    pub class: i32,
    /// DXF 300.
    pub name: String,
}

/// The pre-R2010 table style format's border (ODS 20.4.101.1).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct RowBorder {
    pub line_weight: i16,
    pub visible: bool,
    pub color: Color,
}

/// The pre-R2010 table style format's row style (ODS 20.4.101.1).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct RowStyle {
    /// Hard pointer.
    pub text_style: Handle,
    pub text_height: f64,
    pub alignment: i16,
    pub text_color: Color,
    pub fill_color: Color,
    pub fill_enabled: bool,
    /// Top, horizontal inside, bottom, left, vertical inside, right.
    pub borders: [RowBorder; 6],
    /// R2007+ (DXF 90, 91, 1); 512, 0 and empty before (what AutoCAD
    /// writes for the corpus's R2004 style saved as R2007).
    pub data_type: i32,
    pub unit_type: i32,
    pub format: String,
}

/// A table style in the format before R2010 (ODS 20.4.101.1).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct LegacyTableStyle {
    pub description: String,
    /// DXF 70: 0 down, 1 up.
    pub flow_direction: i16,
    /// DXF 71.
    pub flags: i16,
    pub horizontal_margin: f64,
    pub vertical_margin: f64,
    pub suppress_title: bool,
    pub suppress_header: bool,
    /// Data, title and header rows.
    pub rows: [RowStyle; 3],
}

/// A table style in the R2010+ format (ODS 20.4.101.2).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct CellTableStyle {
    /// ODS "Unknown" `RC`.
    pub unknown_rc: u8,
    pub description: String,
    /// ODS's two unknown `BL`s.
    pub unknown_bl: [i32; 2],
    /// ODS's unknown hard owner.
    pub unknown_handle: Handle,
    /// The cell style named "Table".
    pub table_style: NamedCellStyle,
    /// The other cell styles, each with the unknown `BL` before it (an index
    /// from 1, as ACadSharp (MIT) reads it).
    pub cell_styles: Vec<(i32, NamedCellStyle)>,
}

/// A table style (ODS 20.4.101, TABLESTYLE). Its format changed in R2010;
/// R2018 stores only [`TableStyle::Cells`].
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub enum TableStyle {
    /// Read before R2010. It is written as R2018 in the R2010 form
    /// [`LegacyTableStyle::to_r2010`] builds.
    Legacy(Box<LegacyTableStyle>),
    Cells(Box<CellTableStyle>),
}

/// Constants of the R2010 form that a pre-R2010 style has no field for,
/// with the values AutoCAD writes for the corpus's "Standard" style saved as
/// R2010 (`sample_AC1024.dwg`; the same in the R2013 and R2018 samples).
mod r2010_defaults {
    /// The cell style id after the last fixed one (custom ids start at 101).
    pub const NEXT_CUSTOM_ID: i32 = 101;
    /// Every converted cell style is of type 5 ("table").
    pub const STYLE_TYPE: i32 = 5;
    /// Content layout 1: flow.
    pub const CONTENT_LAYOUT: i32 = 1;
    pub const BLOCK_SCALE: f64 = 1.0;
    /// Border type 1: single.
    pub const BORDER_TYPE: i32 = 1;
    pub const DOUBLE_LINE_SPACING: f64 = 0.045;
    /// The title row's merge flag: merge all (ODS 20.4.101.3, 0x8000).
    pub const TITLE_MERGE: i32 = 0x8000;
    /// The "Table" cell style's content: top-left alignment, BYBLOCK color,
    /// no text style (its text height is the legacy data row's, see
    /// [`LegacyTableStyle::to_r2010`](super::LegacyTableStyle::to_r2010)).
    pub const TABLE_ALIGNMENT: i32 = 1;
    /// Flow direction bottom to top in the table cell style's merge flags
    /// (ODS 20.4.101.3: "Table properties: Flow direction bottom to top =
    /// 0x10000").
    pub const FLOW_UP: i32 = 0x10000;
}

impl LegacyTableStyle {
    /// The R2010 form of the style, as AutoCAD converts it: a "Table" cell
    /// style (id 4) and the title (1), header (2) and data (3) cell styles
    /// that pre-R2010 files keep in their CELLSTYLEMAP, built from the row
    /// styles.
    ///
    /// Mapped: the description; the flow direction (bit 0x10000 of the
    /// table cell style's merge flags); the horizontal and vertical margins
    /// (every cell style's vertical/bottom and horizontal/right margins); per
    /// row its text style, height, alignment and color, its fill color (the
    /// background, `Color::None` when the fill is off), its data type, unit
    /// type and format, and its six borders (line weight, color, visibility;
    /// legacy order top, inside horizontal, bottom, left, inside vertical,
    /// right becomes edges 1, 0x20, 4, 8, 0x10, 2 in edge order). The flags
    /// (DXF 71) go to the first unknown `BL` (0 for both in the corpus).
    /// The constants in `r2010_defaults` fill the rest; they are checked
    /// against the corpus only for its "Standard" style, so Task 26's
    /// oracle is to confirm them.
    ///
    /// The margin spacing (horizontal and vertical) and the "Table" cell
    /// style's text height are both 0.18 in the corpus's "Standard" style,
    /// which cannot tell them apart from the data row's text height (also
    /// 0.18). Both are taken from the data row's text height, the one
    /// legacy field they could plausibly follow; a style with another
    /// height is unverified until Task 26's oracle.
    ///
    /// The suppress-title and suppress-header flags have no known R2010
    /// field: a style with either set is an error (see
    /// [`TableStyle::is_convertible`]). A fill color of a row whose fill is
    /// off is not kept (it has no effect).
    pub fn to_r2010(&self) -> Result<CellTableStyle> {
        use r2010_defaults::*;
        if self.suppress_title || self.suppress_header {
            return Err(encode_err(
                "a pre-R2010 table style with a suppressed title or header has no known R2010 form",
            ));
        }
        let [data, title, header] = &self.rows;
        let spacing = data.text_height;
        let margins = || {
            Some([
                self.vertical_margin,
                self.horizontal_margin,
                self.vertical_margin,
                self.horizontal_margin,
                spacing,
                spacing,
            ])
        };
        let style = |content: ContentFormat, background: Color, merge: i32, borders| CellStyle {
            style_type: STYLE_TYPE,
            data: Some(CellStyleData {
                property_overrides: 0,
                merge_flags: merge,
                background_color: background,
                content_layout: CONTENT_LAYOUT,
                content,
                margin_flags: 0,
                margins: margins(),
                borders,
            }),
        };
        let row_style = |row: &RowStyle, merge: i32| {
            // Legacy border index for each edge, in edge order.
            let edges = [(1, 0), (2, 5), (4, 2), (8, 3), (0x10, 4), (0x20, 1)];
            let borders = edges
                .iter()
                .map(|&(edge, i)| {
                    let b = &row.borders[i];
                    CellBorder {
                        edge,
                        data: Some(CellBorderData {
                            overrides: 0,
                            border_type: BORDER_TYPE,
                            color: b.color.clone(),
                            line_weight: i32::from(b.line_weight),
                            line_type: Handle(0),
                            invisible: i32::from(!b.visible),
                            double_line_spacing: DOUBLE_LINE_SPACING,
                        }),
                    }
                })
                .collect();
            let content = ContentFormat {
                property_overrides: 0,
                property_flags: 0,
                value_data_type: row.data_type,
                value_unit_type: row.unit_type,
                value_format: row.format.clone(),
                rotation: 0.0,
                block_scale: BLOCK_SCALE,
                alignment: i32::from(row.alignment),
                color: row.text_color.clone(),
                text_style: row.text_style,
                text_height: row.text_height,
            };
            let background = if row.fill_enabled {
                row.fill_color.clone()
            } else {
                Color::None
            };
            style(content, background, merge, borders)
        };
        let named = |style, id, class, name: &str| NamedCellStyle {
            style,
            id,
            class,
            name: name.into(),
        };
        let table_content = ContentFormat {
            property_overrides: 0,
            property_flags: 0,
            value_data_type: data.data_type,
            value_unit_type: data.unit_type,
            value_format: data.format.clone(),
            rotation: 0.0,
            block_scale: BLOCK_SCALE,
            alignment: TABLE_ALIGNMENT,
            color: Color::ByBlock,
            text_style: Handle(0),
            text_height: data.text_height,
        };
        let flow = if self.flow_direction == 1 { FLOW_UP } else { 0 };
        Ok(CellTableStyle {
            unknown_rc: 0,
            description: self.description.clone(),
            unknown_bl: [i32::from(self.flags), NEXT_CUSTOM_ID],
            unknown_handle: Handle(0),
            table_style: named(
                style(table_content, Color::None, flow, Vec::new()),
                4,
                2,
                "Table",
            ),
            cell_styles: vec![
                (1, named(row_style(title, TITLE_MERGE), 1, 1, "_TITLE")),
                (2, named(row_style(header, 0), 2, 1, "_HEADER")),
                (3, named(row_style(data, 0), 3, 2, "_DATA")),
            ],
        })
    }
}

impl CellBorder {
    fn decode(r: &mut ObjReader) -> Result<Self> {
        let edge = r.data.read_bl()?;
        let data = if edge != 0 {
            Some(CellBorderData {
                overrides: r.data.read_bl()?,
                border_type: r.data.read_bl()?,
                color: r.read_cmc()?,
                line_weight: r.data.read_bl()?,
                line_type: r.href_abs()?,
                invisible: r.data.read_bl()?,
                double_line_spacing: r.data.read_bd()?,
            })
        } else {
            None
        };
        Ok(CellBorder { edge, data })
    }

    fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        w.data.write_bl(self.edge);
        match (&self.data, self.edge) {
            (None, 0) => Ok(()),
            (Some(b), e) if e != 0 => {
                w.data.write_bl(b.overrides);
                w.data.write_bl(b.border_type);
                w.write_cmc(&b.color)?;
                w.data.write_bl(b.line_weight);
                put(w, HARD_POINTER, b.line_type)?;
                w.data.write_bl(b.invisible);
                w.data.write_bd(b.double_line_spacing);
                Ok(())
            }
            _ => Err(encode_err(
                "a cell border has data exactly when its edge is not 0",
            )),
        }
    }
}

impl ContentFormat {
    fn decode(r: &mut ObjReader) -> Result<Self> {
        Ok(ContentFormat {
            property_overrides: r.data.read_bl()?,
            property_flags: r.data.read_bl()?,
            value_data_type: r.data.read_bl()?,
            value_unit_type: r.data.read_bl()?,
            value_format: r.text()?,
            rotation: r.data.read_bd()?,
            block_scale: r.data.read_bd()?,
            alignment: r.data.read_bl()?,
            color: r.read_cmc()?,
            text_style: r.href_abs()?,
            text_height: r.data.read_bd()?,
        })
    }

    fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        w.data.write_bl(self.property_overrides);
        w.data.write_bl(self.property_flags);
        w.data.write_bl(self.value_data_type);
        w.data.write_bl(self.value_unit_type);
        w.text(&self.value_format)?;
        w.data.write_bd(self.rotation);
        w.data.write_bd(self.block_scale);
        w.data.write_bl(self.alignment);
        w.write_cmc(&self.color)?;
        put(w, HARD_POINTER, self.text_style)?;
        w.data.write_bd(self.text_height);
        Ok(())
    }
}

/// The bit of the margin flags that says the margins are stored.
const MARGINS_PRESENT: i16 = 1;

impl CellStyle {
    fn decode(r: &mut ObjReader) -> Result<Self> {
        let style_type = r.data.read_bl()?;
        if !read_bool_bs(r, "cell style data")? {
            return Ok(CellStyle {
                style_type,
                data: None,
            });
        }
        let property_overrides = r.data.read_bl()?;
        let merge_flags = r.data.read_bl()?;
        let background_color = r.read_cmc()?;
        let content_layout = r.data.read_bl()?;
        let content = ContentFormat::decode(r)?;
        let flags = r.data.read_bs()?;
        let margins = if flags & MARGINS_PRESENT != 0 {
            let mut m = [0.0; 6];
            for x in &mut m {
                *x = r.data.read_bd()?;
            }
            Some(m)
        } else {
            None
        };
        let n = read_count(r, MIN_BL, "cell borders")?;
        let borders = (0..n)
            .map(|_| CellBorder::decode(r))
            .collect::<Result<_>>()?;
        Ok(CellStyle {
            style_type,
            data: Some(CellStyleData {
                property_overrides,
                merge_flags,
                background_color,
                content_layout,
                content,
                margin_flags: flags & !MARGINS_PRESENT,
                margins,
                borders,
            }),
        })
    }

    fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        w.data.write_bl(self.style_type);
        let Some(d) = &self.data else {
            w.data.write_bs(0);
            return Ok(());
        };
        w.data.write_bs(1);
        w.data.write_bl(d.property_overrides);
        w.data.write_bl(d.merge_flags);
        w.write_cmc(&d.background_color)?;
        w.data.write_bl(d.content_layout);
        d.content.encode(w)?;
        let present = if d.margins.is_some() {
            MARGINS_PRESENT
        } else {
            0
        };
        w.data
            .write_bs((d.margin_flags & !MARGINS_PRESENT) | present);
        for x in d.margins.iter().flatten() {
            w.data.write_bd(*x);
        }
        write_count(w, d.borders.len(), "cell borders")?;
        for b in &d.borders {
            b.encode(w)?;
        }
        Ok(())
    }
}

impl NamedCellStyle {
    fn decode(r: &mut ObjReader) -> Result<Self> {
        let style = CellStyle::decode(r)?;
        Ok(NamedCellStyle {
            style,
            id: r.data.read_bl()?,
            class: r.data.read_bl()?,
            name: r.text()?,
        })
    }

    fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        self.style.encode(w)?;
        w.data.write_bl(self.id);
        w.data.write_bl(self.class);
        w.text(&self.name)
    }
}

/// A table style color: R2004 `CMC` even in an R2000 file (TABLESTYLE is
/// newer than R2000; AutoCAD writes it there in its R2004 form).
fn read_style_cmc(r: &mut ObjReader) -> Result<Color> {
    let (rgb, flags) = r.data.read_cmc_head()?;
    let color_name = if flags & 1 != 0 {
        r.text()?
    } else {
        String::new()
    };
    let book_name = if flags & 2 != 0 {
        r.text()?
    } else {
        String::new()
    };
    crate::bits::cmc_from_parts(rgb, flags, color_name, book_name)
}

impl RowStyle {
    fn decode(r: &mut ObjReader) -> Result<Self> {
        let text_style = r.href_abs()?;
        let text_height = r.data.read_bd()?;
        let alignment = r.data.read_bs()?;
        let text_color = read_style_cmc(r)?;
        let fill_color = read_style_cmc(r)?;
        let fill_enabled = r.data.read_b()?;
        let border = |r: &mut ObjReader| -> Result<RowBorder> {
            Ok(RowBorder {
                line_weight: r.data.read_bs()?,
                visible: r.data.read_b()?,
                color: read_style_cmc(r)?,
            })
        };
        let borders = [
            border(r)?,
            border(r)?,
            border(r)?,
            border(r)?,
            border(r)?,
            border(r)?,
        ];
        let (data_type, unit_type, format) = if r.version >= Version::R2007 {
            (r.data.read_bl()?, r.data.read_bl()?, r.text()?)
        } else {
            (512, 0, String::new())
        };
        Ok(RowStyle {
            text_style,
            text_height,
            alignment,
            text_color,
            fill_color,
            fill_enabled,
            borders,
            data_type,
            unit_type,
            format,
        })
    }
}

impl TableStyle {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        if r.version < Version::R2010 {
            let description = r.text()?;
            let flow_direction = r.data.read_bs()?;
            let flags = r.data.read_bs()?;
            let horizontal_margin = r.data.read_bd()?;
            let vertical_margin = r.data.read_bd()?;
            let suppress_title = r.data.read_b()?;
            let suppress_header = r.data.read_b()?;
            let rows = [
                RowStyle::decode(r)?,
                RowStyle::decode(r)?,
                RowStyle::decode(r)?,
            ];
            return Ok(TableStyle::Legacy(Box::new(LegacyTableStyle {
                description,
                flow_direction,
                flags,
                horizontal_margin,
                vertical_margin,
                suppress_title,
                suppress_header,
                rows,
            })));
        }
        let unknown_rc = r.data.read_rc()?;
        let description = r.text()?;
        let unknown_bl = [r.data.read_bl()?, r.data.read_bl()?];
        let unknown_handle = r.href_abs()?;
        let table_style = NamedCellStyle::decode(r)?;
        let n = read_count(r, MIN_BL, "cell styles")?;
        let cell_styles = (0..n)
            .map(|_| Ok((r.data.read_bl()?, NamedCellStyle::decode(r)?)))
            .collect::<Result<_>>()?;
        Ok(TableStyle::Cells(Box::new(CellTableStyle {
            unknown_rc,
            description,
            unknown_bl,
            unknown_handle,
            table_style,
            cell_styles,
        })))
    }

    /// Whether the style can be written as R2018: in the R2010+ format, or
    /// a pre-R2010 style [`LegacyTableStyle::to_r2010`] converts.
    pub fn is_convertible(&self) -> bool {
        match self {
            TableStyle::Cells(_) => true,
            TableStyle::Legacy(l) => l.to_r2010().is_ok(),
        }
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        let converted;
        let s = match self {
            TableStyle::Cells(s) => s.as_ref(),
            TableStyle::Legacy(l) => {
                converted = l.to_r2010()?;
                &converted
            }
        };
        w.data.write_rc(s.unknown_rc);
        w.text(&s.description)?;
        w.data.write_bl(s.unknown_bl[0]);
        w.data.write_bl(s.unknown_bl[1]);
        put(w, HARD_OWNER, s.unknown_handle)?;
        s.table_style.encode(w)?;
        write_count(w, s.cell_styles.len(), "cell styles")?;
        for (index, style) in &s.cell_styles {
            w.data.write_bl(*index);
            style.encode(w)?;
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::bits::BitWriter;
    use crate::bits::Color;

    use crate::handle::{Handle, HandleRef};
    use crate::object::entities::tests::roundtrip;
    use crate::object::objects::test_util::{H, book};

    use crate::object::stream::tests::assemble;
    use crate::object::stream::{ObjReader, ObjWriter};

    use crate::version::Version;

    fn border(edge: i32) -> CellBorder {
        CellBorder {
            edge,
            data: (edge != 0).then(|| CellBorderData {
                overrides: 8,
                border_type: 2,
                color: book(),
                line_weight: -2,
                line_type: Handle(0x16),
                invisible: 1,
                double_line_spacing: 0.045,
            }),
        }
    }

    fn cell_style(id: i32, name: &str, data: bool) -> NamedCellStyle {
        NamedCellStyle {
            style: CellStyle {
                style_type: 5,
                data: data.then(|| CellStyleData {
                    property_overrides: 0x200,
                    merge_flags: 0x8000,
                    background_color: Color::None,
                    content_layout: 1,
                    content: ContentFormat {
                        property_overrides: 1,
                        property_flags: 0x100,
                        value_data_type: 512,
                        value_unit_type: 2,
                        value_format: "%lu2".into(),
                        rotation: 0.5,
                        block_scale: 1.0,
                        alignment: 5,
                        color: Color::ByBlock,
                        text_style: Handle(0x11),
                        text_height: 0.18,
                    },
                    margin_flags: 0x10,
                    margins: (id != 2).then_some([0.06, 0.06, 0.06, 0.06, 0.18, 0.18]),
                    borders: vec![border(1), border(0), border(32)],
                }),
            },
            id,
            class: 2,
            name: name.into(),
        }
    }

    pub(crate) fn table_style() -> TableStyle {
        TableStyle::Cells(Box::new(CellTableStyle {
            unknown_rc: 0,
            description: "Standard ü".into(),
            unknown_bl: [0, 101],
            unknown_handle: Handle(0x44),
            table_style: cell_style(4, "Table", true),
            cell_styles: vec![
                (1, cell_style(1, "_TITLE", true)),
                (2, cell_style(2, "_HEADER", true)),
                (3, cell_style(3, "_DATA", false)),
            ],
        }))
    }

    #[test]
    fn roundtrip_table_style() {
        roundtrip(&table_style(), TableStyle::encode, TableStyle::decode, 500);
        // A border's data goes with a non-zero edge.
        let mut bad = table_style();
        if let TableStyle::Cells(c) = &mut bad {
            if let Some(d) = &mut c.table_style.style.data {
                d.borders = vec![CellBorder {
                    edge: 0,
                    data: border(1).data,
                }];
            }
        }
        assert!(bad.encode(&mut ObjWriter::new(H)).is_err());
    }

    fn legacy_table_style() -> TableStyle {
        let row = RowStyle {
            text_style: Handle(0x11),
            text_height: 0.18,
            alignment: 2,
            text_color: Color::ByBlock,
            fill_color: Color::Index(7),
            fill_enabled: false,
            borders: std::array::from_fn(|_| RowBorder {
                line_weight: -2,
                visible: true,
                color: Color::ByBlock,
            }),
            data_type: 512,
            unit_type: 0,
            format: String::new(),
        };
        TableStyle::Legacy(Box::new(LegacyTableStyle {
            description: "Standard".into(),
            flow_direction: 0,
            flags: 0,
            horizontal_margin: 0.06,
            vertical_margin: 0.06,
            suppress_title: false,
            suppress_header: true,
            rows: [row.clone(), row.clone(), row],
        }))
    }

    /// A pre-R2010 table style is read in R2000 with R2004 colors; one with
    /// a suppressed header has no R2010 form and is not written as R2018.
    #[test]
    fn legacy_table_style_is_read() {
        let TableStyle::Legacy(want) = legacy_table_style() else {
            unreachable!()
        };
        for v in [Version::R2000, Version::R2004] {
            let mut d = BitWriter::new();
            let mut h = BitWriter::new();
            d.write_tv_raw(b"Standard").unwrap();
            d.write_bs(0);
            d.write_bs(0);
            d.write_bd(0.06);
            d.write_bd(0.06);
            d.write_b(false);
            d.write_b(true);
            for row in &want.rows {
                h.write_handle_ref(HandleRef {
                    code: 5,
                    value: row.text_style.0,
                })
                .unwrap();
                d.write_bd(row.text_height);
                d.write_bs(row.alignment);
                d.write_cmc(&row.text_color, Version::R2004).unwrap();
                d.write_cmc(&row.fill_color, Version::R2004).unwrap();
                d.write_b(row.fill_enabled);
                for b in &row.borders {
                    d.write_bs(b.line_weight);
                    d.write_b(b.visible);
                    d.write_cmc(&b.color, Version::R2004).unwrap();
                }
            }
            let bytes = assemble(v, 500, &d, None, &h);
            let (mut r, _) = ObjReader::split(&bytes, v, 30).unwrap();
            let t = TableStyle::decode(&mut r).unwrap();
            assert_eq!(r.data.remaining_bits(), 0, "{v:?}");
            assert_eq!(t, legacy_table_style(), "{v:?}");
            assert!(!t.is_convertible());
            assert!(t.encode(&mut ObjWriter::new(H)).is_err());
        }
    }

    /// A pre-R2010 style converts to the R2010 form: its fields go to the
    /// cell styles, and the encoded style decodes back as that form.
    #[test]
    fn legacy_table_style_converts() {
        let TableStyle::Legacy(mut l) = legacy_table_style() else {
            unreachable!()
        };
        l.suppress_header = false;
        l.flow_direction = 1;
        l.flags = 3;
        l.horizontal_margin = 0.25;
        l.vertical_margin = 0.5;
        let [data, title, header] = &mut l.rows;
        data.alignment = 7;
        data.format = "%lu2".into();
        title.fill_enabled = true;
        title.fill_color = Color::Rgb { r: 1, g: 2, b: 3 };
        title.text_color = Color::Index(1);
        header.text_style = Handle(0x99);
        header.borders[1].visible = false; // inside horizontal
        header.borders[5].line_weight = 30; // right
        let c = l.to_r2010().unwrap();
        assert_eq!(c.description, "Standard");
        assert_eq!(c.unknown_bl, [3, 101]);
        let table = c.table_style.style.data.as_ref().unwrap();
        assert_eq!(table.merge_flags, 0x10000);
        assert_eq!(table.margins, Some([0.5, 0.25, 0.5, 0.25, 0.18, 0.18]));
        let names: Vec<(i32, i32, i32, &str)> = c
            .cell_styles
            .iter()
            .map(|(i, s)| (*i, s.id, s.class, s.name.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                (1, 1, 1, "_TITLE"),
                (2, 2, 1, "_HEADER"),
                (3, 3, 2, "_DATA")
            ]
        );
        let data_of = |i: usize| c.cell_styles[i].1.style.data.as_ref().unwrap();
        let t = data_of(0);
        assert_eq!(t.merge_flags, 0x8000);
        assert_eq!(t.background_color, Color::Rgb { r: 1, g: 2, b: 3 });
        assert_eq!(t.content.color, Color::Index(1));
        let h = data_of(1);
        assert_eq!(h.background_color, Color::None);
        assert_eq!(h.content.text_style, Handle(0x99));
        let edge = |e: i32| {
            h.borders
                .iter()
                .find(|b| b.edge == e)
                .and_then(|b| b.data.clone())
                .unwrap()
        };
        assert_eq!(edge(0x20).invisible, 1);
        assert_eq!(edge(2).line_weight, 30);
        assert_eq!(edge(1).invisible, 0);
        let d = data_of(2);
        assert_eq!(
            (d.content.alignment, d.content.value_format.as_str()),
            (7, "%lu2")
        );
        // Written as R2018, it reads back as the converted form.
        let t = TableStyle::Legacy(l);
        assert!(t.is_convertible());
        let mut w = ObjWriter::new(H);
        t.encode(&mut w).unwrap();
        let bytes = w.finish(500).unwrap();
        let back = TableStyle::decode(&mut crate::object::entities::tests::reader(&bytes));
        assert_eq!(back.unwrap(), TableStyle::Cells(Box::new(c)));
    }

    /// The margin spacing and the "Table" cell style's text height follow
    /// the legacy data row's text height, not a constant.
    #[test]
    fn legacy_conversion_takes_spacing_and_table_height_from_the_data_row() {
        let TableStyle::Legacy(mut l) = legacy_table_style() else {
            unreachable!()
        };
        l.suppress_header = false;
        l.rows[0].text_height = 0.5; // data row
        l.rows[1].text_height = 0.7;
        l.rows[2].text_height = 0.9;
        let c = l.to_r2010().unwrap();
        let table = c.table_style.style.data.as_ref().unwrap();
        assert_eq!(table.content.text_height, 0.5);
        for s in std::iter::once(&c.table_style).chain(c.cell_styles.iter().map(|(_, s)| s)) {
            let margins = s.style.data.as_ref().unwrap().margins.unwrap();
            assert_eq!(margins[4..], [0.5, 0.5]);
        }
    }
}
