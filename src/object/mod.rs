//! Objects in the `AcDb:AcDbObjects` section (ODS chapter 20).
//!
//! [`stream`] splits one object into its data, string and handle streams;
//! [`common`] decodes the data every object and entity starts with; [`raw`]
//! keeps objects of unsupported types as bits. [`decode_object`] dispatches
//! on the object type: fixed type codes by number ([`type_code`]), and codes
//! from 500 up by the DXF name of their class. Typed decoders ([`tables`],
//! [`entities`], ...) are added per type; every other object decodes to
//! `Unknown`.
//!
//! # Non-canonical round trips
//!
//! An R2018 object encoded again after [`decode_object`] decodes to the same
//! value, and is the same bytes except where the source stored something in
//! a form the encoder does not produce:
//!
//! - text whose stored length counts a trailing NUL (some writers, such as
//!   ACadSharp, count a terminator): the NULs are dropped on read (see
//!   [`ObjReader::text`]; extended data strings likewise), so the text is
//!   written shorter;
//! - an R2004+ object whose "no extension dictionary" flag is clear but
//!   whose extension dictionary handle is null: read as no extension
//!   dictionary, written with the flag set and no handle;
//! - values stored in a longer form than needed (a `BS` 0 stored in full,
//!   as AutoCAD 2025 writes some dimension fields): written in the
//!   shortest form;
//! - the end padding of a typed object's handle stream (fewer than 8 bits):
//!   written as one bits, as AutoCAD pads. Raw objects keep theirs;
//! - the reference codes of the object's own handle and of an extended data
//!   application's handle (only their values are kept): written as 0 and 5
//!   (a hard pointer), as AutoCAD writes them.
//!
//! The corpus tests allow such objects (at most one in fifty of a type per
//! file) and check that they decode equal.

pub mod common;
pub mod entities;
pub mod objects;
pub mod raw;
pub(crate) mod remap;
pub mod stream;
pub mod tables;

use std::borrow::Cow;
use std::collections::HashMap;

pub use common::{
    AsRead, EntityCommon, LINEWEIGHT_BYLAYER, LinetypeRef, MaterialRef, ObjectCommon, PlotStyleRef,
    XDataApp, XDataItem, decode_entity_common, decode_object_common, encode_entity_common,
    encode_object_common,
};
pub use entities::{
    ATTDEF_MULTI_LINE, ATTRIB_MULTI_LINE, ATTRIB_SINGLE_LINE, AnnotativeData, Arc, AttDef, Attrib,
    Block, Circle, Ellipse, EmbeddedMText, Face3d, Insert, LWPOLYLINE_CLOSED, LWPOLYLINE_PLINEGEN,
    Line, LwPolyline, MInsert, MLine, MLineElement, MLineVertex, MTEXT_CONTEXT_VERSION, MText,
    MTextBackground, MTextColumns, MTextContext, Point, PolyfaceMesh, PolygonMesh, Polyline2d,
    Polyline3d, Ray, SPLINE_FLAGS1_CLOSED, SPLINE_FLAGS1_METHOD_FIT,
    SPLINE_FLAGS1_USE_KNOT_PARAMETER, SPLINE_KNOT_CHORD, SPLINE_KNOT_CUSTOM, Shape, Solid, Spline,
    SplineAsRead, SplineControl, SplineFit, SplineForm, Text, Trace, Vertex2d, Vertex3d,
    VertexMesh, VertexPface, VertexPfaceFace, Xline,
};
pub use entities::{
    AcisEntity, AcisGuid, AcisKind, AcisPayload, BoundaryPath, CLIP_RECTANGLE, GradientColor,
    Hatch, HatchEdge, HatchGradient, HatchPattern, HatchSpline, HatchVertex, Image, Ole2Frame,
    PATH_DERIVED, PATH_POLYLINE, PathEdges, PatternLine, PatternSettings, PolylinePath,
    ProxyEntity, Viewport, decrypt_sat,
};
pub use entities::{
    ContentKind, DimCommon, DimKind, Dimension, Leader, LeaderBreak, LeaderLine, LeaderRoot,
    MLeaderBlock, MLeaderBlockLabel, MLeaderContent, MLeaderContext, MLeaderText, MultiLeader,
    ORDINATE_X, Tolerance,
};
pub use objects::{
    CellBorder, CellBorderData, CellStyle, CellStyleData, CellTableStyle, ContentFormat,
    DetailViewStyle, Dictionary, DictionaryVar, DictionaryWithDefault, Group, GroupCodeType,
    IDENTITY_4X4, ImageDef, ImageDefReactor, LayerIndex, LayerIndexEntry, Layout, LegacyTableStyle,
    MLeaderStyle, MLineLinetype, MLineStyle, MLineStyleElement, Material, MaterialColor,
    MaterialMap, ModelDocViewStyle, NamedCellStyle, OpaqueBits, PlotSettings, ProxyObject,
    RasterVariables, RowBorder, RowStyle, Scale, SectionViewStyle, SortEntsTable, SpatialFilter,
    SpatialIndex, TableStyle, VISUAL_STYLE_KINDS, VisualStyle, VisualStyleValue, WipeoutVariables,
    XRecord, XRecordValue, group_code_type,
};
pub use raw::{HandleTail, RawObject, UnreadableObject};
pub use stream::{ObjReader, ObjWriter, ObjectPreamble};
pub use tables::{
    AppId, BlockRecord, DimStyle, EntityList, Layer, Linetype, TableControl, TableEntry, TableKind,
    TextStyle, Ucs, View, VpEntHdr, Vport,
};

use crate::diag::{Diagnostic, Severity};
use crate::error::{Error, Result};
use crate::handle::Handle;
use crate::section::classes::{DxfClass, FIRST_CLASS_NUMBER, ITEM_CLASS_ENTITY};
use crate::version::Version;
use entities::dim::DimType;

/// Fixed object type codes (ODS 20.3). Codes from 500 up are class numbers
/// (see [`crate::section::DxfClass`]).
#[allow(missing_docs)] // the ODS names
pub mod type_code {
    pub const TEXT: u16 = 1;
    pub const ATTRIB: u16 = 2;
    pub const ATTDEF: u16 = 3;
    pub const BLOCK: u16 = 4;
    pub const ENDBLK: u16 = 5;
    pub const SEQEND: u16 = 6;
    pub const INSERT: u16 = 7;
    pub const MINSERT: u16 = 8;
    pub const VERTEX_2D: u16 = 10;
    pub const VERTEX_3D: u16 = 11;
    pub const VERTEX_MESH: u16 = 12;
    pub const VERTEX_PFACE: u16 = 13;
    pub const VERTEX_PFACE_FACE: u16 = 14;
    pub const POLYLINE_2D: u16 = 15;
    pub const POLYLINE_3D: u16 = 16;
    pub const ARC: u16 = 17;
    pub const CIRCLE: u16 = 18;
    pub const LINE: u16 = 19;
    pub const DIM_ORDINATE: u16 = 20;
    pub const DIM_LINEAR: u16 = 21;
    pub const DIM_ALIGNED: u16 = 22;
    pub const DIM_ANG3PT: u16 = 23;
    pub const DIM_ANG2LN: u16 = 24;
    pub const DIM_RADIUS: u16 = 25;
    pub const DIM_DIAMETER: u16 = 26;
    pub const POINT: u16 = 27;
    pub const FACE3D: u16 = 28;
    pub const POLYLINE_PFACE: u16 = 29;
    pub const POLYLINE_MESH: u16 = 30;
    pub const SOLID: u16 = 31;
    pub const TRACE: u16 = 32;
    pub const SHAPE: u16 = 33;
    pub const VIEWPORT: u16 = 34;
    pub const ELLIPSE: u16 = 35;
    pub const SPLINE: u16 = 36;
    pub const REGION: u16 = 37;
    pub const SOLID3D: u16 = 38;
    pub const BODY: u16 = 39;
    pub const RAY: u16 = 40;
    pub const XLINE: u16 = 41;
    pub const DICTIONARY: u16 = 42;
    pub const OLEFRAME: u16 = 43;
    pub const MTEXT: u16 = 44;
    pub const LEADER: u16 = 45;
    pub const TOLERANCE: u16 = 46;
    pub const MLINE: u16 = 47;
    pub const BLOCK_CONTROL: u16 = 48;
    pub const BLOCK_HEADER: u16 = 49;
    pub const LAYER_CONTROL: u16 = 50;
    pub const LAYER: u16 = 51;
    pub const STYLE_CONTROL: u16 = 52;
    pub const STYLE: u16 = 53;
    pub const LTYPE_CONTROL: u16 = 56;
    pub const LTYPE: u16 = 57;
    pub const VIEW_CONTROL: u16 = 60;
    pub const VIEW: u16 = 61;
    pub const UCS_CONTROL: u16 = 62;
    pub const UCS: u16 = 63;
    pub const VPORT_CONTROL: u16 = 64;
    pub const VPORT: u16 = 65;
    pub const APPID_CONTROL: u16 = 66;
    pub const APPID: u16 = 67;
    pub const DIMSTYLE_CONTROL: u16 = 68;
    pub const DIMSTYLE: u16 = 69;
    pub const VP_ENT_HDR_CONTROL: u16 = 70;
    pub const VP_ENT_HDR: u16 = 71;
    pub const GROUP: u16 = 72;
    pub const MLINESTYLE: u16 = 73;
    pub const OLE2FRAME: u16 = 74;
    pub const LONG_TRANSACTION: u16 = 76;
    pub const LWPOLYLINE: u16 = 77;
    pub const HATCH: u16 = 78;
    pub const XRECORD: u16 = 79;
    /// `ACDBPLACEHOLDER` in ODS.
    pub const PLACEHOLDER: u16 = 80;
    pub const VBA_PROJECT: u16 = 81;
    pub const LAYOUT: u16 = 82;
    pub const ACAD_PROXY_ENTITY: u16 = 0x1F2;
    pub const ACAD_PROXY_OBJECT: u16 = 0x1F3;
}

