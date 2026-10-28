//! Common object and entity data (ODS 20.1, 20.2, 20.4.1, 20.4.2) and extended
//! data (ODS chapter 28).

use super::stream::{ObjReader, ObjWriter, resolve};
use crate::bits::Color;
use crate::diag::{Diagnostic, Severity};
use crate::error::encode_error as encode_err;
use crate::error::{Error, Result};
use crate::handle::{Handle, HandleRef};
use crate::strings;
use crate::version::Version;

/// One extended-data item. The group code is the variant (ODS 28).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum XDataItem {
    /// 1000.
    String(String),
    /// 1002: `true` is `{`.
    ControlBrace(bool),
    /// 1003.
    LayerRef(Handle),
    /// 1004.
    Binary(Vec<u8>),
    /// 1005.
    Handle(Handle),
    /// 1010.
    Point([f64; 3]),
    /// 1011.
    WorldPos([f64; 3]),
    /// 1012.
    WorldDisp([f64; 3]),
    /// 1013.
    WorldDir([f64; 3]),
    /// 1040.
    Real(f64),
    /// 1041.
    Distance(f64),
    /// 1042.
    Scale(f64),
    /// 1070.
    Short(i16),
    /// 1071.
    Long(i32),
}

/// The extended data of one registered application.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct XDataApp {
    /// The APPID object.
    pub app: Handle,
    /// The data items.
    pub items: Vec<XDataItem>,
}

/// Data every object starts with.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ObjectCommon {
    /// The object's handle.
    pub handle: Handle,
    /// Owner (soft pointer). For an entity whose `entmode` is not 0 the file
    /// stores no owner and this is `Handle(0)`; the owner is then the model or
    /// paper space block record.
    pub owner: Handle,
    /// The owner was stored as an offset from the object's own handle
    /// (reference codes 6, 8, 0xA, 0xC) rather than as a soft pointer.
    /// AutoCAD uses both forms with no rule the corpus reveals, so the form
    /// is kept to write the object back as read; either names the same
    /// owner.
    pub owner_relative: bool,
    /// The objects notified of its changes (soft pointers).
    pub reactors: Vec<Handle>,
    /// Its extension dictionary, if it has one.
    pub xdict: Option<Handle>,
    /// Extended data, by application.
    pub xdata: Vec<XDataApp>,
    /// R2013+: the object has binary data in the data store section.
    pub has_ds_binary_data: bool,
}

/// An entity's linetype.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum LinetypeRef {
    /// The layer's linetype.
    ByLayer,
    /// The block's linetype.
    ByBlock,
    /// Continuous (no linetype object referred to).
    Continuous,
    /// This LTYPE.
    Handle(Handle),
}

/// An entity's plot style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum PlotStyleRef {
    /// The layer's plot style.
    ByLayer,
    /// The block's plot style.
    ByBlock,
    /// Flag value 10, "CONTINUOUS plotstyle" (ODS 20.2).
    Continuous,
    /// This plot style (a PLACEHOLDER, or a DICTIONARY entry).
    Handle(Handle),
}

/// An entity's material (R2007+).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum MaterialRef {
    /// The layer's material.
    ByLayer,
    /// The block's material.
    ByBlock,
    /// Flag value 10, "global material" (ODS 20.2).
    Global,
    /// This MATERIAL.
    Handle(Handle),
}

/// Values as read from a file, kept to write an object back bit for bit
/// while the fields they derive from are unchanged. Not part of an object's
/// value: any two compare equal. Objects built by hand use
/// `AsRead::default()` (nothing read).
#[derive(Debug, Clone)]
pub struct AsRead<T>(Option<T>);

impl<T> Default for AsRead<T> {
    fn default() -> Self {
        AsRead(None)
    }
}

impl<T> AsRead<T> {
    pub(crate) fn new(v: T) -> Self {
        AsRead(Some(v))
    }

    pub(crate) fn get(&self) -> Option<&T> {
        self.0.as_ref()
    }
}

impl<T> PartialEq for AsRead<T> {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

/// The raw lineweight byte of a ByLayer lineweight.
pub const LINEWEIGHT_BYLAYER: u8 = 29;

/// Data every entity starts with.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EntityCommon {
    /// The common data of every object.
    pub obj: ObjectCommon,
    /// 0: owner stored; 1: paper space; 2: model space; 3: "not used" (ODS
    /// 20.2), kept as read and written back, with no owner stored.
    pub entmode: u8,
    /// The layer (hard pointer).
    pub layer: Handle,
    /// The linetype.
    pub linetype: LinetypeRef,
    /// The plot style.
    pub plotstyle: PlotStyleRef,
    /// The material.
    pub material: MaterialRef,
    /// With a `color_book` this is the ACI color stored beside it (the format
    /// keeps no true color for a book color).
    pub color: Color,
    /// A true color as read and the ACI index stored beside it (AutoCAD's
    /// nearest index, for readers without true color). While `color` still
    /// equals it the stored index is written back; after an edit the index
    /// is recomputed (see [`EntityCommon::true_color_aci`]).
    #[cfg_attr(feature = "serde", serde(skip))]
    pub read_color: AsRead<(Color, u16)>,
    /// Raw ENC transparency long.
    pub transparency: Option<u32>,
    /// Color book (DBCOLOR) object.
    pub color_book: Option<Handle>,
    /// Linetype scale.
    pub ltype_scale: f64,
    /// Raw lineweight byte: 0-23 index the standard lineweights,
    /// [`LINEWEIGHT_BYLAYER`] is ByLayer, 30 ByBlock, 31 Default.
    pub lineweight: u8,
    /// Not drawn.
    pub invisible: bool,
    /// R2007+.
    pub shadow_flags: u8,
    /// R2010+ visual styles (hard pointers).
    pub full_visual_style: Option<Handle>,
    /// R2010+ face visual style (hard pointer).
    pub face_visual_style: Option<Handle>,
    /// R2010+ edge visual style (hard pointer).
    pub edge_visual_style: Option<Handle>,
    /// Proxy graphics, as stored (see [`crate::proxy_graphics`] to decode them).
    pub proxy_graphics: Option<Vec<u8>>,
    /// R2000 previous/next entity links when stored (`nolinks` 0). Not written:
    /// R2004+ entities have no links.
    pub prev_next: Option<(Handle, Handle)>,
}

fn corrupt(msg: impl Into<String>) -> Error {
    Error::Corrupt(msg.into())
}

/// Soft pointer, hard owner and hard pointer reference codes (ODS 2.13).
const SOFT_POINTER: u8 = 4;
const HARD_OWNER: u8 = 3;
const HARD_POINTER: u8 = 5;

fn abs_ref(code: u8, h: Handle) -> HandleRef {
    HandleRef { code, value: h.0 }
}

impl ObjectCommon {
    /// The common data of a new object: no handle (the drawing assigns one),
    /// no owner, reactors, extension dictionary or extended data.
    pub fn new() -> ObjectCommon {
        ObjectCommon {
            handle: Handle(0),
            owner: Handle(0),
            owner_relative: false,
            reactors: Vec::new(),
            xdict: None,
            xdata: Vec::new(),
            has_ds_binary_data: false,
        }
    }
}

impl Default for ObjectCommon {
    fn default() -> Self {
        ObjectCommon::new()
    }
}

