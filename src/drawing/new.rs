//! [`Drawing::new`]: a complete, empty R2018 drawing.
//!
//! A new drawing holds the objects AutoCAD creates for a drawing started
//! from its default template (`acad.dwt`, imperial), built here from typed
//! values (the same structs the decoders produce, written by the normal
//! encoders). Which objects, how they link, and their values follow the
//! template objects of the smallest R2018 corpus file
//! (`acadsharp-dynblock-lookup`); the values are written out below as
//! named constants and literals. `tests/from_scratch.rs`
//! (`new_matches_the_corpus_template`) compares the drawing with those
//! objects field by field, references compared by what they name, and
//! lists the differences kept.
//!
//! The objects:
//!
//! - the nine table controls; layer `0`; linetypes `ByBlock`, `ByLayer` and
//!   `Continuous`; text styles `Standard` and `Annotative`; dimension styles
//!   `Standard` and `Annotative` (from the header's dimension variables);
//!   the `*Active` viewport; the applications `ACAD`, `AcadAnnoPO`,
//!   `AcadAnnotative`, `ACAD_DSTYLE_DIMJAG`, `ACAD_DSTYLE_DIMTALN`,
//!   `ACAD_MLEADERVER` and `ACAD_NAV_VCDISPLAY` (the annotative styles' and
//!   the viewport's extended data name them);
//! - the `*Model_Space` and `*Paper_Space` block records with their BLOCK
//!   and ENDBLK, and the `Model` and `Layout1` layouts, each layout and block
//!   record naming the other;
//! - the named object dictionary with `ACAD_COLOR` (empty), `ACAD_GROUP`
//!   (empty), `ACAD_LAYOUT`, `ACAD_MATERIAL` (`ByBlock`, `ByLayer`,
//!   `Global`), `ACAD_MLEADERSTYLE` (`Standard`, `Annotative`),
//!   `ACAD_MLINESTYLE` (`Standard`), `ACAD_PLOTSETTINGS` (empty),
//!   `ACAD_PLOTSTYLENAME` (`Normal`, a placeholder, the default),
//!   `ACAD_SCALELIST` (the 33 standard scales), `ACAD_TABLESTYLE`
//!   (`Standard`), `ACAD_VISUALSTYLE` (the 24 standard visual styles),
//!   `ACAD_SECTIONVIEWSTYLE` and `ACAD_DETAILVIEWSTYLE` (`Imperial24`) and
//!   `AcDbVariableDictionary` (`CANNOSCALE`, `CMLEADERSTYLE`, `CTABLESTYLE`,
//!   `CVIEWDETAILSTYLE`, `CVIEWSECTIONSTYLE`, `DIMASSOC`, `HIDETEXT`,
//!   `LAYEREVAL`, `LAYERNOTIFY`);
//! - the extension dictionaries AutoCAD gives some of these: empty ones on
//!   layer `0`, the model space block record, the `Model` layout and the
//!   table style; the layer table's, with an empty `ACAD_LAYERSTATES`; the
//!   `Global` material's, with its four map tiling records.
//!
//! Left out of the corpus template: the record of the AutoCAD product that
//! saved the file (`ACAD_CIP_PREVIOUS_PRODUCT_INFO`) and of its last save
//! (`ACAD_LAST_SAVED_VERSION_INFO`), the second layout `Layout2` (the brief
//! asks for `Model` and `Layout1`), and the materials' `FBXASSET` records
//! (AutoCAD's packaged binary assets; with them go the `ByBlock` and
//! `ByLayer` materials' extension dictionaries, which hold nothing else).
//!
//! Values that differ from the corpus file, which are its drawing's own:
//! the `*Active` viewport shows the limits (center (6, 4.5), height 9, the
//! 12 x 9 limits' aspect ratio); the `Model` layout plots at 1:1; no object
//! claims data in a data storage section (the corpus's model layout has a
//! thumbnail there).
//!
//! # Handles
//!
//! Handles are given from 1 in the order of [`build`]: the table controls,
//! the named object dictionary and its dictionaries, then the applications,
//! linetypes, materials, layer `0`, text and dimension styles, the other
//! styles, scales and variables, the active viewport, and last the model
//! and paper space blocks with their layouts. Dictionary
//! entries are kept sorted by name (byte order), as AutoCAD keeps them.
//! [`Drawing::new_at`] gives the same drawing, and so the same bytes, for
//! the same time.
//!
//! # Header
//!
//! [`defaults_r2018`] with every handle variable naming its object, found by
//! name: the table controls, the named object dictionary and its entries,
//! `$CLAYER` layer `0`, `$CELTYPE` `ByLayer`, `$TEXTSTYLE` and `$DIMTXSTY`
//! `Standard`, `$DIMSTYLE` `Standard`, `$CMLSTYLE` `Standard`, `$CMATERIAL`
//! material `ByLayer`, the model and paper space block records, the three
//! built-in linetypes, `$INTERFEREOBJVS` visual style `Realistic` and
//! `$INTERFEREVPVS` `Wireframe` (as in the corpus file; `$DRAGVS` and the
//! UCS and dimension block handles are null there too). `$TDCREATE` and
//! `$TDUPDATE` are the creation time; `$FINGERPRINTGUID` and
//! `$VERSIONGUID` are derived from it (two [`AcisGuid::derived`] values;
//! [`Drawing::new`] also mixes in a per-process count).
//!
//! # Classes
//!
//! The corpus file's first 17 classes ([`CLASSES`], defined by
//! [`known_class`]), which the other R2018 corpus file has alike: the
//! classes AutoCAD registers for an empty drawing (its later classes come
//! with its dynamic blocks). Classes without objects are kept, as AutoCAD
//! keeps them; the counts are those of the new drawing's objects. Adding an
//! object of another class type adds its class (see `drawing::api`).
//!
//! # Other sections
//!
//! An empty summary (no title, author, ...) whose dates are the header's,
//! [`AppInfo::opendwg`], an empty preview (no images) and no data storage
//! section (the writer adds one when an ACIS entity needs it). The file
//! header and section metadata are the writer's R2018 defaults.

use std::collections::BTreeMap;
use std::f64::consts::{FRAC_PI_2, PI};
use std::sync::atomic::{AtomicU64, Ordering};

use super::Drawing;
use super::write::{ReadState, class_counts, julian_now};
use crate::bits::Color;
use crate::handle::Handle;
use crate::object::tables::{Lighting, UcsFrame, ViewMode};
use crate::object::{
    AcisGuid, AppId, AsRead, Block, BlockRecord, CellBorder, CellBorderData, CellStyle,
    CellStyleData, CellTableStyle, ClassIndex, ContentFormat, DetailViewStyle, Dictionary,
    DictionaryVar, DictionaryWithDefault, DimStyle, Entity, EntityCommon, EntityList, IDENTITY_4X4,
    Layer, Layout, Linetype, MLeaderStyle, MLineLinetype, MLineStyle, MLineStyleElement, Material,
    MaterialColor, MaterialMap, ModelDocViewStyle, NamedCellStyle, NonGraphical, Object,
    ObjectCommon, PlotSettings, Scale, SectionViewStyle, TableControl, TableEntry, TableKind,
    TableStyle, TextStyle, VisualStyle, VisualStyleValue, Vport, XDataApp, XDataItem, XRecord,
    XRecordValue,
};
use crate::section::classes::{FIRST_CLASS_NUMBER, known_class};
use crate::section::header::defaults_r2018;
use crate::section::{AppInfo, DxfClass, HeaderVars, JulianDate, Preview, SummaryInfo};

/// The classes of a new drawing, in order from class number 500: the first
/// 17 classes of the corpus file `acadsharp-dynblock-lookup`, with the
/// definitions of [`known_class`]. `tests/from_scratch.rs` checks them
/// against the corpus.
const CLASSES: [&str; 17] = [
    "ACDBDICTIONARYWDFLT",
    "ACDBPLACEHOLDER",
    "LAYOUT",
    "DICTIONARYVAR",
    "TABLESTYLE",
    "MATERIAL",
    "VISUALSTYLE",
    "SCALE",
    "MLEADERSTYLE",
    "CELLSTYLEMAP",
    "EXACXREFPANELOBJECT",
    "NPOCOLLECTION",
    "LAYER_INDEX",
    "SPATIAL_INDEX",
    "IDBUFFER",
    "ACDBSECTIONVIEWSTYLE",
    "ACDBDETAILVIEWSTYLE",
];