/// The name and kind (`true` for an entity) of a fixed type code, or `None`
/// for codes ODS leaves unassigned (9, 0x36, 0x37, 0x3A, 0x3B, 0x4B, ...).
pub fn fixed_type(code: u16) -> Option<(&'static str, bool)> {
    use type_code::*;
    Some(match code {
        TEXT => ("TEXT", true),
        ATTRIB => ("ATTRIB", true),
        ATTDEF => ("ATTDEF", true),
        BLOCK => ("BLOCK", true),
        ENDBLK => ("ENDBLK", true),
        SEQEND => ("SEQEND", true),
        INSERT => ("INSERT", true),
        MINSERT => ("MINSERT", true),
        VERTEX_2D => ("VERTEX_2D", true),
        VERTEX_3D => ("VERTEX_3D", true),
        VERTEX_MESH => ("VERTEX_MESH", true),
        VERTEX_PFACE => ("VERTEX_PFACE", true),
        VERTEX_PFACE_FACE => ("VERTEX_PFACE_FACE", true),
        POLYLINE_2D => ("POLYLINE_2D", true),
        POLYLINE_3D => ("POLYLINE_3D", true),
        ARC => ("ARC", true),
        CIRCLE => ("CIRCLE", true),
        LINE => ("LINE", true),
        DIM_ORDINATE => ("DIMENSION_ORDINATE", true),
        DIM_LINEAR => ("DIMENSION_LINEAR", true),
        DIM_ALIGNED => ("DIMENSION_ALIGNED", true),
        DIM_ANG3PT => ("DIMENSION_ANG3PT", true),
        DIM_ANG2LN => ("DIMENSION_ANG2LN", true),
        DIM_RADIUS => ("DIMENSION_RADIUS", true),
        DIM_DIAMETER => ("DIMENSION_DIAMETER", true),
        POINT => ("POINT", true),
        FACE3D => ("3DFACE", true),
        POLYLINE_PFACE => ("POLYLINE_PFACE", true),
        POLYLINE_MESH => ("POLYLINE_MESH", true),
        SOLID => ("SOLID", true),
        TRACE => ("TRACE", true),
        SHAPE => ("SHAPE", true),
        VIEWPORT => ("VIEWPORT", true),
        ELLIPSE => ("ELLIPSE", true),
        SPLINE => ("SPLINE", true),
        REGION => ("REGION", true),
        SOLID3D => ("3DSOLID", true),
        BODY => ("BODY", true),
        RAY => ("RAY", true),
        XLINE => ("XLINE", true),
        DICTIONARY => ("DICTIONARY", false),
        OLEFRAME => ("OLEFRAME", true),
        MTEXT => ("MTEXT", true),
        LEADER => ("LEADER", true),
        TOLERANCE => ("TOLERANCE", true),
        MLINE => ("MLINE", true),
        BLOCK_CONTROL => ("BLOCK_CONTROL", false),
        BLOCK_HEADER => ("BLOCK_HEADER", false),
        LAYER_CONTROL => ("LAYER_CONTROL", false),
        LAYER => ("LAYER", false),
        STYLE_CONTROL => ("STYLE_CONTROL", false),
        STYLE => ("STYLE", false),
        LTYPE_CONTROL => ("LTYPE_CONTROL", false),
        LTYPE => ("LTYPE", false),
        VIEW_CONTROL => ("VIEW_CONTROL", false),
        VIEW => ("VIEW", false),
        UCS_CONTROL => ("UCS_CONTROL", false),
        UCS => ("UCS", false),
        VPORT_CONTROL => ("VPORT_CONTROL", false),
        VPORT => ("VPORT", false),
        APPID_CONTROL => ("APPID_CONTROL", false),
        APPID => ("APPID", false),
        DIMSTYLE_CONTROL => ("DIMSTYLE_CONTROL", false),
        DIMSTYLE => ("DIMSTYLE", false),
        VP_ENT_HDR_CONTROL => ("VP_ENT_HDR_CONTROL", false),
        VP_ENT_HDR => ("VP_ENT_HDR", false),
        GROUP => ("GROUP", false),
        MLINESTYLE => ("MLINESTYLE", false),
        OLE2FRAME => ("OLE2FRAME", true),
        LONG_TRANSACTION => ("LONG_TRANSACTION", false),
        LWPOLYLINE => ("LWPOLYLINE", true),
        HATCH => ("HATCH", true),
        XRECORD => ("XRECORD", false),
        PLACEHOLDER => ("ACDBPLACEHOLDER", false),
        VBA_PROJECT => ("VBA_PROJECT", false),
        LAYOUT => ("LAYOUT", false),
        ACAD_PROXY_ENTITY => ("ACAD_PROXY_ENTITY", true),
        ACAD_PROXY_OBJECT => ("ACAD_PROXY_OBJECT", false),
        _ => return None,
    })
}

/// An entity's type-specific data.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum Entity {
    /// An entity of a type the library does not decode, kept as bits.
    Unknown(RawObject),
    /// LINE.
    Line(Line),
    /// POINT.
    Point(Point),
    /// CIRCLE.
    Circle(Circle),
    /// ARC.
    Arc(Arc),
    /// ELLIPSE.
    Ellipse(Ellipse),
    /// RAY.
    Ray(Ray),
    /// XLINE.
    Xline(Xline),
    /// SOLID (a filled triangle or quadrilateral).
    Solid(Solid),
    /// TRACE.
    Trace(Trace),
    /// 3DFACE.
    Face3d(Face3d),
    /// SHAPE.
    Shape(Shape),
    /// BLOCK: the start of a block's entities.
    Block(Block),
    /// ENDBLK: the end of a block's entities.
    EndBlk,
    /// SEQEND: the end of an insert's attributes or a polyline's vertices.
    SeqEnd,
    /// INSERT: a block reference.
    Insert(Insert),
    /// MINSERT: a block reference in rows and columns.
    MInsert(MInsert),
    /// TEXT.
    Text(Text),
    /// ATTRIB: an insert's attribute.
    Attrib(Box<Attrib>),
    /// ATTDEF: an attribute definition in a block.
    AttDef(Box<AttDef>),
    /// MTEXT.
    MText(Box<MText>),
    /// LWPOLYLINE.
    LwPolyline(Box<LwPolyline>),
    /// POLYLINE (2D).
    Polyline2d(Polyline2d),
    /// POLYLINE (3D).
    Polyline3d(Polyline3d),
    /// POLYLINE (polyface mesh).
    PolyfaceMesh(PolyfaceMesh),
    /// POLYLINE (polygon mesh).
    PolygonMesh(PolygonMesh),
    /// VERTEX of a 2D polyline.
    Vertex2d(Vertex2d),
    /// VERTEX of a 3D polyline.
    Vertex3d(Vertex3d),
    /// VERTEX of a polygon mesh.
    VertexMesh(VertexMesh),
    /// VERTEX of a polyface mesh.
    VertexPface(VertexPface),
    /// VERTEX: a polyface mesh face.
    VertexPfaceFace(VertexPfaceFace),
    /// SPLINE.
    Spline(Box<Spline>),
    /// MLINE.
    MLine(Box<MLine>),
    /// Every dimension type, the class types ARC_DIMENSION and
    /// LARGE_RADIAL_DIMENSION included.
    Dimension(Box<Dimension>),
    /// LEADER.
    Leader(Box<Leader>),
    /// TOLERANCE.
    Tolerance(Tolerance),
    /// The class type MULTILEADER.
    MultiLeader(Box<MultiLeader>),
    /// HATCH.
    Hatch(Box<Hatch>),
    /// VIEWPORT.
    Viewport(Box<Viewport>),
    /// The class type IMAGE.
    Image(Box<Image>),
    /// The class type WIPEOUT.
    Wipeout(Box<Image>),
    /// REGION, 3DSOLID or BODY.
    Acis(Box<AcisEntity>),
    /// OLE2FRAME.
    Ole2Frame(Box<Ole2Frame>),
    /// ACAD_PROXY_ENTITY.
    ProxyEntity(Box<ProxyEntity>),
}

/// A non-graphical object's type-specific data.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum NonGraphical {
    /// An object of a type the library does not decode, kept as bits.
    Unknown(RawObject),
    /// Any of the ten table control objects.
    TableControl(TableControl),
    /// BLOCK_RECORD: a block (model and paper space included).
    BlockRecord(BlockRecord),
    /// LAYER.
    Layer(Layer),
    /// STYLE: a text style or shape file.
    TextStyle(TextStyle),
    /// LTYPE: a linetype.
    Linetype(Linetype),
    /// VIEW: a named view.
    View(Box<View>),
    /// UCS: a named coordinate system.
    Ucs(Ucs),
    /// VPORT: a tiled viewport configuration.
    Vport(Box<Vport>),
    /// APPID: a registered application.
    AppId(AppId),
    /// DIMSTYLE: a dimension style.
    DimStyle(Box<DimStyle>),
    /// VP_ENT_HDR (R2000 only).
    VpEntHdr(VpEntHdr),
    /// DICTIONARY.
    Dictionary(Dictionary),
    /// The class type ACDBDICTIONARYWDFLT.
    DictionaryWithDefault(DictionaryWithDefault),
    /// XRECORD: application data.
    XRecord(XRecord),
    /// LAYOUT: a layout tab.
    Layout(Box<Layout>),
    /// The class type PLOTSETTINGS.
    PlotSettings(Box<PlotSettings>),
    /// GROUP.
    Group(Group),
    /// MLINESTYLE.
    MLineStyle(MLineStyle),
    /// The class type MLEADERSTYLE.
    MLeaderStyle(Box<MLeaderStyle>),
    /// The class type TABLESTYLE.
    TableStyle(TableStyle),
    /// The class type IMAGEDEF.
    ImageDef(ImageDef),
    /// The class type IMAGEDEF_REACTOR.
    ImageDefReactor(ImageDefReactor),
    /// The class type RASTERVARIABLES.
    RasterVariables(RasterVariables),
    /// The class type WIPEOUTVARIABLES.
    WipeoutVariables(WipeoutVariables),
    /// The class type SCALE.
    Scale(Scale),
    /// The class type DICTIONARYVAR.
    DictionaryVar(DictionaryVar),
    /// ACDBPLACEHOLDER: no data of its own.
    Placeholder,
    /// The class type SORTENTSTABLE.
    SortEntsTable(SortEntsTable),
    /// ACAD_PROXY_OBJECT.
    ProxyObject(Box<ProxyObject>),
    /// The class type SPATIAL_INDEX.
    SpatialIndex(Box<SpatialIndex>),
    /// The class type LAYER_INDEX.
    LayerIndex(LayerIndex),
    /// The class type SPATIAL_FILTER.
    SpatialFilter(SpatialFilter),
    /// The class type MATERIAL (typed from R2007).
    Material(Box<Material>),
    /// The class type VISUALSTYLE (typed from R2013).
    VisualStyle(Box<VisualStyle>),
    /// The class type ACDBSECTIONVIEWSTYLE (typed from R2007).
    SectionViewStyle(Box<SectionViewStyle>),
    /// The class type ACDBDETAILVIEWSTYLE (typed from R2007).
    DetailViewStyle(Box<DetailViewStyle>),
    /// An object whose common data could not be read, kept as stored; it
    /// cannot be written (see [`UnreadableObject`]).
    Unreadable(Box<UnreadableObject>),
}

// The entities with constructors (`Line::new`, ...) convert into an
// [`Entity`], so `drawing.add_entity(block, common, Line::new(a, b).into())`.
macro_rules! entity_from {
    ($($ty:ident => $variant:ident($wrap:expr)),* $(,)?) => {$(
        impl From<$ty> for Entity {
            fn from(e: $ty) -> Entity {
                Entity::$variant($wrap(e))
            }
        }
    )*};
}

entity_from! {
    Line => Line(std::convert::identity),
    Circle => Circle(std::convert::identity),
    Text => Text(std::convert::identity),
    MText => MText(Box::new),
    Insert => Insert(std::convert::identity),
}