impl EntityCommon {
    /// The common data of a new entity on `layer`, with ByLayer defaults:
    /// ByLayer color, linetype, lineweight, plot style and material, linetype
    /// scale 1, visible, no transparency (ByLayer), no visual styles.
    /// The handle, owner and entity mode are set by the drawing when the
    /// entity is added (see [`crate::Drawing::add_entity`]).
    pub fn new(layer: Handle) -> EntityCommon {
        EntityCommon {
            obj: ObjectCommon::new(),
            entmode: 0,
            layer,
            linetype: LinetypeRef::ByLayer,
            plotstyle: PlotStyleRef::ByLayer,
            material: MaterialRef::ByLayer,
            color: Color::ByLayer,
            read_color: AsRead::default(),
            transparency: None,
            color_book: None,
            ltype_scale: 1.0,
            lineweight: LINEWEIGHT_BYLAYER,
            invisible: false,
            shadow_flags: 0,
            full_visual_style: None,
            face_visual_style: None,
            edge_visual_style: None,
            proxy_graphics: None,
            prev_next: None,
        }
    }

    /// The ACI index written beside a true color: the index read with it
    /// while `color` is unchanged, else the nearest index
    /// ([`crate::bits::nearest_aci`]); 0 for other colors.
    pub fn true_color_aci(&self) -> u16 {
        let Color::Rgb { r, g, b } = self.color else {
            return 0;
        };
        match self.read_color.get() {
            Some((read, aci)) if *read == self.color => *aci,
            _ => u16::from(crate::bits::nearest_aci(r, g, b)),
        }
    }
}

/// Decode the common data of a non-entity object, from its handle onward.
pub fn decode_object_common(r: &mut ObjReader) -> Result<ObjectCommon> {
    let (handle, xdata) = read_handle_and_xdata(r)?;
    let (owner, owner_relative) = read_owner(r)?;
    let (reactors, xdict, has_ds_binary_data) = read_reactors_xdict(r)?;
    Ok(ObjectCommon {
        handle,
        owner,
        owner_relative,
        reactors,
        xdict,
        xdata,
        has_ds_binary_data,
    })
}

/// Decode the common data of an entity, from its handle onward.
///
/// Data stream order follows ODS 20.4.1, except: `nolinks` is read only before
/// R2004 and the material flags and shadow byte come before the plot style
/// flags, as ACadSharp (MIT, `DwgObjectReader.readEntityMode`) reads them; both
/// were checked against the sample corpus. Handle stream order is ODS 20.4.2.
pub fn decode_entity_common(r: &mut ObjReader) -> Result<EntityCommon> {
    let v = r.version;
    let (handle, xdata) = read_handle_and_xdata(r)?;
    let proxy_graphics = if r.data.read_b()? {
        let len = if v >= Version::R2010 {
            r.data.read_bll()?
        } else {
            u64::from(r.data.read_rl()? as u32)
        };
        if len > r.data.remaining_bits() / 8 {
            return Err(Error::Truncated {
                context: "proxy graphics",
            });
        }
        Some(r.data.read_bytes(len as usize)?)
    } else {
        None
    };
    let mut e = decode_entity_mode(r)?;
    e.obj.handle = handle;
    e.obj.xdata = xdata;
    e.proxy_graphics = proxy_graphics;
    Ok(e)
}

/// Decode an entity's common data from the entity mode onward: everything
/// but the handle, extended data and proxy graphics, which are left empty.
/// An R2018 multi-line ATTRIB or ATTDEF embeds an MTEXT whose common data
/// starts here (ODS 20.4.4).
pub(crate) fn decode_entity_mode(r: &mut ObjReader) -> Result<EntityCommon> {
    let v = r.version;
    let entmode = r.data.read_bb()?;
    let (owner, owner_relative) = if entmode == 0 {
        read_owner(r)?
    } else {
        (Handle(0), false)
    };
    let (reactors, xdict, has_ds_binary_data) = read_reactors_xdict(r)?;
    let prev_next = if v < Version::R2004 && !r.data.read_b()? {
        Some((r.href_abs()?, r.href_abs()?))
    } else {
        None
    };
    let (color, transparency, book_follows, rgb_aci) = r.data.read_enc_with_aci(v)?;
    let read_color = match color {
        Color::Rgb { .. } => AsRead::new((color.clone(), rgb_aci)),
        _ => AsRead::default(),
    };
    let color_book = if book_follows {
        Some(r.href_abs()?)
    } else {
        None
    };
    let ltype_scale = r.data.read_bd()?;
    let ltype_flags = r.data.read_bb()?;
    let (material_flags, shadow_flags) = if v >= Version::R2007 {
        (r.data.read_bb()?, r.data.read_rc()?)
    } else {
        (0, 0)
    };
    let plotstyle_flags = r.data.read_bb()?;
    let visual_styles = if v >= Version::R2010 {
        [r.data.read_b()?, r.data.read_b()?, r.data.read_b()?]
    } else {
        [false; 3]
    };
    let invisible = r.data.read_bs()? & 1 != 0;
    let lineweight = r.data.read_rc()?;

    let layer = r.href_abs()?;
    let linetype = match ltype_flags {
        0 => LinetypeRef::ByLayer,
        1 => LinetypeRef::ByBlock,
        2 => LinetypeRef::Continuous,
        _ => LinetypeRef::Handle(r.href_abs()?),
    };
    let material = match material_flags {
        0 => MaterialRef::ByLayer,
        1 => MaterialRef::ByBlock,
        2 => MaterialRef::Global,
        _ => MaterialRef::Handle(r.href_abs()?),
    };
    let plotstyle = match plotstyle_flags {
        0 => PlotStyleRef::ByLayer,
        1 => PlotStyleRef::ByBlock,
        2 => PlotStyleRef::Continuous,
        _ => PlotStyleRef::Handle(r.href_abs()?),
    };
    let mut styles = [None; 3];
    for (slot, present) in styles.iter_mut().zip(visual_styles) {
        if present {
            *slot = Some(r.href_abs()?);
        }
    }
    let [full_visual_style, face_visual_style, edge_visual_style] = styles;
    Ok(EntityCommon {
        obj: ObjectCommon {
            handle: Handle(0),
            owner,
            owner_relative,
            reactors,
            xdict,
            xdata: Vec::new(),
            has_ds_binary_data,
        },
        entmode,
        layer,
        linetype,
        plotstyle,
        material,
        color,
        read_color,
        transparency,
        color_book,
        ltype_scale,
        lineweight,
        invisible,
        shadow_flags,
        full_visual_style,
        face_visual_style,
        edge_visual_style,
        proxy_graphics: None,
        prev_next,
    })
}

/// Write the common data of a non-entity object in R2018 form.
pub fn encode_object_common(w: &mut ObjWriter, c: &ObjectCommon) -> Result<()> {
    write_handle_and_xdata(w, c)?;
    write_owner(w, c)?;
    write_reactors_xdict(w, c)
}

/// Write the common data of an entity in R2018 form. `prev_next` is not
/// written: R2004+ entities have no links.
pub fn encode_entity_common(w: &mut ObjWriter, e: &EntityCommon) -> Result<()> {
    write_handle_and_xdata(w, &e.obj)?;
    match &e.proxy_graphics {
        Some(g) => {
            w.data.write_b(true);
            w.data.write_bll(g.len() as u64)?;
            w.data.write_bytes(g);
        }
        None => w.data.write_b(false),
    }
    encode_entity_mode(w, e)
}

