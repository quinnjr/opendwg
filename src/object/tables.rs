//! Symbol tables (ODS 20.4.51-20.4.70): the table control objects and their
//! entries (block records, layers, text styles, linetypes, views, UCSs,
//! viewports, registered applications, dimension styles and viewport entity
//! headers).
//!
//! Each type decodes and encodes only its type-specific data; the common data
//! is handled by [`super::decode_object`]. Fields that a version does not
//! store are filled with AutoCAD's defaults on read, so an entry reads the
//! same from every version where the format allows.
//!
//! Deviations from ODS, found on the corpus (every typed decoder must consume
//! its object to the last bit, or the object is kept raw), each pinned by a
//! test in `tests/tables.rs`:
//!
//! - From R2007 the `64-flag`, `xrefindex+1` and `Xdep` fields of a table
//!   entry are one `BS`: `xrefindex+1` with bit 0x100 set when the entry
//!   depends on an xref, as ACadSharp (MIT, `DwgObjectReader
//!   .readXrefDependantBit`) reads it. The 64-flag ("referenced") has no
//!   place there and is dropped on read from older files.
//! - DIMSTYLE_CONTROL: after the entry count an `RC` counts hard pointers to
//!   dimension styles that follow the entries (not in ODS or ACadSharp).
//! - LAYER: the trailing unknown hard pointer is stored from R2013 only (ODS:
//!   all versions; ACadSharp: R2013+). Bit 2 of the flags short is set when
//!   the layer is off (ODS: "on"; ACadSharp agrees with the corpus), and
//!   AutoCAD's R2000 files leave the color index positive for an off layer.
//! - STYLE: the shape-file bit comes before the vertical bit (ODS: after; as
//!   ACadSharp `readTextStyle` reads it).
//! - DIMSTYLE: DIMCLRD, DIMCLRE and DIMCLRT are `CMC` (ODS: `BS`, which is
//!   the same bits before R2004; as ACadSharp `readDimStyle` reads them).
//!
//! Unconfirmed by the corpus, which has no xrefs: the ODS reading of xref
//! BLOCK_HEADERs (no first/last entity before R2004; ACadSharp also skips the
//! R2004+ owned-object count for them, ODS does not, and ODS is followed).

use super::stream::{ObjReader, ObjWriter};
use crate::bits::Color;
use crate::error::encode_error as encode_err;
use crate::error::{Error, Result};
use crate::handle::{Handle, HandleRef};
use crate::version::Version;

/// Handle reference codes (ODS 2.13).
pub(crate) const SOFT_OWNER: u8 = 2;
pub(crate) const HARD_OWNER: u8 = 3;
pub(crate) const SOFT_POINTER: u8 = 4;
pub(crate) const HARD_POINTER: u8 = 5;

fn corrupt(msg: &str) -> Error {
    Error::Corrupt(msg.to_owned())
}

pub(crate) fn put(w: &mut ObjWriter, code: u8, h: Handle) -> Result<()> {
    w.href(HandleRef { code, value: h.0 })
}

/// A count read from the file, checked against the handle stream: every
/// reference takes at least a byte.
pub(crate) fn handle_count(r: &ObjReader, n: i32, what: &str) -> Result<usize> {
    usize::try_from(n)
        .ok()
        .filter(|&n| n as u64 <= r.handles.remaining_bits() / 8)
        .ok_or_else(|| corrupt(&format!("{what} count {n} exceeds the handle stream")))
}

pub(crate) fn encode_count(n: usize, what: &str) -> Result<i32> {
    i32::try_from(n).map_err(|_| encode_err(format!("too many {what} to encode")))
}

/// The tables, by the kind of entry they hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum TableKind {
    /// BLOCK_RECORD.
    Block,
    /// LAYER.
    Layer,
    /// STYLE (text styles and shape files).
    Style,
    /// LTYPE.
    Linetype,
    /// VIEW.
    View,
    /// UCS.
    Ucs,
    /// VPORT.
    Vport,
    /// APPID.
    AppId,
    /// DIMSTYLE.
    DimStyle,
    /// VP_ENT_HDR (R2000 only).
    VpEntHdr,
}

impl TableKind {
    /// Every table kind.
    pub const ALL: [TableKind; 10] = [
        TableKind::Block,
        TableKind::Layer,
        TableKind::Style,
        TableKind::Linetype,
        TableKind::View,
        TableKind::Ucs,
        TableKind::Vport,
        TableKind::AppId,
        TableKind::DimStyle,
        TableKind::VpEntHdr,
    ];

    /// The type code of the table's control object.
    pub fn control_code(self) -> u16 {
        use super::type_code::*;
        match self {
            TableKind::Block => BLOCK_CONTROL,
            TableKind::Layer => LAYER_CONTROL,
            TableKind::Style => STYLE_CONTROL,
            TableKind::Linetype => LTYPE_CONTROL,
            TableKind::View => VIEW_CONTROL,
            TableKind::Ucs => UCS_CONTROL,
            TableKind::Vport => VPORT_CONTROL,
            TableKind::AppId => APPID_CONTROL,
            TableKind::DimStyle => DIMSTYLE_CONTROL,
            TableKind::VpEntHdr => VP_ENT_HDR_CONTROL,
        }
    }

    /// The type code of the table's entries.
    pub fn entry_code(self) -> u16 {
        use super::type_code::*;
        match self {
            TableKind::Block => BLOCK_HEADER,
            TableKind::Layer => LAYER,
            TableKind::Style => STYLE,
            TableKind::Linetype => LTYPE,
            TableKind::View => VIEW,
            TableKind::Ucs => UCS,
            TableKind::Vport => VPORT,
            TableKind::AppId => APPID,
            TableKind::DimStyle => DIMSTYLE,
            TableKind::VpEntHdr => VP_ENT_HDR,
        }
    }

    /// The table whose control object has type `code`.
    pub fn from_control_code(code: u16) -> Option<TableKind> {
        TableKind::ALL
            .into_iter()
            .find(|k| k.control_code() == code)
    }

    /// How many extra entries follow the counted ones: `*MODEL_SPACE` and
    /// `*PAPER_SPACE` for blocks, `BYLAYER` and `BYBLOCK` for linetypes; a
    /// count stored in the object for dimension styles.
    fn fixed_extra_count(self) -> Option<usize> {
        match self {
            TableKind::Block | TableKind::Linetype => Some(2),
            TableKind::DimStyle => None,
            _ => Some(0),
        }
    }

    /// The reference code of the extra entries.
    fn extra_code(self) -> u8 {
        match self {
            TableKind::DimStyle => HARD_POINTER,
            _ => HARD_OWNER,
        }
    }
}

/// A table control object (ODS 20.4.51, 20.4.53, ...): the table's entries.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct TableControl {
    pub kind: TableKind,
    /// The entries (soft owner), in file order. AutoCAD keeps null handles
    /// for erased entries; they are kept.
    pub entries: Vec<Handle>,
    /// References after the counted entries:
    /// - blocks: `[*MODEL_SPACE, *PAPER_SPACE]` (hard owner);
    /// - linetypes: `[BYBLOCK, BYLAYER]` (hard owner; ODS lists BYLAYER
    ///   first, every corpus file has BYBLOCK first);
    /// - dimension styles: hard pointers to some of the styles, counted by an
    ///   `RC` after the entry count (not in ODS; seen in every corpus file,
    ///   purpose unknown);
    /// - other tables: none.
    pub extra: Vec<Handle>,
}

impl TableControl {
    pub(crate) fn decode(r: &mut ObjReader, kind: TableKind) -> Result<Self> {
        let n = r.data.read_bl()?;
        let n = handle_count(r, n, "table entry")?;
        let extra_n = match kind.fixed_extra_count() {
            Some(n) => n,
            None => usize::from(r.data.read_rc()?),
        };
        let entries = (0..n).map(|_| r.href_abs()).collect::<Result<_>>()?;
        let extra = (0..extra_n).map(|_| r.href_abs()).collect::<Result<_>>()?;
        Ok(TableControl {
            kind,
            entries,
            extra,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        w.data
            .write_bl(encode_count(self.entries.len(), "table entries")?);
        match self.kind.fixed_extra_count() {
            Some(n) if n != self.extra.len() => {
                return Err(encode_err(format!(
                    "{:?} table control needs {n} extra entries, has {}",
                    self.kind,
                    self.extra.len()
                )));
            }
            Some(_) => {}
            None => w.data.write_rc(
                u8::try_from(self.extra.len())
                    .map_err(|_| encode_err("at most 255 extra dimension style references"))?,
            ),
        }
        for &h in &self.entries {
            put(w, SOFT_OWNER, h)?;
        }
        for &h in &self.extra {
            put(w, self.kind.extra_code(), h)?;
        }
        Ok(())
    }
}

/// The data every table entry starts with: its name and xref fields, and the
/// xref block it came from.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TableEntry {
    /// The entry's name.
    pub name: String,
    /// `xrefindex+1`: 0 for an entry not from an xref, else the index of the
    /// xref's block record plus one.
    pub xref_index: i16,
    /// Bit 16 of DXF group 70: the entry depends on an xref.
    pub xref_dependent: bool,
    /// The xref block record (hard pointer); null when not from an xref.
    pub xref_block: Handle,
}

/// The R2007+ bit of the xref short that marks an xref-dependent entry.
const XREF_DEPENDENT: i16 = 0x100;

impl TableEntry {
    /// The name and xref fields, and the xref block handle (the first
    /// reference after the common data). Before R2007 the 64-flag
    /// ("referenced", DXF 70 bit 64) is read and dropped: R2007+ files have
    /// no place for it.
    fn decode(r: &mut ObjReader) -> Result<Self> {
        let name = r.text()?;
        let (xref_index, xref_dependent) = if r.version >= Version::R2007 {
            let x = r.data.read_bs()?;
            (x & !XREF_DEPENDENT, x & XREF_DEPENDENT != 0)
        } else {
            let _referenced = r.data.read_b()?;
            let index = r.data.read_bs()?;
            (index, r.data.read_b()?)
        };
        let xref_block = r.href_abs()?;
        Ok(TableEntry {
            name,
            xref_index,
            xref_dependent,
            xref_block,
        })
    }

    /// Whether R2018 can store the xref index: its one `BS` keeps bit 0x100
    /// for the dependent flag, so an index (read from a pre-R2007 file) with
    /// that bit set has no R2018 form; only that bit matters. Saving a
    /// drawing with such an entry fails with [`Error::UnconvertibleObjects`],
    /// and [`super::encode_object`] fails to encode it.
    pub fn is_convertible(&self) -> bool {
        self.xref_index & XREF_DEPENDENT == 0
    }

    fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        if !self.is_convertible() {
            return Err(encode_err(
                "an xref index with bit 0x100 set cannot be encoded",
            ));
        }
        w.text(&self.name)?;
        let dep = if self.xref_dependent {
            XREF_DEPENDENT
        } else {
            0
        };
        w.data.write_bs(self.xref_index | dep);
        put(w, HARD_POINTER, self.xref_block)
    }
}

/// The entities a block record, an insert or a polyline owns: a block's
/// entities, an insert's attributes, a polyline's vertices.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum EntityList {
    /// R2000: the first and last entity (soft pointers); the others are
    /// found through each entity's previous/next links. [`crate::Drawing`]
    /// replaces this with [`EntityList::Owned`] after reading.
    Linked {
        /// The first entity.
        first: Handle,
        /// The last entity.
        last: Handle,
    },
    /// R2004+: every entity, in order. They are written as hard owners
    /// (code 3) for a block record, an INSERT and every POLYLINE, and as
    /// soft pointers (code 4) for an MINSERT, as ODS gives them (ODS gives
    /// soft pointers for polyface and polygon mesh vertices too; the corpus
    /// has hard owners, see `entities::poly`).
    Owned(Vec<Handle>),
}

/// A block record (ODS 20.4.52, BLOCK_HEADER).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct BlockRecord {
    pub entry: TableEntry,
    pub anonymous: bool,
    pub has_attdefs: bool,
    pub is_xref: bool,
    pub xref_overlaid: bool,
    /// An xref that is not loaded.
    pub xref_unloaded: bool,
    pub base_point: [f64; 3],
    pub xref_path: String,
    pub description: String,
    /// Preview image bytes (DXF 310).
    pub preview: Vec<u8>,
    /// R2007+ (DXF 70); 0 (unitless) before.
    pub insert_units: i16,
    /// R2007+ (DXF 280); `true` before.
    pub explodable: bool,
    /// R2007+ block scaling (DXF 281); 0 (any scaling) before.
    pub block_scaling: u8,
    /// The BLOCK entity (hard owner).
    pub block_entity: Handle,
    pub entities: EntityList,
    /// The ENDBLK entity (hard owner).
    pub end_block: Handle,
    /// Inserts of this block (soft pointers).
    pub inserts: Vec<Handle>,
    /// The layout (hard pointer); null for blocks that are not layouts.
    pub layout: Handle,
}

impl BlockRecord {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let v = r.version;
        let entry = TableEntry::decode(r)?;
        let anonymous = r.data.read_b()?;
        let has_attdefs = r.data.read_b()?;
        let is_xref = r.data.read_b()?;
        let xref_overlaid = r.data.read_b()?;
        let xref_unloaded = r.data.read_b()?;
        // An xref block owns no entities: from R2004 it stores no owned
        // object count (ODS has the count always; ACadSharp's
        // `readBlockHeader` and Civil 3D's xref block in `camber-26` agree).
        let xref = is_xref || xref_overlaid;
        let owned_count = if v >= Version::R2004 && !xref {
            Some(r.data.read_bl()?)
        } else {
            None
        };
        let base_point = r.data.read_3bd()?;
        let xref_path = r.text()?;
        // A run of non-zero bytes, one per insert, ended by a zero byte.
        let mut insert_count = 0usize;
        while r.data.read_rc()? != 0 {
            insert_count += 1;
        }
        let description = r.text()?;
        let preview_len = r.data.read_bl()?;
        let preview_len = usize::try_from(preview_len)
            .ok()
            .filter(|&n| n as u64 <= r.data.remaining_bits() / 8)
            .ok_or(Error::Truncated {
                context: "block preview",
            })?;
        let preview = r.data.read_bytes(preview_len)?;
        let (insert_units, explodable, block_scaling) = if v >= Version::R2007 {
            (r.data.read_bs()?, r.data.read_b()?, r.data.read_rc()?)
        } else {
            (0, true, 0)
        };

        let block_entity = r.href_abs()?;
        let entities = match owned_count {
            Some(n) => {
                let n = handle_count(r, n, "owned entity")?;
                EntityList::Owned((0..n).map(|_| r.href_abs()).collect::<Result<_>>()?)
            }
            None if xref => EntityList::Owned(Vec::new()),
            None => EntityList::Linked {
                first: r.href_abs()?,
                last: r.href_abs()?,
            },
        };
        let end_block = r.href_abs()?;
        if insert_count as u64 > r.handles.remaining_bits() / 8 {
            return Err(corrupt("insert count exceeds the handle stream"));
        }
        let inserts = (0..insert_count)
            .map(|_| r.href_abs())
            .collect::<Result<_>>()?;
        let layout = r.href_abs()?;
        Ok(BlockRecord {
            entry,
            anonymous,
            has_attdefs,
            is_xref,
            xref_overlaid,
            xref_unloaded,
            base_point,
            xref_path,
            description,
            preview,
            insert_units,
            explodable,
            block_scaling,
            block_entity,
            entities,
            end_block,
            inserts,
            layout,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        let EntityList::Owned(owned) = &self.entities else {
            return Err(encode_err(
                "an R2000 block record's first/last entities must be converted to a list",
            ));
        };
        let xref = self.is_xref || self.xref_overlaid;
        if xref && !owned.is_empty() {
            return Err(encode_err("an xref block record cannot own entities"));
        }
        self.entry.encode(w)?;
        w.data.write_b(self.anonymous);
        w.data.write_b(self.has_attdefs);
        w.data.write_b(self.is_xref);
        w.data.write_b(self.xref_overlaid);
        w.data.write_b(self.xref_unloaded);
        if !xref {
            w.data
                .write_bl(encode_count(owned.len(), "owned entities")?);
        }
        w.data.write_3bd(self.base_point);
        w.text(&self.xref_path)?;
        for _ in &self.inserts {
            w.data.write_rc(1);
        }
        w.data.write_rc(0);
        w.text(&self.description)?;
        w.data
            .write_bl(encode_count(self.preview.len(), "preview bytes")?);
        w.data.write_bytes(&self.preview);
        w.data.write_bs(self.insert_units);
        w.data.write_b(self.explodable);
        w.data.write_rc(self.block_scaling);

        put(w, HARD_OWNER, self.block_entity)?;
        for &h in owned {
            put(w, HARD_OWNER, h)?;
        }
        put(w, HARD_OWNER, self.end_block)?;
        for &h in &self.inserts {
            put(w, SOFT_POINTER, h)?;
        }
        put(w, HARD_POINTER, self.layout)
    }
}

/// A layer (ODS 20.4.54).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Layer {
    /// Name and xref fields.
    pub entry: TableEntry,
    /// Frozen.
    pub frozen: bool,
    /// The layer is off: bit 2 of the flags short. Before R2004 a negative
    /// color index also means off (ODS), though AutoCAD's R2000 files keep
    /// the index positive and set the bit; either one reads as off.
    pub off: bool,
    /// Frozen in new viewports.
    pub frozen_in_new_viewports: bool,
    /// Locked.
    pub locked: bool,
    /// Plotted.
    pub plot: bool,
    /// Lineweight index (5 bits): 0-23 index the standard lineweights
    /// (0.00 mm to 2.11 mm), 29 is ByLayer, 30 ByBlock, 31 Default.
    pub lineweight: u8,
    /// Color.
    pub color: Color,
    /// Plot style (hard pointer): the PLACEHOLDER by default.
    pub plotstyle: Handle,
    /// R2007+ material (hard pointer); null before.
    pub material: Handle,
    /// Linetype (hard pointer).
    pub linetype: Handle,
    /// R2013+ hard pointer of unknown use (ODS: "always seems to be NULL").
    pub unknown_handle: Handle,
}

const LAYER_FROZEN: i16 = 1;
const LAYER_OFF: i16 = 2;
const LAYER_FROZEN_NEW_VP: i16 = 4;
const LAYER_LOCKED: i16 = 8;
const LAYER_PLOT: i16 = 0x10;
const LAYER_LINEWEIGHT_SHIFT: u32 = 5;

impl Layer {
    /// A new layer named `name`: on, thawed, unlocked, plotted, colour 7
    /// (white/black), the default lineweight. Its linetype, plot style and
    /// material are left null: [`crate::Drawing::add_layer`] gives such a
    /// layer the Continuous linetype and layer `0`'s plot style and
    /// material. Change the fields before adding it for other values.
    pub fn new(name: &str) -> Layer {
        Layer {
            entry: TableEntry {
                name: name.to_owned(),
                ..TableEntry::default()
            },
            frozen: false,
            off: false,
            frozen_in_new_viewports: false,
            locked: false,
            plot: true,
            lineweight: 31,
            color: Color::Index(7),
            plotstyle: Handle(0),
            material: Handle(0),
            linetype: Handle(0),
            unknown_handle: Handle(0),
        }
    }

    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let v = r.version;
        let entry = TableEntry::decode(r)?;
        let flags = r.data.read_bs()?;
        if flags & !0x3FF != 0 {
            return Err(corrupt(&format!(
                "layer flags {flags:#X} have unknown bits"
            )));
        }
        let (color, negated) = r.read_cmc_layer()?;
        let off = flags & LAYER_OFF != 0 || negated;
        let plotstyle = r.href_abs()?;
        let material = if v >= Version::R2007 {
            r.href_abs()?
        } else {
            Handle(0)
        };
        let linetype = r.href_abs()?;
        let unknown_handle = if v >= Version::R2013 {
            r.href_abs()?
        } else {
            Handle(0)
        };
        Ok(Layer {
            entry,
            frozen: flags & LAYER_FROZEN != 0,
            off,
            frozen_in_new_viewports: flags & LAYER_FROZEN_NEW_VP != 0,
            locked: flags & LAYER_LOCKED != 0,
            plot: flags & LAYER_PLOT != 0,
            lineweight: ((flags >> LAYER_LINEWEIGHT_SHIFT) & 0x1F) as u8,
            color,
            plotstyle,
            material,
            linetype,
            unknown_handle,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        if self.lineweight > 0x1F {
            return Err(encode_err("layer lineweight index is 5 bits"));
        }
        self.entry.encode(w)?;
        let mut flags = i16::from(self.lineweight) << LAYER_LINEWEIGHT_SHIFT;
        for (set, bit) in [
            (self.frozen, LAYER_FROZEN),
            (self.off, LAYER_OFF),
            (self.frozen_in_new_viewports, LAYER_FROZEN_NEW_VP),
            (self.locked, LAYER_LOCKED),
            (self.plot, LAYER_PLOT),
        ] {
            if set {
                flags |= bit;
            }
        }
        w.data.write_bs(flags);
        w.write_cmc(&self.color)?;
        put(w, HARD_POINTER, self.plotstyle)?;
        put(w, HARD_POINTER, self.material)?;
        put(w, HARD_POINTER, self.linetype)?;
        put(w, HARD_POINTER, self.unknown_handle)
    }
}

/// A text style or shape file (ODS 20.4.56, STYLE).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct TextStyle {
    pub entry: TableEntry,
    /// A shape file rather than a font (DXF 70 bit 1).
    pub shape_file: bool,
    /// Vertical text (DXF 70 bit 4).
    pub vertical: bool,
    pub fixed_height: f64,
    pub width_factor: f64,
    pub oblique_angle: f64,
    /// Generation flags (DXF 71): 2 backward, 4 upside down.
    pub generation: u8,
    pub last_height: f64,
    pub font_name: String,
    pub bigfont_name: String,
}