impl NonGraphical {
    /// How the object's type is identified in a file.
    fn type_key(&self) -> TypeKey<'_> {
        use type_code::*;
        TypeKey::Fixed(match self {
            NonGraphical::Unknown(r) => return raw_type_key(r),
            NonGraphical::Unreadable(u) => {
                return match (&u.class_name, u.type_code) {
                    (Some(name), _) => TypeKey::Class(name),
                    (None, Some(code)) => TypeKey::Fixed(code),
                    // Named so in the unconvertible list; never written.
                    (None, None) => TypeKey::Class("UNREADABLE"),
                };
            }
            NonGraphical::TableControl(c) => c.kind.control_code(),
            NonGraphical::BlockRecord(_) => BLOCK_HEADER,
            NonGraphical::Layer(_) => LAYER,
            NonGraphical::TextStyle(_) => STYLE,
            NonGraphical::Linetype(_) => LTYPE,
            NonGraphical::View(_) => VIEW,
            NonGraphical::Ucs(_) => UCS,
            NonGraphical::Vport(_) => VPORT,
            NonGraphical::AppId(_) => APPID,
            NonGraphical::DimStyle(_) => DIMSTYLE,
            NonGraphical::VpEntHdr(_) => VP_ENT_HDR,
            NonGraphical::Dictionary(_) => DICTIONARY,
            NonGraphical::XRecord(_) => XRECORD,
            NonGraphical::Layout(_) => LAYOUT,
            NonGraphical::Group(_) => GROUP,
            NonGraphical::MLineStyle(_) => MLINESTYLE,
            NonGraphical::Placeholder => PLACEHOLDER,
            NonGraphical::ProxyObject(_) => ACAD_PROXY_OBJECT,
            NonGraphical::DictionaryWithDefault(_) => {
                return TypeKey::Class("ACDBDICTIONARYWDFLT");
            }
            NonGraphical::PlotSettings(_) => return TypeKey::Class("PLOTSETTINGS"),
            NonGraphical::MLeaderStyle(_) => return TypeKey::Class("MLEADERSTYLE"),
            NonGraphical::TableStyle(_) => return TypeKey::Class("TABLESTYLE"),
            NonGraphical::ImageDef(_) => return TypeKey::Class("IMAGEDEF"),
            NonGraphical::ImageDefReactor(_) => return TypeKey::Class("IMAGEDEF_REACTOR"),
            NonGraphical::RasterVariables(_) => return TypeKey::Class("RASTERVARIABLES"),
            NonGraphical::WipeoutVariables(_) => return TypeKey::Class("WIPEOUTVARIABLES"),
            NonGraphical::Scale(_) => return TypeKey::Class("SCALE"),
            NonGraphical::Material(_) => return TypeKey::Class("MATERIAL"),
            NonGraphical::VisualStyle(_) => return TypeKey::Class("VISUALSTYLE"),
            NonGraphical::SectionViewStyle(_) => return TypeKey::Class("ACDBSECTIONVIEWSTYLE"),
            NonGraphical::DetailViewStyle(_) => return TypeKey::Class("ACDBDETAILVIEWSTYLE"),
            NonGraphical::DictionaryVar(_) => return TypeKey::Class("DICTIONARYVAR"),
            NonGraphical::SortEntsTable(_) => return TypeKey::Class("SORTENTSTABLE"),
            NonGraphical::SpatialIndex(_) => return TypeKey::Class("SPATIAL_INDEX"),
            NonGraphical::LayerIndex(_) => return TypeKey::Class("LAYER_INDEX"),
            NonGraphical::SpatialFilter(_) => return TypeKey::Class("SPATIAL_FILTER"),
        })
    }

    /// Whether the object can be written as R2018. Raw bits from R2000 and
    /// R2004 cannot, nor raw bits of a modeled type read from a version
    /// before the one whose layout of the type is R2018's (see
    /// [`raw_is_convertible`] and [`layout_stable_since`]: MATERIAL from
    /// R2007, VISUALSTYLE from R2013, other types R2018), but a VISUALSTYLE
    /// AutoCAD marked as saved in its earlier layout, which is copied (see
    /// [`Object::is_convertible`]); neither can an object whose common data
    /// could not be read ([`NonGraphical::Unreadable`]), an R2000 block
    /// record whose entity list has not been built (see [`EntityList`]), a
    /// table entry whose xref index R2018 cannot store (see [`TableEntry`]),
    /// a pre-R2010 table style with a suppressed title or header row (see
    /// [`TableStyle::is_convertible`]), a multiline style with
    /// a linetype index, spatial index bits read before R2007, or an XRECORD
    /// read before R2007 that keeps the bytes of an unknown item (see
    /// [`XRecord::is_convertible`]).
    fn is_convertible(&self) -> bool {
        if self.table_entry().is_some_and(|e| !e.is_convertible()) {
            return false;
        }
        match self {
            NonGraphical::Unknown(r) => raw_is_convertible(r, false),
            NonGraphical::Unreadable(_) => false,
            NonGraphical::BlockRecord(b) => matches!(b.entities, EntityList::Owned(_)),
            NonGraphical::TableStyle(t) => t.is_convertible(),
            NonGraphical::MLineStyle(m) => m.is_convertible(),
            NonGraphical::SpatialIndex(s) => s.is_convertible(),
            NonGraphical::XRecord(x) => x.is_convertible(),
            _ => true,
        }
    }

    /// The name and xref fields of a table entry.
    pub fn table_entry(&self) -> Option<&TableEntry> {
        Some(match self {
            NonGraphical::BlockRecord(x) => &x.entry,
            NonGraphical::Layer(x) => &x.entry,
            NonGraphical::TextStyle(x) => &x.entry,
            NonGraphical::Linetype(x) => &x.entry,
            NonGraphical::View(x) => &x.entry,
            NonGraphical::Ucs(x) => &x.entry,
            NonGraphical::Vport(x) => &x.entry,
            NonGraphical::AppId(x) => &x.entry,
            NonGraphical::DimStyle(x) => &x.entry,
            NonGraphical::VpEntHdr(x) => &x.entry,
            _ => return None,
        })
    }

    fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        match self {
            NonGraphical::Unknown(r) => write_raw(w, r),
            NonGraphical::Unreadable(_) => Err(Error::Encode(
                "an object whose common data could not be read cannot be written".into(),
            )),
            NonGraphical::TableControl(c) => c.encode(w),
            NonGraphical::BlockRecord(b) => b.encode(w),
            NonGraphical::Layer(l) => l.encode(w),
            NonGraphical::TextStyle(t) => t.encode(w),
            NonGraphical::Linetype(l) => l.encode(w),
            NonGraphical::View(v) => v.encode(w),
            NonGraphical::Ucs(u) => u.encode(w),
            NonGraphical::Vport(v) => v.encode(w),
            NonGraphical::AppId(a) => a.encode(w),
            NonGraphical::DimStyle(d) => d.encode(w),
            NonGraphical::VpEntHdr(v) => v.encode(w),
            NonGraphical::Dictionary(d) => d.encode(w),
            NonGraphical::DictionaryWithDefault(d) => d.encode(w),
            NonGraphical::XRecord(x) => x.encode(w),
            NonGraphical::Layout(l) => l.encode(w),
            NonGraphical::PlotSettings(p) => p.encode(w),
            NonGraphical::Group(g) => g.encode(w),
            NonGraphical::MLineStyle(m) => m.encode(w),
            NonGraphical::MLeaderStyle(m) => m.encode(w),
            NonGraphical::TableStyle(t) => t.encode(w),
            NonGraphical::ImageDef(i) => i.encode(w),
            NonGraphical::ImageDefReactor(i) => i.encode(w),
            NonGraphical::RasterVariables(v) => v.encode(w),
            NonGraphical::WipeoutVariables(v) => v.encode(w),
            NonGraphical::Scale(s) => s.encode(w),
            NonGraphical::DictionaryVar(v) => v.encode(w),
            NonGraphical::Placeholder => Ok(()),
            NonGraphical::SortEntsTable(s) => s.encode(w),
            NonGraphical::ProxyObject(p) => p.encode(w),
            NonGraphical::SpatialIndex(s) => s.encode(w),
            NonGraphical::LayerIndex(l) => l.encode(w),
            NonGraphical::SpatialFilter(s) => s.encode(w),
            NonGraphical::Material(m) => m.encode(w),
            NonGraphical::VisualStyle(v) => v.encode(w),
            NonGraphical::SectionViewStyle(v) => v.encode(w),
            NonGraphical::DetailViewStyle(v) => v.encode(w),
        }
    }
}

impl Entity {
    fn type_key(&self) -> TypeKey<'_> {
        use type_code::*;
        TypeKey::Fixed(match self {
            Entity::Unknown(r) => return raw_type_key(r),
            Entity::Line(_) => LINE,
            Entity::Point(_) => POINT,
            Entity::Circle(_) => CIRCLE,
            Entity::Arc(_) => ARC,
            Entity::Ellipse(_) => ELLIPSE,
            Entity::Ray(_) => RAY,
            Entity::Xline(_) => XLINE,
            Entity::Solid(_) => SOLID,
            Entity::Trace(_) => TRACE,
            Entity::Face3d(_) => FACE3D,
            Entity::Shape(_) => SHAPE,
            Entity::Block(_) => BLOCK,
            Entity::EndBlk => ENDBLK,
            Entity::SeqEnd => SEQEND,
            Entity::Insert(_) => INSERT,
            Entity::MInsert(_) => MINSERT,
            Entity::Text(_) => TEXT,
            Entity::Attrib(_) => ATTRIB,
            Entity::AttDef(_) => ATTDEF,
            Entity::MText(_) => MTEXT,
            Entity::LwPolyline(_) => LWPOLYLINE,
            Entity::Polyline2d(_) => POLYLINE_2D,
            Entity::Polyline3d(_) => POLYLINE_3D,
            Entity::PolyfaceMesh(_) => POLYLINE_PFACE,
            Entity::PolygonMesh(_) => POLYLINE_MESH,
            Entity::Vertex2d(_) => VERTEX_2D,
            Entity::Vertex3d(_) => VERTEX_3D,
            Entity::VertexMesh(_) => VERTEX_MESH,
            Entity::VertexPface(_) => VERTEX_PFACE,
            Entity::VertexPfaceFace(_) => VERTEX_PFACE_FACE,
            Entity::Spline(_) => SPLINE,
            Entity::MLine(_) => MLINE,
            Entity::Dimension(d) => match d.kind.type_key() {
                Ok(code) => code,
                Err(class) => return TypeKey::Class(class),
            },
            Entity::Leader(_) => LEADER,
            Entity::Tolerance(_) => TOLERANCE,
            Entity::MultiLeader(_) => return TypeKey::Class("MULTILEADER"),
            Entity::Hatch(_) => HATCH,
            Entity::Viewport(_) => VIEWPORT,
            Entity::Image(_) => return TypeKey::Class("IMAGE"),
            Entity::Wipeout(_) => return TypeKey::Class("WIPEOUT"),
            Entity::Acis(a) => a.kind.type_code(),
            Entity::Ole2Frame(_) => OLE2FRAME,
            Entity::ProxyEntity(_) => ACAD_PROXY_ENTITY,
        })
    }

    /// The vertices of a 2D, 3D, polyface mesh or polygon mesh polyline.
    pub fn polyline_vertices(&self) -> Option<&EntityList> {
        Some(match self {
            Entity::Polyline2d(p) => &p.vertices,
            Entity::Polyline3d(p) => &p.vertices,
            Entity::PolyfaceMesh(p) => &p.vertices,
            Entity::PolygonMesh(p) => &p.vertices,
            _ => return None,
        })
    }

    /// The SEQEND ending a polyline's vertices.
    pub fn polyline_seqend(&self) -> Option<Handle> {
        Some(match self {
            Entity::Polyline2d(p) => p.seqend,
            Entity::Polyline3d(p) => p.seqend,
            Entity::PolyfaceMesh(p) => p.seqend,
            Entity::PolygonMesh(p) => p.seqend,
            _ => return None,
        })
    }

    /// The vertices of a polyline, to change.
    pub fn polyline_vertices_mut(&mut self) -> Option<&mut EntityList> {
        Some(match self {
            Entity::Polyline2d(p) => &mut p.vertices,
            Entity::Polyline3d(p) => &mut p.vertices,
            Entity::PolyfaceMesh(p) => &mut p.vertices,
            Entity::PolygonMesh(p) => &mut p.vertices,
            _ => return None,
        })
    }

    /// Whether the entity can be written as R2018. Raw bits from R2000 and
    /// R2004 cannot, nor raw bits of a modeled type from before R2018 (see
    /// [`raw_is_convertible`]); neither can an R2000 insert or polyline
    /// whose attribute or vertex list has not been built (see
    /// [`EntityList`]), a multileader with the arrowhead list stored up to
    /// R2007 (see [`MultiLeader::is_convertible`]), nor an ACIS entity whose
    /// data is SAT text, read before R2013 from any version (version 1 SAT
    /// blocks, or inline data that is not SAB; see [`AcisEntity`]). Other
    /// ACIS entities read before R2013 are converted by the drawing writer,
    /// and fail to encode alone.
    fn is_convertible(&self) -> bool {
        if let Some(list) = self.polyline_vertices() {
            return matches!(list, EntityList::Owned(_));
        }
        match self {
            Entity::Unknown(r) => raw_is_convertible(r, true),
            Entity::MultiLeader(m) => m.is_convertible(),
            Entity::Acis(a) => a.is_convertible(),
            Entity::Insert(i) | Entity::MInsert(MInsert { insert: i, .. }) => {
                matches!(i.attribs, EntityList::Owned(_))
            }
            _ => true,
        }
    }

    fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        match self {
            Entity::Unknown(r) => write_raw(w, r),
            Entity::Line(l) => l.encode(w),
            Entity::Point(p) => p.encode(w),
            Entity::Circle(c) => c.encode(w),
            Entity::Arc(a) => a.encode(w),
            Entity::Ellipse(e) => e.encode(w),
            Entity::Ray(r) | Entity::Xline(r) => r.encode(w),
            Entity::Solid(s) | Entity::Trace(s) => s.encode(w),
            Entity::Face3d(f) => f.encode(w),
            Entity::Shape(s) => s.encode(w),
            Entity::Block(b) => b.encode(w),
            Entity::EndBlk | Entity::SeqEnd => Ok(()),
            Entity::Insert(i) => i.encode(w),
            Entity::MInsert(m) => m.encode(w),
            Entity::Text(t) => t.encode(w),
            Entity::Attrib(a) => a.encode(w),
            Entity::AttDef(a) => a.encode(w),
            Entity::MText(m) => m.encode(w),
            Entity::LwPolyline(p) => p.encode(w),
            Entity::Polyline2d(p) => p.encode(w),
            Entity::Polyline3d(p) => p.encode(w),
            Entity::PolyfaceMesh(p) => p.encode(w),
            Entity::PolygonMesh(p) => p.encode(w),
            Entity::Vertex2d(v) => v.encode(w),
            Entity::Vertex3d(v) | Entity::VertexMesh(v) | Entity::VertexPface(v) => v.encode(w),
            Entity::VertexPfaceFace(f) => f.encode(w),
            Entity::Spline(s) => s.encode(w),
            Entity::MLine(m) => m.encode(w),
            Entity::Dimension(d) => d.encode(w),
            Entity::Leader(l) => l.encode(w),
            Entity::Tolerance(t) => t.encode(w),
            Entity::MultiLeader(m) => m.encode(w),
            Entity::Hatch(h) => h.encode(w),
            Entity::Viewport(v) => v.encode(w),
            Entity::Image(i) | Entity::Wipeout(i) => i.encode(w),
            Entity::Acis(a) => a.encode(w),
            Entity::Ole2Frame(o) => o.encode(w),
            Entity::ProxyEntity(p) => p.encode(w),
        }
    }
}