/// Write an entity's common data from the entity mode onward (see
/// [`decode_entity_mode`]).
pub(crate) fn encode_entity_mode(w: &mut ObjWriter, e: &EntityCommon) -> Result<()> {
    if e.entmode > 3 {
        return Err(encode_err("entity mode is a 2-bit value"));
    }
    w.data.write_bb(e.entmode);
    if e.entmode == 0 {
        write_owner(w, &e.obj)?;
    }
    write_reactors_xdict(w, &e.obj)?;
    w.data.write_enc_with_aci(
        &e.color,
        e.transparency,
        e.color_book.is_some(),
        e.true_color_aci(),
        ObjWriter::VERSION,
    )?;
    if let Some(book) = e.color_book {
        w.href(abs_ref(HARD_POINTER, book))?;
    }
    w.data.write_bd(e.ltype_scale);
    w.href(abs_ref(HARD_POINTER, e.layer))?;
    let (ltype_flags, ltype) = match e.linetype {
        LinetypeRef::ByLayer => (0, None),
        LinetypeRef::ByBlock => (1, None),
        LinetypeRef::Continuous => (2, None),
        LinetypeRef::Handle(h) => (3, Some(h)),
    };
    let (material_flags, material) = match e.material {
        MaterialRef::ByLayer => (0, None),
        MaterialRef::ByBlock => (1, None),
        MaterialRef::Global => (2, None),
        MaterialRef::Handle(h) => (3, Some(h)),
    };
    let (plotstyle_flags, plotstyle) = match e.plotstyle {
        PlotStyleRef::ByLayer => (0, None),
        PlotStyleRef::ByBlock => (1, None),
        PlotStyleRef::Continuous => (2, None),
        PlotStyleRef::Handle(h) => (3, Some(h)),
    };
    w.data.write_bb(ltype_flags);
    w.data.write_bb(material_flags);
    w.data.write_rc(e.shadow_flags);
    w.data.write_bb(plotstyle_flags);
    let styles = [
        e.full_visual_style,
        e.face_visual_style,
        e.edge_visual_style,
    ];
    for s in styles {
        w.data.write_b(s.is_some());
    }
    w.data.write_bs(i16::from(e.invisible));
    w.data.write_rc(e.lineweight);
    for h in [ltype, material, plotstyle]
        .into_iter()
        .chain(styles)
        .flatten()
    {
        w.href(abs_ref(HARD_POINTER, h))?;
    }
    Ok(())
}

/// The owner reference, and whether it is an offset from the object's handle.
fn read_owner(r: &mut ObjReader) -> Result<(Handle, bool)> {
    let owner = r.href()?;
    let relative = matches!(owner.code, 6 | 8 | 0xA | 0xC);
    Ok((resolve(owner, r.handle)?, relative))
}

/// The owner reference: a soft pointer, or with `owner_relative` an offset
/// from the handle of the object being written (codes 6 and 8 for +1 and -1,
/// 0xA and 0xC otherwise; an embedded MTEXT's offsets count from its
/// attribute). A null owner, or one equal to the handle, is a soft pointer.
fn write_owner(w: &mut ObjWriter, c: &ObjectCommon) -> Result<()> {
    let (h, o) = (w.handle.0, c.owner.0);
    let r = match o.cmp(&h) {
        _ if !c.owner_relative || o == 0 || o == h => abs_ref(SOFT_POINTER, c.owner),
        std::cmp::Ordering::Greater if o - h == 1 => HandleRef { code: 6, value: 0 },
        std::cmp::Ordering::Greater => HandleRef {
            code: 0xA,
            value: o - h,
        },
        _ if h - o == 1 => HandleRef { code: 8, value: 0 },
        _ => HandleRef {
            code: 0xC,
            value: h - o,
        },
    };
    w.href(r)
}

fn read_handle_and_xdata(r: &mut ObjReader) -> Result<(Handle, Vec<XDataApp>)> {
    let handle = Handle(r.data.read_handle_ref()?.value);
    r.handle = handle;
    let xdata = read_xdata(r)?;
    Ok((handle, xdata))
}

fn write_handle_and_xdata(w: &mut ObjWriter, c: &ObjectCommon) -> Result<()> {
    w.data.write_handle_ref(abs_ref(0, c.handle))?;
    write_xdata(w, &c.xdata)
}

/// Reactor count and handles, the xdictionary and (R2013+) the data store bit.
fn read_reactors_xdict(r: &mut ObjReader) -> Result<(Vec<Handle>, Option<Handle>, bool)> {
    let count = u64::try_from(r.data.read_bl()?).map_err(|_| corrupt("negative reactor count"))?;
    // Each reference takes at least a byte of handle stream.
    if count > r.handles.remaining_bits() / 8 {
        return Err(corrupt("reactor count exceeds the handle stream"));
    }
    let reactors = (0..count)
        .map(|_| r.href_abs())
        .collect::<Result<Vec<_>>>()?;
    let xdict_missing = r.version >= Version::R2004 && r.data.read_b()?;
    // A clear flag with a null handle reads as no extension dictionary and
    // is written with the flag set: a non-canonical round trip (see
    // `crate::object`).
    let xdict = if xdict_missing {
        None
    } else {
        Some(r.href_abs()?).filter(|h| h.0 != 0)
    };
    let has_ds_binary_data = r.version >= Version::R2013 && r.data.read_b()?;
    Ok((reactors, xdict, has_ds_binary_data))
}

fn write_reactors_xdict(w: &mut ObjWriter, c: &ObjectCommon) -> Result<()> {
    let count =
        i32::try_from(c.reactors.len()).map_err(|_| encode_err("too many reactors to encode"))?;
    w.data.write_bl(count);
    for &h in &c.reactors {
        w.href(abs_ref(SOFT_POINTER, h))?;
    }
    w.data.write_b(c.xdict.is_none());
    if let Some(x) = c.xdict {
        w.href(abs_ref(HARD_OWNER, x))?;
    }
    w.data.write_b(c.has_ds_binary_data);
    Ok(())
}

// ---- extended data (ODS 28) ----

/// Extended data: repeated `BS size`, application handle, `size` bytes of
/// items, until a size of 0.
fn read_xdata(r: &mut ObjReader) -> Result<Vec<XDataApp>> {
    let mut apps = Vec::new();
    loop {
        // AutoCAD caps one application's data at 16 KiB; read the BS unsigned
        // so a large size is bounded by the data, not rejected as negative.
        let size = r.data.read_bs()? as u16;
        if size == 0 {
            return Ok(apps);
        }
        let app = Handle(r.data.read_handle_ref()?.value);
        let bytes = r.data.read_bytes(usize::from(size))?;
        let items = parse_xdata_items(r, &bytes)?;
        apps.push(XDataApp { app, items });
    }
}

fn write_xdata(w: &mut ObjWriter, apps: &[XDataApp]) -> Result<()> {
    for app in apps {
        let bytes = encode_xdata_items(&app.items)?;
        let size = u16::try_from(bytes.len())
            .ok()
            .filter(|&n| n != 0)
            .ok_or_else(|| encode_err("extended data of an application must be 1..=65535 bytes"))?;
        w.data.write_bs(size as i16);
        w.data.write_handle_ref(abs_ref(HARD_POINTER, app.app))?;
        w.data.write_bytes(&bytes);
    }
    w.data.write_bs(0);
    Ok(())
}

/// A byte cursor over one application's extended data.
struct Bytes<'a>(&'a [u8]);

