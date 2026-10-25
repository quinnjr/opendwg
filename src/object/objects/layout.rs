//! Layouts and plot settings (ODS 20.4.84).

use crate::error::Result;
use crate::handle::Handle;
use crate::object::entities::dim::write_count;
use crate::object::stream::{ObjReader, ObjWriter};
use crate::object::tables::{HARD_POINTER, SOFT_POINTER, handle_count, put};
use crate::version::Version;

/// Plot settings (ODS 20.4.84, the PlotSettings part of a LAYOUT; the
/// PLOTSETTINGS object stores the same).
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct PlotSettings {
    /// DXF 1.
    pub page_setup_name: String,
    /// DXF 2: the printer or configuration file.
    pub printer: String,
    /// DXF 70.
    pub flags: i16,
    /// DXF 40-43, millimetres: left, bottom, right, top.
    pub margins: [f64; 4],
    /// DXF 44, 45, millimetres.
    pub paper_width: f64,
    pub paper_height: f64,
    /// DXF 4.
    pub paper_size: String,
    /// DXF 46, 47, millimetres.
    pub plot_origin: [f64; 2],
    /// DXF 72.
    pub paper_units: i16,
    /// DXF 73.
    pub rotation: i16,
    /// DXF 74.
    pub plot_type: i16,
    /// DXF 48, 49 and 140, 141.
    pub window_min: [f64; 2],
    pub window_max: [f64; 2],
    /// R2000 only (DXF 6): the plot view by name. From R2004 the view is
    /// [`PlotSettings::plot_view`]; reading an R2000 drawing sets it to the
    /// VIEW of this name, and saving resolves it again. A name no view has
    /// when saving is cleared (the plot view is then null), with a warning.
    pub plot_view_name: String,
    /// DXF 142, 143: the custom print scale (paper units, drawing units).
    pub custom_scale: [f64; 2],
    /// DXF 7.
    pub style_sheet: String,
    /// DXF 75.
    pub scale_type: i16,
    /// DXF 147.
    pub scale_factor: f64,
    /// DXF 148, 149.
    pub paper_image_origin: [f64; 2],
    /// R2004+ DXF 76, 77, 78; before R2004 0, 2 and 300 (what AutoCAD writes
    /// for the corpus's R2000 layouts saved as R2004).
    pub shade_plot_mode: i16,
    pub shade_plot_resolution: i16,
    pub shade_plot_dpi: i16,
    /// R2004+: the plot view (hard pointer); null before.
    pub plot_view: Handle,
    /// R2007+: the visual style (soft pointer); null before.
    pub visual_style: Handle,
}