/// [`CLASSES`] as classes, with no instances.
///
/// # Panics
///
/// When a name of [`CLASSES`] has no [`known_class`] definition (the unit
/// test `classes_are_known` checks none lacks one).
fn template_classes() -> Vec<DxfClass> {
    (FIRST_CLASS_NUMBER..)
        .zip(CLASSES)
        .map(|(number, name)| known_class(name, number).expect("a known class"))
        .collect()
}

impl Default for Drawing {
    /// [`Drawing::new`].
    fn default() -> Self {
        Drawing::new()
    }
}

impl Drawing {
    /// A new, empty drawing created now (UTC; see [`Drawing::new_at`]).
    /// Its GUIDs also derive from a count of the drawings this process has
    /// created, so that two drawings created within one millisecond differ.
    pub fn new() -> Drawing {
        static CREATED: AtomicU64 = AtomicU64::new(0);
        let n = CREATED.fetch_add(1, Ordering::Relaxed);
        Drawing::create(julian_now(), &format!(" #{n}"))
    }

    /// A new, empty drawing created at `created` ($TDCREATE and $TDUPDATE):
    /// the objects of AutoCAD's default template (see the module
    /// documentation of `drawing::new`). The same time always gives the
    /// same drawing.
    pub fn new_at(created: JulianDate) -> Drawing {
        Drawing::create(created, "")
    }

