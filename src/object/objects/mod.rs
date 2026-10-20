//! Non-graphical objects other than the symbol tables (ODS 20.4.44-45,
//! 20.4.72-74, 20.4.79-104): dictionaries, extended records, layouts and
//! plot settings, groups, multiline, multileader and table styles, image
//! definitions and their reactors, raster and wipeout variables, scales,
//! dictionary variables, placeholders, sort tables, proxy objects and the
//! spatial and layer indexes and filters.
//!
//! Each type decodes and encodes only its type-specific data; the common data
//! is handled by [`super::decode_object`]. Fields a version does not store
//! are filled on read with the value AutoCAD writes when it saves the same
//! object in a later version (checked on the corpus where it has the type).
//!
//! Where the corpus or ACadSharp (MIT) differ from ODS, each pinned by a
//! test in `tests/objects.rs` (or, where the corpus has no example, a unit
//! test here):
//!
//! - A DICTIONARY's entries are soft owners even when the dictionary is a
//!   hard owner (DXF 280 = 1; the DXF export writes them as 360).
//! - A DICTIONARYVAR's value is a text field (ODS: `BS`).
//! - A LAYOUT's viewport count is a `BL` (ODS: `RL`), as ACadSharp
//!   `readLayout` reads it; R2000 layouts store no list, and the drawing
//!   fills it in from the layout's block (see `crate::drawing`).
//! - MLEADERSTYLE: the annotative bit (DXF 296) comes before the
//!   property-changed bit (295) (ODS: after); R2013+ adds a `B` (DXF 298),
//!   as ACadSharp `readMultiLeaderStyle` reads it. Before R2010 the
//!   ACAD_MLEADERVER extended data marks the new format (as ODS says; every
//!   corpus style has it).
//! - TABLESTYLE: in an R2000 file its colors are R2004 `CMC`s; a cell
//!   without a background has the color method 0xC8
//!   ([`crate::bits::Color::None`]). The R2010+ layout follows ODS
//!   20.4.101.2-4 as ACadSharp `readTableStyle`/`readCellStyle` read it. A
//!   pre-R2010 style is written in the R2010 form AutoCAD gives the same
//!   style (see [`LegacyTableStyle::to_r2010`]; the corpus's R2000, R2004
//!   and R2007 styles convert to exactly its R2010 one).
//! - MLINESTYLE: the pre-R2018 linetype index 32767 is BYLAYER (and 32766
//!   BYBLOCK, as ACadSharp `CadMLineStyleTemplate` maps them); the drawing
//!   resolves them to the LTYPE control's handles.
//! - WIPEOUTVARIABLES (not in ODS) is one `BS`, as ACadSharp
//!   `readWipeoutVariables` reads it.
//! - XRECORD item values are typed by the DXF group code ranges of
//!   ACadSharp `GroupCodeValue.TransformValue`; before R2007 their text is
//!   an `RS` length, an `RC` code page and the bytes (ODS: `RC` length).
//!
//! Not in the corpus, following ODS (hermetic tests only): PLOTSETTINGS,
//! ACAD_PROXY_OBJECT and LAYER_INDEX.

mod dict;
mod image;
mod layout;
mod misc;
mod render;
mod styles;
mod table_style;
mod xrecord;

pub use dict::{Dictionary, DictionaryVar, DictionaryWithDefault};
pub use image::{ImageDef, ImageDefReactor, RasterVariables, WipeoutVariables};
pub use layout::{Layout, PlotSettings};
pub use misc::{
    Group, LayerIndex, LayerIndexEntry, OpaqueBits, Scale, SortEntsTable, SpatialFilter,
    SpatialIndex,
};
pub use render::{
    DetailViewStyle, IDENTITY_4X4, Material, MaterialColor, MaterialMap, ModelDocViewStyle,
    SectionViewStyle, VISUAL_STYLE_KINDS, VisualStyle, VisualStyleValue,
};
pub use styles::{MLeaderStyle, MLineLinetype, MLineStyle, MLineStyleElement};
pub use table_style::{
    CellBorder, CellBorderData, CellStyle, CellStyleData, CellTableStyle, ContentFormat,
    LegacyTableStyle, NamedCellStyle, RowBorder, RowStyle, TableStyle,
};
pub use xrecord::{GroupCodeType, XRecord, XRecordValue, group_code_type};
pub(crate) use xrecord::{format_handle_text, handle_text_is_reference, parse_handle_text};

pub use super::entities::ProxyEntity as ProxyObject;

#[cfg(test)]
pub(crate) mod test_util {
    use crate::bits::Color;
    use crate::handle::Handle;

    pub(crate) const H: Handle = Handle(0x80);

