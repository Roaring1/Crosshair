use crosshair_core::render::{render_crosshair, Raster};
use crosshair_core::Config;
use smithay_client_toolkit::output::OutputInfo;

pub struct OutputLayout {
    pub raster: Raster,
    pub name: String,
    pub scale: u32,
    pub logical_width: u32,
    pub logical_height: u32,
    pub top: i32,
    pub left: i32,
}

pub fn build(config: &Config, info: &OutputInfo) -> Result<OutputLayout, String> {
    let (width, height) = info.logical_size
        .or_else(|| info.modes.iter().find(|mode| mode.current).map(|mode| {
            let scale = info.scale_factor.max(1);
            (mode.dimensions.0 / scale, mode.dimensions.1 / scale)
        }))
        .ok_or_else(|| format!(
            "Could not determine logical dimensions for output {}",
            info.name.as_deref().unwrap_or("unknown")
        ))?;
    if width <= 0 || height <= 0 {
        return Err(format!("Output {} reported invalid dimensions", info.name.as_deref().unwrap_or("unknown")));
    }

    let name = info.name.clone().filter(|name| !name.is_empty())
        .unwrap_or_else(|| format!("Output {}", info.id));
    let scale = info.scale_factor.clamp(1, 4) as u32;
    let mut scaled = config.clone();
    scaled.size = scaled.size.saturating_mul(scale as u16).min(800);
    scaled.thickness = scaled.thickness.saturating_mul(scale as u16).min(400);
    scaled.gap = scaled.gap.saturating_mul(scale as u16).min(800);
    scaled.offset_x = scaled.offset_x.saturating_mul(scale as i16);
    scaled.offset_y = scaled.offset_y.saturating_mul(scale as i16);
    scaled.outline.thickness = scaled.outline.thickness.saturating_mul(scale as u16).min(200);
    scaled.shadow.offset_x = scaled.shadow.offset_x.saturating_mul(scale as i16);
    scaled.shadow.offset_y = scaled.shadow.offset_y.saturating_mul(scale as i16);
    scaled.reference_dot.size = scaled.reference_dot.size.saturating_mul(scale as u16).min(400);
    scaled.reference_dot.offset_x = scaled.reference_dot.offset_x.saturating_mul(scale as i16);
    scaled.reference_dot.offset_y = scaled.reference_dot.offset_y.saturating_mul(scale as i16);

    let raster = render_crosshair(&scaled).map_err(|error| error.to_string())?;
    let logical_width = raster.width.div_ceil(scale);
    let logical_height = raster.height.div_ceil(scale);
    let x_origin = raster.origin_x / scale as i32;
    let y_origin = raster.origin_y / scale as i32;
    // The surface is anchored only at top/left. The raster origin is
    // relative to the actual reticle center and already includes the chosen
    // whole-crosshair offset.
    Ok(OutputLayout {
        name,
        raster,
        scale,
        logical_width,
        logical_height,
        top: (height / 2) + y_origin,
        left: (width / 2) + x_origin,
    })
}