    /// [`Drawing::new_at`], with `salt` added to the seed of the GUIDs.
    fn create(created: JulianDate, salt: &str) -> Drawing {
        let mut header = defaults_r2018();
        let objects = build(&header);
        name_objects(&mut header, &objects);
        header.handseed = Handle(objects.last_key_value().map_or(1, |(h, _)| h.0 + 1));
        header.tdcreate = created;
        header.tdupdate = created;
        let seed = format!("opendwg new drawing {} {}{salt}", created.day, created.ms);
        let guid = |n| {
            AcisGuid::derived(&seed, Handle(n))
                .to_string()
                .to_uppercase()
        };
        header.fingerprintguid = guid(1);
        header.versionguid = guid(2);
        let summary = SummaryInfo {
            total_edit_time: header.tdindwg,
            create: header.tdcreate,
            modified: header.tdupdate,
            ..SummaryInfo::default()
        };
        let mut classes = template_classes();
        let counts = class_counts(&objects, &ClassIndex::new(&classes));
        for c in &mut classes {
            c.instance_count = counts.get(&c.class_number).copied().unwrap_or(0);
        }
        Drawing {
            source_version: None,
            header,
            classes,
            objects,
            ref_index: Default::default(),
            summary: Some(summary),
            preview: Some(Preview::default()),
            app_info: Some(AppInfo::opendwg()),
            data_storage: None,
            extra_sections: BTreeMap::new(),
            diagnostics: Vec::new(),
            read: ReadState::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Building the objects.

/// The new drawing's objects, handed out handles in creation order.
struct Builder {
    objects: BTreeMap<Handle, Object>,
    next: u64,
}

impl Builder {
    fn alloc(&mut self) -> Handle {
        self.next += 1;
        Handle(self.next)
    }

    /// A new non-graphical object owned by `owner`, with `reactors`.
    fn add(&mut self, owner: Handle, reactors: Vec<Handle>, object: NonGraphical) -> Handle {
        let h = self.alloc();
        let common = ObjectCommon {
            handle: h,
            owner,
            reactors,
            ..ObjectCommon::new()
        };
        self.objects.insert(h, Object::Other { common, object });
        h
    }

    fn other(&mut self, h: Handle) -> &mut NonGraphical {
        match self.objects.get_mut(&h) {
            Some(Object::Other { object, .. }) => object,
            _ => unreachable!("{h} is a non-graphical object of the new drawing"),
        }
    }

    /// A new entry `name` of dictionary `dict`, owned by it and naming it
    /// among its reactors.
    fn entry(&mut self, dict: Handle, name: &str, object: NonGraphical) -> Handle {
        let h = self.add(dict, vec![dict], object);
        match self.other(dict) {
            NonGraphical::Dictionary(d) => d.entries.push((name.into(), h)),
            NonGraphical::DictionaryWithDefault(d) => d.dictionary.entries.push((name.into(), h)),
            _ => unreachable!("{dict} is a dictionary"),
        }
        h
    }

    /// A new empty dictionary entry `name` of `dict`.
    fn dictionary(&mut self, dict: Handle, name: &str) -> Handle {
        self.entry(dict, name, NonGraphical::Dictionary(dictionary(0)))
    }

    /// A new record of the table `control` owns.
    fn record(&mut self, control: Handle, object: NonGraphical) -> Handle {
        let h = self.add(control, Vec::new(), object);
        if let NonGraphical::TableControl(c) = self.other(control) {
            c.entries.push(h);
        }
        h
    }

    /// A new, empty extension dictionary of `h`.
    fn xdict(&mut self, h: Handle) -> Handle {
        let x = self.add(h, Vec::new(), NonGraphical::Dictionary(dictionary(1)));
        self.objects
            .get_mut(&h)
            .expect("an object")
            .object_common_mut()
            .xdict = Some(x);
        x
    }

    fn set_xdata(&mut self, h: Handle, xdata: Vec<XDataApp>) {
        self.objects
            .get_mut(&h)
            .expect("an object")
            .object_common_mut()
            .xdata = xdata;
    }

    /// A block record of `control` named `name` for `layout`, with its
    /// BLOCK and ENDBLK (entity mode `entmode`: 2 model space, 1 paper
    /// space, as AutoCAD stores them; no stored owner).
    fn block(&mut self, control: Handle, name: &str, entmode: u8, layer0: Handle) -> Handle {
        let record = self.record(
            control,
            NonGraphical::BlockRecord(BlockRecord {
                entry: entry(name),
                anonymous: false,
                has_attdefs: false,
                is_xref: false,
                xref_overlaid: false,
                xref_unloaded: false,
                base_point: [0.0; 3],
                xref_path: String::new(),
                description: String::new(),
                preview: Vec::new(),
                insert_units: 0,
                explodable: true,
                block_scaling: 0,
                block_entity: Handle(0),
                entities: EntityList::Owned(Vec::new()),
                end_block: Handle(0),
                inserts: Vec::new(),
                layout: Handle(0),
            }),
        );
        let mut marker = |entity: Entity| {
            let h = self.alloc();
            let mut common = EntityCommon::new(layer0);
            common.obj.handle = h;
            common.entmode = entmode;
            self.objects.insert(h, Object::Entity { common, entity });
            h
        };
        let begin = marker(Entity::Block(Block { name: name.into() }));
        let end = marker(Entity::EndBlk);
        if let NonGraphical::BlockRecord(b) = self.other(record) {
            b.block_entity = begin;
            b.end_block = end;
        }
        record
    }
}

fn entry(name: &str) -> TableEntry {
    TableEntry {
        name: name.into(),
        ..TableEntry::default()
    }
}

/// An empty dictionary; `hard_owner` 1 for extension dictionaries, as
/// AutoCAD writes them.
fn dictionary(hard_owner: u8) -> Dictionary {
    Dictionary {
        entries: Vec::new(),
        cloning: 1,
        hard_owner,
    }
}

fn control(kind: TableKind) -> NonGraphical {
    NonGraphical::TableControl(TableControl {
        kind,
        entries: Vec::new(),
        extra: Vec::new(),
    })
}

/// Extended data of application `app`.
fn xdata(app: Handle, items: Vec<XDataItem>) -> XDataApp {
    XDataApp { app, items }
}

/// The `ACAD` extended data AutoCAD gives a TrueType text style: the font's
/// family name and its flags (34: the character set and pitch bits of
/// Arial).
fn font_xdata(acad: Handle) -> XDataApp {
    xdata(
        acad,
        vec![XDataItem::String("Arial".into()), XDataItem::Long(34)],
    )
}

/// The `AcadAnnotative` extended data of an annotative style: annotative,
/// version 1.
fn annotative_xdata(app: Handle) -> XDataApp {
    xdata(
        app,
        vec![
            XDataItem::String("AnnotativeData".into()),
            XDataItem::ControlBrace(true),
            XDataItem::Short(1),
            XDataItem::Short(1),
            XDataItem::ControlBrace(false),
        ],
    )
}

/// The default text font and text height of `acad.dwt`.
const FONT: &str = "arial.ttf";
const TEXT_HEIGHT: f64 = 0.2;

fn text_style(name: &str) -> NonGraphical {
    NonGraphical::TextStyle(TextStyle {
        entry: entry(name),
        shape_file: false,
        vertical: false,
        fixed_height: 0.0,
        width_factor: 1.0,
        oblique_angle: 0.0,
        generation: 0,
        last_height: TEXT_HEIGHT,
        font_name: FONT.into(),
        bigfont_name: String::new(),
    })
}

fn app_id(name: &str) -> NonGraphical {
    NonGraphical::AppId(AppId {
        entry: entry(name),
        unknown: 0,
    })
}

/// A linetype without dashes; alignment 65 (`A`, the only one DXF allows).
fn linetype(name: &str, description: &str) -> NonGraphical {
    NonGraphical::Linetype(Linetype {
        entry: entry(name),
        description: description.into(),
        pattern_length: 0.0,
        alignment: b'A',
        dashes: Vec::new(),
        text_area: Vec::new(),
    })
}

/// A dimension style with the header's dimension variables (the template's
/// `Standard` style and the header agree), text style `text_style`.
pub(super) fn dim_style(name: &str, h: &HeaderVars, text_style: Handle) -> DimStyle {
    DimStyle {
        entry: entry(name),
        dimpost: h.dimpost.clone(),
        dimapost: h.dimapost.clone(),
        dimscale: h.dimscale,
        dimasz: h.dimasz,
        dimexo: h.dimexo,
        dimdli: h.dimdli,
        dimexe: h.dimexe,
        dimrnd: h.dimrnd,
        dimdle: h.dimdle,
        dimtp: h.dimtp,
        dimtm: h.dimtm,
        dimfxl: h.dimfxl.unwrap_or(1.0),
        dimjogang: h.dimjogang.unwrap_or(std::f64::consts::FRAC_PI_4),
        dimtfill: h.dimtfill.unwrap_or(0),
        dimtfillclr: h.dimtfillclr.clone().unwrap_or(Color::ByBlock),
        dimtol: h.dimtol,
        dimlim: h.dimlim,
        dimtih: h.dimtih,
        dimtoh: h.dimtoh,
        dimse1: h.dimse1,
        dimse2: h.dimse2,
        dimtad: h.dimtad,
        dimzin: h.dimzin,
        dimazin: h.dimazin,
        dimarcsym: h.dimarcsym.unwrap_or(0),
        dimtxt: h.dimtxt,
        dimcen: h.dimcen,
        dimtsz: h.dimtsz,
        dimaltf: h.dimaltf,
        dimlfac: h.dimlfac,
        dimtvp: h.dimtvp,
        dimtfac: h.dimtfac,
        dimgap: h.dimgap,
        dimaltrnd: h.dimaltrnd,
        dimalt: h.dimalt,
        dimaltd: h.dimaltd,
        dimtofl: h.dimtofl,
        dimsah: h.dimsah,
        dimtix: h.dimtix,
        dimsoxd: h.dimsoxd,
        dimclrd: h.dimclrd.clone(),
        dimclre: h.dimclre.clone(),
        dimclrt: h.dimclrt.clone(),
        dimadec: h.dimadec,
        dimdec: h.dimdec,
        dimtdec: h.dimtdec,
        dimaltu: h.dimaltu,
        dimalttd: h.dimalttd,
        dimaunit: h.dimaunit,
        dimfrac: h.dimfrac,
        dimlunit: h.dimlunit,
        dimdsep: h.dimdsep,
        dimtmove: h.dimtmove,
        dimjust: h.dimjust,
        dimsd1: h.dimsd1,
        dimsd2: h.dimsd2,
        dimtolj: h.dimtolj,
        dimtzin: h.dimtzin,
        dimaltz: h.dimaltz,
        dimalttz: h.dimalttz,
        dimupt: h.dimupt,
        dimfit: h.dimatfit,
        dimfxlon: h.dimfxlon.unwrap_or(false),
        dimtxtdirection: h.dimtxtdirection.unwrap_or(false),
        dimaltmzf: h.dimaltmzf.unwrap_or(100.0),
        dimaltmzs: h.dimaltmzs.clone().unwrap_or_default(),
        dimmzf: h.dimmzf.unwrap_or(100.0),
        dimmzs: h.dimmzs.clone().unwrap_or_default(),
        dimlwd: h.dimlwd,
        dimlwe: h.dimlwe,
        flag: false,
        dimtxsty: text_style,
        dimldrblk: Handle(0),
        dimblk: Handle(0),
        dimblk1: Handle(0),
        dimblk2: Handle(0),
        dimltype: Handle(0),
        dimltex1: Handle(0),
        dimltex2: Handle(0),
    }
}

/// Plot settings of `acad.dwt`'s layouts: `Model` on no plotter
/// (`none_device`) on ANSI A paper, 1:1; `Layout1` without a page setup.
/// The margins and paper size are millimetres, stored as `f32` values.
fn plot_settings(model: bool) -> PlotSettings {
    let mm = |v: f32| f64::from(v);
    let (printer, flags, margins, paper, paper_size, rotation, plot_type, scale_type) = if model {
        (
            "none_device",
            11952,
            [mm(6.35), mm(19.05), 6.350006103515625, 19.04998779296875],
            [mm(215.9), mm(279.4)],
            "ANSI_A_(8.50_x_11.00_Inches)",
            1,
            0,
            0,
        )
    } else {
        ("None", 688, [0.0; 4], [0.0; 2], "", 0, 5, 16)
    };
    PlotSettings {
        page_setup_name: String::new(),
        printer: printer.into(),
        flags,
        margins,
        paper_width: paper[0],
        paper_height: paper[1],
        paper_size: paper_size.into(),
        plot_origin: [0.0; 2],
        paper_units: 0,
        rotation,
        plot_type,
        window_min: [0.0; 2],
        window_max: [0.0; 2],
        plot_view_name: String::new(),
        custom_scale: [1.0, 1.0],
        style_sheet: String::new(),
        scale_type,
        scale_factor: 1.0,
        paper_image_origin: [0.0; 2],
        shade_plot_mode: 0,
        shade_plot_resolution: 2,
        shade_plot_dpi: 300,
        plot_view: Handle(0),
        visual_style: Handle(0),
    }
}

fn layout(name: &str, tab_order: i32, h: &HeaderVars, block_record: Handle) -> Layout {
    Layout {
        plot: plot_settings(tab_order == 0),
        name: name.into(),
        tab_order,
        flags: 1,
        ucs_origin: [0.0; 3],
        limits_min: h.limmin,
        limits_max: h.limmax,
        insertion_base: [0.0; 3],
        ucs_x_axis: [1.0, 0.0, 0.0],
        ucs_y_axis: [0.0, 1.0, 0.0],
        elevation: 0.0,
        ortho_type: 0,
        extents_min: [0.0; 3],
        extents_max: [0.0; 3],
        block_record,
        active_viewport: Handle(0),
        base_ucs: Handle(0),
        named_ucs: Handle(0),
        viewports: Vec::new(),
    }
}

/// The `*Active` viewport, showing the drawing limits, with `acad.dwt`'s
/// grid (0.5, every 5th major), snap and lighting.
fn active_vport(h: &HeaderVars, visual_style: Handle) -> Vport {
    let [x0, y0] = h.limmin;
    let [x1, y1] = h.limmax;
    Vport {
        entry: entry("*Active"),
        view_height: y1 - y0,
        aspect_ratio_times_height: x1 - x0,
        view_center: [(x0 + x1) / 2.0, (y0 + y1) / 2.0],
        view_target: [0.0; 3],
        view_direction: [0.0, 0.0, 1.0],
        view_twist: 0.0,
        lens_length: 50.0,
        front_clip: 0.0,
        back_clip: 0.0,
        view_mode: ViewMode {
            perspective: false,
            front_clip: false,
            back_clip: false,
            front_clip_not_at_eye: true,
        },
        render_mode: 0,
        lighting: Lighting {
            use_default_lights: true,
            default_lighting_type: 1,
            brightness: 0.0,
            contrast: 0.0,
            ambient_color: Color::Rgb {
                r: 51,
                g: 51,
                b: 51,
            },
        },
        lower_left: [0.0, 0.0],
        upper_right: [1.0, 1.0],
        ucs_follow: false,
        circle_zoom: 1000,
        fast_zoom: true,
        ucs_icon: 3,
        grid_on: true,
        grid_spacing: [0.5, 0.5],
        snap_on: false,
        snap_style: false,
        snap_isopair: 0,
        snap_rotation: 0.0,
        snap_base: [0.0, 0.0],
        snap_spacing: [0.5, 0.5],
        unknown_bit: false,
        ucs_per_viewport: true,
        ucs: UcsFrame {
            origin: [0.0; 3],
            x_axis: [1.0, 0.0, 0.0],
            y_axis: [0.0, 1.0, 0.0],
            elevation: 0.0,
            orthographic_type: 0,
        },
        grid_flags: 3,
        grid_major: 5,
        background: Handle(0),
        visual_style,
        sun: Handle(0),
        named_ucs: Handle(0),
        base_ucs: Handle(0),
    }
}

/// The `Standard` multiline style: two lines half a unit either side, ByLayer.
fn mline_style(bylayer: Handle) -> MLineStyle {
    let line = |offset| MLineStyleElement {
        offset,
        color: Color::ByLayer,
        linetype: MLineLinetype::Handle(bylayer),
    };
    MLineStyle {
        name: "STANDARD".into(),
        description: String::new(),
        flags: 0,
        fill_color: Color::ByLayer,
        start_angle: FRAC_PI_2,
        end_angle: FRAC_PI_2,
        elements: vec![line(0.5), line(-0.5)],
    }
}

/// The `Standard` multileader style (also the `Annotative` entry, which is
/// annotative): MTEXT content in `text_style`, straight leaders of linetype
/// `byblock`, the header's dimension sizes.
fn mleader_style(text_style: Handle, byblock: Handle, annotative: bool) -> MLeaderStyle {
    MLeaderStyle {
        version: 2,
        content_type: 2,
        draw_mleader_order: 1,
        draw_leader_order: 0,
        max_leader_points: 2,
        first_segment_angle: 0.0,
        second_segment_angle: 0.0,
        leader_type: 1,
        line_color: Color::ByBlock,
        line_type: byblock,
        line_weight: -2,
        landing_enabled: true,
        landing_gap: 0.09,
        dogleg_enabled: true,
        landing_distance: 0.36,
        description: "Standard".into(),
        arrow_head: Handle(0),
        arrow_head_size: 0.18,
        default_text: String::new(),
        text_style,
        left_attachment: 1,
        right_attachment: 1,
        text_angle_type: 1,
        text_alignment: 0,
        text_color: Color::ByBlock,
        text_height: 0.18,
        text_frame: false,
        align_text_left: false,
        align_space: 0.18,
        block: Handle(0),
        block_color: Color::ByBlock,
        block_scale: [1.0; 3],
        block_scale_enabled: true,
        block_rotation: 0.0,
        block_rotation_enabled: true,
        block_connection: 0,
        scale: 1.0,
        property_changed: false,
        annotative,
        break_size: 0.125,
        attachment_direction: 0,
        top_attachment: 9,
        bottom_attachment: 9,
        unknown_298: false,
    }
}

/// The `Standard` table style: the table's own cell style and the `_TITLE`,
/// `_HEADER` and `_DATA` cell styles, text in `text_style`, ByBlock
/// borders on every edge.
fn table_style(text_style: Handle) -> TableStyle {
    // Cell margins: left, top, right, bottom, then the horizontal and
    // vertical spacing.
    const MARGINS: [f64; 6] = [0.06, 0.06, 0.06, 0.06, 0.18, 0.18];
    let content = |alignment, text_style, text_height| ContentFormat {
        property_overrides: 0,
        property_flags: 0,
        value_data_type: 512,
        value_unit_type: 0,
        value_format: String::new(),
        rotation: 0.0,
        block_scale: 1.0,
        alignment,
        color: Color::ByBlock,
        text_style,
        text_height,
    };
    // The six edges: top, right, bottom, left, inside vertical, inside
    // horizontal.
    let borders = || {
        [1, 2, 4, 8, 16, 32]
            .into_iter()
            .map(|edge| CellBorder {
                edge,
                data: Some(CellBorderData {
                    overrides: 0,
                    border_type: 1,
                    color: Color::ByBlock,
                    line_weight: -2,
                    line_type: Handle(0),
                    invisible: 0,
                    double_line_spacing: 0.045,
                }),
            })
            .collect()
    };
    let style = |merge_flags, content, borders| CellStyle {
        style_type: 5,
        data: Some(CellStyleData {
            property_overrides: 0,
            merge_flags,
            background_color: Color::None,
            content_layout: 1,
            content,
            margin_flags: 0,
            margins: Some(MARGINS),
            borders,
        }),
    };
    let named = |style, id, class, name: &str| NamedCellStyle {
        style,
        id,
        class,
        name: name.into(),
    };
    TableStyle::Cells(Box::new(CellTableStyle {
        unknown_rc: 0,
        description: "Standard".into(),
        unknown_bl: [0, 101],
        unknown_handle: Handle(0),
        table_style: named(
            style(0, content(1, Handle(0), 0.18), Vec::new()),
            4,
            2,
            "Table",
        ),
        cell_styles: vec![
            (
                1,
                named(
                    style(0x8000, content(5, text_style, 0.25), borders()),
                    1,
                    1,
                    "_TITLE",
                ),
            ),
            (
                2,
                named(
                    style(0, content(5, text_style, 0.18), borders()),
                    2,
                    1,
                    "_HEADER",
                ),
            ),
            (
                3,
                named(
                    style(0, content(2, text_style, 0.18), borders()),
                    3,
                    2,
                    "_DATA",
                ),
            ),
        ],
    }))
}

/// A material: no color overrides; maps with projection 2, tiling 1 and
/// auto-transform 1, from an image file not named (the refraction map from
/// the current scene); `map_scale` scales the diffuse, reflection, opacity
/// and bump maps (the `Global` material's 0.0208, others' 1).
fn material(name: &str, map_scale: f64) -> Material {
    let color = || MaterialColor {
        method: 0,
        factor: 1.0,
        rgb: None,
    };
    let map = |scaled: bool, file: bool| {
        let mut matrix = IDENTITY_4X4;
        if scaled {
            matrix[0] = map_scale;
            matrix[5] = map_scale;
        }
        MaterialMap {
            blend_factor: 1.0,
            projection: 2,
            tiling: 1,
            auto_transform: 1,
            matrix,
            source: u8::from(file),
            file_name: file.then(String::new),
        }
    };
    Material {
        name: name.into(),
        description: String::new(),
        ambient: color(),
        diffuse: color(),
        diffuse_map: map(true, true),
        specular: color(),
        specular_map: map(false, true),
        gloss: 0.5,
        reflection_map: map(true, true),
        opacity: 1.0,
        opacity_map: map(true, true),
        bump_map: map(true, true),
        refraction_index: 1.0,
        refraction_map: map(false, false),
        translucence: 0.0,
        self_illumination: 0.0,
        reflectivity: 0.0,
        illumination_model: 0,
        channel_flags: 63,
        mode: 0,
    }
}

/// The scale of the `Global` material's maps.
const GLOBAL_MAP_SCALE: f32 = 0.0208;

/// The 33 standard annotation scales of `ACAD_SCALELIST` (entries `A0` to
/// `D2`): name, paper units, drawing units.
#[rustfmt::skip]
const SCALES: [(&str, f64, f64); 33] = [
    ("1:1", 1.0, 1.0), ("1:2", 1.0, 2.0), ("1:4", 1.0, 4.0), ("1:5", 1.0, 5.0),
    ("1:8", 1.0, 8.0), ("1:10", 1.0, 10.0), ("1:16", 1.0, 16.0), ("1:20", 1.0, 20.0),
    ("1:30", 1.0, 30.0), ("1:40", 1.0, 40.0), ("1:50", 1.0, 50.0), ("1:100", 1.0, 100.0),
    ("2:1", 2.0, 1.0), ("4:1", 4.0, 1.0), ("8:1", 8.0, 1.0), ("10:1", 10.0, 1.0),
    ("100:1", 100.0, 1.0),
    ("1/128\" = 1'-0\"", 1.0 / 128.0, 12.0), ("1/64\" = 1'-0\"", 1.0 / 64.0, 12.0),
    ("1/32\" = 1'-0\"", 1.0 / 32.0, 12.0), ("1/16\" = 1'-0\"", 1.0 / 16.0, 12.0),
    ("3/32\" = 1'-0\"", 3.0 / 32.0, 12.0), ("1/8\" = 1'-0\"", 1.0 / 8.0, 12.0),
    ("3/16\" = 1'-0\"", 3.0 / 16.0, 12.0), ("1/4\" = 1'-0\"", 1.0 / 4.0, 12.0),
    ("3/8\" = 1'-0\"", 3.0 / 8.0, 12.0), ("1/2\" = 1'-0\"", 1.0 / 2.0, 12.0),
    ("3/4\" = 1'-0\"", 3.0 / 4.0, 12.0), ("1\" = 1'-0\"", 1.0, 12.0),
    ("1-1/2\" = 1'-0\"", 1.5, 12.0), ("3\" = 1'-0\"", 3.0, 12.0),
    ("6\" = 1'-0\"", 6.0, 12.0), ("1'-0\" = 1'-0\"", 12.0, 12.0),
];

/// The key of the `n`th entry of `ACAD_SCALELIST`: `A0`, `A1`, ... `D2`.
fn scale_key(n: usize) -> String {
    let letter = char::from(b'A' + (n / 10) as u8);
    format!("{letter}{}", n % 10)
}

/// A visual style property value, as the tables below write it.
#[derive(Clone)]
enum P {
    I(i32),
    D(f64),
    C(Color),
    B(bool),
    T(&'static str),
}
use P::{B, C, D, I, T};

impl P {
    fn value(self) -> VisualStyleValue {
        match self {
            I(v) => VisualStyleValue::Int(v),
            D(v) => VisualStyleValue::Double(v),
            C(v) => VisualStyleValue::Color(v),
            B(v) => VisualStyleValue::Bool(v),
            T(v) => VisualStyleValue::Text(v.into()),
        }
    }
}

fn visual_style((name, style_type, internal, overrides): VisualStyleRow) -> VisualStyle {
    let mut properties: Vec<(VisualStyleValue, i16)> = VISUAL_STYLE_DEFAULTS
        .iter()
        .map(|(p, f)| (p.clone().value(), *f))
        .collect();
    for (i, p, f) in overrides {
        properties[*i] = (p.clone().value(), *f);
    }
    VisualStyle {
        name: name.into(),
        style_type,
        // DXF 177 (the files store 2).
        unknown_bs: 2,
        internal,
        properties,
    }
}

/// The standard visual style named `name` (compared without case), as
/// AutoCAD's default template has it; for any other name, the values of
/// `2dWireframe` under that name.
pub(super) fn standard_visual_style(name: &str) -> VisualStyle {
    let row = VISUAL_STYLES
        .iter()
        .find(|r| r.0.eq_ignore_ascii_case(name))
        .or_else(|| VISUAL_STYLES.iter().find(|r| r.0 == "2dWireframe"))
        .expect("2dWireframe is a standard visual style");
    let mut style = visual_style(*row);
    style.name = name.into();
    style
}

/// Whether `name` is one of the standard visual styles (without case).
pub(super) fn is_standard_visual_style(name: &str) -> bool {
    VISUAL_STYLES.iter().any(|r| r.0.eq_ignore_ascii_case(name))
}

/// The values most standard visual styles share, by property position
/// (see [`crate::object::VISUAL_STYLE_KINDS`]), each with its DXF 176 value.
#[rustfmt::skip]
const VISUAL_STYLE_DEFAULTS: [(P, i16); 58] = [
    (I(2), 1), (I(2), 1), (I(1), 1), (I(0), 1),
    (D(0.6), 1), (D(30.0), 1), (C(Color::Rgb { r: 255, g: 255, b: 255 }), 1), (I(1), 1),
    (I(4), 1), (C(Color::Index(7)), 1), (C(Color::None), 1), (I(1), 1),
    (I(1), 1), (D(1.0), 1), (I(8), 1), (C(Color::Index(7)), 1),
    (D(1.0), 1), (I(1), 1), (I(6), 1), (I(2), 1),
    (C(Color::Index(7)), 1), (I(5), 1), (I(0), 1), (I(0), 1),
    (B(false), 1), (I(1), 1), (D(0.0), 1), (I(0), 1),
    (B(false), 1), (B(true), 1), (B(true), 1), (B(false), 1),
    (B(false), 1), (B(false), 1), (B(false), 1), (B(false), 1),
    (B(false), 1), (I(50), 1), (D(0.0), 1), (D(1.0), 1),
    (I(0), 1), (C(Color::Rgb { r: 0, g: 0, b: 0 }), 1), (I(50), 1), (I(3), 1),
    (C(Color::Rgb { r: 0, g: 0, b: 255 }), 1), (B(false), 1), (I(50), 1), (I(50), 1),
    (I(50), 1), (B(false), 1), (I(50), 1), (C(Color::ByLayer), 0),
    (D(1.0), 0), (I(2), 1), (T("strokes_ogs.tif"), 1), (B(false), 1),
    (D(1.0), 1), (D(1.0), 1),
];

/// A standard visual style: name, type (DXF 70), internal-use flag (DXF
/// 291), and the properties (position, value, DXF 176) where it departs
/// from [`VISUAL_STYLE_DEFAULTS`].
type VisualStyleRow = (&'static str, i32, bool, &'static [(usize, P, i16)]);

/// The standard visual styles: name, type (DXF 70), internal-use flag
/// (DXF 291) and the properties where each departs from
/// [`VISUAL_STYLE_DEFAULTS`], as AutoCAD's default template has them.
#[rustfmt::skip]
const VISUAL_STYLES: [VisualStyleRow; 24] = [
    ("Flat", 0, true, &[
        (1, I(1), 1), (3, I(2), 1), (7, I(0), 1),
        (8, I(0), 1), (25, I(13), 1),
    ]),
    ("FlatWithEdges", 1, true, &[
        (1, I(1), 1), (3, I(2), 1), (8, I(0), 1),
        (14, I(0), 1), (15, C(Color::None), 1), (25, I(13), 1),
    ]),
    ("Gouraud", 2, true, &[
        (3, I(2), 1), (7, I(0), 1), (8, I(0), 1),
        (14, I(0), 1), (25, I(13), 1),
    ]),
    ("GouraudWithEdges", 3, true, &[
        (3, I(2), 1), (8, I(0), 1), (14, I(0), 1),
        (15, C(Color::None), 1), (25, I(13), 1),
    ]),
    ("Basic", 7, true, &[
        (0, I(1), 1), (1, I(0), 1), (7, I(0), 1),
    ]),
    ("2dWireframe", 4, false, &[
        (0, I(0), 1), (14, I(0), 1), (15, C(Color::None), 1),
        (28, B(true), 1), (30, B(false), 1),
    ]),
    ("Wireframe", 5, false, &[
        (0, I(0), 1), (2, I(0), 1), (14, I(0), 1),
        (15, C(Color::None), 1), (21, I(3), 1),
    ]),
    ("Hidden", 6, false, &[
        (0, I(1), 1), (2, I(2), 1), (7, I(2), 1),
        (8, I(2), 1), (11, I(2), 1), (13, D(40.0), 1),
        (14, I(0), 1), (15, C(Color::None), 1), (21, I(3), 1),
    ]),
    ("Conceptual", 9, false, &[
        (0, I(3), 1), (7, I(2), 1), (8, I(2), 1),
        (13, D(40.0), 1), (21, I(3), 1),
    ]),
    ("Realistic", 8, false, &[
        (1, I(3), 1), (2, I(0), 1), (3, I(2), 1),
        (7, I(0), 1), (8, I(0), 1), (15, C(Color::None), 1),
        (21, I(3), 1), (25, I(13), 1),
    ]),
    ("Dim", 11, true, &[
        (26, D(-50.0), 1),
    ]),
    ("Brighten", 12, true, &[
        (26, D(50.0), 1),
    ]),
    ("Thicken", 13, true, &[
        (14, I(12), 1),
    ]),
    ("Linepattern", 14, true, &[
        (11, I(7), 1), (12, I(7), 1),
    ]),
    ("Facepattern", 15, true, &[
    ]),
    ("ColorChange", 16, true, &[
        (2, I(3), 1), (6, C(Color::Rgb { r: 128, g: 128, b: 128 }), 1), (15, C(Color::Rgb { r: 128, g: 128, b: 128 }), 1),
    ]),
    ("JitterOff", 20, true, &[
        (0, I(2), 0), (1, I(2), 0), (2, I(0), 0),
        (3, I(0), 0), (4, D(0.6), 0), (5, D(30.0), 0),
        (6, C(Color::Rgb { r: 255, g: 255, b: 255 }), 0), (7, I(1), 0), (8, I(4), 0),
        (9, C(Color::Index(7)), 0), (10, C(Color::None), 0), (11, I(1), 0),
        (12, I(1), 0), (13, D(1.0), 0), (14, I(10), 2),
        (15, C(Color::Index(7)), 0), (16, D(1.0), 0), (17, I(1), 0),
        (18, I(6), 0), (19, I(2), 0), (20, C(Color::Index(7)), 0),
        (21, I(5), 0), (22, I(0), 0), (23, I(0), 0),
        (24, B(false), 0), (25, I(1), 0), (26, D(0.0), 0),
        (27, I(0), 0),
    ]),
    ("OverhangOff", 21, true, &[
        (0, I(2), 0), (1, I(2), 0), (2, I(0), 0),
        (3, I(0), 0), (4, D(0.6), 0), (5, D(30.0), 0),
        (6, C(Color::Rgb { r: 255, g: 255, b: 255 }), 0), (7, I(1), 0), (8, I(4), 0),
        (9, C(Color::Index(7)), 0), (10, C(Color::None), 0), (11, I(1), 0),
        (12, I(1), 0), (13, D(1.0), 0), (14, I(9), 2),
        (15, C(Color::Index(7)), 0), (16, D(1.0), 0), (17, I(1), 0),
        (18, I(6), 0), (19, I(2), 0), (20, C(Color::Index(7)), 0),
        (21, I(5), 0), (22, I(0), 0), (23, I(0), 0),
        (24, B(false), 0), (25, I(1), 0), (26, D(0.0), 0),
        (27, I(0), 0),
    ]),
    ("EdgeColorOff", 22, true, &[
        (0, I(2), 0), (1, I(2), 0), (2, I(0), 0),
        (3, I(0), 0), (4, D(0.6), 0), (5, D(30.0), 0),
        (6, C(Color::Rgb { r: 255, g: 255, b: 255 }), 0), (7, I(1), 0), (8, I(4), 0),
        (9, C(Color::Index(7)), 0), (10, C(Color::None), 0), (11, I(1), 0),
        (12, I(1), 0), (13, D(1.0), 0), (14, I(8), 2),
        (15, C(Color::Index(7)), 0), (16, D(1.0), 0), (17, I(1), 0),
        (18, I(6), 0), (19, I(2), 0), (20, C(Color::Index(7)), 0),
        (21, I(5), 0), (22, I(0), 0), (23, I(0), 0),
        (24, B(false), 0), (25, I(1), 0), (26, D(0.0), 0),
        (27, I(0), 0),
    ]),
    ("Shades of Gray", 23, false, &[
        (2, I(3), 1), (7, I(2), 1), (8, I(2), 1),
        (10, C(Color::Index(7)), 1), (13, D(40.0), 1), (21, I(3), 1),
    ]),
    ("Sketchy", 24, false, &[
        (0, I(1), 1), (2, I(2), 1), (7, I(2), 1),
        (8, I(2), 1), (10, C(Color::Index(7)), 1), (13, D(40.0), 1),
        (14, I(11), 1), (21, I(6), 1),
    ]),
    ("X-Ray", 25, false, &[
        (3, I(1), 1), (4, D(0.5), 1), (8, I(0), 1),
        (21, I(3), 1), (25, I(13), 1),
    ]),
    ("Shaded with edges", 26, false, &[
        (3, I(2), 1), (8, I(2), 1), (11, I(2), 1),
        (15, C(Color::None), 1), (21, I(3), 1), (25, I(5), 1),
    ]),
    ("Shaded", 27, false, &[
        (3, I(2), 1), (7, I(0), 1), (15, C(Color::None), 1),
        (20, C(Color::Rgb { r: 120, g: 120, b: 120 }), 1), (21, I(3), 1), (25, I(5), 1),
    ]),
];

/// The `Imperial24` model documentation view style part: no flags, the
/// description is the name.
fn view_style_base() -> ModelDocViewStyle {
    ModelDocViewStyle {
        unknown_70: 0,
        name: IMPERIAL24.into(),
        unknown_290: false,
        description: IMPERIAL24.into(),
        unknown_90: 0,
    }
}

/// The name of `acad.dwt`'s section and detail view styles (24-unit text
/// heights, imperial).
const IMPERIAL24: &str = "Imperial24";

/// The field text of the view labels: the view type, the identifiers and
/// the scale.
const SECTION_LABEL: &str = "%<\\AcVar ViewType \\f \"%tc1\">% %<\\AcVar ViewSectionStartId>%-%<\\AcVar ViewSectionEndId>%\\PSCALE %<\\AcVar ViewScale \\f \"%sn\">%";
const DETAIL_LABEL: &str = "%<\\AcVar ViewType \\f \"%tc1\">% %<\\AcVar ViewDetailId>%\\PSCALE %<\\AcVar ViewScale \\f \"%sn\">%";

/// The `Imperial24` section view style: text in `text_style`, lines in
/// `continuous`, sizes for 0.24-unit text, ANSI31 hatching at six angles.
fn section_view_style(text_style: Handle, continuous: Handle) -> SectionViewStyle {
    SectionViewStyle {
        base: view_style_base(),
        unknown_70: 0,
        flags: 78,
        identifier_style: text_style,
        identifier_color: Color::ByLayer,
        identifier_height: 0.24,
        arrow_start: Handle(0),
        arrow_end: Handle(0),
        arrow_color: Color::ByLayer,
        arrow_size: 0.24,
        identifier_exclude: "I, O, Q, S, X, Z".into(),
        identifier_offset: 0.48,
        plane_linetype: continuous,
        plane_lineweight: 25,
        plane_color: Color::ByLayer,
        bend_linetype: continuous,
        bend_lineweight: 50,
        bend_color: Color::ByLayer,
        bend_length: 0.24,
        end_length: 0.24,
        label_style: text_style,
        label_color: Color::ByLayer,
        label_height: 0.24,
        label_attachment: 1,
        label_offset: 0.75,
        label_alignment: 1,
        label_text: SECTION_LABEL.into(),
        hatch_color: Color::ByLayer,
        hatch_background: Color::None,
        hatch_pattern: "ANSI31".into(),
        hatch_scale: 1.0,
        hatch_transparency: 0,
        hatch_flags: [false, false],
        identifier_position: 3,
        identifier_extension: 0.18,
        identifier_unknown_90: 1,
        end_overshoot: 0.0,
        hatch_angles: vec![
            0.0,
            FRAC_PI_2,
            PI / 12.0,
            5.0 * PI / 12.0,
            -PI / 12.0,
            // 7 pi / 12, as AutoCAD stores it (one unit in the last place
            // above the nearest double).
            1.8325957145940461,
        ],
    }
}

/// The `Imperial24` detail view style, like [`section_view_style`].
fn detail_view_style(text_style: Handle, continuous: Handle) -> DetailViewStyle {
    DetailViewStyle {
        base: view_style_base(),
        unknown_70: 0,
        flags: 3,
        identifier_style: text_style,
        identifier_color: Color::ByLayer,
        identifier_height: 0.24,
        identifier_offset: 0.36,
        identifier_placement: 3,
        identifier_exclude: String::new(),
        arrow: Handle(0),
        arrow_color: Color::ByLayer,
        arrow_size: 0.24,
        boundary_linetype: continuous,
        boundary_lineweight: 25,
        boundary_color: Color::ByLayer,
        label_style: text_style,
        label_color: Color::ByLayer,
        label_height: 0.24,
        label_attachment: 1,
        label_offset: 0.75,
        label_alignment: 1,
        label_text: DETAIL_LABEL.into(),
        connection_linetype: continuous,
        connection_lineweight: 25,
        connection_color: Color::ByLayer,
        border_linetype: continuous,
        border_lineweight: 25,
        border_color: Color::ByLayer,
        model_edge: 0,
    }
}

/// A material map tiling record (`BUMPTILE`, ...): DXF 270 and 271 both 1.
fn tile_record() -> NonGraphical {
    NonGraphical::XRecord(XRecord {
        items: vec![(270, XRecordValue::Int16(1)), (271, XRecordValue::Int16(1))],
        cloning: 1,
        object_ids: Vec::new(),
        handle_tail: Default::default(),
        read_version: AsRead::default(),
    })
}

fn variable(value: &str) -> NonGraphical {
    NonGraphical::DictionaryVar(DictionaryVar {
        schema: 0,
        value: value.into(),
    })
}

/// The new drawing's objects (see the module documentation), handles from
/// 1 in creation order.
fn build(h: &HeaderVars) -> BTreeMap<Handle, Object> {
    let mut b = Builder {
        objects: BTreeMap::new(),
        next: 0,
    };
    let null = Handle(0);
    // The table controls, in AutoCAD's order.
    let [
        blocks,
        layers,
        styles,
        linetypes,
        _views,
        _ucs,
        vports,
        apps,
        dimstyles,
    ] = [
        TableKind::Block,
        TableKind::Layer,
        TableKind::Style,
        TableKind::Linetype,
        TableKind::View,
        TableKind::Ucs,
        TableKind::Vport,
        TableKind::AppId,
        TableKind::DimStyle,
    ]
    .map(|kind| b.add(null, Vec::new(), control(kind)));

    // The named object dictionary and its dictionaries.
    let nod = b.add(null, Vec::new(), NonGraphical::Dictionary(dictionary(0)));
    b.dictionary(nod, "ACAD_GROUP");
    let plot_styles = b.entry(
        nod,
        "ACAD_PLOTSTYLENAME",
        NonGraphical::DictionaryWithDefault(DictionaryWithDefault {
            dictionary: dictionary(0),
            default_entry: null,
        }),
    );
    let normal = b.entry(plot_styles, "Normal", NonGraphical::Placeholder);
    if let NonGraphical::DictionaryWithDefault(d) = b.other(plot_styles) {
        d.default_entry = normal;
    }
    let mline_styles = b.dictionary(nod, "ACAD_MLINESTYLE");
    b.dictionary(nod, "ACAD_PLOTSETTINGS");
    let layouts = b.dictionary(nod, "ACAD_LAYOUT");
    let variables = b.dictionary(nod, "AcDbVariableDictionary");
    let materials = b.dictionary(nod, "ACAD_MATERIAL");
    b.dictionary(nod, "ACAD_COLOR");
    let table_styles = b.dictionary(nod, "ACAD_TABLESTYLE");
    let visual_styles = b.dictionary(nod, "ACAD_VISUALSTYLE");
    let scales = b.dictionary(nod, "ACAD_SCALELIST");
    let mleader_styles = b.dictionary(nod, "ACAD_MLEADERSTYLE");
    let section_styles = b.dictionary(nod, "ACAD_SECTIONVIEWSTYLE");
    let detail_styles = b.dictionary(nod, "ACAD_DETAILVIEWSTYLE");

    // Applications, linetypes, materials, layer 0, text styles.
    let acad = b.record(apps, app_id("ACAD"));
    b.record(apps, app_id("AcadAnnoPO"));
    let annotative = b.record(apps, app_id("AcadAnnotative"));
    let dimjag = b.record(apps, app_id("ACAD_DSTYLE_DIMJAG"));
    let dimtaln = b.record(apps, app_id("ACAD_DSTYLE_DIMTALN"));
    b.record(apps, app_id("ACAD_MLEADERVER"));
    let vc_display = b.record(apps, app_id("ACAD_NAV_VCDISPLAY"));
    let byblock = b.add(linetypes, Vec::new(), linetype("ByBlock", ""));
    let bylayer = b.add(linetypes, Vec::new(), linetype("ByLayer", ""));
    if let NonGraphical::TableControl(c) = b.other(linetypes) {
        c.extra = vec![byblock, bylayer];
    }
    let continuous = b.record(linetypes, linetype("Continuous", "Solid line"));
    let global_scale = f64::from(GLOBAL_MAP_SCALE);
    for name in ["ByLayer", "ByBlock"] {
        b.entry(
            materials,
            name,
            NonGraphical::Material(Box::new(material(name, 1.0))),
        );
    }
    let global = b.entry(
        materials,
        "Global",
        NonGraphical::Material(Box::new(material("Global", global_scale))),
    );
    // AutoCAD's ACAD extended data on the Global material (its legacy
    // material settings: -1, 3, 0, "", 0, 0).
    b.set_xdata(
        global,
        vec![xdata(
            acad,
            vec![
                XDataItem::Short(-1),
                XDataItem::Short(3),
                XDataItem::Short(0),
                XDataItem::String(String::new()),
                XDataItem::Long(0),
                XDataItem::Short(0),
            ],
        )],
    );
    let tiles = b.xdict(global);
    for name in ["BUMPTILE", "DIFFUSETILE", "OPACITYTILE", "REFLECTIONTILE"] {
        b.entry(tiles, name, tile_record());
    }
    let layer0 = b.record(
        layers,
        NonGraphical::Layer(Layer {
            entry: entry("0"),
            frozen: false,
            off: false,
            frozen_in_new_viewports: false,
            locked: false,
            plot: true,
            // 31: the default lineweight.
            lineweight: 31,
            color: Color::Index(7),
            plotstyle: normal,
            material: global,
            linetype: continuous,
            unknown_handle: null,
        }),
    );
    b.xdict(layer0);
    let layer_states = b.xdict(layers);
    b.dictionary(layer_states, "ACAD_LAYERSTATES");
    let standard = b.record(styles, text_style("Standard"));
    b.set_xdata(standard, vec![font_xdata(acad)]);
    let annotative_style = b.record(styles, text_style("Annotative"));
    b.set_xdata(
        annotative_style,
        vec![annotative_xdata(annotative), font_xdata(acad)],
    );

    // Dimension styles: Standard and its annotative twin (scale 0: the
    // annotation scale applies), with the jog height and text alignment
    // AutoCAD records for it in extended data.
    let dim_standard = b.record(
        dimstyles,
        NonGraphical::DimStyle(Box::new(dim_style("Standard", h, standard))),
    );
    let mut anno = dim_style("Annotative", h, standard);
    anno.dimscale = 0.0;
    let dim_annotative = b.record(dimstyles, NonGraphical::DimStyle(Box::new(anno)));
    b.set_xdata(
        dim_annotative,
        vec![
            annotative_xdata(annotative),
            // DIMJAG (group 388): 1.5 text heights; DIMTALN (392): off.
            xdata(dimjag, vec![XDataItem::Short(388), XDataItem::Real(1.5)]),
            xdata(dimtaln, vec![XDataItem::Short(392), XDataItem::Short(0)]),
        ],
    );
    if let NonGraphical::TableControl(c) = b.other(dimstyles) {
        c.extra = vec![dim_standard, dim_annotative];
    }

    // Multiline, table and multileader styles.
    b.entry(
        mline_styles,
        "Standard",
        NonGraphical::MLineStyle(mline_style(bylayer)),
    );
    let table = b.entry(
        table_styles,
        "Standard",
        NonGraphical::TableStyle(table_style(standard)),
    );
    b.xdict(table);
    for (name, annotative) in [("Standard", false), ("Annotative", true)] {
        let style = mleader_style(standard, byblock, annotative);
        b.entry(
            mleader_styles,
            name,
            NonGraphical::MLeaderStyle(Box::new(style)),
        );
    }

    // Visual styles, scales, view styles.
    let mut by_name = BTreeMap::new();
    for row in VISUAL_STYLES {
        let h = b.entry(
            visual_styles,
            row.0,
            NonGraphical::VisualStyle(Box::new(visual_style(row))),
        );
        by_name.insert(row.0, h);
    }
    for (n, &(name, paper_units, drawing_units)) in SCALES.iter().enumerate() {
        let scale = Scale {
            unknown: 0,
            name: name.into(),
            paper_units,
            drawing_units,
            unit_scale: n == 0,
        };
        b.entry(scales, &scale_key(n), NonGraphical::Scale(scale));
    }
    b.entry(
        section_styles,
        IMPERIAL24,
        NonGraphical::SectionViewStyle(Box::new(section_view_style(standard, continuous))),
    );
    b.entry(
        detail_styles,
        IMPERIAL24,
        NonGraphical::DetailViewStyle(Box::new(detail_view_style(standard, continuous))),
    );

    // The drawing's variables: associative dimensions (2), hidden text (1),
    // the current table, multileader and view styles, the annotation scale
    // 1:1, layer evaluation and notification off.
    for (name, value) in [
        ("DIMASSOC", "2"),
        ("HIDETEXT", "1"),
        ("CTABLESTYLE", "STANDARD"),
        ("CMLEADERSTYLE", "STANDARD"),
        ("CANNOSCALE", "1:1"),
        ("LAYEREVAL", "0"),
        ("LAYERNOTIFY", "0"),
        ("CVIEWDETAILSTYLE", IMPERIAL24),
        ("CVIEWSECTIONSTYLE", IMPERIAL24),
    ] {
        b.entry(variables, name, variable(value));
    }

    // The active viewport (2D wireframe; the ACAD_NAV_VCDISPLAY extended
    // data shows the navigation bar and view cube: 3).
    let wireframe = by_name["2dWireframe"];
    let active = b.record(
        vports,
        NonGraphical::Vport(Box::new(active_vport(h, wireframe))),
    );
    b.set_xdata(active, vec![xdata(vc_display, vec![XDataItem::Short(3)])]);

    // Model and paper space, and their layouts.
    let model_space = b.block(blocks, "*Model_Space", 2, layer0);
    b.xdict(model_space);
    let paper_space = b.block(blocks, "*Paper_Space", 1, layer0);
    // AutoCAD lists them as the block control's extra entries only.
    if let NonGraphical::TableControl(c) = b.other(blocks) {
        c.entries.clear();
        c.extra = vec![model_space, paper_space];
    }
    let mut model = layout("Model", 0, h, model_space);
    model.active_viewport = active;
    let model = b.entry(layouts, "Model", NonGraphical::Layout(Box::new(model)));
    b.xdict(model);
    let layout1 = b.entry(
        layouts,
        "Layout1",
        NonGraphical::Layout(Box::new(layout("Layout1", 1, h, paper_space))),
    );
    for (block, layout) in [(model_space, model), (paper_space, layout1)] {
        if let NonGraphical::BlockRecord(r) = b.other(block) {
            r.layout = layout;
        }
    }

    // Dictionary entries sorted by name, as AutoCAD keeps them.
    for obj in b.objects.values_mut() {
        match obj {
            Object::Other {
                object: NonGraphical::Dictionary(d),
                ..
            } => d.entries.sort_by(|x, y| x.0.cmp(&y.0)),
            Object::Other {
                object: NonGraphical::DictionaryWithDefault(d),
                ..
            } => d.dictionary.entries.sort_by(|x, y| x.0.cmp(&y.0)),
            _ => {}
        }
    }
    b.objects
}

/// Point the header's handle variables at the objects they name (see the
/// module documentation). A name the template lacks leaves its variable
/// null (`tests/from_scratch.rs` checks none does).
fn name_objects(h: &mut HeaderVars, objects: &BTreeMap<Handle, Object>) {
    let control = |kind: TableKind| {
        objects
            .iter()
            .find_map(|(&x, o)| match o {
                Object::Other {
                    object: NonGraphical::TableControl(c),
                    ..
                } if c.kind == kind => Some(x),
                _ => None,
            })
            .unwrap_or_default()
    };
    let record = |kind: TableKind, name: &str| {
        let c = control(kind);
        objects
            .iter()
            .find_map(|(&x, o)| match o {
                Object::Other { common, object }
                    if common.owner == c
                        && object.table_entry().is_some_and(|e| e.name == name) =>
                {
                    Some(x)
                }
                _ => None,
            })
            .unwrap_or_default()
    };
    // The root dictionary is the one no object owns.
    let root = objects
        .iter()
        .find_map(|(&x, o)| match o {
            Object::Other {
                common,
                object: NonGraphical::Dictionary(_),
            } if common.owner.0 == 0 => Some(x),
            _ => None,
        })
        .unwrap_or_default();
    let entry = |dict: Handle, name: &str| {
        match objects.get(&dict) {
            Some(Object::Other {
                object: NonGraphical::Dictionary(d),
                ..
            }) => d.entries.iter().find(|(n, _)| n == name).map(|&(_, x)| x),
            _ => None,
        }
        .unwrap_or_default()
    };
    let named = |name: &str| entry(root, name);

    h.block_control = control(TableKind::Block);
    h.layer_control = control(TableKind::Layer);
    h.style_control = control(TableKind::Style);
    h.linetype_control = control(TableKind::Linetype);
    h.view_control = control(TableKind::View);
    h.ucs_control = control(TableKind::Ucs);
    h.vport_control = control(TableKind::Vport);
    h.appid_control = control(TableKind::AppId);
    h.dimstyle_control = control(TableKind::DimStyle);
    h.dict_named_objects = root;
    h.dict_acad_group = named("ACAD_GROUP");
    h.dict_acad_mlinestyle = named("ACAD_MLINESTYLE");
    h.dict_layouts = named("ACAD_LAYOUT");
    h.dict_plotsettings = named("ACAD_PLOTSETTINGS");
    h.dict_plotstyles = named("ACAD_PLOTSTYLENAME");
    h.dict_materials = Some(named("ACAD_MATERIAL"));
    h.dict_colors = Some(named("ACAD_COLOR"));
    h.dict_visualstyle = Some(named("ACAD_VISUALSTYLE"));
    h.clayer = record(TableKind::Layer, "0");
    h.textstyle = record(TableKind::Style, "Standard");
    h.dimtxsty = h.textstyle;
    h.dimstyle = record(TableKind::DimStyle, "Standard");
    h.ltype_byblock = record(TableKind::Linetype, "ByBlock");
    h.ltype_bylayer = record(TableKind::Linetype, "ByLayer");
    h.ltype_continuous = record(TableKind::Linetype, "Continuous");
    h.celtype = h.ltype_bylayer;
    h.cmlstyle = entry(h.dict_acad_mlinestyle, "Standard");
    h.cmaterial = Some(entry(named("ACAD_MATERIAL"), "ByLayer"));
    h.block_record_model_space = record(TableKind::Block, "*Model_Space");
    h.block_record_paper_space = record(TableKind::Block, "*Paper_Space");
    let styles = named("ACAD_VISUALSTYLE");
    h.interfereobjvs = Some(entry(styles, "Realistic"));
    h.interferevpvs = Some(entry(styles, "Wireframe"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_are_known() {
        let classes = template_classes();
        assert_eq!(classes.len(), 17);
        assert!(classes.iter().all(|c| !c.is_entity()));
        assert_eq!(classes[16].class_number, 516);
    }
}
