use crate::{Config, CoreError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    /// Signed logical-pixel offset from the center of the output to the buffer's top-left.
    pub origin_x: i32,
    pub origin_y: i32,
    /// Native little-endian Wayland ARGB8888 memory order (B, G, R, A).
    /// RGB is premultiplied by alpha as required by wl_shm.
    pub bgra_premultiplied: Vec<u8>,
}

impl Raster {
    pub fn stride(&self) -> i32 {
        (self.width * 4) as i32
    }
}

#[derive(Clone, Copy)]
struct Rgba {
    r: f32,
    g: f32,
    b: f32,
    a: f32,
}

impl Rgba {
    fn from_hex(value: &str, opacity: f32) -> Result<Self, CoreError> {
        let clean = value.strip_prefix('#').unwrap_or(value);
        if clean.len() != 6 {
            return Err(CoreError::Config("color must be #RRGGBB".into()));
        }
        let parse = |index| {
            u8::from_str_radix(&clean[index..index + 2], 16)
                .map(|v| v as f32 / 255.0)
                .map_err(|_| CoreError::Config("invalid color".into()))
        };
        Ok(Self {
            r: parse(0)?,
            g: parse(2)?,
            b: parse(4)?,
            a: opacity.clamp(0.0, 1.0),
        })
    }
}

#[derive(Clone, Copy)]
struct Bounds {
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
}

impl Bounds {
    fn include_circle(&mut self, x: f32, y: f32, radius: f32) {
        self.min_x = self.min_x.min(x - radius);
        self.min_y = self.min_y.min(y - radius);
        self.max_x = self.max_x.max(x + radius);
        self.max_y = self.max_y.max(y + radius);
    }
}

pub fn render_crosshair(config: &Config) -> Result<Raster, CoreError> {
    let mut c = config.clone();
    c.normalize();

    let stroke = c.thickness as f32;
    let outline = if c.outline.enabled {
        c.outline.thickness as f32
    } else {
        0.0
    };
    let shadow = c.shadow.enabled;
    let size = c.size as f32;
    let gap = c.gap as f32;
    let shape_extent = match c.shape.as_str() {
        "dot" => size,
        "ring" => size + stroke / 2.0,
        "x" | "x_dot" => std::f32::consts::SQRT_2 * (gap + size) + stroke / 2.0,
        "cross" | "cross_dot" => gap + size + stroke / 2.0,
        "plus" => size + stroke / 2.0,
        "dot_ring" => size + stroke / 2.0,
        _ => size,
    };
    let center_dot_extent = match c.shape.as_str() {
        "dot" => 0.0,
        "dot_ring" | "cross_dot" | "x_dot" => (stroke * 0.9).max(1.0),
        _ => 0.0,
    };
    let main_extent = shape_extent.max(center_dot_extent) + outline + 2.0;
    let mut bounds = Bounds {
        min_x: c.offset_x as f32 - main_extent,
        min_y: c.offset_y as f32 - main_extent,
        max_x: c.offset_x as f32 + main_extent,
        max_y: c.offset_y as f32 + main_extent,
    };
    if shadow {
        let sh_extent = main_extent;
        bounds.include_circle(
            c.offset_x as f32 + c.shadow.offset_x as f32,
            c.offset_y as f32 + c.shadow.offset_y as f32,
            sh_extent,
        );
    }
    if c.reference_dot.enabled {
        let radius = c.reference_dot.size as f32;
        bounds.include_circle(
            c.offset_x as f32 + c.reference_dot.offset_x as f32,
            c.offset_y as f32 + c.reference_dot.offset_y as f32,
            radius + 1.0,
        );
    }

    let left = bounds.min_x.floor() as i32;
    let top = bounds.min_y.floor() as i32;
    let right = bounds.max_x.ceil() as i32;
    let bottom = bounds.max_y.ceil() as i32;
    let width = (right - left + 1).max(1) as u32;
    let height = (bottom - top + 1).max(1) as u32;
    let bytes = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| CoreError::Config("crosshair drawing is too large".into()))?;
    if bytes > 16 * 1024 * 1024 {
        return Err(CoreError::Config(
            "crosshair drawing exceeds its memory limit".into(),
        ));
    }

    let mut raster = Raster {
        width,
        height,
        origin_x: left,
        origin_y: top,
        bgra_premultiplied: vec![0; bytes],
    };
    let cx = c.offset_x as f32 - left as f32;
    let cy = c.offset_y as f32 - top as f32;

    // The renderer is deliberately event-driven: it creates this tiny buffer
    // only when the crosshair is shown or its appearance changes.
    if c.shadow.enabled {
        let sh = Rgba::from_hex(&c.shadow.color, c.shadow.opacity * c.opacity)?;
        draw_shape(
            &mut raster,
            &c.shape,
            cx + c.shadow.offset_x as f32,
            cy + c.shadow.offset_y as f32,
            size,
            gap,
            stroke,
            sh,
            false,
            0.0,
        );
    }

    if c.outline.enabled && c.outline.thickness > 0 {
        let color = Rgba::from_hex(&c.outline.color, c.outline.opacity * c.opacity)?;
        draw_shape(
            &mut raster,
            &c.shape,
            cx,
            cy,
            size,
            gap,
            stroke,
            color,
            true,
            c.outline.thickness as f32,
        );
    }

    let main = Rgba::from_hex(&c.color, c.opacity)?;
    draw_shape(
        &mut raster,
        &c.shape,
        cx,
        cy,
        size,
        gap,
        stroke,
        main,
        false,
        0.0,
    );

    if c.reference_dot.enabled {
        let color = Rgba::from_hex(&c.reference_dot.color, c.reference_dot.opacity)?;
        fill_circle(
            &mut raster,
            cx + c.reference_dot.offset_x as f32,
            cy + c.reference_dot.offset_y as f32,
            c.reference_dot.size as f32,
            color,
        );
    }

    Ok(raster)
}

