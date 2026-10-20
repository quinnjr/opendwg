//! Image definitions and their reactors (ODS 20.4.81, 20.4.82), raster
//! variables (20.4.91) and wipeout variables.

use crate::error::Result;
use crate::object::stream::{ObjReader, ObjWriter};

/// An image definition (ODS 20.4.81, IMAGEDEF).
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ImageDef {
    /// DXF 90.
    pub class_version: i32,
    /// DXF 10: the size in pixels. Like `pixel_size`, a cache of the
    /// external image file's properties, kept as read (the file is not
    /// read here).
    pub size: [f64; 2],
    /// DXF 1.
    pub path: String,
    /// DXF 280.
    pub loaded: bool,
    /// DXF 281: 0 none, 2 centimetres, 5 inches.
    pub resolution_units: u8,
    /// DXF 11: the size of a pixel in drawing units.
    pub pixel_size: [f64; 2],
}

impl ImageDef {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        Ok(ImageDef {
            class_version: r.data.read_bl()?,
            size: r.data.read_2rd()?,
            path: r.text()?,
            loaded: r.data.read_b()?,
            resolution_units: r.data.read_rc()?,
            pixel_size: r.data.read_2rd()?,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        w.data.write_bl(self.class_version);
        w.data.write_2rd(self.size);
        w.text(&self.path)?;
        w.data.write_b(self.loaded);
        w.data.write_rc(self.resolution_units);
        w.data.write_2rd(self.pixel_size);
        Ok(())
    }
}

/// An image definition reactor (ODS 20.4.82, IMAGEDEF_REACTOR).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ImageDefReactor {
    /// DXF 90.
    pub class_version: i32,
}

/// Raster image settings (ODS 20.4.91, RASTERVARIABLES).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RasterVariables {
    /// DXF 90.
    pub class_version: i32,
    /// DXF 70.
    pub display_frame: i16,
    /// DXF 71.
    pub display_quality: i16,
    /// DXF 72.
    pub units: i16,
}

/// Wipeout settings (WIPEOUTVARIABLES, not in ODS; as ACadSharp (MIT)
/// `readWipeoutVariables` reads it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct WipeoutVariables {
    /// DXF 70.
    pub display_frame: i16,
}

impl ImageDefReactor {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        Ok(ImageDefReactor {
            class_version: r.data.read_bl()?,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        w.data.write_bl(self.class_version);
        Ok(())
    }
}

impl RasterVariables {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        Ok(RasterVariables {
            class_version: r.data.read_bl()?,
            display_frame: r.data.read_bs()?,
            display_quality: r.data.read_bs()?,
            units: r.data.read_bs()?,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        w.data.write_bl(self.class_version);
        w.data.write_bs(self.display_frame);
        w.data.write_bs(self.display_quality);
        w.data.write_bs(self.units);
        Ok(())
    }
}

impl WipeoutVariables {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        Ok(WipeoutVariables {
            display_frame: r.data.read_bs()?,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        w.data.write_bs(self.display_frame);
        Ok(())
    }
}