/// One object of a drawing: its common data and its type-specific data.
// Entities are most of a drawing's objects, so boxing the larger variant
// would add an allocation to most objects rather than save memory.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum Object {
    /// An entity: drawn, owned by a block (or an insert or polyline).
    Entity {
        /// The common entity data (layer, color, ...).
        common: EntityCommon,
        /// The type-specific data.
        entity: Entity,
    },
    /// A non-graphical object (tables, dictionaries, styles, ...).
    Other {
        /// The common object data.
        common: ObjectCommon,
        /// The type-specific data.
        object: NonGraphical,
    },
}

impl Object {
    /// The handle stored in the object's common data.
    pub fn handle(&self) -> Handle {
        match self {
            Object::Entity { common, .. } => common.obj.handle,
            Object::Other { common, .. } => common.handle,
        }
    }

    /// The object's common data (an entity's without the entity part).
    pub fn object_common(&self) -> &ObjectCommon {
        match self {
            Object::Entity { common, .. } => &common.obj,
            Object::Other { common, .. } => common,
        }
    }

    /// The object's common data, to change in place (through
    /// [`crate::Drawing::get_mut`], whose caveats apply).
    pub fn object_common_mut(&mut self) -> &mut ObjectCommon {
        match self {
            Object::Entity { common, .. } => &mut common.obj,
            Object::Other { common, .. } => common,
        }
    }

    /// The raw data of an object kept as bits.
    pub fn raw(&self) -> Option<&RawObject> {
        match self {
            Object::Entity {
                entity: Entity::Unknown(r),
                ..
            }
            | Object::Other {
                object: NonGraphical::Unknown(r),
                ..
            } => Some(r),
            _ => None,
        }
    }

    fn type_key(&self) -> TypeKey<'_> {
        match self {
            Object::Entity { entity, .. } => entity.type_key(),
            Object::Other { object, .. } => object.type_key(),
        }
    }

    /// The DXF name of the class the object's type is, for class types
    /// (`None` for fixed types).
    pub(crate) fn class_name(&self) -> Option<&str> {
        match self.type_key() {
            TypeKey::Class(name) => Some(name),
            TypeKey::Fixed(_) => None,
        }
    }

    /// Whether the object can be written as R2018 (see
    /// [`NonGraphical::is_convertible`] and [`Entity::is_convertible`]);
    /// [`encode_object`] refuses it otherwise.
    pub(crate) fn is_convertible(&self) -> bool {
        match self {
            Object::Entity { entity, .. } => entity.is_convertible(),
            // A raw VISUALSTYLE saved in an earlier layout is copied as read
            // (see `saved_by_object_version`).
            Object::Other {
                common,
                object: NonGraphical::Unknown(r),
            } if r.class_name.as_deref() == Some("VISUALSTYLE")
                && saved_by_object_version(common) =>
            {
                r.is_convertible_to(ObjWriter::VERSION)
            }
            Object::Other { object, .. } => object.is_convertible(),
        }
    }

    /// The type name: the DXF class name for class types, the ODS name for
    /// fixed types, `TYPE_<code>` for an unassigned code.
    pub fn type_name(&self) -> Cow<'_, str> {
        key_name(self.type_key())
    }
}

/// The class types [`typed_entity`] decodes, sorted.
const MODELED_ENTITY_CLASSES: [&str; 9] = [
    "ACAD_PROXY_ENTITY",
    "ARC_DIMENSION",
    "HATCH",
    "IMAGE",
    "LARGE_RADIAL_DIMENSION",
    "LWPOLYLINE",
    "MULTILEADER",
    "OLE2FRAME",
    "WIPEOUT",
];

/// The class types [`typed_object`] decodes in R2018 files, sorted.
const MODELED_OBJECT_CLASSES: [&str; 26] = [
    "ACAD_PROXY_OBJECT",
    "ACDBDETAILVIEWSTYLE",
    "ACDBDICTIONARYWDFLT",
    "ACDBPLACEHOLDER",
    "ACDBSECTIONVIEWSTYLE",
    "DICTIONARY",
    "DICTIONARYVAR",
    "DICTIONARYWDFLT",
    "GROUP",
    "IMAGEDEF",
    "IMAGEDEF_REACTOR",
    "LAYER_INDEX",
    "LAYOUT",
    "MATERIAL",
    "MLEADERSTYLE",
    "MLINESTYLE",
    "PLOTSETTINGS",
    "RASTERVARIABLES",
    "SCALE",
    "SORTENTSTABLE",
    "SPATIAL_FILTER",
    "SPATIAL_INDEX",
    "TABLESTYLE",
    "VISUALSTYLE",
    "WIPEOUTVARIABLES",
    "XRECORD",
];

/// Whether the library decodes objects of type `key` (entities when
/// `entity`) into typed values in an R2018 file: whether a typed decoder
/// exists for it. Class types are looked up in [`MODELED_ENTITY_CLASSES`]
/// and [`MODELED_OBJECT_CLASSES`]; fixed types in a table made once by
/// asking the dispatchers. Both agree with the dispatchers (a test checks
/// every fixed code and class name against [`probe_modeled`]).
fn is_modeled(key: TypeKey, entity: bool) -> bool {
    use std::sync::OnceLock;
    static FIXED: OnceLock<Vec<[bool; 2]>> = OnceLock::new();
    match key {
        TypeKey::Class(name) => {
            let names: &[&str] = if entity {
                &MODELED_ENTITY_CLASSES
            } else {
                &MODELED_OBJECT_CLASSES
            };
            names.binary_search(&name).is_ok()
        }
        TypeKey::Fixed(code) => FIXED
            .get_or_init(|| {
                (0..FIRST_CLASS_NUMBER)
                    .map(|c| {
                        [false, true].map(|entity| probe_modeled(TypeKey::Fixed(c), entity))
                    })
                    .collect()
            })
            .get(usize::from(code))
            .is_some_and(|kinds| kinds[usize::from(entity)]),
    }
}

/// [`is_modeled`] asked of the dispatchers: they only report whether they
/// handle `key`; on no input they fail at once, and the result is dropped.
fn probe_modeled(key: TypeKey, entity: bool) -> bool {
    use crate::bits::BitReader;
    let mut r = ObjReader::from_streams(
        BitReader::new(&[]),
        None,
        BitReader::new(&[]),
        ObjWriter::VERSION,
        30,
    );
    if entity {
        typed_entity(key, &mut r).is_some()
    } else {
        typed_object(key, &mut r).is_some()
    }
}

/// Whether a raw object can be written as R2018. A raw object of a type the
/// library models (one whose typed decode failed) only when read from a
/// version whose layout of the type is R2018's ([`layout_stable_since`]):
/// otherwise its bits would be copied into the wrong layout. A raw object of
/// any other type follows [`RawObject::is_convertible_to`] (bits from R2007
/// up share the stream layout).
fn raw_is_convertible(r: &RawObject, entity: bool) -> bool {
    let name = r.class_name.as_deref();
    // A type the library models has a known R2018 layout: raw bits of it
    // from a version before that layout (read before the type is modeled,
    // or a typed decode that failed) would be read as R2018's and misread.
    // An R2007 VISUALSTYLE written by AutoCAD 2008 or the ODA SDK
    // (`adsk-cab-04`, `arx-12`, `cc-16` of the extended corpus) fails to
    // decode as R2018's; one AutoCAD marks as saved in its older layout is
    // copied (see `Object::is_convertible`).
    if is_modeled(raw_type_key(r), entity) {
        let stable = name
            .and_then(layout_stable_since)
            .unwrap_or(ObjWriter::VERSION);
        r.source_version >= stable && r.is_convertible_to(ObjWriter::VERSION)
    } else {
        r.is_convertible_to(ObjWriter::VERSION)
    }
}

/// The first version whose layout of class type `name` is R2018's, for the
/// modeled types whose layout the corpus shows unchanged before R2018: a
/// raw object of the type from that version on (a MATERIAL with a
/// procedural map, which the typed decoder does not read) is copied as read.
/// MATERIAL from R2007 (the R2007-R2013 samples' materials read as the
/// R2018 sample's), VISUALSTYLE from R2013 (the R2013 sample's styles
/// decode with R2018's layout). The section and detail view styles differ
/// before R2018 and other types are taken to differ: `None`.
fn layout_stable_since(name: &str) -> Option<Version> {
    Some(match name {
        "MATERIAL" => Version::R2007,
        "VISUALSTYLE" => Version::R2013,
        _ => return None,
    })
}

/// Whether `common`'s extended data names `AcDbSavedByObjectVersion`, as
/// AutoCAD marks an object it saved in an earlier version of its type's
/// layout (every VISUALSTYLE of the pre-R2018 corpus files has it, with the
/// value 0, and the R2007 and R2010 ones have fewer properties). Such a
/// VISUALSTYLE is kept raw and copied as read.
pub(crate) fn saved_by_object_version(common: &ObjectCommon) -> bool {
    common.xdata.iter().any(|app| {
        app.items
            .iter()
            .any(|i| matches!(i, XDataItem::String(s) if s == "AcDbSavedByObjectVersion"))
    })
}

/// The first version whose objects of class type `name` the library
/// decodes typed, for the types it models only from some version on (their
/// layout before is not known): MATERIAL and the section and detail view
/// styles from R2007 (the corpus's oldest), VISUALSTYLE from R2013 (R2010
/// stores fewer properties). Raw objects of these types from earlier
/// versions are kept raw, and written only from a version whose layout is
/// R2018's (see [`raw_is_convertible`]).
fn modeled_since(name: &str) -> Option<Version> {
    Some(match name {
        "MATERIAL" => Version::R2007,
        "VISUALSTYLE" => Version::R2013,
        "ACDBSECTIONVIEWSTYLE" | "ACDBDETAILVIEWSTYLE" => Version::R2007,
        _ => return None,
    })
}

fn raw_type_key(r: &RawObject) -> TypeKey<'_> {
    match &r.class_name {
        Some(name) => TypeKey::Class(name),
        None => TypeKey::Fixed(r.type_code),
    }
}

fn key_name(key: TypeKey<'_>) -> Cow<'_, str> {
    match key {
        TypeKey::Class(name) => Cow::Borrowed(name),
        TypeKey::Fixed(code) => match fixed_type(code) {
            Some((name, _)) => Cow::Borrowed(name),
            None => Cow::Owned(format!("TYPE_{code}")),
        },
    }
}

/// What [`decode_object`] needs besides the bytes. Problems that do not stop
/// decoding are appended to `diagnostics`.
#[derive(Debug)]
#[non_exhaustive]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct DecodeCtx<'a> {
    pub version: Version,
    pub codepage: u16,
    pub classes: &'a [DxfClass],
    pub diagnostics: Vec<Diagnostic>,
    /// The APPID named ACAD_MLEADERVER, if any: before R2010 a multileader
    /// style with extended data of that application is in the new format
    /// (ODS 20.4.87 "IsNewFormat"). [`crate::Drawing`] finds it before
    /// decoding the objects.
    pub mleader_style_marker: Option<Handle>,
    /// `classes` by number, built on first use (see
    /// [`DecodeCtx::class_index`]).
    class_index: std::cell::OnceCell<ClassIndex<'a>>,
}