    pub(crate) fn book() -> Color {
        Color::Book {
            rgb: 0x102030,
            color_name: "Grün".into(),
            book_name: "BOOK".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::dict::tests::dictionary;
    use super::layout::tests::{layout, plot_settings};
    use super::styles::tests::mleader_style;
    use super::table_style::tests::table_style;
    use super::test_util::H;
    use super::xrecord::tests::xrecord;
    use super::*;
    use crate::bits::Color;
    use crate::handle::Handle;
    use crate::version::Version;

    /// Whole objects (common data included) with flipped bits or cut short
    /// decode or fail, but never panic; intact ones decode equal.
    #[test]
    fn hostile_objects_never_panic() {
        use crate::object::{
            DecodeCtx, NonGraphical as N, Object, ObjectCommon, decode_object, encode_object,
        };
        use crate::section::classes::{DxfClass, ITEM_CLASS_OBJECT};
        let names = [
            "ACDBDICTIONARYWDFLT",
            "PLOTSETTINGS",
            "MLEADERSTYLE",
            "TABLESTYLE",
            "IMAGEDEF",
            "IMAGEDEF_REACTOR",
            "RASTERVARIABLES",
            "WIPEOUTVARIABLES",
            "SCALE",
            "DICTIONARYVAR",
            "SORTENTSTABLE",
            "SPATIAL_INDEX",
            "LAYER_INDEX",
            "SPATIAL_FILTER",
            "MATERIAL",
            "VISUALSTYLE",
            "ACDBSECTIONVIEWSTYLE",
            "ACDBDETAILVIEWSTYLE",
        ];
        let classes: Vec<DxfClass> = names
            .iter()
            .zip(500..)
            .map(|(name, n)| DxfClass {
                class_number: n,
                proxy_flags: 0,
                app_name: "ObjectDBX Classes".into(),
                cpp_class_name: "AcDbThing".into(),
                dxf_name: (*name).into(),
                was_zombie: false,
                item_class_id: ITEM_CLASS_OBJECT,
                instance_count: 0,
                dwg_version: 0,
                maintenance_version: 0,
                unknown: [0; 2],
            })
            .collect();
        let objects = [
            N::Dictionary(dictionary()),
            N::DictionaryWithDefault(DictionaryWithDefault {
                dictionary: dictionary(),
                default_entry: Handle(0xF),
            }),
            N::XRecord(xrecord()),
            N::Layout(Box::new(layout())),
            N::PlotSettings(Box::new(plot_settings())),
            N::Group(Group {
                name: "G".into(),
                unnamed: 1,
                selectable: 1,
                entities: vec![Handle(0x10)],
            }),
            N::MLeaderStyle(Box::new(mleader_style())),
            N::TableStyle(table_style()),
            N::ImageDefReactor(ImageDefReactor { class_version: 2 }),
            N::Scale(Scale::default()),
            N::DictionaryVar(DictionaryVar {
                schema: 0,
                value: "1:1".into(),
            }),
            N::Placeholder,
            N::SortEntsTable(SortEntsTable {
                block: Handle(0x1F),
                entries: vec![(Handle(0x35A), Handle(0x353))],
            }),
            N::ProxyObject(Box::new(crate::object::entities::misc::tests::proxy())),
            N::LayerIndex(LayerIndex {
                timestamps: [1, 2],
                entries: vec![LayerIndexEntry {
                    index: 3,
                    name: "0".into(),
                    handle: Handle(0x10),
                }],
            }),
            N::MLineStyle(MLineStyle {
                name: "STANDARD".into(),
                description: String::new(),
                flags: 0x112,
                fill_color: Color::Index(3),
                start_angle: 1.5,
                end_angle: 1.5,
                elements: vec![MLineStyleElement {
                    offset: 0.5,
                    color: Color::ByLayer,
                    linetype: MLineLinetype::Handle(Handle(0x15)),
                }],
            }),
            N::ImageDef(ImageDef {
                class_version: 0,
                size: [994.0, 965.0],
                path: "image.jpg".into(),
                loaded: true,
                resolution_units: 5,
                pixel_size: [0.25, 0.25],
            }),
            N::RasterVariables(RasterVariables {
                class_version: 0,
                display_frame: 1,
                display_quality: 1,
                units: 5,
            }),
            N::WipeoutVariables(WipeoutVariables { display_frame: 1 }),
            N::SpatialIndex(Box::new(SpatialIndex {
                timestamps: [7, 8],
                bits: OpaqueBits {
                    data: vec![0xA5, 0x80],
                    data_bits: 9,
                    strings: None,
                },
                handles: vec![],
                source_version: Version::R2018,
            })),
            N::SpatialFilter(SpatialFilter {
                bits: OpaqueBits {
                    data: vec![0x12, 0x34, 0x56],
                    data_bits: 23,
                    strings: None,
                },
                handles: vec![],
            }),
            N::Material(Box::new(render::tests::material())),
            N::VisualStyle(Box::new(render::tests::visual_style())),
            N::SectionViewStyle(Box::new(render::tests::section_view_style())),
            N::DetailViewStyle(Box::new(render::tests::detail_view_style())),
        ];
        for object in objects {
            let obj = Object::Other {
                common: ObjectCommon {
                    handle: H,
                    owner: Handle(1),
                    owner_relative: false,
                    reactors: vec![Handle(2)],
                    xdict: Some(Handle(3)),
                    xdata: vec![],
                    has_ds_binary_data: false,
                },
                object,
            };
            let good = encode_object(&obj, &classes).unwrap();
            let ctx = || DecodeCtx::new(Version::R2018, 30, &classes);
            let mut c = ctx();
            assert_eq!(decode_object(&good, H, &mut c).unwrap(), obj);
            assert!(c.diagnostics.is_empty(), "{:?}", c.diagnostics);
            for i in 0..good.len() {
                for bit in [0x01u8, 0x10, 0x80] {
                    let mut b = good.clone();
                    b[i] ^= bit;
                    let _ = decode_object(&b, H, &mut ctx());
                }
                let _ = decode_object(&good[..i], H, &mut ctx());
            }
        }
    }
}
