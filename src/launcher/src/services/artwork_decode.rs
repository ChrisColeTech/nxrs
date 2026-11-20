//! Decoding a cover or background: format from the content (`icon_AmericanEnglish.dat` is a JPEG), EXIF
//! orientation applied, at most 2048 px. Errors are the decoder's words.

use crate::constants::layout::ARTWORK_MAX;
use image::ImageDecoder;

pub fn decode(path: &str) -> Result<image::RgbaImage, String> {
    let reader = image::ImageReader::open(path).map_err(|e| e.to_string())?;
    let reader = reader.with_guessed_format().map_err(|e| e.to_string())?;
    let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = image::DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    img.apply_orientation(orientation);
    if img.width() > ARTWORK_MAX || img.height() > ARTWORK_MAX {
        img = img.resize(
            ARTWORK_MAX,
            ARTWORK_MAX,
            image::imageops::FilterType::Triangle,
        );
    }
    Ok(img.to_rgba8())
}

/// Writes an RGBA picture as PNG (`--screenshot`), at most `max_width` wide.
pub fn write_png(
    path: &str,
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    max_width: u32,
) -> Result<(), String> {
    let Some(img) = image::RgbaImage::from_raw(width, height, rgba) else {
        return Err("bad image size".into());
    };
    let mut img = image::DynamicImage::ImageRgba8(img);
    if img.width() > max_width {
        let h =
            (u64::from(img.height()) * u64::from(max_width) / u64::from(img.width().max(1))) as u32;
        img = img.resize_exact(max_width, h.max(1), image::imageops::FilterType::Triangle);
    }
    if let Some(dir) = std::path::Path::new(path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    img.save_with_format(path, image::ImageFormat::Png)
        .map_err(|e| e.to_string())
}