impl TextStyle {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let entry = TableEntry::decode(r)?;
        Ok(TextStyle {
            entry,
            shape_file: r.data.read_b()?,
            vertical: r.data.read_b()?,
            fixed_height: r.data.read_bd()?,
            width_factor: r.data.read_bd()?,
            oblique_angle: r.data.read_bd()?,
            generation: r.data.read_rc()?,
            last_height: r.data.read_bd()?,
            font_name: r.text()?,
            bigfont_name: r.text()?,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        self.entry.encode(w)?;
        w.data.write_b(self.shape_file);
        w.data.write_b(self.vertical);
        w.data.write_bd(self.fixed_height);
        w.data.write_bd(self.width_factor);
        w.data.write_bd(self.oblique_angle);
        w.data.write_rc(self.generation);
        w.data.write_bd(self.last_height);
        w.text(&self.font_name)?;
        w.text(&self.bigfont_name)
    }
}

/// One dash of a linetype.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct Dash {
    pub length: f64,
    /// Shape number (flag 0x04) or offset into the text area (flag 0x02).
    pub shape_number: i16,
    pub offset: [f64; 2],
    pub scale: f64,
    pub rotation: f64,
    /// DXF 74: 0x01 absolute rotation, 0x02 text, 0x04 shape.
    pub flags: i16,
    /// The text style or shape file of a text or shape (hard pointer).
    pub style: Handle,
}

/// The dash flag of a dash that draws text.
pub const DASH_TEXT: i16 = 0x02;

/// A linetype (ODS 20.4.58, LTYPE).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct Linetype {
    pub entry: TableEntry,
    pub description: String,
    pub pattern_length: f64,
    /// Always `b'A'`.
    pub alignment: u8,
    pub dashes: Vec<Dash>,
    /// The text of text dashes in R2007+ form: 512 bytes of NUL-terminated
    /// UTF-16LE strings when a dash has [`DASH_TEXT`], else empty.
    ///
    /// Before R2007 the area is 256 bytes of code page text, stored even
    /// without text dashes. It is converted on read when every text dash
    /// starts at offset 0 (the corpus only has that case, so the offset unit
    /// of the R2007+ area is unconfirmed); otherwise the linetype is kept
    /// raw. A pre-R2007 area without text dashes is not kept.
    pub text_area: Vec<u8>,
}

const TEXT_AREA_R2004: usize = 256;
const TEXT_AREA_R2007: usize = 512;

impl Linetype {
    fn has_text(&self) -> bool {
        self.dashes.iter().any(|d| d.flags & DASH_TEXT != 0)
    }

    /// The text drawn by dash `i`, when it is a text dash: the UTF-16 string
    /// at `shape_number` code units into the text area.
    pub fn text(&self, i: usize) -> Option<String> {
        let d = self.dashes.get(i).filter(|d| d.flags & DASH_TEXT != 0)?;
        let start = usize::try_from(d.shape_number).ok()?.checked_mul(2)?;
        let units: Vec<u16> = self
            .text_area
            .get(start..)?
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&u| u != 0)
            .collect();
        Some(String::from_utf16_lossy(&units))
    }

    /// Convert a pre-R2007 text area (see [`Linetype::text_area`]).
    fn convert_text_area(r: &mut ObjReader, dashes: &[Dash], area: &[u8]) -> Result<Vec<u8>> {
        let texts = dashes.iter().filter(|d| d.flags & DASH_TEXT != 0);
        if texts.clone().next().is_none() {
            return Ok(Vec::new());
        }
        if texts.clone().any(|d| d.shape_number != 0) {
            return Err(corrupt(
                "linetype text at an offset other than 0 cannot be converted to R2007+ form",
            ));
        }
        let end = area.iter().position(|&b| b == 0).unwrap_or(area.len());
        let (text, malformed) = crate::strings::decode_checked(r.codepage, &area[..end]);
        if malformed {
            r.diagnostics.push(crate::diag::Diagnostic {
                severity: crate::diag::Severity::Warning,
                handle: Some(r.handle),
                message: format!(
                    "linetype text is not valid in code page {}; malformed bytes became U+FFFD",
                    r.codepage
                ),
            });
        }
        let mut out: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        if out.len() + 2 > TEXT_AREA_R2007 {
            return Err(corrupt("linetype text does not fit the R2007+ text area"));
        }
        out.resize(TEXT_AREA_R2007, 0);
        Ok(out)
    }

    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let entry = TableEntry::decode(r)?;
        let description = r.text()?;
        let pattern_length = r.data.read_bd()?;
        let alignment = r.data.read_rc()?;
        let n = r.data.read_rc()?;
        let mut dashes = Vec::with_capacity(usize::from(n));
        for _ in 0..n {
            dashes.push(Dash {
                length: r.data.read_bd()?,
                shape_number: r.data.read_bs()?,
                offset: r.data.read_2rd()?,
                scale: r.data.read_bd()?,
                rotation: r.data.read_bd()?,
                flags: r.data.read_bs()?,
                style: Handle(0),
            });
        }
        let text_area = if r.version < Version::R2007 {
            let area = r.data.read_bytes(TEXT_AREA_R2004)?;
            Self::convert_text_area(r, &dashes, &area)?
        } else if dashes.iter().any(|d| d.flags & DASH_TEXT != 0) {
            r.data.read_bytes(TEXT_AREA_R2007)?
        } else {
            Vec::new()
        };
        for d in &mut dashes {
            d.style = r.href_abs()?;
        }
        Ok(Linetype {
            entry,
            description,
            pattern_length,
            alignment,
            dashes,
            text_area,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        let n = u8::try_from(self.dashes.len())
            .map_err(|_| encode_err("a linetype has at most 255 dashes"))?;
        let want = if self.has_text() { TEXT_AREA_R2007 } else { 0 };
        if self.text_area.len() != want {
            return Err(encode_err(format!(
                "linetype text area must be {want} bytes, is {}",
                self.text_area.len()
            )));
        }
        self.entry.encode(w)?;
        w.text(&self.description)?;
        w.data.write_bd(self.pattern_length);
        w.data.write_rc(self.alignment);
        w.data.write_rc(n);
        for d in &self.dashes {
            w.data.write_bd(d.length);
            w.data.write_bs(d.shape_number);
            w.data.write_2rd(d.offset);
            w.data.write_bd(d.scale);
            w.data.write_bd(d.rotation);
            w.data.write_bs(d.flags);
        }
        w.data.write_bytes(&self.text_area);
        for d in &self.dashes {
            put(w, HARD_POINTER, d.style)?;
        }
        Ok(())
    }
}

/// View mode bits stored as four `B` (DXF 71): perspective (1), front
/// clipping (2), back clipping (4), and the *opposite* of 16 (front clip not
/// at the eye).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct ViewMode {
    pub perspective: bool,
    pub front_clip: bool,
    pub back_clip: bool,
    /// The stored fourth bit (front clip not at the eye).
    pub front_clip_not_at_eye: bool,
}

impl ViewMode {
    fn decode(r: &mut ObjReader) -> Result<Self> {
        Ok(ViewMode {
            perspective: r.data.read_b()?,
            front_clip: r.data.read_b()?,
            back_clip: r.data.read_b()?,
            front_clip_not_at_eye: r.data.read_b()?,
        })
    }

    fn encode(&self, w: &mut ObjWriter) {
        w.data.write_b(self.perspective);
        w.data.write_b(self.front_clip);
        w.data.write_b(self.back_clip);
        w.data.write_b(self.front_clip_not_at_eye);
    }
}

/// R2007+ lighting of a view or viewport, with AutoCAD's defaults for older
/// files.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct Lighting {
    pub use_default_lights: bool,
    pub default_lighting_type: u8,
    pub brightness: f64,
    pub contrast: f64,
    pub ambient_color: Color,
}

impl Default for Lighting {
    fn default() -> Self {
        Lighting {
            use_default_lights: true,
            default_lighting_type: 1,
            brightness: 0.0,
            contrast: 0.0,
            ambient_color: Color::Index(250),
        }
    }
}

impl Lighting {
    fn decode(r: &mut ObjReader) -> Result<Self> {
        if r.version < Version::R2007 {
            return Ok(Lighting::default());
        }
        Ok(Lighting {
            use_default_lights: r.data.read_b()?,
            default_lighting_type: r.data.read_rc()?,
            brightness: r.data.read_bd()?,
            contrast: r.data.read_bd()?,
            ambient_color: r.read_cmc()?,
        })
    }

    fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        w.data.write_b(self.use_default_lights);
        w.data.write_rc(self.default_lighting_type);
        w.data.write_bd(self.brightness);
        w.data.write_bd(self.contrast);
        w.write_cmc(&self.ambient_color)
    }
}

/// A UCS as stored in a view or viewport.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct UcsFrame {
    pub origin: [f64; 3],
    pub x_axis: [f64; 3],
    pub y_axis: [f64; 3],
    pub elevation: f64,
    pub orthographic_type: i16,
}

impl UcsFrame {
    fn decode(r: &mut ObjReader) -> Result<Self> {
        Ok(UcsFrame {
            origin: r.data.read_3bd()?,
            x_axis: r.data.read_3bd()?,
            y_axis: r.data.read_3bd()?,
            elevation: r.data.read_bd()?,
            orthographic_type: r.data.read_bs()?,
        })
    }