impl PlotSettings {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let v = r.version;
        let page_setup_name = r.text()?;
        let printer = r.text()?;
        let flags = r.data.read_bs()?;
        let mut margins = [0.0; 4];
        for m in &mut margins {
            *m = r.data.read_bd()?;
        }
        let paper_width = r.data.read_bd()?;
        let paper_height = r.data.read_bd()?;
        let paper_size = r.text()?;
        let plot_origin = [r.data.read_bd()?, r.data.read_bd()?];
        let paper_units = r.data.read_bs()?;
        let rotation = r.data.read_bs()?;
        let plot_type = r.data.read_bs()?;
        let window_min = [r.data.read_bd()?, r.data.read_bd()?];
        let window_max = [r.data.read_bd()?, r.data.read_bd()?];
        let plot_view_name = if v < Version::R2004 {
            r.text()?
        } else {
            String::new()
        };
        let custom_scale = [r.data.read_bd()?, r.data.read_bd()?];
        let style_sheet = r.text()?;
        let scale_type = r.data.read_bs()?;
        let scale_factor = r.data.read_bd()?;
        let paper_image_origin = [r.data.read_bd()?, r.data.read_bd()?];
        let (shade_plot_mode, shade_plot_resolution, shade_plot_dpi, plot_view) =
            if v >= Version::R2004 {
                (
                    r.data.read_bs()?,
                    r.data.read_bs()?,
                    r.data.read_bs()?,
                    r.href_abs()?,
                )
            } else {
                (0, 2, 300, Handle(0))
            };
        let visual_style = if v >= Version::R2007 {
            r.href_abs()?
        } else {
            Handle(0)
        };
        Ok(PlotSettings {
            page_setup_name,
            printer,
            flags,
            margins,
            paper_width,
            paper_height,
            paper_size,
            plot_origin,
            paper_units,
            rotation,
            plot_type,
            window_min,
            window_max,
            plot_view_name,
            custom_scale,
            style_sheet,
            scale_type,
            scale_factor,
            paper_image_origin,
            shade_plot_mode,
            shade_plot_resolution,
            shade_plot_dpi,
            plot_view,
            visual_style,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        w.text(&self.page_setup_name)?;
        w.text(&self.printer)?;
        w.data.write_bs(self.flags);
        for m in self.margins {
            w.data.write_bd(m);
        }
        w.data.write_bd(self.paper_width);
        w.data.write_bd(self.paper_height);
        w.text(&self.paper_size)?;
        for c in self.plot_origin {
            w.data.write_bd(c);
        }
        w.data.write_bs(self.paper_units);
        w.data.write_bs(self.rotation);
        w.data.write_bs(self.plot_type);
        for c in self.window_min.into_iter().chain(self.window_max) {
            w.data.write_bd(c);
        }
        for c in self.custom_scale {
            w.data.write_bd(c);
        }
        w.text(&self.style_sheet)?;
        w.data.write_bs(self.scale_type);
        w.data.write_bd(self.scale_factor);
        for c in self.paper_image_origin {
            w.data.write_bd(c);
        }
        w.data.write_bs(self.shade_plot_mode);
        w.data.write_bs(self.shade_plot_resolution);
        w.data.write_bs(self.shade_plot_dpi);
        put(w, HARD_POINTER, self.plot_view)?;
        put(w, SOFT_POINTER, self.visual_style)
    }
}

/// A layout (ODS 20.4.84): a model or paper space tab with its plot
/// settings.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct Layout {
    pub plot: PlotSettings,
    /// DXF 1.
    pub name: String,
    /// DXF 71.
    pub tab_order: i32,
    /// DXF 70.
    pub flags: i16,
    /// DXF 13.
    pub ucs_origin: [f64; 3],
    /// DXF 10, 11: the layout's limits. Like the extents, a copy the
    /// layout keeps of the header's `$LIMMIN`/`$LIMMAX` (`$PLIMMIN`/
    /// `$PLIMMAX` for the active paper space layout); kept as read, not
    /// synchronised with the header.
    pub limits_min: [f64; 2],
    pub limits_max: [f64; 2],
    /// DXF 12.
    pub insertion_base: [f64; 3],
    /// DXF 16, 17.
    pub ucs_x_axis: [f64; 3],
    pub ucs_y_axis: [f64; 3],
    /// DXF 146.
    pub elevation: f64,
    /// DXF 76.
    pub ortho_type: i16,
    /// DXF 14, 15: the extents, a geometry cache (like the header's
    /// `$EXTMIN`/`$EXTMAX`, `$PEXTMIN`/`$PEXTMAX`) that AutoCAD recomputes;
    /// kept as read.
    pub extents_min: [f64; 3],
    pub extents_max: [f64; 3],
    /// DXF 330: the layout's block record (soft pointer).
    pub block_record: Handle,
    /// DXF 331: the last active viewport (soft pointer).
    pub active_viewport: Handle,
    /// DXF 346: the base UCS (hard pointer).
    pub base_ucs: Handle,
    /// DXF 345: the named UCS (hard pointer).
    pub named_ucs: Handle,
    /// R2004+: the layout's viewports (soft pointers), a cached list that
    /// AutoCAD keeps of the VIEWPORT entities in the layout's block and may
    /// include viewports owned elsewhere; kept as read. R2000 files store
    /// none; the drawing fills it from the layout's block on read. Adding or
    /// removing a VIEWPORT does not update it here: the drawing refreshes
    /// it at save (from `drawing::block_viewports`).
    pub viewports: Vec<Handle>,
}