impl<'a> Bytes<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.0.len() {
            return Err(Error::Truncated {
                context: "extended data",
            });
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }

    fn u16(&mut self) -> Result<u16> {
        self.array().map(u16::from_le_bytes)
    }

    fn f64(&mut self) -> Result<f64> {
        self.array().map(f64::from_le_bytes)
    }

    fn point(&mut self) -> Result<[f64; 3]> {
        Ok([self.f64()?, self.f64()?, self.f64()?])
    }

    /// 8 bytes, most significant first, like handle reference bytes.
    fn handle(&mut self) -> Result<Handle> {
        self.array().map(|b| Handle(u64::from_be_bytes(b)))
    }
}

fn parse_xdata_items(r: &mut ObjReader, bytes: &[u8]) -> Result<Vec<XDataItem>> {
    let mut b = Bytes(bytes);
    let mut items = Vec::new();
    while !b.0.is_empty() {
        let code = b.u8()?;
        items.push(match code {
            0 => XDataItem::String(read_xdata_string(r, &mut b)?),
            2 => match b.u8()? {
                0 => XDataItem::ControlBrace(true),
                1 => XDataItem::ControlBrace(false),
                v => return Err(corrupt(format!("extended data brace value {v}"))),
            },
            3 => XDataItem::LayerRef(b.handle()?),
            4 => {
                let n = b.u8()?;
                XDataItem::Binary(b.take(usize::from(n))?.to_vec())
            }
            5 => XDataItem::Handle(b.handle()?),
            10 => XDataItem::Point(b.point()?),
            11 => XDataItem::WorldPos(b.point()?),
            12 => XDataItem::WorldDisp(b.point()?),
            13 => XDataItem::WorldDir(b.point()?),
            40 => XDataItem::Real(b.f64()?),
            41 => XDataItem::Distance(b.f64()?),
            42 => XDataItem::Scale(b.f64()?),
            70 => XDataItem::Short(b.u16()? as i16),
            71 => XDataItem::Long(i32::from_le_bytes(b.array()?)),
            c => {
                return Err(corrupt(format!(
                    "extended data group code {}",
                    1000 + u32::from(c)
                )));
            }
        });
    }
    Ok(items)
}

/// An extended data string. R2007+: `RS` length and UTF-16LE units. Before:
/// `RS` length, `RC` code page, and bytes in that code page. ODS 28 says `RC`
/// length then `RS` code page; ACadSharp (MIT, `readExtendedDataRecords`)
/// reads `RS` then `RC`, and R2000 sample bytes (`00 | 14 00 | 1E | 20 bytes`)
/// agree with ACadSharp.
///
/// Trailing NULs are removed, as for object text: AutoCAD writes none (an R2018
/// sample stores "Arial" with length 5), but other writers (ACadSharp) count a
/// terminator in the length. The encoder writes none.
fn read_xdata_string(r: &mut ObjReader, b: &mut Bytes) -> Result<String> {
    let mut text = if r.version >= Version::R2007 {
        let n = usize::from(b.u16()?);
        let raw = b.take(n.checked_mul(2).ok_or_else(|| corrupt("xdata string"))?)?;
        let (text, replaced) = strings::decode_utf16le_checked(raw);
        if replaced {
            r.diagnostics
                .push(lossy_diag(r.handle, "an unpaired UTF-16 surrogate"));
        }
        text
    } else {
        let n = usize::from(b.u16()?);
        let codepage = u16::from(b.u8()?);
        // The ODA SDK (`cc-15`, `cc-21`) and the writer of civilcraft's
        // R2000 fixtures (`cc-02`, `cc-04`, ...) store code page 0 where
        // AutoCAD stores the drawing's (30 in every AutoCAD R2000/R2004
        // corpus file): 382 strings, all ASCII ("AcDbSavedByObjectVersion",
        // "... Data", "DSTYLE"), whose lengths and following items read
        // right. Off-spec but harmless for ASCII: read as Windows-1252, with
        // the warning.
        if !strings::is_supported(codepage) {
            r.diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                handle: Some(r.handle),
                message: format!(
                    "extended data string code page {codepage} is not supported; read as \
                     Windows-1252"
                ),
            });
        }
        let (text, malformed) = strings::decode_checked(codepage, b.take(n)?);
        if malformed {
            r.diagnostics
                .push(lossy_diag(r.handle, "malformed code page bytes"));
        }
        text
    };
    text.truncate(text.trim_end_matches('\0').len());
    Ok(text)
}

fn lossy_diag(handle: Handle, what: &str) -> Diagnostic {
    Diagnostic {
        severity: Severity::Warning,
        handle: Some(handle),
        message: format!("extended data string has {what}; it became U+FFFD"),
    }
}