    fn encode(&self, w: &mut ObjWriter) {
        w.data.write_3bd(self.origin);
        w.data.write_3bd(self.x_axis);
        w.data.write_3bd(self.y_axis);
        w.data.write_bd(self.elevation);
        w.data.write_bs(self.orthographic_type);
    }
}

/// A named view (ODS 20.4.60).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct View {
    pub entry: TableEntry,
    pub height: f64,
    pub width: f64,
    pub center: [f64; 2],
    pub target: [f64; 3],
    pub direction: [f64; 3],
    pub twist_angle: f64,
    pub lens_length: f64,
    pub front_clip: f64,
    pub back_clip: f64,
    pub view_mode: ViewMode,
    pub render_mode: u8,
    pub lighting: Lighting,
    /// A paper space view (DXF 70 bit 1).
    pub paper_space: bool,
    /// The view's UCS when one is associated (DXF 72).
    pub ucs: Option<UcsFrame>,
    /// R2007+ (DXF 73); `false` before.
    pub camera_plottable: bool,
    /// R2007+ background (soft pointer).
    pub background: Handle,
    /// R2007+ visual style (hard pointer).
    pub visual_style: Handle,
    /// R2007+ sun (hard owner).
    pub sun: Handle,
    /// The UCS's base UCS (hard pointer): stored only with a `ucs`.
    pub base_ucs: Handle,
    /// The named UCS (hard pointer): stored only with a `ucs`.
    pub named_ucs: Handle,
    /// R2007+ live section (soft pointer).
    pub live_section: Handle,
}

impl View {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let r2007 = r.version >= Version::R2007;
        let entry = TableEntry::decode(r)?;
        let height = r.data.read_bd()?;
        let width = r.data.read_bd()?;
        let center = r.data.read_2rd()?;
        let target = r.data.read_3bd()?;
        let direction = r.data.read_3bd()?;
        let twist_angle = r.data.read_bd()?;
        let lens_length = r.data.read_bd()?;
        let front_clip = r.data.read_bd()?;
        let back_clip = r.data.read_bd()?;
        let view_mode = ViewMode::decode(r)?;
        let render_mode = r.data.read_rc()?;
        let lighting = Lighting::decode(r)?;
        let paper_space = r.data.read_b()?;
        let ucs = if r.data.read_b()? {
            Some(UcsFrame::decode(r)?)
        } else {
            None
        };
        let camera_plottable = r2007 && r.data.read_b()?;
        let (background, visual_style, sun) = if r2007 {
            (r.href_abs()?, r.href_abs()?, r.href_abs()?)
        } else {
            (Handle(0), Handle(0), Handle(0))
        };
        // Only a view with a UCS stores the UCS handles (ODS 20.4.60;
        // AutoCAD 2025's views without one in `adsk-house-01`).
        let (base_ucs, named_ucs) = if ucs.is_some() {
            (r.href_abs()?, r.href_abs()?)
        } else {
            (Handle(0), Handle(0))
        };
        let live_section = if r2007 { r.href_abs()? } else { Handle(0) };
        Ok(View {
            entry,
            height,
            width,
            center,
            target,
            direction,
            twist_angle,
            lens_length,
            front_clip,
            back_clip,
            view_mode,
            render_mode,
            lighting,
            paper_space,
            ucs,
            camera_plottable,
            background,
            visual_style,
            sun,
            base_ucs,
            named_ucs,
            live_section,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        self.entry.encode(w)?;
        w.data.write_bd(self.height);
        w.data.write_bd(self.width);
        w.data.write_2rd(self.center);
        w.data.write_3bd(self.target);
        w.data.write_3bd(self.direction);
        w.data.write_bd(self.twist_angle);
        w.data.write_bd(self.lens_length);
        w.data.write_bd(self.front_clip);
        w.data.write_bd(self.back_clip);
        self.view_mode.encode(w);
        w.data.write_rc(self.render_mode);
        self.lighting.encode(w)?;
        w.data.write_b(self.paper_space);
        w.data.write_b(self.ucs.is_some());
        if let Some(u) = &self.ucs {
            u.encode(w);
        }
        w.data.write_b(self.camera_plottable);
        put(w, SOFT_POINTER, self.background)?;
        put(w, HARD_POINTER, self.visual_style)?;
        put(w, HARD_OWNER, self.sun)?;
        if self.ucs.is_some() {
            put(w, HARD_POINTER, self.base_ucs)?;
            put(w, HARD_POINTER, self.named_ucs)?;
        }
        put(w, SOFT_POINTER, self.live_section)
    }
}

/// A named UCS (ODS 20.4.62).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct Ucs {
    pub entry: TableEntry,
    pub origin: [f64; 3],
    pub x_axis: [f64; 3],
    pub y_axis: [f64; 3],
    pub elevation: f64,
    /// DXF 79.
    pub orthographic_view_type: i16,
    /// DXF 71.
    pub orthographic_type: i16,
    pub base_ucs: Handle,
    pub named_ucs: Handle,
}

impl Ucs {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let entry = TableEntry::decode(r)?;
        Ok(Ucs {
            entry,
            origin: r.data.read_3bd()?,
            x_axis: r.data.read_3bd()?,
            y_axis: r.data.read_3bd()?,
            elevation: r.data.read_bd()?,
            orthographic_view_type: r.data.read_bs()?,
            orthographic_type: r.data.read_bs()?,
            base_ucs: r.href_abs()?,
            named_ucs: r.href_abs()?,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        self.entry.encode(w)?;
        w.data.write_3bd(self.origin);
        w.data.write_3bd(self.x_axis);
        w.data.write_3bd(self.y_axis);
        w.data.write_bd(self.elevation);
        w.data.write_bs(self.orthographic_view_type);
        w.data.write_bs(self.orthographic_type);
        put(w, HARD_POINTER, self.base_ucs)?;
        put(w, HARD_POINTER, self.named_ucs)
    }
}

/// A viewport table entry (ODS 20.4.64, VPORT).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct Vport {
    pub entry: TableEntry,
    pub view_height: f64,
    /// As stored: the aspect ratio times the view height.
    pub aspect_ratio_times_height: f64,
    pub view_center: [f64; 2],
    pub view_target: [f64; 3],
    pub view_direction: [f64; 3],
    pub view_twist: f64,
    pub lens_length: f64,
    pub front_clip: f64,
    pub back_clip: f64,
    pub view_mode: ViewMode,
    pub render_mode: u8,
    pub lighting: Lighting,
    pub lower_left: [f64; 2],
    pub upper_right: [f64; 2],
    /// UCSFOLLOW (DXF 71 bit 8).
    pub ucs_follow: bool,
    pub circle_zoom: i16,
    pub fast_zoom: bool,
    /// UCSICON bits (DXF 74): 1 on, 2 at origin.
    pub ucs_icon: u8,
    pub grid_on: bool,
    pub grid_spacing: [f64; 2],
    pub snap_on: bool,
    pub snap_style: bool,
    pub snap_isopair: i16,
    pub snap_rotation: f64,
    pub snap_base: [f64; 2],
    pub snap_spacing: [f64; 2],
    /// R2000+ bit of unknown use (ODS "Unknown").
    pub unknown_bit: bool,
    pub ucs_per_viewport: bool,
    pub ucs: UcsFrame,
    /// R2007+ grid flags (DXF 60); 3 before.
    pub grid_flags: i16,
    /// R2007+ grid lines per major line (DXF 61); 5 before.
    pub grid_major: i16,
    /// R2007+ background (soft pointer).
    pub background: Handle,
    /// R2007+ visual style (hard pointer).
    pub visual_style: Handle,
    /// R2007+ sun (hard owner).
    pub sun: Handle,
    pub named_ucs: Handle,
    pub base_ucs: Handle,
}

impl Vport {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let r2007 = r.version >= Version::R2007;
        let entry = TableEntry::decode(r)?;
        let view_height = r.data.read_bd()?;
        let aspect_ratio_times_height = r.data.read_bd()?;
        let view_center = r.data.read_2rd()?;
        let view_target = r.data.read_3bd()?;
        let view_direction = r.data.read_3bd()?;
        let view_twist = r.data.read_bd()?;
        let lens_length = r.data.read_bd()?;
        let front_clip = r.data.read_bd()?;
        let back_clip = r.data.read_bd()?;
        let view_mode = ViewMode::decode(r)?;
        let render_mode = r.data.read_rc()?;
        let lighting = Lighting::decode(r)?;
        let lower_left = r.data.read_2rd()?;
        let upper_right = r.data.read_2rd()?;
        let ucs_follow = r.data.read_b()?;
        let circle_zoom = r.data.read_bs()?;
        let fast_zoom = r.data.read_b()?;
        let ucs_icon = u8::from(r.data.read_b()?) | u8::from(r.data.read_b()?) << 1;
        let grid_on = r.data.read_b()?;
        let grid_spacing = r.data.read_2rd()?;
        let snap_on = r.data.read_b()?;
        let snap_style = r.data.read_b()?;
        let snap_isopair = r.data.read_bs()?;
        let snap_rotation = r.data.read_bd()?;
        let snap_base = r.data.read_2rd()?;
        let snap_spacing = r.data.read_2rd()?;
        let unknown_bit = r.data.read_b()?;
        let ucs_per_viewport = r.data.read_b()?;
        let ucs = UcsFrame::decode(r)?;
        let (grid_flags, grid_major) = if r2007 {
            (r.data.read_bs()?, r.data.read_bs()?)
        } else {
            (3, 5)
        };
        let (background, visual_style, sun) = if r2007 {
            (r.href_abs()?, r.href_abs()?, r.href_abs()?)
        } else {
            (Handle(0), Handle(0), Handle(0))
        };
        let named_ucs = r.href_abs()?;
        let base_ucs = r.href_abs()?;
        Ok(Vport {
            entry,
            view_height,
            aspect_ratio_times_height,
            view_center,
            view_target,
            view_direction,
            view_twist,
            lens_length,
            front_clip,
            back_clip,
            view_mode,
            render_mode,
            lighting,
            lower_left,
            upper_right,
            ucs_follow,
            circle_zoom,
            fast_zoom,
            ucs_icon,
            grid_on,
            grid_spacing,
            snap_on,
            snap_style,
            snap_isopair,
            snap_rotation,
            snap_base,
            snap_spacing,
            unknown_bit,
            ucs_per_viewport,
            ucs,
            grid_flags,
            grid_major,
            background,
            visual_style,
            sun,
            named_ucs,
            base_ucs,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        if self.ucs_icon > 3 {
            return Err(encode_err("UCSICON is two bits"));
        }
        self.entry.encode(w)?;
        w.data.write_bd(self.view_height);
        w.data.write_bd(self.aspect_ratio_times_height);
        w.data.write_2rd(self.view_center);
        w.data.write_3bd(self.view_target);
        w.data.write_3bd(self.view_direction);
        w.data.write_bd(self.view_twist);
        w.data.write_bd(self.lens_length);
        w.data.write_bd(self.front_clip);
        w.data.write_bd(self.back_clip);
        self.view_mode.encode(w);
        w.data.write_rc(self.render_mode);
        self.lighting.encode(w)?;
        w.data.write_2rd(self.lower_left);
        w.data.write_2rd(self.upper_right);
        w.data.write_b(self.ucs_follow);
        w.data.write_bs(self.circle_zoom);
        w.data.write_b(self.fast_zoom);
        w.data.write_b(self.ucs_icon & 1 != 0);
        w.data.write_b(self.ucs_icon & 2 != 0);
        w.data.write_b(self.grid_on);
        w.data.write_2rd(self.grid_spacing);
        w.data.write_b(self.snap_on);
        w.data.write_b(self.snap_style);
        w.data.write_bs(self.snap_isopair);
        w.data.write_bd(self.snap_rotation);
        w.data.write_2rd(self.snap_base);
        w.data.write_2rd(self.snap_spacing);
        w.data.write_b(self.unknown_bit);
        w.data.write_b(self.ucs_per_viewport);
        self.ucs.encode(w);
        w.data.write_bs(self.grid_flags);
        w.data.write_bs(self.grid_major);
        put(w, SOFT_POINTER, self.background)?;
        put(w, HARD_POINTER, self.visual_style)?;
        put(w, HARD_OWNER, self.sun)?;
        put(w, HARD_POINTER, self.named_ucs)?;
        put(w, HARD_POINTER, self.base_ucs)
    }
}

/// A registered application (ODS 20.4.66, APPID).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct AppId {
    pub entry: TableEntry,
    /// Undocumented DXF 71 byte.
    pub unknown: u8,
}

