use std::io::Cursor;

use image::ImageFormat;
use wow_blp::convert::blp_to_image;
use wow_blp::parser::load_blp_from_buf;
use wow_mpq::Archive;

use crate::mpq;

/// Convert raw BLP bytes to PNG bytes.
pub fn blp_to_png(blp_data: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let blp_image = load_blp_from_buf(blp_data)?;
    let dynamic_image = blp_to_image(&blp_image, 0)?;
    let mut png_buf: Vec<u8> = Vec::new();
    dynamic_image.write_to(&mut Cursor::new(&mut png_buf), ImageFormat::Png)?;
    Ok(png_buf)
}

/// Read a BLP texture from archives and convert to PNG.
/// Returns `(basename_without_extension, png_bytes)`.
pub fn load_texture(
    archives: &mut [Archive],
    blp_path: &str,
) -> Result<(String, Vec<u8>), Box<dyn std::error::Error>> {
    let blp_data = mpq::read_file(archives, blp_path)?;
    let png_buf = blp_to_png(&blp_data)?;

    let basename = blp_path
        .rsplit(&['\\', '/'][..])
        .next()
        .unwrap_or(blp_path);
    let name = basename
        .strip_suffix(".blp")
        .or_else(|| basename.strip_suffix(".BLP"))
        .unwrap_or(basename)
        .to_string();

    println!("Converted texture: {} ({} bytes PNG)", basename, png_buf.len());
    Ok((name, png_buf))
}