/// Items in R2018 form.
fn encode_xdata_items(items: &[XDataItem]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for item in items {
        match item {
            XDataItem::String(s) => {
                let units: Vec<u16> = s.encode_utf16().collect();
                let n = u16::try_from(units.len())
                    .map_err(|_| encode_err("extended data string too long"))?;
                out.push(0);
                out.extend_from_slice(&n.to_le_bytes());
                out.extend(units.iter().flat_map(|u| u.to_le_bytes()));
            }
            XDataItem::ControlBrace(open) => out.extend_from_slice(&[2, u8::from(!open)]),
            XDataItem::LayerRef(h) => {
                out.push(3);
                out.extend_from_slice(&h.0.to_be_bytes());
            }
            XDataItem::Binary(bytes) => {
                let n = u8::try_from(bytes.len())
                    .map_err(|_| encode_err("extended data binary chunk over 255 bytes"))?;
                out.extend_from_slice(&[4, n]);
                out.extend_from_slice(bytes);
            }
            XDataItem::Handle(h) => {
                out.push(5);
                out.extend_from_slice(&h.0.to_be_bytes());
            }
            XDataItem::Point(p)
            | XDataItem::WorldPos(p)
            | XDataItem::WorldDisp(p)
            | XDataItem::WorldDir(p) => {
                out.push(match item {
                    XDataItem::Point(_) => 10,
                    XDataItem::WorldPos(_) => 11,
                    XDataItem::WorldDisp(_) => 12,
                    _ => 13,
                });
                out.extend(p.iter().flat_map(|v| v.to_le_bytes()));
            }
            XDataItem::Real(v) | XDataItem::Distance(v) | XDataItem::Scale(v) => {
                out.push(match item {
                    XDataItem::Real(_) => 40,
                    XDataItem::Distance(_) => 41,
                    _ => 42,
                });
                out.extend_from_slice(&v.to_le_bytes());
            }
            XDataItem::Short(v) => {
                out.push(70);
                out.extend_from_slice(&v.to_le_bytes());
            }
            XDataItem::Long(v) => {
                out.push(71);
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::BitWriter;
    use crate::object::stream::tests::assemble;

    fn all_items() -> Vec<XDataItem> {
        vec![
            XDataItem::String("hello wörld 日本".into()),
            XDataItem::String(String::new()),
            XDataItem::ControlBrace(true),
            XDataItem::LayerRef(Handle(0x10)),
            XDataItem::Binary(vec![1, 2, 3, 0xFF]),
            XDataItem::Handle(Handle(0x0012_3456_789A)),
            XDataItem::Point([1.0, -2.5, 3.25]),
            XDataItem::WorldPos([4.0, 5.0, 6.0]),
            XDataItem::WorldDisp([0.5, 0.0, -0.5]),
            XDataItem::WorldDir([0.0, 0.0, 1.0]),
            XDataItem::Real(3.5),
            XDataItem::Distance(10.0),
            XDataItem::Scale(0.25),
            XDataItem::Short(-12),
            XDataItem::Long(70000),
            XDataItem::ControlBrace(false),
        ]
    }

    fn sample_entity() -> EntityCommon {
        EntityCommon {
            obj: ObjectCommon {
                handle: Handle(0x2A0),
                owner: Handle(0x1F),
                owner_relative: false,
                reactors: vec![Handle(0x300), Handle(0x29F)],
                xdict: Some(Handle(0x2A1)),
                xdata: vec![
                    XDataApp {
                        app: Handle(0x12),
                        items: all_items(),
                    },
                    XDataApp {
                        app: Handle(0x99),
                        items: vec![XDataItem::Short(1)],
                    },
                ],
                has_ds_binary_data: true,
            },
            entmode: 0,
            layer: Handle(0x10),
            linetype: LinetypeRef::Handle(Handle(0x16)),
            plotstyle: PlotStyleRef::Handle(Handle(0xE)),
            material: MaterialRef::Handle(Handle(0x40)),
            color: Color::Rgb {
                r: 10,
                g: 20,
                b: 30,
            },
            transparency: Some(0x0200_0080),
            read_color: AsRead::default(),
            color_book: None,
            ltype_scale: 2.5,
            lineweight: 13,
            invisible: true,
            shadow_flags: 2,
            full_visual_style: Some(Handle(0x61)),
            face_visual_style: None,
            edge_visual_style: Some(Handle(0x63)),
            proxy_graphics: Some(vec![9, 8, 7, 6]),
            prev_next: None,
        }
    }

    fn roundtrip_entity(e: &EntityCommon) -> EntityCommon {
        let mut w = ObjWriter::new(e.obj.handle);
        encode_entity_common(&mut w, e).unwrap();
        w.data.write_bd(42.0);
        let bytes = w.finish(0x13).unwrap();
        let (mut r, pre) = ObjReader::split(&bytes, Version::R2018, 30).unwrap();
        assert_eq!(pre.type_code, 0x13);
        let got = decode_entity_common(&mut r).unwrap();
        assert_eq!(r.data.read_bd().unwrap(), 42.0);
        assert_eq!(r.data.remaining_bits(), 0);
        assert!(r.handles.remaining_bits() < 8);
        assert!(r.diagnostics.is_empty());
        got
    }

    /// The owner as a soft pointer or as an offset from the handle, each in
    /// the form AutoCAD uses, read back with its form.
    #[test]
    fn owner_reference_forms() {
        let cases = [
            (0x81, true, HandleRef { code: 6, value: 0 }),
            (0x7F, true, HandleRef { code: 8, value: 0 }),
            (
                0x95,
                true,
                HandleRef {
                    code: 0xA,
                    value: 0x15,
                },
            ),
            (
                0x10,
                true,
                HandleRef {
                    code: 0xC,
                    value: 0x70,
                },
            ),
            (
                0x10,
                false,
                HandleRef {
                    code: 4,
                    value: 0x10,
                },
            ),
            // A null owner, or the object itself, is a soft pointer.
            (0, true, HandleRef { code: 4, value: 0 }),
            (
                0x80,
                true,
                HandleRef {
                    code: 4,
                    value: 0x80,
                },
            ),
        ];
        for (owner, relative, stored) in cases {
            let mut e = sample_entity();
            e.obj.handle = Handle(0x80);
            e.obj.owner = Handle(owner);
            e.obj.owner_relative = relative;
            let mut w = ObjWriter::new(Handle(0x80));
            encode_entity_common(&mut w, &e).unwrap();
            let bytes = w.finish(0x13).unwrap();
            let (mut r, _) = ObjReader::split(&bytes, Version::R2018, 30).unwrap();
            assert_eq!(r.handles.read_handle_ref().unwrap(), stored, "{owner:X}");
            let (mut r, _) = ObjReader::split(&bytes, Version::R2018, 30).unwrap();
            let back = decode_entity_common(&mut r).unwrap();
            let kept = relative && owner != 0 && owner != 0x80;
            assert_eq!(back.obj.owner, Handle(owner));
            assert_eq!(back.obj.owner_relative, kept, "{owner:X}");
        }
    }

    /// AutoCAD stores a true color's nearest ACI in the ENC word and the
    /// RGB long with method byte 0xC2 (POINT 29A of the R2018 sample: word
    /// 0x80E6, long 0xC2F50A93).
    #[test]
    fn true_color_keeps_its_aci() {
        let mut w = BitWriter::new();
        let c = Color::Rgb {
            r: 0xF5,
            g: 0x0A,
            b: 0x93,
        };
        w.write_enc_with_aci(&c, None, false, 0xE6, Version::R2018)
            .unwrap();
        let mut want = BitWriter::new();
        want.write_bs(0x80E6u16 as i16);
        want.write_bl(0xC2F5_0A93u32 as i32);
        assert_eq!(w.into_bytes(), want.clone().into_bytes());
        let bytes = want.into_bytes();
        assert_eq!(
            crate::bits::BitReader::new(&bytes)
                .read_enc_with_aci(Version::R2018)
                .unwrap(),
            (c.clone(), None, false, 0xE6)
        );
        // The ACI must fit 9 bits beside a true color, and is ignored (not
        // checked) for other colors.
        let mut w = BitWriter::new();
        w.write_enc_with_aci(&Color::ByLayer, None, false, 0x200, Version::R2018)
            .unwrap();
        assert_eq!(w.bit_len(), 2);
        assert!(
            BitWriter::new()
                .write_enc_with_aci(&c, None, false, 0x200, Version::R2018)
                .is_err()
        );
    }

    /// The ACI stored beside a true color is written back while the color
    /// is unchanged, and recomputed after an edit (never left stale, and
    /// never 0, which old readers take for ByBlock).
    #[test]
    fn true_color_aci_follows_color_edits() {
        let encode = |e: &EntityCommon| {
            let mut w = ObjWriter::new(e.obj.handle);
            encode_entity_common(&mut w, e).unwrap();
            w.finish(0x13).unwrap()
        };
        let decode = |b: &[u8]| {
            let (mut r, _) = ObjReader::split(b, Version::R2018, 30).unwrap();
            decode_entity_common(&mut r).unwrap()
        };
        let teal = Color::Rgb {
            r: 10,
            g: 20,
            b: 30,
        };
        // As read: ACI 0x1E, not the nearest index (a third-party writer's
        // choice).
        let mut e = sample_entity();
        e.color = teal.clone();
        e.read_color = AsRead::new((teal, 0x1E));
        let bytes = encode(&e);
        let back = decode(&bytes);
        assert_eq!(back.true_color_aci(), 0x1E);
        assert_eq!(encode(&back), bytes);
        // Edited: the nearest index.
        let mut edited = back.clone();
        edited.color = Color::Rgb {
            r: 245,
            g: 10,
            b: 147,
        };
        assert_eq!(decode(&encode(&edited)).true_color_aci(), 230);
        // Built by hand from ByLayer: the nearest index too.
        let mut fresh = sample_entity();
        fresh.color = Color::Rgb {
            r: 80,
            g: 179,
            b: 255,
        };
        assert_eq!(fresh.true_color_aci(), 151);
        assert_eq!(decode(&encode(&fresh)).true_color_aci(), 151);
        // Other colors carry none.
        fresh.color = Color::Index(5);
        assert_eq!(fresh.true_color_aci(), 0);
    }

    #[test]
    fn objwriter_reader_roundtrip_common() {
        // True color and transparency.
        let e = sample_entity();
        assert_eq!(roundtrip_entity(&e), e);
        // A book color: ENC holds an ACI color and the DBCOLOR handle follows;
        // the format has no true color alongside a book color.
        let mut e = sample_entity();
        e.color = Color::Index(42);
        e.color_book = Some(Handle(0x5A));
        assert_eq!(roundtrip_entity(&e), e);
        let mut rgb_book = sample_entity();
        rgb_book.color_book = Some(Handle(0x5A));
        let mut w = ObjWriter::new(rgb_book.obj.handle);
        assert!(encode_entity_common(&mut w, &rgb_book).is_err());
    }

    #[test]
    fn entity_common_variants_roundtrip() {
        let mut e = sample_entity();
        e.entmode = 2;
        e.obj.owner = Handle(0);
        e.obj.reactors.clear();
        e.obj.xdict = None;
        e.obj.xdata.clear();
        e.obj.has_ds_binary_data = false;
        e.linetype = LinetypeRef::Continuous;
        e.plotstyle = PlotStyleRef::ByBlock;
        e.material = MaterialRef::Global;
        e.color = Color::ByLayer;
        e.transparency = None;
        e.color_book = None;
        e.full_visual_style = None;
        e.edge_visual_style = None;
        e.proxy_graphics = None;
        assert_eq!(roundtrip_entity(&e), e);
        e.linetype = LinetypeRef::ByBlock;
        e.plotstyle = PlotStyleRef::ByLayer;
        e.material = MaterialRef::ByBlock;
        e.color = Color::Index(7);
        e.entmode = 1;
        assert_eq!(roundtrip_entity(&e), e);
        e.plotstyle = PlotStyleRef::Continuous;
        e.entmode = 3;
        assert_eq!(roundtrip_entity(&e), e);
        e.entmode = 4;
        assert!(encode_entity_common(&mut ObjWriter::new(e.obj.handle), &e).is_err());
    }

    #[test]
    fn object_common_roundtrip() {
        let c = sample_entity().obj;
        let mut w = ObjWriter::new(c.handle);
        encode_object_common(&mut w, &c).unwrap();
        w.text("name").unwrap();
        let bytes = w.finish(0x33).unwrap();
        let (mut r, _) = ObjReader::split(&bytes, Version::R2018, 30).unwrap();
        assert_eq!(decode_object_common(&mut r).unwrap(), c);
        assert_eq!(r.text().unwrap(), "name");
        assert_eq!(r.data.remaining_bits(), 0);
        assert!(r.handles.remaining_bits() < 8);
    }

    #[test]
    fn encode_rejects_unwritable() {
        let mut c = sample_entity().obj;
        c.xdata[1].items.clear();
        assert!(encode_object_common(&mut ObjWriter::new(c.handle), &c).is_err());
        let mut c = sample_entity().obj;
        c.xdata[1].items = vec![XDataItem::Binary(vec![0; 256])];
        assert!(encode_object_common(&mut ObjWriter::new(c.handle), &c).is_err());
    }

    /// R2000 entity, built by hand per ODS 20.4.1/20.4.2, with links and an
    /// owner-less entmode.
    fn r2000_entity(nolinks: bool) -> Vec<u8> {
        let mut d = BitWriter::new();
        d.write_handle_ref(HandleRef {
            code: 0,
            value: 0x50,
        })
        .unwrap();
        d.write_bs(0); // no EED
        d.write_b(false); // no graphics
        d.write_bb(2); // model space: no owner
        d.write_bl(1); // reactors
        d.write_b(nolinks);
        d.write_bs(5); // ENC: ACI 5
        d.write_bd(1.0);
        d.write_bb(0); // ltype bylayer
        d.write_bb(3); // plotstyle handle
        d.write_bs(0); // visible
        d.write_rc(0x1D); // lineweight
        let mut hs = BitWriter::new();
        hs.write_handle_ref(HandleRef {
            code: 4,
            value: 0x60,
        })
        .unwrap(); // reactor
        hs.write_handle_ref(HandleRef { code: 3, value: 0 })
            .unwrap(); // null xdict
        if !nolinks {
            hs.write_handle_ref(HandleRef { code: 8, value: 0 })
                .unwrap(); // prev = 0x4F
            hs.write_handle_ref(HandleRef {
                code: 0xA,
                value: 3,
            })
            .unwrap(); // next = 0x53
        }
        hs.write_handle_ref(HandleRef {
            code: 5,
            value: 0x10,
        })
        .unwrap(); // layer
        hs.write_handle_ref(HandleRef {
            code: 5,
            value: 0xF,
        })
        .unwrap(); // plotstyle
        assemble(Version::R2000, 0x13, &d, None, &hs)
    }

    #[test]
    fn r2000_entity_common() {
        for nolinks in [false, true] {
            let bytes = r2000_entity(nolinks);
            let (mut r, _) = ObjReader::split(&bytes, Version::R2000, 30).unwrap();
            let e = decode_entity_common(&mut r).unwrap();
            assert_eq!(e.obj.handle, Handle(0x50));
            assert_eq!(e.obj.owner, Handle(0));
            assert_eq!(e.obj.reactors, [Handle(0x60)]);
            assert_eq!(e.obj.xdict, None);
            assert_eq!(e.entmode, 2);
            assert_eq!(e.color, Color::Index(5));
            assert_eq!(e.layer, Handle(0x10));
            assert_eq!(e.linetype, LinetypeRef::ByLayer);
            assert_eq!(e.plotstyle, PlotStyleRef::Handle(Handle(0xF)));
            assert_eq!(e.material, MaterialRef::ByLayer);
            assert_eq!(e.lineweight, 0x1D);
            let links = (!nolinks).then_some((Handle(0x4F), Handle(0x53)));
            assert_eq!(e.prev_next, links);
            assert_eq!(r.data.remaining_bits(), 0);
            assert!(r.handles.remaining_bits() < 8);
        }
    }

    #[test]
    fn r2004_xdata_string_uses_codepage() {
        let mut d = BitWriter::new();
        d.write_handle_ref(HandleRef {
            code: 0,
            value: 0x50,
        })
        .unwrap();
        // RS length, RC code page, bytes (see read_xdata_string).
        let item = [0u8, 2, 0, 38, 0x82, 0xA0];
        d.write_bs(item.len() as i16);
        d.write_handle_ref(HandleRef {
            code: 5,
            value: 0x12,
        })
        .unwrap();
        d.write_bytes(&item);
        d.write_bs(0);
        d.write_bl(0); // reactors
        d.write_b(true); // no xdict
        let mut hs = BitWriter::new();
        hs.write_handle_ref(HandleRef { code: 4, value: 1 })
            .unwrap();
        let bytes = assemble(Version::R2004, 0x33, &d, None, &hs);
        // Drawing code page 30; the item names its own (38).
        let (mut r, _) = ObjReader::split(&bytes, Version::R2004, 30).unwrap();
        let c = decode_object_common(&mut r).unwrap();
        assert_eq!(
            c.xdata,
            [XDataApp {
                app: Handle(0x12),
                items: vec![XDataItem::String("あ".into())]
            }]
        );
        assert_eq!(c.owner, Handle(1));
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
    }

    #[test]
    fn r2004_xdata_string_unknown_codepage_warns() {
        let mut d = BitWriter::new();
        d.write_handle_ref(HandleRef {
            code: 0,
            value: 0x50,
        })
        .unwrap();
        // Code page 11 (DOS 437) is not supported: read as 1252, with a warning.
        let item = [0u8, 1, 0, 11, b'A'];
        d.write_bs(item.len() as i16);
        d.write_handle_ref(HandleRef {
            code: 5,
            value: 0x12,
        })
        .unwrap();
        d.write_bytes(&item);
        d.write_bs(0);
        d.write_bl(0); // reactors
        d.write_b(true); // no xdict
        let mut hs = BitWriter::new();
        hs.write_handle_ref(HandleRef { code: 4, value: 1 })
            .unwrap();
        let bytes = assemble(Version::R2004, 0x33, &d, None, &hs);
        let (mut r, _) = ObjReader::split(&bytes, Version::R2004, 30).unwrap();
        let c = decode_object_common(&mut r).unwrap();
        assert_eq!(c.xdata[0].items, [XDataItem::String("A".into())]);
        assert_eq!(r.diagnostics.len(), 1, "{:?}", r.diagnostics);
        assert_eq!(r.diagnostics[0].handle, Some(Handle(0x50)));
        assert!(r.diagnostics[0].message.contains("code page 11"));
    }

    // ---- hostile input ----

    fn object_with_data(d: &BitWriter, hs: &BitWriter) -> Vec<u8> {
        assemble(Version::R2018, 0x33, d, None, hs)
    }

    #[test]
    fn hostile_xdata_size() {
        let mut d = BitWriter::new();
        d.write_handle_ref(HandleRef { code: 0, value: 1 }).unwrap();
        d.write_bs(-1); // 0xFFFF bytes of EED
        d.write_handle_ref(HandleRef {
            code: 5,
            value: 0x12,
        })
        .unwrap();
        d.write_bytes(&[0; 8]);
        let bytes = object_with_data(&d, &BitWriter::new());
        let (mut r, _) = ObjReader::split(&bytes, Version::R2018, 30).unwrap();
        assert!(matches!(
            decode_object_common(&mut r),
            Err(Error::Truncated { .. })
        ));
    }

    #[test]
    fn hostile_xdata_items() {
        let cases: [&[u8]; 6] = [
            &[0, 0xFF, 0x7F],    // string longer than the item data
            &[4, 200, 1],        // binary longer than the item data
            &[3, 1, 2, 3],       // short layer handle
            &[2, 7],             // bad brace
            &[1, 0, 0],          // 1001 is not valid inside EED
            &[0x63, 0, 0, 0, 0], // unknown code
        ];
        for item in cases {
            let mut d = BitWriter::new();
            d.write_handle_ref(HandleRef { code: 0, value: 1 }).unwrap();
            d.write_bs(item.len() as i16);
            d.write_handle_ref(HandleRef {
                code: 5,
                value: 0x12,
            })
            .unwrap();
            d.write_bytes(item);
            d.write_bs(0);
            d.write_bl(0);
            d.write_b(true);
            d.write_b(false);
            let mut hs = BitWriter::new();
            hs.write_handle_ref(HandleRef { code: 4, value: 1 })
                .unwrap();
            let bytes = object_with_data(&d, &hs);
            let (mut r, _) = ObjReader::split(&bytes, Version::R2018, 30).unwrap();
            assert!(decode_object_common(&mut r).is_err(), "{item:?}");
        }
    }

    #[test]
    fn hostile_reactor_count() {
        for n in [i32::MAX, -1] {
            let mut d = BitWriter::new();
            d.write_handle_ref(HandleRef { code: 0, value: 1 }).unwrap();
            d.write_bs(0);
            d.write_bl(n);
            let mut hs = BitWriter::new();
            hs.write_handle_ref(HandleRef { code: 4, value: 1 })
                .unwrap();
            let bytes = object_with_data(&d, &hs);
            let (mut r, _) = ObjReader::split(&bytes, Version::R2018, 30).unwrap();
            assert!(
                matches!(decode_object_common(&mut r), Err(Error::Corrupt(_))),
                "{n}"
            );
        }
    }

    #[test]
    fn proxy_primitives_decode_on_demand() {
        let mut e = sample_entity();
        // The sample's 4 stored bytes are not a blob.
        assert!(e.proxy_primitives(Version::R2018).unwrap().is_err());
        // A blob of one POP_CLIP primitive.
        e.proxy_graphics = Some(vec![16, 0, 0, 0, 1, 0, 0, 0, 8, 0, 0, 0, 28, 0, 0, 0]);
        assert_eq!(
            e.proxy_primitives(Version::R2018).unwrap().unwrap(),
            [crate::proxy_graphics::ProxyPrimitive::PopClip]
        );
        e.proxy_graphics = None;
        assert!(e.proxy_primitives(Version::R2018).is_none());
    }

    #[test]
    fn hostile_proxy_graphics_size() {
        let mut d = BitWriter::new();
        d.write_handle_ref(HandleRef { code: 0, value: 1 }).unwrap();
        d.write_bs(0);
        d.write_b(true);
        d.write_bll(1 << 50).unwrap();
        let bytes = object_with_data(&d, &BitWriter::new());
        let (mut r, _) = ObjReader::split(&bytes, Version::R2018, 30).unwrap();
        assert!(matches!(
            decode_entity_common(&mut r),
            Err(Error::Truncated { .. })
        ));
    }

    #[test]
    fn corrupted_bytes_never_panic() {
        let mut w = ObjWriter::new(Handle(0x2A0));
        encode_entity_common(&mut w, &sample_entity()).unwrap();
        let bytes = w.finish(0x13).unwrap();
        for i in 0..bytes.len() {
            for v in [0x00, 0xFF, 0x55, 0x80] {
                let mut bad = bytes.clone();
                bad[i] = v;
                if let Ok((mut r, _)) = ObjReader::split(&bad, Version::R2018, 30) {
                    let _ = decode_entity_common(&mut r);
                }
            }
        }
    }

    /// Golden decodes of real objects. The bytes (`testdata/*.bin`, test-only
    /// data, not published with the crate) are copied from the ACadSharp
    /// (MIT, Copyright (c) 2021 Albert Domenech) sample drawings listed in
    /// `corpus/manifest.toml` (commit ed5ff1cf4731): `sample_AC10xx.dwg`, each
    /// object from its `MS` through its CRC. Compiled only when the fixtures
    /// are present (`build.rs` sets `opendwg_fixtures`). They pin the layouts where ODS
    /// is wrong or silent; a symmetric writer/reader round trip cannot.
    #[cfg(opendwg_fixtures)]
    mod golden {
        use super::*;

        fn decode(bytes: &[u8], v: Version) -> (EntityCommon, ObjReader<'_>, u16) {
            let (mut r, pre) = ObjReader::split(bytes, v, 30).unwrap();
            assert!(r.crc_ok());
            let e = decode_entity_common(&mut r).unwrap();
            (e, r, pre.type_code)
        }

        /// LINE body (R2000+): returns (start, end), checks the data stream
        /// ends exactly after it.
        fn line_body(r: &mut ObjReader) -> ([f64; 3], [f64; 3]) {
            let d = &mut r.data;
            let z_zero = d.read_b().unwrap();
            let x1 = d.read_rd().unwrap();
            let x2 = d.read_dd(x1).unwrap();
            let y1 = d.read_rd().unwrap();
            let y2 = d.read_dd(y1).unwrap();
            let (z1, z2) = if z_zero {
                (0.0, 0.0)
            } else {
                let z1 = d.read_rd().unwrap();
                (z1, d.read_dd(z1).unwrap())
            };
            d.read_bt().unwrap();
            d.read_be().unwrap();
            assert_eq!(d.remaining_bits(), 0);
            ([x1, y1, z1], [x2, y2, z2])
        }

        /// CIRCLE body: returns (center, radius), checks the data stream ends
        /// exactly after it.
        fn circle_body(r: &mut ObjReader) -> ([f64; 3], f64) {
            let d = &mut r.data;
            let c = d.read_3bd().unwrap();
            let radius = d.read_bd().unwrap();
            d.read_bt().unwrap();
            d.read_be().unwrap();
            assert_eq!(d.remaining_bits(), 0);
            (c, radius)
        }

        /// Only end padding may remain in the handle stream.
        fn handles_done(r: &ObjReader) {
            assert!(r.handles.remaining_bits() < 8);
        }

        /// R2018 CIRCLE 0x99F: book color. ENC word 0xC02A carries no RGB long.
        const R2018_CIRCLE_BOOK: &[u8] = include_bytes!("testdata/r2018_circle_book.bin");

        #[test]
        fn r2018_circle_with_book_color() {
            let (e, mut r, t) = decode(R2018_CIRCLE_BOOK, Version::R2018);
            assert_eq!(t, 0x12);
            assert_eq!(e.obj.handle, Handle(0x99F));
            assert_eq!(e.entmode, 2);
            assert_eq!(e.color, Color::Index(42));
            assert_eq!(e.color_book, Some(Handle(0x996)));
            assert_eq!(e.layer, Handle(0x10));
            assert_eq!(e.linetype, LinetypeRef::ByLayer);
            assert_eq!(e.material, MaterialRef::ByLayer);
            assert_eq!(e.plotstyle, PlotStyleRef::ByLayer);
            assert_eq!((e.ltype_scale, e.lineweight), (1.0, 29));
            assert_eq!(
                circle_body(&mut r),
                (
                    [30.20357222512345, -119.96686645223852, 0.0],
                    2.382841759818497
                )
            );
            handles_done(&r);
        }

        /// R2007 CIRCLE 0xE00: material handle. Decodes only if the material
        /// flags and shadow byte precede the plot style flags.
        const R2007_CIRCLE_MATERIAL: &[u8] = include_bytes!("testdata/r2007_circle_material.bin");

        #[test]
        fn r2007_circle_with_material() {
            let (e, mut r, t) = decode(R2007_CIRCLE_MATERIAL, Version::R2007);
            assert_eq!(t, 0x12);
            assert_eq!(e.obj.handle, Handle(0xE00));
            assert_eq!(e.material, MaterialRef::Handle(Handle(0xDC0)));
            assert_eq!(e.plotstyle, PlotStyleRef::ByLayer);
            assert_eq!(e.shadow_flags, 0);
            assert_eq!(e.layer, Handle(0x10));
            assert_eq!(e.color, Color::ByLayer);
            assert_eq!(e.color_book, None);
            assert_eq!(
                circle_body(&mut r),
                (
                    [15.269596038777308, -241.45588760887495, 0.0],
                    2.586142288053075
                )
            );
            handles_done(&r);
        }

        /// R2004 LINE 0x2C7: no `nolinks` bit from R2004.
        const R2004_LINE: &[u8] = include_bytes!("testdata/r2004_line.bin");

        #[test]
        fn r2004_line() {
            let (e, mut r, t) = decode(R2004_LINE, Version::R2004);
            assert_eq!(t, 0x13);
            assert_eq!(e.obj.handle, Handle(0x2C7));
            assert_eq!(e.prev_next, None);
            assert_eq!(e.entmode, 2);
            assert_eq!(e.layer, Handle(0x10));
            assert_eq!(e.color, Color::ByLayer);
            assert_eq!((e.ltype_scale, e.lineweight), (1.0, 29));
            assert_eq!(
                line_body(&mut r),
                (
                    [3.5925339989093885, 1.477241896180196, 0.0],
                    [6.8635470339795575, 1.477241896180196, 0.0]
                )
            );
            handles_done(&r);
        }

        /// R2013 LINE 0x2CF: linetype handle, ACI color, R2010+ size `MC` and
        /// visual style bits.
        const R2013_LINE_LTYPE: &[u8] = include_bytes!("testdata/r2013_line_ltype.bin");

        #[test]
        fn r2013_line_with_linetype() {
            let (e, mut r, t) = decode(R2013_LINE_LTYPE, Version::R2013);
            assert_eq!(t, 0x13);
            assert_eq!(e.obj.handle, Handle(0x2CF));
            assert_eq!(e.linetype, LinetypeRef::Handle(Handle(0x280)));
            assert_eq!(e.color, Color::Index(3));
            assert_eq!(e.layer, Handle(0x10));
            assert_eq!(e.lineweight, 3);
            assert_eq!(e.full_visual_style, None);
            assert_eq!(
                line_body(&mut r),
                (
                    [3.5925339989093885, 2.2977100470809333, 0.0],
                    [6.8635470339795575, 2.2977100470809333, 0.0]
                )
            );
            handles_done(&r);
        }

        /// R2000 CIRCLE 0x99F: extended data strings are `RS` length, `RC`
        /// code page, then the bytes. These bytes alone cannot tell that
        /// layout from ODS's (`06 00 1E` read as RC 6 + RS 0x1E00 also gives 6
        /// bytes, and the unknown code page falls back to 1252, which is code
        /// page 30); `r2004_xdata_string_uses_codepage` pins it with code page
        /// 38.
        const R2000_CIRCLE_XDATA: &[u8] = include_bytes!("testdata/r2000_circle_xdata.bin");

        #[test]
        fn r2000_circle_with_xdata_string() {
            let (e, mut r, t) = decode(R2000_CIRCLE_XDATA, Version::R2000);
            assert_eq!(t, 0x12);
            assert_eq!(e.obj.handle, Handle(0x99F));
            assert_eq!(
                e.obj.xdata,
                [XDataApp {
                    app: Handle(0x12),
                    items: vec![
                        XDataItem::String("RTTcAl".into()),
                        XDataItem::Short(-16342),
                        XDataItem::Long(-1_025_339_392),
                        XDataItem::String("RAL CLASSIC$RAL 1006".into()),
                    ],
                }]
            );
            assert_eq!(e.color, Color::Index(42));
            assert_eq!(e.layer, Handle(0x10));
            assert_eq!(e.prev_next, None); // nolinks set
            assert_eq!(
                circle_body(&mut r),
                (
                    [30.20357222512345, -119.96686645223852, 0.0],
                    2.382841759818497
                )
            );
            handles_done(&r);
        }

        /// R2018 LAYER 0x10 ("0"): object common data and a string stream. The
        /// name's `TU` length is 1: AutoCAD stores no NUL terminator.
        const R2018_LAYER_0: &[u8] = include_bytes!("testdata/r2018_layer_0.bin");

        #[test]
        fn r2018_layer_name_has_no_terminator() {
            let (mut r, pre) = ObjReader::split(R2018_LAYER_0, Version::R2018, 30).unwrap();
            assert!(r.crc_ok());
            assert_eq!(pre.type_code, 0x33);
            let c = decode_object_common(&mut r).unwrap();
            assert_eq!(c.handle, Handle(0x10));
            assert_eq!(c.xdict, Some(Handle(0xE6)));
            assert_eq!(c.owner, Handle(0x2));
            let mut raw = r.strings.clone().unwrap();
            assert_eq!(raw.read_bs().unwrap(), 1);
            assert_eq!(r.text().unwrap(), "0");
            // The name is the whole string stream.
            assert_eq!(r.strings.as_ref().unwrap().remaining_bits(), 0);
        }
    }
}