impl AppId {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let entry = TableEntry::decode(r)?;
        Ok(AppId {
            entry,
            unknown: r.data.read_rc()?,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        self.entry.encode(w)?;
        w.data.write_rc(self.unknown);
        Ok(())
    }
}

/// A viewport entity header (ODS 20.4.70, VP_ENT_HDR); R2000 files keep one
/// per paper space viewport.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct VpEntHdr {
    pub entry: TableEntry,
    /// The 1 bit of DXF 70.
    pub flag: bool,
    /// The viewport entity (soft pointer).
    pub viewport: Handle,
    /// The previous header in the chain (hard pointer); null at the end.
    pub prev: Handle,
}

impl VpEntHdr {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let entry = TableEntry::decode(r)?;
        Ok(VpEntHdr {
            entry,
            flag: r.data.read_b()?,
            viewport: r.href_abs()?,
            prev: r.href_abs()?,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        self.entry.encode(w)?;
        w.data.write_b(self.flag);
        put(w, SOFT_POINTER, self.viewport)?;
        put(w, HARD_POINTER, self.prev)
    }
}

/// A dimension style (ODS 20.4.68, DIMSTYLE). Fields are the DIMxxx system
/// variables of the same names; R2007+ and R2010+ fields hold AutoCAD's
/// defaults when read from older files.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct DimStyle {
    pub entry: TableEntry,
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
    /// R2007+; 1.0 before.
    pub dimfxl: f64,
    /// R2007+; 45 degrees (in radians) before.
    pub dimjogang: f64,
    /// R2007+; 0 before.
    pub dimtfill: i16,
    /// R2007+; ByBlock before.
    pub dimtfillclr: Color,
    pub dimtol: bool,
    pub dimlim: bool,
    pub dimtih: bool,
    pub dimtoh: bool,
    pub dimse1: bool,
    pub dimse2: bool,
    pub dimtad: i16,
    pub dimzin: i16,
    pub dimazin: i16,
    /// R2007+; 0 before.
    pub dimarcsym: i16,
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
    pub dimfit: i16,
    /// R2007+; `false` before.
    pub dimfxlon: bool,
    /// R2010+; `false` before.
    pub dimtxtdirection: bool,
    /// R2010+; 100.0 before.
    pub dimaltmzf: f64,
    /// R2010+; empty before.
    pub dimaltmzs: String,
    /// R2010+; 100.0 before.
    pub dimmzf: f64,
    /// R2010+; empty before.
    pub dimmzs: String,
    pub dimlwd: i16,
    pub dimlwe: i16,
    /// Bit 1 of DXF 70 (ODS: "seems to set the 0-bit").
    pub flag: bool,
    /// DIMTXSTY (hard pointer).
    pub dimtxsty: Handle,
    pub dimldrblk: Handle,
    pub dimblk: Handle,
    pub dimblk1: Handle,
    pub dimblk2: Handle,
    /// R2007+ linetypes (hard pointers); null before.
    pub dimltype: Handle,
    pub dimltex1: Handle,
    pub dimltex2: Handle,
}

impl DimStyle {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let r2007 = r.version >= Version::R2007;
        let r2010 = r.version >= Version::R2010;
        let entry = TableEntry::decode(r)?;
        let dimpost = r.text()?;
        let dimapost = r.text()?;
        let [
            dimscale,
            dimasz,
            dimexo,
            dimdli,
            dimexe,
            dimrnd,
            dimdle,
            dimtp,
            dimtm,
        ] = read_bds::<9>(r)?;
        let (dimfxl, dimjogang, dimtfill, dimtfillclr) = if r2007 {
            (
                r.data.read_bd()?,
                r.data.read_bd()?,
                r.data.read_bs()?,
                r.read_cmc()?,
            )
        } else {
            (1.0, std::f64::consts::FRAC_PI_4, 0, Color::ByBlock)
        };
        let [dimtol, dimlim, dimtih, dimtoh, dimse1, dimse2] = read_bits::<6>(r)?;
        let [dimtad, dimzin, dimazin] = read_bss::<3>(r)?;
        let dimarcsym = if r2007 { r.data.read_bs()? } else { 0 };
        let [
            dimtxt,
            dimcen,
            dimtsz,
            dimaltf,
            dimlfac,
            dimtvp,
            dimtfac,
            dimgap,
            dimaltrnd,
        ] = read_bds::<9>(r)?;
        let dimalt = r.data.read_b()?;
        let dimaltd = r.data.read_bs()?;
        let [dimtofl, dimsah, dimtix, dimsoxd] = read_bits::<4>(r)?;
        let dimclrd = r.read_cmc()?;
        let dimclre = r.read_cmc()?;
        let dimclrt = r.read_cmc()?;
        let [
            dimadec,
            dimdec,
            dimtdec,
            dimaltu,
            dimalttd,
            dimaunit,
            dimfrac,
            dimlunit,
            dimdsep,
            dimtmove,
            dimjust,
        ] = read_bss::<11>(r)?;
        let [dimsd1, dimsd2] = read_bits::<2>(r)?;
        let [dimtolj, dimtzin, dimaltz, dimalttz] = read_bss::<4>(r)?;
        let dimupt = r.data.read_b()?;
        let dimfit = r.data.read_bs()?;
        let dimfxlon = r2007 && r.data.read_b()?;
        let (dimtxtdirection, dimaltmzf, dimaltmzs, dimmzf, dimmzs) = if r2010 {
            (
                r.data.read_b()?,
                r.data.read_bd()?,
                r.text()?,
                r.data.read_bd()?,
                r.text()?,
            )
        } else {
            (false, 100.0, String::new(), 100.0, String::new())
        };
        let [dimlwd, dimlwe] = read_bss::<2>(r)?;
        let flag = r.data.read_b()?;
        let dimtxsty = r.href_abs()?;
        let [dimldrblk, dimblk, dimblk1, dimblk2] = read_hrefs::<4>(r)?;
        let [dimltype, dimltex1, dimltex2] = if r2007 {
            read_hrefs::<3>(r)?
        } else {
            [Handle(0); 3]
        };
        Ok(DimStyle {
            entry,
            dimpost,
            dimapost,
            dimscale,
            dimasz,
            dimexo,
            dimdli,
            dimexe,
            dimrnd,
            dimdle,
            dimtp,
            dimtm,
            dimfxl,
            dimjogang,
            dimtfill,
            dimtfillclr,
            dimtol,
            dimlim,
            dimtih,
            dimtoh,
            dimse1,
            dimse2,
            dimtad,
            dimzin,
            dimazin,
            dimarcsym,
            dimtxt,
            dimcen,
            dimtsz,
            dimaltf,
            dimlfac,
            dimtvp,
            dimtfac,
            dimgap,
            dimaltrnd,
            dimalt,
            dimaltd,
            dimtofl,
            dimsah,
            dimtix,
            dimsoxd,
            dimclrd,
            dimclre,
            dimclrt,
            dimadec,
            dimdec,
            dimtdec,
            dimaltu,
            dimalttd,
            dimaunit,
            dimfrac,
            dimlunit,
            dimdsep,
            dimtmove,
            dimjust,
            dimsd1,
            dimsd2,
            dimtolj,
            dimtzin,
            dimaltz,
            dimalttz,
            dimupt,
            dimfit,
            dimfxlon,
            dimtxtdirection,
            dimaltmzf,
            dimaltmzs,
            dimmzf,
            dimmzs,
            dimlwd,
            dimlwe,
            flag,
            dimtxsty,
            dimldrblk,
            dimblk,
            dimblk1,
            dimblk2,
            dimltype,
            dimltex1,
            dimltex2,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        self.entry.encode(w)?;
        w.text(&self.dimpost)?;
        w.text(&self.dimapost)?;
        for v in [
            self.dimscale,
            self.dimasz,
            self.dimexo,
            self.dimdli,
            self.dimexe,
            self.dimrnd,
            self.dimdle,
            self.dimtp,
            self.dimtm,
            self.dimfxl,
            self.dimjogang,
        ] {
            w.data.write_bd(v);
        }
        w.data.write_bs(self.dimtfill);
        w.write_cmc(&self.dimtfillclr)?;
        for b in [
            self.dimtol,
            self.dimlim,
            self.dimtih,
            self.dimtoh,
            self.dimse1,
            self.dimse2,
        ] {
            w.data.write_b(b);
        }
        for v in [self.dimtad, self.dimzin, self.dimazin, self.dimarcsym] {
            w.data.write_bs(v);
        }
        for v in [
            self.dimtxt,
            self.dimcen,
            self.dimtsz,
            self.dimaltf,
            self.dimlfac,
            self.dimtvp,
            self.dimtfac,
            self.dimgap,
            self.dimaltrnd,
        ] {
            w.data.write_bd(v);
        }
        w.data.write_b(self.dimalt);
        w.data.write_bs(self.dimaltd);
        for b in [self.dimtofl, self.dimsah, self.dimtix, self.dimsoxd] {
            w.data.write_b(b);
        }
        for c in [&self.dimclrd, &self.dimclre, &self.dimclrt] {
            w.write_cmc(c)?;
        }
        for v in [
            self.dimadec,
            self.dimdec,
            self.dimtdec,
            self.dimaltu,
            self.dimalttd,
            self.dimaunit,
            self.dimfrac,
            self.dimlunit,
            self.dimdsep,
            self.dimtmove,
            self.dimjust,
        ] {
            w.data.write_bs(v);
        }
        w.data.write_b(self.dimsd1);
        w.data.write_b(self.dimsd2);
        for v in [self.dimtolj, self.dimtzin, self.dimaltz, self.dimalttz] {
            w.data.write_bs(v);
        }
        w.data.write_b(self.dimupt);
        w.data.write_bs(self.dimfit);
        w.data.write_b(self.dimfxlon);
        w.data.write_b(self.dimtxtdirection);
        w.data.write_bd(self.dimaltmzf);
        w.text(&self.dimaltmzs)?;
        w.data.write_bd(self.dimmzf);
        w.text(&self.dimmzs)?;
        w.data.write_bs(self.dimlwd);
        w.data.write_bs(self.dimlwe);
        w.data.write_b(self.flag);
        for h in [
            self.dimtxsty,
            self.dimldrblk,
            self.dimblk,
            self.dimblk1,
            self.dimblk2,
            self.dimltype,
            self.dimltex1,
            self.dimltex2,
        ] {
            put(w, HARD_POINTER, h)?;
        }
        Ok(())
    }
}

fn read_bds<const N: usize>(r: &mut ObjReader) -> Result<[f64; N]> {
    let mut out = [0.0; N];
    for v in &mut out {
        *v = r.data.read_bd()?;
    }
    Ok(out)
}

fn read_bss<const N: usize>(r: &mut ObjReader) -> Result<[i16; N]> {
    let mut out = [0; N];
    for v in &mut out {
        *v = r.data.read_bs()?;
    }
    Ok(out)
}

fn read_bits<const N: usize>(r: &mut ObjReader) -> Result<[bool; N]> {
    let mut out = [false; N];
    for v in &mut out {
        *v = r.data.read_b()?;
    }
    Ok(out)
}

fn read_hrefs<const N: usize>(r: &mut ObjReader) -> Result<[Handle; N]> {
    let mut out = [Handle(0); N];
    for v in &mut out {
        *v = r.href_abs()?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::BitWriter;
    use crate::object::stream::tests::assemble;
    use crate::object::type_code;

    const H: Handle = Handle(0x80);

    fn entry(name: &str) -> TableEntry {
        TableEntry {
            name: name.into(),
            xref_index: 3,
            xref_dependent: true,
            xref_block: Handle(0x41),
        }
    }

    /// Encode `v` as an R2018 object and split it again.
    fn encoded<T>(v: &T, enc: fn(&T, &mut ObjWriter) -> Result<()>, code: u16) -> Vec<u8> {
        let mut w = ObjWriter::new(H);
        enc(v, &mut w).unwrap();
        w.finish(code).unwrap()
    }

    fn reader(bytes: &[u8]) -> ObjReader<'_> {
        let (mut r, _) = ObjReader::split(bytes, Version::R2018, 30).unwrap();
        r.handle = H;
        r
    }

    /// Encode, decode and compare; then cut each stream short at every bit
    /// and check the decode fails cleanly.
    fn roundtrip<T: PartialEq + std::fmt::Debug>(
        v: &T,
        enc: fn(&T, &mut ObjWriter) -> Result<()>,
        dec: fn(&mut ObjReader) -> Result<T>,
        code: u16,
    ) {
        let bytes = encoded(v, enc, code);
        let mut r = reader(&bytes);
        assert_eq!(&dec(&mut r).unwrap(), v);
        assert_eq!(r.data.remaining_bits(), 0);
        assert_eq!(r.strings.as_ref().unwrap().remaining_bits(), 0);
        assert!(r.handles.remaining_bits() < 8);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);

        let full = reader(&bytes);
        let lens = [
            full.data.remaining_bits(),
            full.strings.as_ref().unwrap().remaining_bits(),
            full.handles.remaining_bits() / 8 * 8,
        ];
        for (stream, &len) in lens.iter().enumerate() {
            for cut in 0..len {
                let mut r = reader(&bytes);
                match stream {
                    0 => r.data.limit_remaining(cut),
                    1 => r.strings.as_mut().unwrap().limit_remaining(cut),
                    _ => r.handles.limit_remaining(cut),
                }
                assert!(dec(&mut r).is_err(), "stream {stream} cut to {cut} bits");
            }
        }
    }