impl<'a> DecodeCtx<'a> {
    /// A context for decoding objects of a `version` drawing whose strings use
    /// `codepage`, with no diagnostics and no MLEADERSTYLE marker.
    pub fn new(version: Version, codepage: u16, classes: &'a [DxfClass]) -> Self {
        DecodeCtx {
            version,
            codepage,
            classes,
            diagnostics: Vec::new(),
            mleader_style_marker: None,
            class_index: std::cell::OnceCell::new(),
        }
    }

    /// The classes indexed by number, so that finding an object's class
    /// takes constant time however the classes are numbered (a table whose
    /// numbers are not positional would otherwise make every lookup a
    /// scan). Rebuilt when `classes` was replaced since.
    fn class_index(&mut self) -> &ClassIndex<'a> {
        if self
            .class_index
            .get()
            .is_some_and(|i| !std::ptr::eq(i.classes, self.classes))
        {
            self.class_index = std::cell::OnceCell::new();
        }
        let classes = self.classes;
        self.class_index.get_or_init(|| ClassIndex::new(classes))
    }

    /// Set [`DecodeCtx::mleader_style_marker`]; [`mleader_style_marker`]
    /// computes it from a drawing's objects.
    #[must_use]
    pub fn with_mleader_style_marker(mut self, marker: Option<Handle>) -> Self {
        self.mleader_style_marker = marker;
        self
    }
}

/// The APPID named ACAD_MLEADERVER among `objects` (handle and object), the
/// value for [`DecodeCtx::mleader_style_marker`] when decoding the objects of
/// a drawing of `version` one at a time with [`decode_object`]. Only pre-R2010
/// drawings need it, so later versions give `None`.
/// [`crate::Drawing`] finds it itself.
pub fn mleader_style_marker<'a>(
    version: Version,
    objects: impl IntoIterator<Item = (Handle, &'a Object)>,
) -> Option<Handle> {
    if version >= Version::R2010 {
        return None;
    }
    objects.into_iter().find_map(|(h, o)| match o {
        Object::Other {
            object: NonGraphical::AppId(a),
            ..
        } if a.entry.name.eq_ignore_ascii_case("ACAD_MLEADERVER") => Some(h),
        _ => None,
    })
}

/// How a type is identified for dispatch: fixed types by number, class types
/// by DXF name (never by class number, which varies between drawings).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TypeKey<'a> {
    Fixed(u16),
    Class(&'a str),
}

/// The typed entity decoders; `None` keeps the entity raw. Fixed types
/// older files may store as class types (HATCH, OLE2FRAME,
/// ACAD_PROXY_ENTITY, LWPOLYLINE) are decoded by either key.
fn typed_entity(key: TypeKey, r: &mut ObjReader) -> Option<Result<Entity>> {
    use type_code::*;
    let dim =
        |r: &mut ObjReader, t| Dimension::decode(r, t).map(|d| Entity::Dimension(Box::new(d)));
    let acis = |r: &mut ObjReader, k| AcisEntity::decode(r, k).map(|a| Entity::Acis(Box::new(a)));
    let hatch = |r: &mut ObjReader| Hatch::decode(r).map(|h| Entity::Hatch(Box::new(h)));
    let ole2frame =
        |r: &mut ObjReader| Ole2Frame::decode(r).map(|o| Entity::Ole2Frame(Box::new(o)));
    let proxy =
        |r: &mut ObjReader| ProxyEntity::decode(r).map(|p| Entity::ProxyEntity(Box::new(p)));
    let lwpolyline =
        |r: &mut ObjReader| LwPolyline::decode(r).map(|p| Entity::LwPolyline(Box::new(p)));
    let code = match key {
        TypeKey::Fixed(code) => code,
        TypeKey::Class(name) => {
            return Some(match name {
                "ARC_DIMENSION" => dim(r, DimType::ArcLength),
                "LARGE_RADIAL_DIMENSION" => dim(r, DimType::LargeRadial),
                "MULTILEADER" => MultiLeader::decode(r).map(|m| Entity::MultiLeader(Box::new(m))),
                "IMAGE" => Image::decode(r).map(|i| Entity::Image(Box::new(i))),
                "WIPEOUT" => Image::decode(r).map(|i| Entity::Wipeout(Box::new(i))),
                "HATCH" => hatch(r),
                "OLE2FRAME" => ole2frame(r),
                "ACAD_PROXY_ENTITY" => proxy(r),
                "LWPOLYLINE" => lwpolyline(r),
                _ => return None,
            });
        }
    };
    Some(match code {
        LINE => Line::decode(r).map(Entity::Line),
        POINT => Point::decode(r).map(Entity::Point),
        CIRCLE => Circle::decode(r).map(Entity::Circle),
        ARC => Arc::decode(r).map(Entity::Arc),
        ELLIPSE => Ellipse::decode(r).map(Entity::Ellipse),
        RAY => Ray::decode(r).map(Entity::Ray),
        XLINE => Xline::decode(r).map(Entity::Xline),
        SOLID => Solid::decode(r).map(Entity::Solid),
        TRACE => Trace::decode(r).map(Entity::Trace),
        FACE3D => Face3d::decode(r).map(Entity::Face3d),
        SHAPE => Shape::decode(r).map(Entity::Shape),
        BLOCK => Block::decode(r).map(Entity::Block),
        ENDBLK => Ok(Entity::EndBlk),
        SEQEND => Ok(Entity::SeqEnd),
        INSERT => Insert::decode(r).map(Entity::Insert),
        MINSERT => MInsert::decode(r).map(Entity::MInsert),
        TEXT => Text::decode(r).map(Entity::Text),
        ATTRIB => Attrib::decode(r).map(|a| Entity::Attrib(Box::new(a))),
        ATTDEF => AttDef::decode(r).map(|a| Entity::AttDef(Box::new(a))),
        MTEXT => MText::decode(r).map(|m| Entity::MText(Box::new(m))),
        LWPOLYLINE => lwpolyline(r),
        POLYLINE_2D => Polyline2d::decode(r).map(Entity::Polyline2d),
        POLYLINE_3D => Polyline3d::decode(r).map(Entity::Polyline3d),
        POLYLINE_PFACE => PolyfaceMesh::decode(r).map(Entity::PolyfaceMesh),
        POLYLINE_MESH => PolygonMesh::decode(r).map(Entity::PolygonMesh),
        VERTEX_2D => Vertex2d::decode(r).map(Entity::Vertex2d),
        VERTEX_3D => Vertex3d::decode(r).map(Entity::Vertex3d),
        VERTEX_MESH => Vertex3d::decode(r).map(Entity::VertexMesh),
        VERTEX_PFACE => Vertex3d::decode(r).map(Entity::VertexPface),
        VERTEX_PFACE_FACE => VertexPfaceFace::decode(r).map(Entity::VertexPfaceFace),
        SPLINE => Spline::decode(r).map(|s| Entity::Spline(Box::new(s))),
        MLINE => MLine::decode(r).map(|m| Entity::MLine(Box::new(m))),
        DIM_ORDINATE => dim(r, DimType::Ordinate),
        DIM_LINEAR => dim(r, DimType::Linear),
        DIM_ALIGNED => dim(r, DimType::Aligned),
        DIM_ANG3PT => dim(r, DimType::Angular3Pt),
        DIM_ANG2LN => dim(r, DimType::Angular2Line),
        DIM_RADIUS => dim(r, DimType::Radius),
        DIM_DIAMETER => dim(r, DimType::Diameter),
        LEADER => Leader::decode(r).map(|l| Entity::Leader(Box::new(l))),
        TOLERANCE => Tolerance::decode(r).map(Entity::Tolerance),
        HATCH => hatch(r),
        VIEWPORT => Viewport::decode(r).map(|v| Entity::Viewport(Box::new(v))),
        REGION => acis(r, AcisKind::Region),
        SOLID3D => acis(r, AcisKind::Solid3d),
        BODY => acis(r, AcisKind::Body),
        OLE2FRAME => ole2frame(r),
        ACAD_PROXY_ENTITY => proxy(r),
        _ => return None,
    })
}

/// The typed non-graphical decoders; `None` keeps the object raw. The fixed
/// types also decoded by class name, as older files may store them (LAYOUT
/// and ACDBPLACEHOLDER are class types in R2000), are DICTIONARY, XRECORD,
/// LAYOUT, GROUP, MLINESTYLE, ACDBPLACEHOLDER and ACAD_PROXY_OBJECT; the
/// other names below are class types only.
fn typed_object(key: TypeKey, r: &mut ObjReader) -> Option<Result<NonGraphical>> {
    use type_code::*;
    let code = match key {
        TypeKey::Fixed(code) => code,
        TypeKey::Class(name) => {
            use NonGraphical as N;
            return Some(match name {
                "DICTIONARY" => Dictionary::decode(r).map(N::Dictionary),
                "ACDBDICTIONARYWDFLT" | "DICTIONARYWDFLT" => {
                    DictionaryWithDefault::decode(r).map(N::DictionaryWithDefault)
                }
                "XRECORD" => XRecord::decode(r).map(N::XRecord),
                "LAYOUT" => Layout::decode(r).map(|l| N::Layout(Box::new(l))),
                "PLOTSETTINGS" => PlotSettings::decode(r).map(|p| N::PlotSettings(Box::new(p))),
                "GROUP" => Group::decode(r).map(N::Group),
                "MLINESTYLE" => MLineStyle::decode(r).map(N::MLineStyle),
                "MLEADERSTYLE" => MLeaderStyle::decode(r).map(|m| N::MLeaderStyle(Box::new(m))),
                "TABLESTYLE" => TableStyle::decode(r).map(N::TableStyle),
                "IMAGEDEF" => ImageDef::decode(r).map(N::ImageDef),
                "IMAGEDEF_REACTOR" => ImageDefReactor::decode(r).map(N::ImageDefReactor),
                "RASTERVARIABLES" => RasterVariables::decode(r).map(N::RasterVariables),
                "WIPEOUTVARIABLES" => WipeoutVariables::decode(r).map(N::WipeoutVariables),
                "SCALE" => Scale::decode(r).map(N::Scale),
                "DICTIONARYVAR" => DictionaryVar::decode(r).map(N::DictionaryVar),
                "ACDBPLACEHOLDER" => Ok(N::Placeholder),
                "SORTENTSTABLE" => SortEntsTable::decode(r).map(N::SortEntsTable),
                "ACAD_PROXY_OBJECT" => ProxyObject::decode(r).map(|p| N::ProxyObject(Box::new(p))),
                "SPATIAL_INDEX" => SpatialIndex::decode(r).map(|s| N::SpatialIndex(Box::new(s))),
                "LAYER_INDEX" => LayerIndex::decode(r).map(N::LayerIndex),
                "SPATIAL_FILTER" => SpatialFilter::decode(r).map(N::SpatialFilter),
                n if modeled_since(n).is_some_and(|v| r.version < v) => return None,
                "VISUALSTYLE" if r.saved_by_object_version => return None,
                "MATERIAL" => Material::decode(r).map(|m| N::Material(Box::new(m))),
                "VISUALSTYLE" => VisualStyle::decode(r).map(|v| N::VisualStyle(Box::new(v))),
                "ACDBSECTIONVIEWSTYLE" => {
                    SectionViewStyle::decode(r).map(|v| N::SectionViewStyle(Box::new(v)))
                }
                "ACDBDETAILVIEWSTYLE" => {
                    DetailViewStyle::decode(r).map(|v| N::DetailViewStyle(Box::new(v)))
                }
                _ => return None,
            });
        }
    };
    if let Some(kind) = TableKind::from_control_code(code) {
        return Some(TableControl::decode(r, kind).map(NonGraphical::TableControl));
    }
    Some(match code {
        BLOCK_HEADER => BlockRecord::decode(r).map(NonGraphical::BlockRecord),
        LAYER => Layer::decode(r).map(NonGraphical::Layer),
        STYLE => TextStyle::decode(r).map(NonGraphical::TextStyle),
        LTYPE => Linetype::decode(r).map(NonGraphical::Linetype),
        VIEW => View::decode(r).map(|v| NonGraphical::View(Box::new(v))),
        UCS => Ucs::decode(r).map(NonGraphical::Ucs),
        VPORT => Vport::decode(r).map(|v| NonGraphical::Vport(Box::new(v))),
        APPID => AppId::decode(r).map(NonGraphical::AppId),
        DIMSTYLE => DimStyle::decode(r).map(|d| NonGraphical::DimStyle(Box::new(d))),
        VP_ENT_HDR => VpEntHdr::decode(r).map(NonGraphical::VpEntHdr),
        DICTIONARY => Dictionary::decode(r).map(NonGraphical::Dictionary),
        XRECORD => XRecord::decode(r).map(NonGraphical::XRecord),
        LAYOUT => Layout::decode(r).map(|l| NonGraphical::Layout(Box::new(l))),
        GROUP => Group::decode(r).map(NonGraphical::Group),
        MLINESTYLE => MLineStyle::decode(r).map(NonGraphical::MLineStyle),
        PLACEHOLDER => Ok(NonGraphical::Placeholder),
        ACAD_PROXY_OBJECT => ProxyObject::decode(r).map(|p| NonGraphical::ProxyObject(Box::new(p))),
        _ => return None,
    })
}