fn draw_shape(
    raster: &mut Raster,
    shape: &str,
    cx: f32,
    cy: f32,
    size: f32,
    gap: f32,
    thickness: f32,
    color: Rgba,
    is_outline: bool,
    outline_thickness: f32,
) {
    let width = if is_outline {
        thickness + outline_thickness * 2.0
    } else {
        thickness
    };
    let dot_radius = if is_outline {
        (size + outline_thickness).max(1.0)
    } else {
        size.max(1.0)
    };
    match shape {
        "dot" => fill_circle(raster, cx, cy, dot_radius, color),
        "ring" => stroke_circle(raster, cx, cy, size, width, color),
        "dot_ring" => {
            stroke_circle(raster, cx, cy, size, width, color);
            fill_circle(
                raster,
                cx,
                cy,
                if is_outline {
                    (thickness * 0.9 + outline_thickness).max(1.0)
                } else {
                    (thickness * 0.9).max(1.0)
                },
                color,
            );
        }
        "cross" | "cross_dot" | "plus" => {
            let g = if shape == "plus" { 0.0 } else { gap };
            // Cairo strokes all four subpaths as one operation. Evaluate their
            // union before blending so arms that meet at zero gap are not
            // composited repeatedly and made artificially opaque.
            draw_segments(
                raster,
                &[
                    (cx, cy - g, cx, cy - g - size),
                    (cx, cy + g, cx, cy + g + size),
                    (cx - g, cy, cx - g - size, cy),
                    (cx + g, cy, cx + g + size, cy),
                ],
                width,
                color,
            );
            if shape == "cross_dot" {
                let radius = if is_outline {
                    thickness * 0.9 + outline_thickness
                } else {
                    thickness * 0.9
                };
                fill_circle(raster, cx, cy, radius.max(1.0), color);
            }
        }
        "x" | "x_dot" => {
            let g = gap;
            draw_segments(
                raster,
                &[
                    (cx - g, cy - g, cx - g - size, cy - g - size),
                    (cx + g, cy + g, cx + g + size, cy + g + size),
                    (cx - g, cy + g, cx - g - size, cy + g + size),
                    (cx + g, cy - g, cx + g + size, cy - g - size),
                ],
                width,
                color,
            );
            if shape == "x_dot" {
                let radius = if is_outline {
                    thickness * 0.9 + outline_thickness
                } else {
                    thickness * 0.9
                };
                fill_circle(raster, cx, cy, radius.max(1.0), color);
            }
        }
        _ => {}
    }
}