    /// Whole objects (common data included) with flipped bits or cut short
    /// decode or fail, but never panic.
    #[test]
    fn hostile_table_objects_never_panic() {
        use crate::object::{
            DecodeCtx, NonGraphical, Object, ObjectCommon, decode_object, encode_object,
        };
        let objects = [
            NonGraphical::BlockRecord(block_record()),
            NonGraphical::Layer(layer()),
            NonGraphical::Linetype(linetype()),
            NonGraphical::TableControl(TableControl {
                kind: TableKind::DimStyle,
                entries: vec![Handle(0x27), Handle(0)],
                extra: vec![Handle(0x27)],
            }),
        ];
        for object in objects {
            let obj = Object::Other {
                common: ObjectCommon {
                    handle: H,
                    owner: Handle(1),
                    owner_relative: false,
                    reactors: vec![Handle(2)],
                    xdict: None,
                    xdata: vec![],
                    has_ds_binary_data: false,
                },
                object,
            };
            let good = encode_object(&obj, &[]).unwrap();
            let ctx = || DecodeCtx::new(Version::R2018, 30, &[]);
            let back = decode_object(&good, H, &mut ctx()).unwrap();
            assert_eq!(back, obj);
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

    #[test]
    fn roundtrip_table_control() {
        let block = TableControl {
            kind: TableKind::Block,
            entries: vec![Handle(0x58), Handle(0), Handle(0x5D)],
            extra: vec![Handle(0x1F), Handle(0x259)],
        };
        let dec = |r: &mut ObjReader| TableControl::decode(r, TableKind::Block);
        roundtrip(&block, TableControl::encode, dec, type_code::BLOCK_CONTROL);
        let dims = TableControl {
            kind: TableKind::DimStyle,
            entries: vec![Handle(0x27), Handle(0xE1), Handle(0x39A)],
            extra: vec![Handle(0x39A), Handle(0x27)],
        };
        let dec = |r: &mut ObjReader| TableControl::decode(r, TableKind::DimStyle);
        roundtrip(
            &dims,
            TableControl::encode,
            dec,
            type_code::DIMSTYLE_CONTROL,
        );
        let layers = TableControl {
            kind: TableKind::Layer,
            entries: vec![Handle(0x10)],
            extra: vec![],
        };
        let bytes = encoded(&layers, TableControl::encode, type_code::LAYER_CONTROL);
        let back = TableControl::decode(&mut reader(&bytes), TableKind::Layer).unwrap();
        assert_eq!(back, layers);

        let bad = TableControl {
            extra: vec![Handle(1)],
            ..block
        };
        assert!(bad.encode(&mut ObjWriter::new(H)).is_err());
    }

    #[test]
    fn table_control_count_is_bounded() {
        let mut d = BitWriter::new();
        d.write_bl(i32::MAX);
        let bytes = assemble(
            Version::R2018,
            type_code::LAYER_CONTROL,
            &d,
            None,
            &BitWriter::new(),
        );
        let (mut r, _) = ObjReader::split(&bytes, Version::R2018, 30).unwrap();
        assert!(TableControl::decode(&mut r, TableKind::Layer).is_err());
    }

    fn block_record() -> BlockRecord {
        BlockRecord {
            entry: entry("Bloc \u{e9} \u{56f3}"),
            anonymous: true,
            has_attdefs: true,
            is_xref: false,
            xref_overlaid: false,
            xref_unloaded: true,
            base_point: [1.0, -2.5, 3.0],
            xref_path: "C:\\x.dwg".into(),
            description: "desc".into(),
            preview: vec![1, 2, 3, 0xFF],
            insert_units: 4,
            explodable: false,
            block_scaling: 1,
            block_entity: Handle(0x5A),
            entities: EntityList::Owned(vec![Handle(0x240), Handle(0x245)]),
            end_block: Handle(0x5B),
            inserts: vec![Handle(0x660), Handle(0x661)],
            layout: Handle(0x59),
        }
    }

    #[test]
    fn roundtrip_block_record() {
        let b = block_record();
        roundtrip(
            &b,
            BlockRecord::encode,
            BlockRecord::decode,
            type_code::BLOCK_HEADER,
        );
        let linked = BlockRecord {
            entities: EntityList::Linked {
                first: Handle(1),
                last: Handle(2),
            },
            ..b
        };
        assert!(linked.encode(&mut ObjWriter::new(H)).is_err());
    }

    /// An R2000 BLOCK_HEADER stores the first and last entity; an R2000 xref
    /// block stores neither.
    #[test]
    fn r2000_block_record_is_linked() {
        for xref in [false, true] {
            let mut d = BitWriter::new();
            d.write_tv_raw(b"B").unwrap();
            d.write_b(true); // 64-flag
            d.write_bs(0);
            d.write_b(false);
            for bit in [false, false, xref, false, false] {
                d.write_b(bit);
            }
            d.write_3bd([0.0; 3]);
            d.write_tv_raw(b"").unwrap();
            d.write_rc(1); // one insert
            d.write_rc(0);
            d.write_tv_raw(b"").unwrap();
            d.write_bl(0); // preview
            let mut hs = BitWriter::new();
            let mut refs = vec![(5, 0), (3, 0x20)];
            if !xref {
                refs.extend([(4, 0x30), (4, 0x34)]);
            }
            refs.extend([(3, 0x21), (4, 0x99), (5, 0x22)]);
            for (code, value) in refs {
                hs.write_handle_ref(HandleRef { code, value }).unwrap();
            }
            let bytes = assemble(Version::R2000, type_code::BLOCK_HEADER, &d, None, &hs);
            let (mut r, _) = ObjReader::split(&bytes, Version::R2000, 30).unwrap();
            let b = BlockRecord::decode(&mut r).unwrap();
            let want = if xref {
                EntityList::Owned(vec![])
            } else {
                EntityList::Linked {
                    first: Handle(0x30),
                    last: Handle(0x34),
                }
            };
            assert_eq!(b.entities, want);
            assert_eq!(b.inserts, [Handle(0x99)]);
            assert_eq!(b.layout, Handle(0x22));
            assert_eq!(b.end_block, Handle(0x21));
            // R2007+ fields take AutoCAD's defaults.
            assert_eq!(
                (b.insert_units, b.explodable, b.block_scaling),
                (0, true, 0)
            );
            assert_eq!(r.data.remaining_bits(), 0);
            assert!(r.handles.remaining_bits() < 8);
        }
    }

    /// From R2004 a BLOCK_HEADER stores its owned object count and owned
    /// entities, but an xref or overlaid xref block stores neither (Civil 3D
    /// 2023's xref block `Xref01` in `camber-26` of the extended corpus).
    #[test]
    fn r2004_xref_block_record_has_no_owned_count() {
        for (xref, overlaid) in [(false, false), (true, false), (false, true)] {
            let mut d = BitWriter::new();
            d.write_tv_raw(b"B").unwrap();
            d.write_b(true); // 64-flag
            d.write_bs(0);
            d.write_b(false);
            for bit in [false, false, xref, overlaid, false] {
                d.write_b(bit);
            }
            let plain = !xref && !overlaid;
            if plain {
                d.write_bl(2);
            }
            d.write_3bd([0.0; 3]);
            d.write_tv_raw(b"x.dwg").unwrap();
            d.write_rc(1); // one insert
            d.write_rc(0);
            d.write_tv_raw(b"").unwrap();
            d.write_bl(0); // preview
            let mut hs = BitWriter::new();
            let mut refs = vec![(5, 0), (3, 0x20)];
            if plain {
                refs.extend([(3, 0x30), (3, 0x34)]);
            }
            refs.extend([(3, 0x21), (4, 0x99), (5, 0x22)]);
            for (code, value) in refs {
                hs.write_handle_ref(HandleRef { code, value }).unwrap();
            }
            let bytes = assemble(Version::R2004, type_code::BLOCK_HEADER, &d, None, &hs);
            let (mut r, _) = ObjReader::split(&bytes, Version::R2004, 30).unwrap();
            let b = BlockRecord::decode(&mut r).unwrap();
            let want: &[Handle] = if plain {
                &[Handle(0x30), Handle(0x34)]
            } else {
                &[]
            };
            assert_eq!(b.entities, EntityList::Owned(want.to_vec()));
            assert_eq!((b.is_xref, b.xref_overlaid), (xref, overlaid));
            assert_eq!(b.xref_path, "x.dwg");
            assert_eq!(b.end_block, Handle(0x21));
            assert_eq!(b.inserts, [Handle(0x99)]);
            assert_eq!(b.layout, Handle(0x22));
            assert_eq!(r.data.remaining_bits(), 0);
            assert!(r.handles.remaining_bits() < 8);
            // Written as R2018 and read back, the same.
            roundtrip(
                &b,
                BlockRecord::encode,
                BlockRecord::decode,
                type_code::BLOCK_HEADER,
            );
        }
        // An xref block cannot own entities.
        let b = BlockRecord {
            is_xref: true,
            ..block_record()
        };
        assert!(b.encode(&mut ObjWriter::new(H)).is_err());
    }

    fn layer() -> Layer {
        Layer {
            entry: entry("Layer_Off"),
            frozen: true,
            off: true,
            frozen_in_new_viewports: false,
            locked: true,
            plot: true,
            lineweight: 9,
            color: Color::Rgb {
                r: 125,
                g: 33,
                b: 79,
            },
            plotstyle: Handle(0xF),
            material: Handle(0x98),
            linetype: Handle(0x16),
            unknown_handle: Handle(0),
        }
    }

    #[test]
    fn roundtrip_layer() {
        let l = layer();
        roundtrip(&l, Layer::encode, Layer::decode, type_code::LAYER);
        let book = Layer {
            color: Color::Book {
                rgb: 0xFCC187,
                color_name: "DIC 7".into(),
                book_name: "DIC COLOR GUIDE(R)".into(),
            },
            ..layer()
        };
        roundtrip(&book, Layer::encode, Layer::decode, type_code::LAYER);
        let bad = Layer {
            lineweight: 32,
            ..layer()
        };
        assert!(bad.encode(&mut ObjWriter::new(H)).is_err());
        let bad = Layer {
            entry: TableEntry {
                xref_index: 0x100,
                ..entry("x")
            },
            ..layer()
        };
        assert!(bad.encode(&mut ObjWriter::new(H)).is_err());
    }

    /// An R2000 LAYER: `B 64-flag, BS xrefindex+1, B xdep`, the flags short,
    /// a `BS` color (negative when off, ODS 20.4.54), and three handles.
    fn r2000_layer(flags: i16, color: i16) -> Vec<u8> {
        let mut d = BitWriter::new();
        d.write_tv_raw(b"L").unwrap();
        d.write_b(true);
        d.write_bs(0);
        d.write_b(false);
        d.write_bs(flags);
        d.write_bs(color);
        let mut hs = BitWriter::new();
        for value in [0, 0xF, 0x16] {
            hs.write_handle_ref(HandleRef { code: 5, value }).unwrap();
        }
        assemble(Version::R2000, type_code::LAYER, &d, None, &hs)
    }

    #[test]
    fn r2000_layer_off_reads_from_flag_or_color_sign() {
        for (flags, color, off) in [
            (0x3E0 | 0x10, 7, false),
            (0x3E0 | 0x10 | LAYER_OFF, 7, true), // as AutoCAD writes R2000
            (0x3E0 | 0x10, -7, true),            // as ODS describes
        ] {
            let bytes = r2000_layer(flags, color);
            let (mut r, _) = ObjReader::split(&bytes, Version::R2000, 30).unwrap();
            let l = Layer::decode(&mut r).unwrap();
            assert_eq!(l.off, off, "{flags:#X} {color}");
            assert_eq!(l.color, Color::Index(7));
            assert_eq!((l.lineweight, l.plot), (31, true));
            assert_eq!((l.material, l.unknown_handle), (Handle(0), Handle(0)));
            assert_eq!(r.data.remaining_bits(), 0);
        }
        // Unknown flag bits are not silently dropped.
        let bytes = r2000_layer(0x400, 7);
        let (mut r, _) = ObjReader::split(&bytes, Version::R2000, 30).unwrap();
        assert!(Layer::decode(&mut r).is_err());
    }

    #[test]
    fn roundtrip_text_style() {
        let s = TextStyle {
            entry: entry("Standard"),
            shape_file: true,
            vertical: true,
            fixed_height: 2.5,
            width_factor: 0.8,
            oblique_angle: 0.25,
            generation: 6,
            last_height: 7.5,
            font_name: "arial.ttf".into(),
            bigfont_name: "big.shx".into(),
        };
        roundtrip(&s, TextStyle::encode, TextStyle::decode, type_code::STYLE);
    }

    fn gas_area() -> Vec<u8> {
        let mut a: Vec<u8> = "GAS".encode_utf16().flat_map(u16::to_le_bytes).collect();
        a.resize(512, 0);
        a
    }

    fn linetype() -> Linetype {
        let dash = |length, flags, style| Dash {
            length,
            shape_number: 0,
            offset: [-0.1, -0.05],
            scale: 0.1,
            rotation: 0.5,
            flags,
            style: Handle(style),
        };
        Linetype {
            entry: entry("GAS_LINE"),
            description: "Gas line ----GAS----".into(),
            pattern_length: 0.95,
            alignment: b'A',
            dashes: vec![
                dash(0.5, 0, 0),
                dash(-0.2, 0x0A, 0x11),
                dash(-0.25, 4, 0x27F),
            ],
            text_area: gas_area(),
        }
    }

    #[test]
    fn roundtrip_linetype() {
        let l = linetype();
        assert_eq!(l.text(1).as_deref(), Some("GAS"));
        assert_eq!(l.text(0), None);
        assert_eq!(l.text(9), None);
        roundtrip(&l, Linetype::encode, Linetype::decode, type_code::LTYPE);
        let plain = Linetype {
            dashes: vec![],
            text_area: vec![],
            ..linetype()
        };
        roundtrip(&plain, Linetype::encode, Linetype::decode, type_code::LTYPE);
        let bad = Linetype {
            text_area: vec![0; 256],
            ..linetype()
        };
        assert!(bad.encode(&mut ObjWriter::new(H)).is_err());
    }

    /// An R2004 LTYPE with one text dash at `offset` and the 256-byte code
    /// page text area.
    fn r2004_linetype(offset: i16, text: &[u8]) -> Vec<u8> {
        let mut d = BitWriter::new();
        d.write_tv_raw(b"GAS_LINE").unwrap();
        d.write_b(true);
        d.write_bs(0);
        d.write_b(false);
        d.write_tv_raw(b"Gas").unwrap();
        d.write_bd(0.95);
        d.write_rc(b'A');
        d.write_rc(1);
        d.write_bd(-0.2);
        d.write_bs(offset);
        d.write_2rd([0.0, 0.0]);
        d.write_bd(1.0);
        d.write_bd(0.0);
        d.write_bs(DASH_TEXT);
        let mut area = text.to_vec();
        area.resize(256, 0);
        d.write_bytes(&area);
        let mut hs = BitWriter::new();
        for value in [0, 0x11] {
            hs.write_handle_ref(HandleRef { code: 5, value }).unwrap();
        }
        assemble(Version::R2004, type_code::LTYPE, &d, None, &hs)
    }

    #[test]
    fn r2004_linetype_text_area_converts() {
        // Windows-1252 "Gaz\u{e9}" (30 is ANSI_1252).
        let bytes = r2004_linetype(0, b"Gaz\xE9");
        let (mut r, _) = ObjReader::split(&bytes, Version::R2004, 30).unwrap();
        let l = Linetype::decode(&mut r).unwrap();
        assert_eq!(l.text_area.len(), 512);
        assert_eq!(l.text(0).as_deref(), Some("Gaz\u{e9}"));
        assert_eq!(l.dashes[0].style, Handle(0x11));
        // It now writes as R2018 and reads back the same.
        roundtrip(&l, Linetype::encode, Linetype::decode, type_code::LTYPE);

        let bytes = r2004_linetype(4, b"\0\0\0\0GAS");
        let (mut r, _) = ObjReader::split(&bytes, Version::R2004, 30).unwrap();
        assert!(Linetype::decode(&mut r).is_err());
    }

    fn ucs_frame() -> UcsFrame {
        UcsFrame {
            origin: [1.0, 2.0, 3.0],
            x_axis: [0.0, 1.0, 0.0],
            y_axis: [-1.0, 0.0, 0.0],
            elevation: 4.0,
            orthographic_type: 2,
        }
    }

    fn lighting() -> Lighting {
        Lighting {
            use_default_lights: false,
            default_lighting_type: 2,
            brightness: 0.5,
            contrast: 0.25,
            ambient_color: Color::Rgb { r: 1, g: 2, b: 3 },
        }
    }

    #[test]
    fn roundtrip_view() {
        let v = View {
            entry: entry("MYVIEW"),
            height: 20.0,
            width: 30.0,
            center: [5.0, 6.0],
            target: [1.0, 1.0, 1.0],
            direction: [0.0, 0.0, 1.0],
            twist_angle: 0.1,
            lens_length: 50.0,
            front_clip: 1.0,
            back_clip: -1.0,
            view_mode: ViewMode {
                perspective: true,
                front_clip: false,
                back_clip: true,
                front_clip_not_at_eye: true,
            },
            render_mode: 3,
            lighting: lighting(),
            paper_space: true,
            ucs: Some(ucs_frame()),
            camera_plottable: true,
            background: Handle(0x70),
            visual_style: Handle(0x9F),
            sun: Handle(0x71),
            base_ucs: Handle(0x72),
            named_ucs: Handle(0x73),
            live_section: Handle(0x74),
        };
        roundtrip(&v, View::encode, View::decode, type_code::VIEW);
        // Without a UCS the UCS handles are not stored (AutoCAD 2025's
        // views in `adsk-house-01` of the extended corpus).
        let no_ucs = View {
            ucs: None,
            base_ucs: Handle(0),
            named_ucs: Handle(0),
            ..v
        };
        roundtrip(&no_ucs, View::encode, View::decode, type_code::VIEW);
        let stale = View {
            base_ucs: Handle(0x72),
            ..no_ucs.clone()
        };
        assert_eq!(
            encoded(&stale, View::encode, type_code::VIEW),
            encoded(&no_ucs, View::encode, type_code::VIEW)
        );
    }

    #[test]
    fn roundtrip_ucs() {
        let u = Ucs {
            entry: entry("U1"),
            origin: [1.0, 2.0, 3.0],
            x_axis: [1.0, 0.0, 0.0],
            y_axis: [0.0, 1.0, 0.0],
            elevation: 2.0,
            orthographic_view_type: 1,
            orthographic_type: 3,
            base_ucs: Handle(0x50),
            named_ucs: Handle(0x51),
        };
        roundtrip(&u, Ucs::encode, Ucs::decode, type_code::UCS);
    }

    #[test]
    fn roundtrip_vport() {
        let v = Vport {
            entry: entry("*Active"),
            view_height: 10.0,
            aspect_ratio_times_height: 17.5,
            view_center: [3.0, 4.0],
            view_target: [0.0, 0.0, 1.0],
            view_direction: [0.0, 0.0, 1.0],
            view_twist: 0.2,
            lens_length: 50.0,
            front_clip: 0.5,
            back_clip: 0.25,
            view_mode: ViewMode {
                perspective: false,
                front_clip: true,
                back_clip: false,
                front_clip_not_at_eye: true,
            },
            render_mode: 1,
            lighting: lighting(),
            lower_left: [0.0, 0.0],
            upper_right: [1.0, 1.0],
            ucs_follow: true,
            circle_zoom: 1000,
            fast_zoom: true,
            ucs_icon: 3,
            grid_on: true,
            grid_spacing: [0.5, 0.5],
            snap_on: true,
            snap_style: true,
            snap_isopair: 2,
            snap_rotation: 0.3,
            snap_base: [1.0, 2.0],
            snap_spacing: [0.25, 0.25],
            unknown_bit: true,
            ucs_per_viewport: true,
            ucs: ucs_frame(),
            grid_flags: 2,
            grid_major: 4,
            background: Handle(0x60),
            visual_style: Handle(0x61),
            sun: Handle(0x62),
            named_ucs: Handle(0x63),
            base_ucs: Handle(0x64),
        };
        roundtrip(&v, Vport::encode, Vport::decode, type_code::VPORT);
        let bad = Vport { ucs_icon: 4, ..v };
        assert!(bad.encode(&mut ObjWriter::new(H)).is_err());
    }

    #[test]
    fn roundtrip_app_id() {
        let a = AppId {
            entry: entry("ACAD"),
            unknown: 7,
        };
        roundtrip(&a, AppId::encode, AppId::decode, type_code::APPID);
    }

    /// A hand-built R2018 APPID whose xref short is 0x103: xref index 3,
    /// dependent. The one-`BS` mapping (bit 0x100 the dependent flag, the
    /// rest the index) comes from ACadSharp (MIT, `DwgObjectReader
    /// .readXrefDependantBit`); the corpus has no xref to confirm it.
    #[test]
    fn r2018_xref_short_splits_index_and_dependent_bit() {
        let mut d = BitWriter::new();
        d.write_bs(0x103);
        d.write_rc(7);
        let mut st = BitWriter::new();
        st.write_tu("XREF|APP").unwrap();
        let mut h = BitWriter::new();
        h.write_handle_ref(HandleRef {
            code: HARD_POINTER,
            value: 0x41,
        })
        .unwrap();
        let bytes = assemble(Version::R2018, type_code::APPID, &d, Some(&st), &h);
        let a = AppId::decode(&mut reader(&bytes)).unwrap();
        assert_eq!(
            (
                a.entry.name.as_str(),
                a.entry.xref_index,
                a.entry.xref_dependent,
                a.entry.xref_block
            ),
            ("XREF|APP", 3, true, Handle(0x41))
        );
        assert_eq!(a.unknown, 7);
    }

    /// A pre-R2007 xref index with bit 0x100 set has no R2018 form: writing
    /// the entry is refused as unconvertible, naming it.
    #[test]
    fn high_xref_index_is_unconvertible() {
        use crate::object::{NonGraphical, Object, ObjectCommon, encode_object};
        for index in [0x100, 0x1FF, -1] {
            let mut e = entry("APP");
            e.xref_index = index;
            assert!(!e.is_convertible());
            let obj = Object::Other {
                common: ObjectCommon {
                    handle: H,
                    owner: Handle(1),
                    owner_relative: false,
                    reactors: vec![],
                    xdict: None,
                    xdata: vec![],
                    has_ds_binary_data: false,
                },
                object: NonGraphical::AppId(AppId {
                    entry: e,
                    unknown: 0,
                }),
            };
            match encode_object(&obj, &[]) {
                Err(Error::UnconvertibleObjects(v)) => {
                    assert_eq!(v, vec![(H, "APPID".to_string())])
                }
                other => panic!("{index:#X}: {other:?}"),
            }
        }
        // 0xFF and 0x200 (bit 0x100 clear) still convert.
        for index in [0xFF, 0x200] {
            let mut e = entry("APP");
            e.xref_index = index;
            assert!(e.is_convertible(), "{index:#X}");
        }
    }

    #[test]
    fn roundtrip_vp_ent_hdr() {
        let v = VpEntHdr {
            entry: entry("V"),
            flag: true,
            viewport: Handle(0x267),
            prev: Handle(0xE48),
        };
        roundtrip(
            &v,
            VpEntHdr::encode,
            VpEntHdr::decode,
            type_code::VP_ENT_HDR,
        );
    }

    #[test]
    fn roundtrip_dim_style() {
        let mut n = 0.0;
        let mut f = || {
            n += 0.5;
            n
        };
        let d = DimStyle {
            entry: entry("custom_dim_style"),
            dimpost: "<> mm".into(),
            dimapost: "[]\"".into(),
            dimscale: f(),
            dimasz: f(),
            dimexo: f(),
            dimdli: f(),
            dimexe: f(),
            dimrnd: f(),
            dimdle: f(),
            dimtp: f(),
            dimtm: f(),
            dimfxl: f(),
            dimjogang: f(),
            dimtfill: 2,
            dimtfillclr: Color::Index(5),
            dimtol: true,
            dimlim: false,
            dimtih: true,
            dimtoh: false,
            dimse1: true,
            dimse2: true,
            dimtad: 1,
            dimzin: 8,
            dimazin: 3,
            dimarcsym: 1,
            dimtxt: f(),
            dimcen: f(),
            dimtsz: f(),
            dimaltf: f(),
            dimlfac: f(),
            dimtvp: f(),
            dimtfac: f(),
            dimgap: f(),
            dimaltrnd: f(),
            dimalt: true,
            dimaltd: 3,
            dimtofl: true,
            dimsah: false,
            dimtix: true,
            dimsoxd: true,
            dimclrd: Color::ByBlock,
            dimclre: Color::Rgb { r: 9, g: 8, b: 7 },
            dimclrt: Color::Index(1),
            dimadec: 1,
            dimdec: 2,
            dimtdec: 3,
            dimaltu: 4,
            dimalttd: 5,
            dimaunit: 6,
            dimfrac: 7,
            dimlunit: 8,
            dimdsep: 44,
            dimtmove: 1,
            dimjust: 2,
            dimsd1: true,
            dimsd2: false,
            dimtolj: 1,
            dimtzin: 2,
            dimaltz: 3,
            dimalttz: 4,
            dimupt: true,
            dimfit: 3,
            dimfxlon: true,
            dimtxtdirection: true,
            dimaltmzf: f(),
            dimaltmzs: "c".into(),
            dimmzf: f(),
            dimmzs: "m".into(),
            dimlwd: -2,
            dimlwe: 25,
            flag: true,
            dimtxsty: Handle(0x11),
            dimldrblk: Handle(0x20),
            dimblk: Handle(0x21),
            dimblk1: Handle(0x22),
            dimblk2: Handle(0x23),
            dimltype: Handle(0x24),
            dimltex1: Handle(0x25),
            dimltex2: Handle(0x26),
        };
        roundtrip(&d, DimStyle::encode, DimStyle::decode, type_code::DIMSTYLE);
    }
}