/// Bits a typed decode left unread: data and string stream bits, and handle
/// stream bits beyond the end padding (fewer than 8 bits, as no reference is
/// shorter than a byte).
fn unread_bits(r: &ObjReader) -> Option<String> {
    let data = r.data.remaining_bits();
    let strings = r.strings.as_ref().map_or(0, |s| s.remaining_bits());
    let handles = r.handles.remaining_bits();
    (data != 0 || strings != 0 || handles >= 8)
        .then(|| format!("{data} data, {strings} string and {handles} handle stream bits unread"))
}

/// Run a typed decoder on a copy of `r`. On success the copy replaces `r`;
/// on failure, or when the decoder leaves bits unread (they would be lost on
/// write), `r` is left after the common data and a warning is recorded, so
/// the caller keeps the object raw.
fn try_typed<T>(
    r: &mut ObjReader,
    key: TypeKey,
    type_name: impl FnOnce() -> String,
    decode: fn(TypeKey, &mut ObjReader) -> Option<Result<T>>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<T> {
    let mut attempt = r.clone();
    let failure = match decode(key, &mut attempt)? {
        Ok(v) => match unread_bits(&attempt) {
            None => {
                *r = attempt;
                return Some(v);
            }
            Some(unread) => unread,
        },
        Err(e) => e.to_string(),
    };
    diagnostics.push(Diagnostic {
        severity: Severity::Warning,
        handle: Some(r.handle),
        message: format!("{} decode failed: {failure}; kept raw", type_name()),
    });
    None
}

/// Decode the object starting at `bytes[0]` (the slice may run past it).
/// `handle` is the object map's handle for it; a different handle in the
/// object, a CRC mismatch, an unassigned fixed type or a class type with no
/// class adds a warning. An error means the object's common data could not
/// be read; a failed typed decode keeps it raw.
///
/// An object of a class type with no class in `classes` is read as a
/// non-entity: its kind is unknown. Its bits are written back unchanged only
/// if it really is a non-entity; an entity's common data would be misread.
pub fn decode_object(bytes: &[u8], handle: Handle, ctx: &mut DecodeCtx) -> Result<Object> {
    let (mut r, pre) = ObjReader::split(bytes, ctx.version, ctx.codepage)?;
    r.handle = handle;
    let code = pre.type_code;
    let warn = |diagnostics: &mut Vec<Diagnostic>, message: String| {
        diagnostics.push(Diagnostic {
            severity: Severity::Warning,
            handle: Some(handle),
            message,
        })
    };
    if !r.crc_ok() {
        warn(&mut ctx.diagnostics, "object CRC mismatch".into());
    }
    let class = if code >= FIRST_CLASS_NUMBER {
        ctx.class_index().by_number(code)
    } else {
        None
    };
    let (key, class_name, is_entity) = if let Some(class) = class {
        (
            TypeKey::Class(&class.dxf_name),
            Some(class.dxf_name.clone()),
            class.item_class_id == ITEM_CLASS_ENTITY,
        )
    } else if code >= FIRST_CLASS_NUMBER {
        // Kept raw. Without the class its kind is unknown; the common data of
        // a non-entity is the shorter guess, and is right for non-entities
        // only.
        warn(
            &mut ctx.diagnostics,
            format!("object type {code} has no class; read as a non-entity and kept raw"),
        );
        (TypeKey::Fixed(code), None, false)
    } else {
        let is_entity = match fixed_type(code) {
            Some((_, e)) => e,
            None => {
                warn(
                    &mut ctx.diagnostics,
                    format!("object type {code} is unassigned; read as a non-entity"),
                );
                false
            }
        };
        (TypeKey::Fixed(code), None, is_entity)
    };
    // The type's name, for a failed typed decode's warning only: made then.
    let name = || key_name(key).into_owned();
    let obj = if is_entity {
        let common = decode_entity_common(&mut r)?;
        let entity = match try_typed(&mut r, key, name, typed_entity, &mut ctx.diagnostics) {
            Some(e) => e,
            None => Entity::Unknown(RawObject::decode(&mut r, code, class_name)?),
        };
        Object::Entity { common, entity }
    } else {
        let common = decode_object_common(&mut r)?;
        r.mleader_new_format = ctx
            .mleader_style_marker
            .is_some_and(|m| common.xdata.iter().any(|x| x.app == m));
        r.saved_by_object_version = saved_by_object_version(&common);
        let object = match try_typed(&mut r, key, name, typed_object, &mut ctx.diagnostics) {
            Some(o) => o,
            None => NonGraphical::Unknown(RawObject::decode(&mut r, code, class_name)?),
        };
        Object::Other { common, object }
    };
    if obj.handle() != handle {
        let message = format!("object stores handle {}", obj.handle());
        warn(&mut ctx.diagnostics, message);
    }
    ctx.diagnostics.append(&mut r.diagnostics);
    Ok(obj)
}

/// Encode an object in R2018 form. A class type's code is the number of the
/// class in `classes` with its DXF name (for a raw object, the one it was
/// read with if that still has the name). Objects that cannot be written as
/// R2018 (raw R2000/R2004 bits, see [`NonGraphical`]) fail with
/// [`Error::UnconvertibleObjects`]. An ACIS entity read before R2013 with
/// inline SAB or no data is converted only by [`crate::Drawing::to_bytes`]
/// (which moves its data to the data storage section and gives it a GUID,
/// see [`AcisEntity::to_r2013`]); encoded directly, it fails with
/// [`Error::Encode`].
///
/// Each call indexes `classes` (by number and DXF name, linear in their
/// count) before encoding: encoding many objects this way costs the class
/// count per object. [`crate::Drawing::to_bytes`] indexes them once per
/// write.
pub fn encode_object(obj: &Object, classes: &[DxfClass]) -> Result<Vec<u8>> {
    encode_object_in(obj, &ClassIndex::new(classes))
}

/// [`encode_object`] with the classes indexed.
pub(crate) fn encode_object_in(obj: &Object, classes: &ClassIndex) -> Result<Vec<u8>> {
    let handle = obj.handle();
    if !obj.is_convertible() {
        return Err(Error::UnconvertibleObjects(vec![(
            handle,
            obj.type_name().into_owned(),
        )]));
    }
    let code = written_type_code(obj, classes)?;
    let mut w = ObjWriter::new(handle);
    match obj {
        Object::Entity { common, entity } => {
            encode_entity_common(&mut w, common)?;
            entity.encode(&mut w)?;
        }
        Object::Other { common, object } => {
            encode_object_common(&mut w, common)?;
            object.encode(&mut w)?;
        }
    }
    w.finish(code)
}

/// A drawing's classes by number and by DXF name (the first class with
/// it), so that type codes are found in constant time; [`encode_object`]
/// builds one per call, the drawing writer one per write, a [`DecodeCtx`]
/// one per context.
#[derive(Debug)]
pub(crate) struct ClassIndex<'a> {
    pub classes: &'a [DxfClass],
    by_number: HashMap<u16, usize>,
    by_name: HashMap<&'a str, usize>,
    /// Lookups made, for work-count tests.
    pub probes: std::cell::Cell<usize>,
}

impl<'a> ClassIndex<'a> {
    pub(crate) fn new(classes: &'a [DxfClass]) -> Self {
        let mut by_number = HashMap::with_capacity(classes.len());
        let mut by_name = HashMap::with_capacity(classes.len());
        for (i, c) in classes.iter().enumerate() {
            by_number.entry(c.class_number).or_insert(i);
            by_name.entry(c.dxf_name.as_str()).or_insert(i);
        }
        ClassIndex {
            classes,
            by_number,
            by_name,
            probes: std::cell::Cell::new(0),
        }
    }

    /// The class numbered `code`.
    pub(crate) fn by_number(&self, code: u16) -> Option<&'a DxfClass> {
        self.probes.set(self.probes.get() + 1);
        self.by_number.get(&code).map(|&i| &self.classes[i])
    }

    /// The first class named `name`.
    pub(crate) fn by_name(&self, name: &str) -> Option<&'a DxfClass> {
        self.probes.set(self.probes.get() + 1);
        self.by_name.get(name).map(|&i| &self.classes[i])
    }
}

/// The type code [`encode_object`] writes for `obj`: its fixed code, or the
/// number of the class with its DXF name (for a raw object, the class it was
/// read with if that still has the name).
pub(crate) fn written_type_code(obj: &Object, classes: &ClassIndex) -> Result<u16> {
    Ok(match obj.type_key() {
        TypeKey::Fixed(code) => code,
        TypeKey::Class(name) => {
            // Some drawings have several classes with one DXF name
            // (AecObjects.dwg does); the class a raw object was read with is
            // kept when it matches.
            let read_with = obj.raw().and_then(|r| classes.by_number(r.type_code));
            read_with
                .filter(|c| c.dxf_name == name)
                .or_else(|| classes.by_name(name))
                .ok_or_else(|| Error::Encode(format!("no class named {name}")))?
                .class_number
        }
    })
}

/// The references in an object's type-specific data, resolved, with their
/// reference codes, in write order: found by encoding that data in R2018 form
/// with a recording [`ObjWriter`] (every encoder writes its references
/// through [`ObjWriter::href`]). The common data (owner, reactors, extension
/// dictionary, an entity's layer and styles) is not included. A raw object's
/// kept references are resolved against its handle; a reference that does
/// not resolve is dropped.
///
/// The error is the encoder's when the data cannot be written as R2018 (an
/// R2000 entity list not built, a field out of range, ...): the references
/// written before it are still returned, and the object may have more.
pub(crate) fn type_refs(obj: &Object) -> (Vec<(Handle, u8)>, Result<()>) {
    let handle = obj.handle();
    let mut w = ObjWriter::recording(handle);
    let result = match obj {
        Object::Entity { entity, .. } => entity.encode(&mut w),
        Object::Other { object, .. } => object.encode(&mut w),
    };
    let refs = w
        .recorded_refs()
        .iter()
        .filter_map(|r| Some((r.resolve(handle)?, r.code)))
        .collect();
    (refs, result)
}

/// The references in an entity's common data other than its ownership
/// links: the color book, layer, linetype, material, plot style and visual
/// styles, resolved, with their codes. Found like [`type_refs`], by encoding
/// the common data with a recording [`ObjWriter`] and leaving out the
/// references it writes first: the owner (entity mode 0), the reactors and
/// the extension dictionary.
pub(crate) fn entity_common_refs(common: &EntityCommon) -> (Vec<(Handle, u8)>, Result<()>) {
    let handle = common.obj.handle;
    let mut w = ObjWriter::recording(handle);
    let result = encode_entity_common(&mut w, common);
    let ownership = usize::from(common.entmode == 0)
        + common.obj.reactors.len()
        + usize::from(common.obj.xdict.is_some());
    let refs = w
        .recorded_refs()
        .iter()
        .skip(ownership)
        .filter_map(|r| Some((r.resolve(handle)?, r.code)))
        .collect();
    (refs, result)
}