impl Layout {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let plot = PlotSettings::decode(r)?;
        let name = r.text()?;
        let tab_order = r.data.read_bl()?;
        let flags = r.data.read_bs()?;
        let ucs_origin = r.data.read_3bd()?;
        let limits_min = r.data.read_2rd()?;
        let limits_max = r.data.read_2rd()?;
        let insertion_base = r.data.read_3bd()?;
        let ucs_x_axis = r.data.read_3bd()?;
        let ucs_y_axis = r.data.read_3bd()?;
        let elevation = r.data.read_bd()?;
        let ortho_type = r.data.read_bs()?;
        let extents_min = r.data.read_3bd()?;
        let extents_max = r.data.read_3bd()?;
        let viewport_count = if r.version >= Version::R2004 {
            Some(r.data.read_bl()?)
        } else {
            None
        };
        let block_record = r.href_abs()?;
        let active_viewport = r.href_abs()?;
        let base_ucs = r.href_abs()?;
        let named_ucs = r.href_abs()?;
        let viewports = match viewport_count {
            Some(n) => {
                let n = handle_count(r, n, "layout viewport")?;
                (0..n).map(|_| r.href_abs()).collect::<Result<_>>()?
            }
            None => Vec::new(),
        };
        Ok(Layout {
            plot,
            name,
            tab_order,
            flags,
            ucs_origin,
            limits_min,
            limits_max,
            insertion_base,
            ucs_x_axis,
            ucs_y_axis,
            elevation,
            ortho_type,
            extents_min,
            extents_max,
            block_record,
            active_viewport,
            base_ucs,
            named_ucs,
            viewports,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        self.plot.encode(w)?;
        w.text(&self.name)?;
        w.data.write_bl(self.tab_order);
        w.data.write_bs(self.flags);
        w.data.write_3bd(self.ucs_origin);
        w.data.write_2rd(self.limits_min);
        w.data.write_2rd(self.limits_max);
        w.data.write_3bd(self.insertion_base);
        w.data.write_3bd(self.ucs_x_axis);
        w.data.write_3bd(self.ucs_y_axis);
        w.data.write_bd(self.elevation);
        w.data.write_bs(self.ortho_type);
        w.data.write_3bd(self.extents_min);
        w.data.write_3bd(self.extents_max);
        write_count(w, self.viewports.len(), "layout viewports")?;
        put(w, SOFT_POINTER, self.block_record)?;
        put(w, SOFT_POINTER, self.active_viewport)?;
        put(w, HARD_POINTER, self.base_ucs)?;
        put(w, HARD_POINTER, self.named_ucs)?;
        for &h in &self.viewports {
            put(w, SOFT_POINTER, h)?;
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::bits::BitWriter;

    use crate::error::Error;
    use crate::handle::{Handle, HandleRef};
    use crate::object::entities::tests::{reader, roundtrip};
    use crate::object::objects::test_util::H;

    use crate::object::stream::tests::assemble;
    use crate::object::stream::{ObjReader, ObjWriter};

    use crate::version::Version;

    pub(crate) fn plot_settings() -> PlotSettings {
        PlotSettings {
            page_setup_name: "Setup 1".into(),
            printer: "DWF6 ePlot.pc3".into(),
            flags: 688,
            margins: [6.35, 19.05, 6.35, 19.0],
            paper_width: 215.9,
            paper_height: 279.4,
            paper_size: "Letter_(8.50_x_11.00_Inches)".into(),
            plot_origin: [1.0, 2.0],
            paper_units: 1,
            rotation: 1,
            plot_type: 5,
            window_min: [-1.0, -2.0],
            window_max: [3.0, 4.0],
            plot_view_name: String::new(),
            custom_scale: [1.0, 2.5],
            style_sheet: "monochrome.ctb".into(),
            scale_type: 16,
            scale_factor: 0.25,
            paper_image_origin: [0.5, 0.5],
            shade_plot_mode: 3,
            shade_plot_resolution: 2,
            shade_plot_dpi: 200,
            plot_view: Handle(0x61),
            visual_style: Handle(0x9A),
        }
    }

    pub(crate) fn layout() -> Layout {
        Layout {
            plot: plot_settings(),
            name: "Layout1 ü".into(),
            tab_order: 1,
            flags: 1,
            ucs_origin: [0.0, 1.0, 2.0],
            limits_min: [-0.25, -0.25],
            limits_max: [10.75, 8.25],
            insertion_base: [1.0; 3],
            ucs_x_axis: [1.0, 0.0, 0.0],
            ucs_y_axis: [0.0, 1.0, 0.0],
            elevation: 0.5,
            ortho_type: 1,
            extents_min: [1.05, 0.8, 0.0],
            extents_max: [9.45, 7.2, 0.0],
            block_record: Handle(0x58),
            active_viewport: Handle(0x240),
            base_ucs: Handle(0),
            named_ucs: Handle(0x77),
            viewports: vec![Handle(0x240), Handle(0x245)],
        }
    }

    #[test]
    fn roundtrip_layout_and_plot_settings() {
        roundtrip(&layout(), Layout::encode, Layout::decode, 82);
        roundtrip(
            &plot_settings(),
            PlotSettings::encode,
            PlotSettings::decode,
            500,
        );
    }

    /// An R2000 LAYOUT, written field by field: a plot view name, no shade
    /// plot fields, plot view, visual style or viewport list.
    fn r2000_layout(view_name: &str) -> Vec<u8> {
        let mut d = BitWriter::new();
        d.write_tv_raw(b"").unwrap();
        d.write_tv_raw(b"none_device").unwrap();
        d.write_bs(688);
        for m in [6.35, 6.35, 6.35, 6.35, 215.9, 279.4] {
            d.write_bd(m);
        }
        d.write_tv_raw(b"Letter").unwrap();
        for _ in 0..2 {
            d.write_bd(0.0);
        }
        d.write_bs(0);
        d.write_bs(1);
        d.write_bs(5);
        for _ in 0..4 {
            d.write_bd(0.0);
        }
        d.write_tv_raw(view_name.as_bytes()).unwrap();
        d.write_bd(1.0);
        d.write_bd(1.0);
        d.write_tv_raw(b"").unwrap();
        d.write_bs(16);
        d.write_bd(1.0);
        d.write_bd(0.0);
        d.write_bd(0.0);
        // The layout part.
        d.write_tv_raw(b"Layout1").unwrap();
        d.write_bl(1);
        d.write_bs(1);
        d.write_3bd([0.0; 3]);
        d.write_2rd([0.0; 2]);
        d.write_2rd([12.0, 9.0]);
        d.write_3bd([0.0; 3]);
        d.write_3bd([1.0, 0.0, 0.0]);
        d.write_3bd([0.0, 1.0, 0.0]);
        d.write_bd(0.0);
        d.write_bs(0);
        d.write_3bd([0.0; 3]);
        d.write_3bd([0.0; 3]);
        let mut h = BitWriter::new();
        for (code, value) in [(4, 0x58), (4, 0x240), (5, 0), (5, 0)] {
            h.write_handle_ref(HandleRef { code, value }).unwrap();
        }
        assemble(Version::R2000, 502, &d, None, &h)
    }

    /// R2000 layouts have no shade plot fields (the R2004 defaults are
    /// taken) and name their plot view; R2018 needs the view's handle.
    #[test]
    fn r2000_layout_converts() {
        let bytes = r2000_layout("");
        let (mut r, _) = ObjReader::split(&bytes, Version::R2000, 30).unwrap();
        let l = Layout::decode(&mut r).unwrap();
        assert_eq!(r.data.remaining_bits(), 0);
        assert_eq!(
            (
                l.plot.shade_plot_mode,
                l.plot.shade_plot_resolution,
                l.plot.shade_plot_dpi
            ),
            (0, 2, 300)
        );
        assert_eq!(
            (l.block_record, l.active_viewport),
            (Handle(0x58), Handle(0x240))
        );
        assert!(l.viewports.is_empty());

        let bytes = r2000_layout("MyView");
        let (mut r, _) = ObjReader::split(&bytes, Version::R2000, 30).unwrap();
        let mut l = Layout::decode(&mut r).unwrap();
        assert_eq!(l.plot.plot_view_name, "MyView");
        assert_eq!(l.plot.plot_view, Handle(0));
        // R2018 has no name: the view's handle is written.
        l.plot.plot_view = Handle(0x61);
        let mut w = ObjWriter::new(H);
        l.encode(&mut w).unwrap();
        let out = w.finish(82).unwrap();
        let back = Layout::decode(&mut reader(&out)).unwrap();
        assert_eq!(back.plot.plot_view, Handle(0x61));
        assert_eq!(back.plot.plot_view_name, "");
    }

    /// The viewport count is checked against the handle stream before
    /// anything is allocated.
    #[test]
    fn layout_counts_are_bounded() {
        let mut l = layout();
        l.viewports = vec![Handle(1); 40];
        let mut w = ObjWriter::new(H);
        l.encode(&mut w).unwrap();
        let bytes = w.finish(82).unwrap();
        let mut r = reader(&bytes);
        // Room for the 6 other references and 24 more bytes.
        r.handles.limit_remaining(8 * 30);
        assert!(matches!(
            Layout::decode(&mut r),
            Err(Error::Corrupt(m)) if m.contains("layout viewport count 40")
        ));
    }
}