fn fill_circle(raster: &mut Raster, cx: f32, cy: f32, radius: f32, color: Rgba) {
    draw_coverage(
        raster,
        cx - radius - 1.0,
        cy - radius - 1.0,
        cx + radius + 1.0,
        cy + radius + 1.0,
        color,
        |x, y| {
            let dx = x - cx;
            let dy = y - cy;
            if dx * dx + dy * dy <= radius * radius {
                1.0
            } else {
                0.0
            }
        },
    );
}

fn stroke_circle(raster: &mut Raster, cx: f32, cy: f32, radius: f32, width: f32, color: Rgba) {
    let half = width.max(0.5) / 2.0;
    draw_coverage(
        raster,
        cx - radius - half - 1.0,
        cy - radius - half - 1.0,
        cx + radius + half + 1.0,
        cy + radius + half + 1.0,
        color,
        |x, y| {
            let distance = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
            if (distance - radius).abs() <= half {
                1.0
            } else {
                0.0
            }
        },
    );
}

fn draw_segments(raster: &mut Raster, segments: &[(f32, f32, f32, f32)], width: f32, color: Rgba) {
    if segments.is_empty() {
        return;
    }
    let half = width.max(0.5) / 2.0;
    let min_x = segments
        .iter()
        .map(|(x0, _, x1, _)| x0.min(*x1))
        .fold(f32::INFINITY, f32::min)
        - half
        - 1.0;
    let min_y = segments
        .iter()
        .map(|(_, y0, _, y1)| y0.min(*y1))
        .fold(f32::INFINITY, f32::min)
        - half
        - 1.0;
    let max_x = segments
        .iter()
        .map(|(x0, _, x1, _)| x0.max(*x1))
        .fold(f32::NEG_INFINITY, f32::max)
        + half
        + 1.0;
    let max_y = segments
        .iter()
        .map(|(_, y0, _, y1)| y0.max(*y1))
        .fold(f32::NEG_INFINITY, f32::max)
        + half
        + 1.0;

    draw_coverage(raster, min_x, min_y, max_x, max_y, color, |x, y| {
        let half_sq = half * half;
        if segments.iter().any(|(x0, y0, x1, y1)| {
            let dx = x1 - x0;
            let dy = y1 - y0;
            let length_sq = dx * dx + dy * dy;
            let t = if length_sq <= f32::EPSILON {
                0.0
            } else {
                (((x - x0) * dx + (y - y0) * dy) / length_sq).clamp(0.0, 1.0)
            };
            let nearest_x = x0 + t * dx;
            let nearest_y = y0 + t * dy;
            (x - nearest_x).powi(2) + (y - nearest_y).powi(2) <= half_sq
        }) {
            1.0
        } else {
            0.0
        }
    });
}

fn draw_coverage(
    raster: &mut Raster,
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
    color: Rgba,
    inside: impl Fn(f32, f32) -> f32,
) {
    const SAMPLES: [(f32, f32); 4] = [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)];
    let x0 = (min_x.floor() as i32).clamp(0, raster.width as i32);
    let y0 = (min_y.floor() as i32).clamp(0, raster.height as i32);
    let x1 = (max_x.ceil() as i32).clamp(0, raster.width as i32);
    let y1 = (max_y.ceil() as i32).clamp(0, raster.height as i32);
    for y in y0..y1 {
        for x in x0..x1 {
            let coverage = SAMPLES
                .iter()
                .map(|(sx, sy)| inside(x as f32 + sx, y as f32 + sy))
                .sum::<f32>()
                / SAMPLES.len() as f32;
            if coverage > 0.0 {
                blend_pixel(raster, x as u32, y as u32, color, coverage);
            }
        }
    }
}

