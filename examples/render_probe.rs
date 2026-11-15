use crosshair_core::{Config, render::render_crosshair};
use std::io::{Read, Write};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let config = Config::from_json(&input)?;
    let raster = render_crosshair(&config)?;

    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    output.write_all(&raster.width.to_le_bytes())?;
    output.write_all(&raster.height.to_le_bytes())?;
    output.write_all(&raster.origin_x.to_le_bytes())?;
    output.write_all(&raster.origin_y.to_le_bytes())?;
    output.write_all(&raster.bgra_premultiplied)?;
    Ok(())
}