/// Append the kept type-specific bits of `raw`.
fn write_raw(w: &mut ObjWriter, raw: &RawObject) -> Result<()> {
    w.data.write_bits_from(&raw.data, raw.data_bits)?;
    if let Some((bytes, bits)) = &raw.strings {
        w.write_raw_strings(bytes, *bits)?;
    }
    for &h in &raw.handles {
        w.href(h)?;
    }
    // The kept end padding, so the handle stream ends as it did.
    w.handle_stream()
        .write_bits_from(&raw.tail_handles_raw, raw.tail_handle_bits)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::BitWriter;
    use crate::handle::HandleRef;
    use crate::object::stream::tests::assemble;
    use crate::section::classes::ITEM_CLASS_OBJECT;

    fn class(n: u16, name: &str, entity: bool) -> DxfClass {
        DxfClass {
            class_number: n,
            proxy_flags: 0,
            app_name: "App".into(),
            cpp_class_name: "AcDbThing".into(),
            dxf_name: name.into(),
            was_zombie: false,
            item_class_id: if entity {
                ITEM_CLASS_ENTITY
            } else {
                ITEM_CLASS_OBJECT
            },
            instance_count: 0,
            dwg_version: 0,
            maintenance_version: 0,
            unknown: [0; 2],
        }
    }

    fn ctx(classes: &[DxfClass]) -> DecodeCtx<'_> {
        DecodeCtx::new(Version::R2018, 30, classes)
    }

    fn object_common(h: u64) -> ObjectCommon {
        ObjectCommon {
            handle: Handle(h),
            owner: Handle(0xC),
            owner_relative: false,
            reactors: vec![Handle(0xD)],
            xdict: None,
            xdata: vec![],
            has_ds_binary_data: false,
        }
    }

    /// An R2018 non-entity: common data, type-specific data, a string and
    /// two references, padded with one bits as AutoCAD pads.
    fn custom_object(type_code: u16, h: u64) -> Vec<u8> {
        let mut w = ObjWriter::new(Handle(h));
        encode_object_common(&mut w, &object_common(h)).unwrap();
        w.data.write_bs(300);
        w.data.write_b(true);
        w.text("custom").unwrap();
        w.href(HandleRef {
            code: 5,
            value: 0x99,
        })
        .unwrap();
        w.href(HandleRef { code: 4, value: 0 }).unwrap();
        let pad = (8 - w.handle_stream().bit_len() % 8) % 8;
        for _ in 0..pad {
            w.handle_stream().write_b(true);
        }
        w.finish(type_code).unwrap()
    }

    /// The modelled-type tables agree with the dispatchers for every fixed
    /// code and every class name the library knows of (the tables', the
    /// fixed types', the known classes', and some it does not model), for
    /// both kinds; the class name tables are sorted, for the binary search.
    #[test]
    fn modeled_tables_match_the_dispatchers() {
        assert!(MODELED_ENTITY_CLASSES.is_sorted());
        assert!(MODELED_OBJECT_CLASSES.is_sorted());
        for code in 0..=u16::MAX {
            for entity in [false, true] {
                let key = TypeKey::Fixed(code);
                assert_eq!(
                    is_modeled(key, entity),
                    probe_modeled(key, entity),
                    "{code} {entity}"
                );
            }
        }
        let mut names: Vec<String> = MODELED_ENTITY_CLASSES
            .iter()
            .chain(&MODELED_OBJECT_CLASSES)
            .map(|n| n.to_string())
            .collect();
        names.extend((0..FIRST_CLASS_NUMBER).filter_map(|c| Some(fixed_type(c)?.0.to_owned())));
        names.extend(
            (0..200u16).filter_map(|i| {
                Some(crate::section::classes::known_class_name(usize::from(i))?.to_owned())
            }),
        );
        names.extend(
            ["ACAD_TABLE", "CELLSTYLEMAP", "AECC_STYLE", "", "dictionary"].map(String::from),
        );
        for name in &names {
            for entity in [false, true] {
                let key = TypeKey::Class(name);
                assert_eq!(
                    is_modeled(key, entity),
                    probe_modeled(key, entity),
                    "{name} {entity}"
                );
            }
        }
    }

    #[test]
    fn fixed_type_table_matches_ods() {
        assert_eq!(fixed_type(type_code::LINE), Some(("LINE", true)));
        assert_eq!(fixed_type(type_code::LAYER), Some(("LAYER", false)));
        assert_eq!(fixed_type(type_code::OLE2FRAME), Some(("OLE2FRAME", true)));
        assert_eq!(
            fixed_type(type_code::ACAD_PROXY_ENTITY),
            Some(("ACAD_PROXY_ENTITY", true))
        );
        for unassigned in [0, 9, 0x36, 0x37, 0x3A, 0x3B, 0x4B, 0x53, 0x1F1, 0x1F4] {
            assert_eq!(fixed_type(unassigned), None, "{unassigned:#X}");
        }
    }

    #[test]
    fn raw_object_reencodes_identically() {
        let bytes = custom_object(type_code::VBA_PROJECT, 0x80);
        let mut c = ctx(&[]);
        let obj = decode_object(&bytes, Handle(0x80), &mut c).unwrap();
        assert!(c.diagnostics.is_empty(), "{:?}", c.diagnostics);
        assert_eq!(obj.handle(), Handle(0x80));
        assert_eq!(obj.type_name(), "VBA_PROJECT");
        assert_eq!(obj.object_common(), &object_common(0x80));
        let raw = obj.raw().unwrap();
        assert!(
            (1..8).contains(&raw.tail_handle_bits),
            "{}",
            raw.tail_handle_bits
        );
        assert_eq!(encode_object(&obj, &[]).unwrap(), bytes);
    }

    /// The fixed entity types older files may store as class types decode
    /// by class name as by number: HATCH, OLE2FRAME, ACAD_PROXY_ENTITY and
    /// LWPOLYLINE.
    #[test]
    fn fixed_entities_decode_by_class_name() {
        let encoded = |e: &Entity| {
            let mut w = ObjWriter::new(Handle(0x80));
            e.encode(&mut w).unwrap();
            w.finish(0).unwrap()
        };
        let cases = [
            (
                "LWPOLYLINE",
                Entity::LwPolyline(Box::new(entities::poly::tests::lwpolyline())),
            ),
            (
                "HATCH",
                Entity::Hatch(Box::new(entities::hatch::tests::hatch())),
            ),
            (
                "ACAD_PROXY_ENTITY",
                Entity::ProxyEntity(Box::new(entities::misc::tests::proxy())),
            ),
        ];
        for (name, e) in cases {
            let bytes = encoded(&e);
            let mut r = entities::tests::reader(&bytes);
            let got = typed_entity(TypeKey::Class(name), &mut r).map(|x| x.unwrap());
            assert_eq!(got.as_ref(), Some(&e), "{name}");
        }
    }

    /// A reference after the last one that does not parse (its counter runs
    /// past the stream's end) no longer fails the typed decode of the
    /// objects that read references to the end (XRECORD, SPATIAL_FILTER,
    /// SPATIAL_INDEX, ACAD_PROXY_ENTITY): its bits are kept, and the object
    /// is written back as read.
    #[test]
    fn unparsable_trailing_references_are_kept() {
        let tail = HandleTail {
            // Code 4 with a 15-byte counter, then one byte.
            bytes: vec![0x4F, 0xAA],
            bits: 16,
        };
        let ids = vec![HandleRef {
            code: 5,
            value: 0x99,
        }];
        let classes = [
            class(500, "SPATIAL_FILTER", false),
            class(501, "SPATIAL_INDEX", false),
        ];
        let other = |object| Object::Other {
            common: object_common(0x80),
            object,
        };
        let mut entity_common = EntityCommon::new(Handle(0x10));
        entity_common.obj = object_common(0x80);
        let objects = [
            other(NonGraphical::XRecord(XRecord {
                object_ids: ids.clone(),
                handle_tail: tail.clone(),
                ..XRecord::default()
            })),
            other(NonGraphical::SpatialFilter(SpatialFilter {
                handles: ids.clone(),
                handle_tail: tail.clone(),
                ..SpatialFilter::default()
            })),
            other(NonGraphical::SpatialIndex(Box::new(SpatialIndex {
                timestamps: [1, 2],
                bits: OpaqueBits::default(),
                handles: ids.clone(),
                handle_tail: tail.clone(),
                source_version: Version::R2018,
            }))),
            Object::Entity {
                common: entity_common,
                entity: Entity::ProxyEntity(Box::new(ProxyEntity {
                    object_ids: ids.clone(),
                    handle_tail: tail.clone(),
                    ..entities::misc::tests::proxy()
                })),
            },
        ];
        for obj in objects {
            let name = obj.type_name().into_owned();
            let bytes = encode_object(&obj, &classes).unwrap();
            let mut c = ctx(&classes);
            let back = decode_object(&bytes, Handle(0x80), &mut c).unwrap();
            assert!(c.diagnostics.is_empty(), "{name}: {:?}", c.diagnostics);
            assert!(back.raw().is_none(), "{name}: typed");
            // The kept bits are the tail and the end padding after it.
            let kept = match &back {
                Object::Other {
                    object: NonGraphical::XRecord(x),
                    ..
                } => &x.handle_tail,
                Object::Other {
                    object: NonGraphical::SpatialFilter(f),
                    ..
                } => &f.handle_tail,
                Object::Other {
                    object: NonGraphical::SpatialIndex(i),
                    ..
                } => &i.handle_tail,
                Object::Entity {
                    entity: Entity::ProxyEntity(p),
                    ..
                } => &p.handle_tail,
                _ => unreachable!(),
            };
            assert!(
                kept.bits >= 16 && kept.bytes.starts_with(&tail.bytes),
                "{name}"
            );
            let again = encode_object(&back, &classes).unwrap();
            assert_eq!(again, bytes, "{name}: written back as read");
            let mut c = ctx(&classes);
            assert_eq!(decode_object(&again, Handle(0x80), &mut c).unwrap(), back);
        }
    }

    /// A class table whose numbers are not positional (reversed and sparse
    /// here) costs one indexed lookup per object, and one index per
    /// context: decoding is linear in the objects, not objects x classes.
    #[test]
    fn class_lookups_are_constant_per_decoded_object() {
        let n = 3_000u16;
        let names: Vec<String> = (0..n).map(|i| format!("ACME_{i}")).collect();
        let classes: Vec<DxfClass> = (0..n)
            .map(|i| class(500 + 2 * (n - 1 - i), &names[usize::from(i)], false))
            .collect();
        let mut c = ctx(&classes);
        let objects = 2_000u16;
        for k in 0..objects {
            let code = 500 + 2 * k;
            let obj = decode_object(&custom_object(code, 0x100), Handle(0x100), &mut c).unwrap();
            assert_eq!(obj.type_name(), names[usize::from(n - 1 - k)]);
        }
        assert_eq!(c.class_index().probes.get(), usize::from(objects));
        // Replacing the classes rebuilds the index.
        let other = [class(500, "OTHER", false)];
        c.classes = &other;
        let obj = decode_object(&custom_object(500, 0x100), Handle(0x100), &mut c).unwrap();
        assert_eq!(obj.type_name(), "OTHER");
        assert_eq!(c.class_index().probes.get(), 1);
    }

    #[test]
    fn class_object_is_dispatched_by_dxf_name() {
        let classes = [class(500, "ACME_A", false), class(501, "ACME_THING", false)];
        let bytes = custom_object(501, 0x80);
        let mut c = ctx(&classes);
        let obj = decode_object(&bytes, Handle(0x80), &mut c).unwrap();
        assert!(matches!(obj, Object::Other { .. }));
        assert_eq!(obj.type_name(), "ACME_THING");
        assert_eq!(obj.raw().unwrap().class_name.as_deref(), Some("ACME_THING"));
        // Written into a drawing where the class has another number.
        let other = [class(500, "ACME_THING", false)];
        let out = encode_object(&obj, &other).unwrap();
        let (_, pre) = ObjReader::split(&out, Version::R2018, 30).unwrap();
        assert_eq!(pre.type_code, 500);
        assert!(matches!(
            encode_object(&obj, &[]),
            Err(Error::Encode(m)) if m.contains("ACME_THING")
        ));
        // Two classes with one DXF name: the one read with is kept.
        let twins = [
            class(500, "ACME_THING", false),
            class(501, "ACME_THING", false),
        ];
        assert_eq!(encode_object(&obj, &twins).unwrap(), bytes);
    }

    #[test]
    fn class_entity_decodes_entity_common() {
        let classes = [class(500, "WIPEOUT", true)];
        let mut d = BitWriter::new();
        d.write_handle_ref(HandleRef {
            code: 0,
            value: 0x40,
        })
        .unwrap();
        d.write_bs(0); // no xdata
        d.write_b(false); // no proxy graphics
        d.write_bb(2); // model space: no owner
        d.write_bl(0); // reactors
        d.write_b(true); // no xdict
        d.write_b(false); // no ds data
        d.write_enc(&crate::bits::Color::ByLayer, None, false, Version::R2018)
            .unwrap();
        d.write_bd(1.0);
        d.write_bb(0); // linetype by layer
        d.write_bb(0); // material by layer
        d.write_rc(0); // shadow
        d.write_bb(0); // plot style by layer
        d.write_b(false);
        d.write_b(false);
        d.write_b(false);
        d.write_bs(0);
        d.write_rc(0);
        d.write_bd(2.5); // type-specific
        let mut hs = BitWriter::new();
        hs.write_handle_ref(HandleRef {
            code: 5,
            value: 0x10,
        })
        .unwrap(); // layer
        let bytes = assemble(Version::R2018, 500, &d, None, &hs);
        let mut c = ctx(&classes);
        let obj = decode_object(&bytes, Handle(0x40), &mut c).unwrap();
        let Object::Entity { common, entity } = &obj else {
            panic!("not an entity: {obj:?}");
        };
        assert_eq!(common.layer, Handle(0x10));
        let Entity::Unknown(raw) = entity else {
            panic!("not raw: {entity:?}");
        };
        assert_eq!(raw.class_name.as_deref(), Some("WIPEOUT"));
        assert_eq!(raw.data_bits, 2 + 64);
    }

    #[test]
    fn missing_class_is_kept_raw() {
        let bytes = custom_object(520, 0x80);
        let mut c = ctx(&[]);
        let obj = decode_object(&bytes, Handle(0x80), &mut c).unwrap();
        assert_eq!(obj.type_name(), "TYPE_520");
        assert_eq!(obj.raw().unwrap().class_name, None);
        assert_eq!(c.diagnostics.len(), 1, "{:?}", c.diagnostics);
        assert_eq!(c.diagnostics[0].severity, Severity::Warning);
        assert!(c.diagnostics[0].message.contains("520 has no class"));
        // Its bits survive: written back with its own type code.
        assert_eq!(encode_object(&obj, &[]).unwrap(), bytes);
    }

    /// A typed decoder that reads a field, then fails.
    fn failing(_: TypeKey, r: &mut ObjReader) -> Option<Result<i16>> {
        Some(r.data.read_bs().and(Err(Error::Corrupt("boom".into()))))
    }

    /// A typed decoder that reads every field of [`custom_object`].
    fn succeeding(_: TypeKey, r: &mut ObjReader) -> Option<Result<i16>> {
        Some((|| {
            let v = r.data.read_bs()?;
            r.data.read_b()?;
            r.text()?;
            r.href()?;
            r.href()?;
            Ok(v)
        })())
    }

    /// A typed decoder that reads only the first field.
    fn partial(_: TypeKey, r: &mut ObjReader) -> Option<Result<i16>> {
        Some(r.data.read_bs())
    }

    fn after_common(bytes: &[u8]) -> ObjReader<'_> {
        let (mut r, _) = ObjReader::split(bytes, Version::R2018, 30).unwrap();
        decode_object_common(&mut r).unwrap();
        r
    }

    #[test]
    fn failed_typed_decode_keeps_the_object_raw() {
        let bytes = custom_object(type_code::XRECORD, 0x80);
        let mut r = after_common(&bytes);
        let pos = r.data.bit_pos();
        let mut diags = vec![];
        let key = TypeKey::Fixed(type_code::XRECORD);
        assert_eq!(try_typed(&mut r, key, || "XRECORD".into(), failing, &mut diags), None);
        assert_eq!(
            diags,
            [Diagnostic {
                severity: Severity::Warning,
                handle: Some(Handle(0x80)),
                message: "XRECORD decode failed: corrupt data: boom; kept raw".into(),
            }]
        );
        assert_eq!(r.data.bit_pos(), pos);
        let raw = RawObject::decode(&mut r, type_code::XRECORD, None).unwrap();
        let mut plain = after_common(&bytes);
        let expect = RawObject::decode(&mut plain, type_code::XRECORD, None).unwrap();
        assert_eq!(raw, expect);
        assert_eq!(raw.data_bits, 18 + 1); // the BS 300 (2 + 16 bits) and the B
    }

    #[test]
    fn successful_typed_decode_advances_the_reader() {
        let bytes = custom_object(type_code::XRECORD, 0x80);
        let mut r = after_common(&bytes);
        let pos = r.data.bit_pos();
        let mut diags = vec![];
        let key = TypeKey::Fixed(type_code::XRECORD);
        assert_eq!(
            try_typed(&mut r, key, || "XRECORD".into(), succeeding, &mut diags),
            Some(300)
        );
        assert!(diags.is_empty());
        assert_eq!(r.data.bit_pos(), pos + 19);
        assert_eq!(r.data.remaining_bits(), 0);
        assert_eq!(
            try_typed(&mut r, key, || "XRECORD".into(), typed_none, &mut diags),
            None
        );
        assert_eq!(r.data.bit_pos(), pos + 19);
        assert!(diags.is_empty());
    }

    #[test]
    fn typed_decode_leaving_bits_keeps_the_object_raw() {
        let bytes = custom_object(type_code::XRECORD, 0x80);
        let mut r = after_common(&bytes);
        let pos = r.data.bit_pos();
        let mut diags = vec![];
        let key = TypeKey::Fixed(type_code::XRECORD);
        assert_eq!(try_typed(&mut r, key, || "XRECORD".into(), partial, &mut diags), None);
        assert_eq!(r.data.bit_pos(), pos);
        assert_eq!(diags.len(), 1);
        // Left: the B; the string (a 10-bit BS length and 6 UTF-16 units);
        // two references (16 and 8 bits) and 2 bits of end padding.
        assert_eq!(
            diags[0].message,
            "XRECORD decode failed: 1 data, 106 string and 26 handle stream bits unread; \
             kept raw"
        );
    }

    fn typed_none(_: TypeKey, _: &mut ObjReader) -> Option<Result<i16>> {
        None
    }

    #[test]
    fn warnings_for_unassigned_type_crc_and_handle() {
        let mut bytes = custom_object(0x36, 0x80);
        let mut c = ctx(&[]);
        let obj = decode_object(&bytes, Handle(0x81), &mut c).unwrap();
        assert_eq!(obj.type_name(), "TYPE_54");
        let msgs: Vec<_> = c.diagnostics.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(msgs.len(), 2, "{msgs:?}");
        assert!(msgs[0].contains("unassigned"));
        assert!(msgs[1].contains("stores handle 80"));
        assert!(c.diagnostics.iter().all(|d| d.handle == Some(Handle(0x81))));

        let n = bytes.len();
        bytes[n - 1] ^= 0xFF;
        let mut c = ctx(&[]);
        decode_object(&bytes, Handle(0x80), &mut c).unwrap();
        assert_eq!(c.diagnostics.len(), 2, "{:?}", c.diagnostics);
        assert!(c.diagnostics[0].message.contains("CRC"));
    }

    #[test]
    fn r2000_raw_is_unconvertible() {
        let mut d = BitWriter::new();
        d.write_handle_ref(HandleRef {
            code: 0,
            value: 0x80,
        })
        .unwrap();
        d.write_bs(0);
        d.write_bl(0);
        d.write_bl(77);
        let mut hs = BitWriter::new();
        hs.write_handle_ref(HandleRef {
            code: 4,
            value: 0xC,
        })
        .unwrap();
        hs.write_handle_ref(HandleRef { code: 3, value: 0 })
            .unwrap();
        let bytes = assemble(Version::R2000, type_code::VBA_PROJECT, &d, None, &hs);
        let mut c = DecodeCtx {
            version: Version::R2000,
            ..ctx(&[])
        };
        let obj = decode_object(&bytes, Handle(0x80), &mut c).unwrap();
        assert!(matches!(
            encode_object(&obj, &[]),
            Err(Error::UnconvertibleObjects(v)) if v == [(Handle(0x80), "VBA_PROJECT".to_owned())]
        ));
    }

    fn raw_entity(code: u16, class_name: Option<&str>, v: Version) -> Object {
        Object::Entity {
            common: entities::dim::tests::entity_common(),
            entity: Entity::Unknown(RawObject {
                type_code: code,
                class_name: class_name.map(str::to_owned),
                source_version: v,
                data: vec![0x80],
                data_bits: 1,
                strings: Some((vec![], 0)),
                handles: vec![],
                tail_handles_raw: vec![],
                tail_handle_bits: 0,
            }),
        }
    }

    /// Raw bits of a type the library models (its typed decode failed) are
    /// only written when read from R2018; raw bits of a type it does not
    /// model follow the R2007+ rule.
    #[test]
    fn raw_modeled_types_convert_only_from_r2018() {
        let classes = [class(500, "ACME_THING", true), class(501, "IMAGE", true)];
        let refused = |obj: &Object| {
            matches!(
                encode_object(obj, &classes),
                Err(Error::UnconvertibleObjects(v)) if v == [(obj.handle(), obj.type_name().into_owned())]
            )
        };
        for v in [Version::R2007, Version::R2010, Version::R2013] {
            assert!(refused(&raw_entity(type_code::SOLID3D, None, v)), "{v:?}");
            assert!(refused(&raw_entity(type_code::HATCH, None, v)), "{v:?}");
            assert!(refused(&raw_entity(501, Some("IMAGE"), v)), "{v:?}");
            // A custom class: its bits share the R2007+ layout.
            let custom = raw_entity(500, Some("ACME_THING"), v);
            assert!(encode_object(&custom, &classes).is_ok(), "{v:?}");
        }
        assert!(refused(&raw_entity(
            500,
            Some("ACME_THING"),
            Version::R2004
        )));
        // A modeled type read from R2018 whose typed decode fails (an ACIS
        // entity cut short: only the wireframe bit) is kept raw and written
        // back as it was.
        let obj = raw_entity(type_code::SOLID3D, None, Version::R2018);
        let bytes = encode_object(&obj, &classes).unwrap();
        let mut c = ctx(&classes);
        let back = decode_object(&bytes, obj.handle(), &mut c).unwrap();
        assert!(back.raw().is_some(), "{back:?}");
        assert!(c.diagnostics[0].message.contains("3DSOLID decode failed"));
        assert_eq!(encode_object(&back, &classes).unwrap(), bytes);
        // The same from R2010 is refused.
        let Object::Entity {
            entity: Entity::Unknown(r),
            ..
        } = &back
        else {
            unreachable!()
        };
        let mut old = r.clone();
        old.source_version = Version::R2010;
        let old = Object::Entity {
            common: entities::dim::tests::entity_common(),
            entity: Entity::Unknown(old),
        };
        assert!(refused(&old));
        // Non-entities too: a raw LAYER or XRECORD from R2010 is refused, a
        // raw VBA_PROJECT (not modeled) converts.
        let raw_object = |code| Object::Other {
            common: object_common(0x80),
            object: NonGraphical::Unknown(RawObject {
                type_code: code,
                class_name: None,
                source_version: Version::R2010,
                data: vec![],
                data_bits: 0,
                strings: None,
                handles: vec![],
                tail_handles_raw: vec![],
                tail_handle_bits: 0,
            }),
        };
        assert!(refused(&raw_object(type_code::LAYER)));
        assert!(refused(&raw_object(type_code::XRECORD)));
        assert!(encode_object(&raw_object(type_code::VBA_PROJECT), &classes).is_ok());
    }

    #[test]
    fn hostile_objects_never_panic() {
        let classes = [class(500, "ACME", true)];
        let good = custom_object(type_code::XRECORD, 0x80);
        for i in 0..good.len() {
            for bit in [0x01u8, 0x10, 0x80] {
                let mut b = good.clone();
                b[i] ^= bit;
                let _ = decode_object(&b, Handle(0x80), &mut ctx(&classes));
            }
            let _ = decode_object(&good[..i], Handle(0x80), &mut ctx(&classes));
        }
    }
}
