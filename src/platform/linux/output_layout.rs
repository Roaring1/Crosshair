use crosshair_core::Config;
use crosshair_core::render::{Raster, render_crosshair_scaled};
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
    let current_mode = info
        .modes
        .iter()
        .find(|mode| mode.current)
        .map(|mode| mode.dimensions);
    build_from_metadata(
        config,
        info.id,
        info.name.as_deref(),
        info.logical_size,
        current_mode,
        info.scale_factor,
    )
}

fn build_from_metadata(
    config: &Config,
    output_id: u32,
    output_name: Option<&str>,
    logical_size: Option<(i32, i32)>,
    current_mode: Option<(i32, i32)>,
    output_scale: i32,
) -> Result<OutputLayout, String> {
    let scale = output_scale.clamp(1, 4) as u32;
    if logical_size.is_none() && output_scale > 4 && current_mode.is_some() {
        return Err(format!(
            "Output {} reports scale {output_scale}x but has no logical dimensions; refusing an ambiguous fallback",
            output_name.unwrap_or("unknown")
        ));
    }
    let (width, height) = logical_size
        .or_else(|| {
            current_mode.map(|(physical_width, physical_height)| {
                let buffer_scale = scale as i32;
                (
                    physical_width / buffer_scale,
                    physical_height / buffer_scale,
                )
            })
        })
        .ok_or_else(|| {
            format!(
                "Could not determine logical dimensions for output {}",
                output_name.unwrap_or("unknown")
            )
        })?;
    if width <= 0 || height <= 0 {
        return Err(format!(
            "Output {} reported invalid dimensions",
            output_name.unwrap_or("unknown")
        ));
    }

    let name = output_name
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("Output {output_id}"));
    let raster = render_crosshair_scaled(config, scale).map_err(|error| error.to_string())?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_compositor_logical_size_and_uses_scaled_rendering() {
        let mut config = Config::default();
        config.size = 200;
        config.outline.enabled = false;
        config.shadow.enabled = false;

        let layout = build_from_metadata(
            &config,
            2,
            Some("DP-1"),
            Some((1920, 1080)),
            Some((3840, 2160)),
            2,
        )
        .unwrap();
        let logical_raster = crosshair_core::render::render_crosshair(&config).unwrap();

        assert_eq!(layout.name, "DP-1");
        assert_eq!(layout.scale, 2);
        assert!(layout.raster.width > logical_raster.width * 19 / 10);
        assert!(layout.raster.height > logical_raster.height * 19 / 10);
        assert_eq!(layout.logical_width, layout.raster.width.div_ceil(2));
        assert_eq!(layout.logical_height, layout.raster.height.div_ceil(2));
        assert_eq!(layout.left, 1920 / 2 + layout.raster.origin_x / 2);
        assert_eq!(layout.top, 1080 / 2 + layout.raster.origin_y / 2);
    }

    #[test]
    fn falls_back_to_current_mode_divided_by_reported_scale() {
        let layout =
            build_from_metadata(&Config::default(), 7, None, None, Some((3840, 2160)), 2).unwrap();

        assert_eq!(layout.name, "Output 7");
        assert_eq!(layout.scale, 2);
        assert_eq!(layout.left, 1920 / 2 + layout.raster.origin_x / 2);
        assert_eq!(layout.top, 1080 / 2 + layout.raster.origin_y / 2);
    }

    #[test]
    fn invalid_or_missing_output_dimensions_fail_closed() {
        assert!(
            build_from_metadata(
                &Config::default(),
                1,
                Some("DP-1"),
                Some((0, 1080)),
                Some((3840, 2160)),
                2,
            )
            .is_err()
        );
        assert!(build_from_metadata(&Config::default(), 1, None, None, None, 1).is_err());
    }

    #[test]
    fn refuses_extreme_scale_fallback_without_logical_dimensions() {
        let error = match build_from_metadata(
            &Config::default(),
            3,
            Some("DP-2"),
            None,
            Some((7680, 4320)),
            16,
        ) {
            Ok(_) => panic!("ambiguous extreme-scale fallback should fail"),
            Err(error) => error,
        };

        assert!(error.contains("scale 16x"));
        assert!(error.contains("no logical dimensions"));
    }

    #[test]
    fn buffer_scale_is_bounded_even_when_output_reports_an_extreme_value() {
        let layout = build_from_metadata(
            &Config::default(),
            1,
            Some("DP-1"),
            Some((1920, 1080)),
            None,
            16,
        )
        .unwrap();
        assert_eq!(layout.scale, 4);
    }
}