fn blend_pixel(raster: &mut Raster, x: u32, y: u32, color: Rgba, coverage: f32) {
    let source_alpha = (color.a * coverage).clamp(0.0, 1.0);
    if source_alpha <= 0.0 {
        return;
    }
    let index = ((y * raster.width + x) * 4) as usize;
    let destination_alpha = raster.bgra_premultiplied[index + 3] as f32 / 255.0;
    let remain = 1.0 - source_alpha;
    let dest_b = raster.bgra_premultiplied[index] as f32 / 255.0;
    let dest_g = raster.bgra_premultiplied[index + 1] as f32 / 255.0;
    let dest_r = raster.bgra_premultiplied[index + 2] as f32 / 255.0;
    let out_b = color.b * source_alpha + dest_b * remain;
    let out_g = color.g * source_alpha + dest_g * remain;
    let out_r = color.r * source_alpha + dest_r * remain;
    let out_a = source_alpha + destination_alpha * remain;
    raster.bgra_premultiplied[index] = (out_b * 255.0).round() as u8;
    raster.bgra_premultiplied[index + 1] = (out_g * 255.0).round() as u8;
    raster.bgra_premultiplied[index + 2] = (out_r * 255.0).round() as u8;
    raster.bgra_premultiplied[index + 3] = (out_a * 255.0).round() as u8;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_dot_raster_has_transparent_corners_and_visible_center() {
        let raster = render_crosshair(&Config::default()).unwrap();
        assert!(raster.width < 32 && raster.height < 32);
        assert_eq!(&raster.bgra_premultiplied[0..4], &[0, 0, 0, 0]);
        let center = ((raster.height / 2 * raster.width + raster.width / 2) * 4) as usize;
        assert!(raster.bgra_premultiplied[center + 3] > 0);
    }

    #[test]
    fn diagonal_crosshair_bounds_contain_all_endpoints() {
        let mut config = Config::default();
        config.shape = "x".into();
        config.size = 200;
        config.gap = 200;
        let raster = render_crosshair(&config).unwrap();
        assert!(raster.width > 500 && raster.height > 500);
        assert_eq!(
            raster.bgra_premultiplied.len(),
            raster.width as usize * raster.height as usize * 4
        );
    }

    #[test]
    fn reference_dot_and_global_offset_expand_the_buffer() {
        let mut config = Config::default();
        config.reference_dot.enabled = true;
        config.reference_dot.offset_y = 180;
        config.offset_x = 16;
        let raster = render_crosshair(&config).unwrap();
        assert!(raster.height >= 180);
        assert!(raster.width >= 11 && raster.width < 32);
    }

    #[test]
    fn every_shape_keeps_its_drawn_pixels_inside_the_small_surface() {
        for shape in [
            "dot",
            "cross",
            "x",
            "plus",
            "ring",
            "dot_ring",
            "cross_dot",
            "x_dot",
        ] {
            let mut config = Config::default();
            config.shape = shape.into();
            config.size = 1;
            config.thickness = 100;
            config.gap = 0;
            config.outline.enabled = true;
            config.outline.thickness = 20;
            let raster = render_crosshair(&config).unwrap();
            let w = raster.width as usize;
            let h = raster.height as usize;
            for x in 0..w {
                assert_eq!(
                    raster.bgra_premultiplied[(x * 4) + 3],
                    0,
                    "{shape}: top edge"
                );
                assert_eq!(
                    raster.bgra_premultiplied[((h - 1) * w + x) * 4 + 3],
                    0,
                    "{shape}: bottom edge"
                );
            }
            for y in 0..h {
                assert_eq!(
                    raster.bgra_premultiplied[(y * w) * 4 + 3],
                    0,
                    "{shape}: left edge"
                );
                assert_eq!(
                    raster.bgra_premultiplied[(y * w + w - 1) * 4 + 3],
                    0,
                    "{shape}: right edge"
                );
            }
        }
    }

    #[test]
    fn plus_shape_does_not_allocate_for_the_unused_gap_setting() {
        let mut config = Config::default();
        config.shape = "plus".into();
        config.gap = 0;
        let compact = render_crosshair(&config).unwrap();
        config.gap = 200;
        let unchanged = render_crosshair(&config).unwrap();
        assert_eq!(
            (compact.width, compact.height),
            (unchanged.width, unchanged.height)
        );
    }

    #[test]
    fn reference_dot_uses_its_own_opacity_color_and_offset() {
        let mut config = Config::default();
        config.opacity = 0.05;
        config.outline.enabled = false;
        config.reference_dot.enabled = true;
        config.reference_dot.color = "#12abef".into();
        config.reference_dot.opacity = 0.5;
        config.reference_dot.size = 3;
        config.reference_dot.offset_x = 40;
        config.reference_dot.offset_y = -30;

        let raster = render_crosshair(&config).unwrap();
        let x = (i32::from(config.offset_x) + i32::from(config.reference_dot.offset_x)
            - raster.origin_x) as u32;
        let y = (i32::from(config.offset_y) + i32::from(config.reference_dot.offset_y)
            - raster.origin_y) as u32;
        let index = ((y * raster.width + x) * 4) as usize;
        let pixel = &raster.bgra_premultiplied[index..index + 4];

        assert_eq!(
            pixel[3], 128,
            "reference-dot opacity is independent of global opacity"
        );
        assert_eq!(
            &pixel[..3],
            &[120, 86, 9],
            "reference-dot color is premultiplied BGRA"
        );
    }

    #[test]
    fn global_offset_moves_the_raster_origin_without_changing_its_pixels() {
        let mut config = Config::default();
        config.shape = "x_dot".into();
        config.shadow.enabled = true;
        let original = render_crosshair(&config).unwrap();

        config.offset_x = 11;
        config.offset_y = -8;
        let shifted = render_crosshair(&config).unwrap();

        assert_eq!(
            (shifted.width, shifted.height),
            (original.width, original.height)
        );
        assert_eq!(
            (shifted.origin_x, shifted.origin_y),
            (original.origin_x + 11, original.origin_y - 8)
        );
        assert_eq!(shifted.bgra_premultiplied, original.bgra_premultiplied);
    }

    #[test]
    fn intersecting_stroke_subpaths_are_blended_once() {
        for shape in ["cross", "plus", "x"] {
            let mut config = Config::default();
            config.shape = shape.into();
            config.size = 10;
            config.thickness = 6;
            config.gap = 0;
            config.opacity = 0.5;
            config.outline.enabled = false;

            let raster = render_crosshair(&config).unwrap();
            let center_x = (i32::from(config.offset_x) - raster.origin_x) as u32;
            let center_y = (i32::from(config.offset_y) - raster.origin_y) as u32;
            let index = ((center_y * raster.width + center_x) * 4) as usize;
            assert_eq!(
                raster.bgra_premultiplied[index + 3],
                128,
                "{shape}: intersecting path segments must not accumulate alpha",
            );
        }
    }

    #[test]
    fn shadow_and_outline_are_composited_as_premultiplied_alpha() {
        let mut config = Config::default();
        config.shadow.enabled = true;
        config.outline.enabled = true;
        config.opacity = 0.5;
        let raster = render_crosshair(&config).unwrap();
        for pixel in raster.bgra_premultiplied.chunks_exact(4) {
            assert!(pixel[0] <= pixel[3]);
            assert!(pixel[1] <= pixel[3]);
            assert!(pixel[2] <= pixel[3]);
        }
    }
}
