//! Decoding a cover or background: format from the content (`icon_AmericanEnglish.dat` is a JPEG, a
//! 3DS build's `control.smdh` is an SMDH), EXIF orientation applied, at most 2048 px. Errors are
//! the decoder's words.

use crate::constants::layout::ARTWORK_MAX;
use image::ImageDecoder;

pub fn decode(path: &str) -> Result<image::RgbaImage, String> {
    // the SMDH is raw bytes, not a format the image decoder knows: dispatch on its magic
    if std::fs::read(path)
        .ok()
        .is_some_and(|bytes| bytes.starts_with(b"SMDH"))
    {
        return smdh(path).map(|(img, _)| img);
    }
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

/// A 3DS title's card data: the large icon (48x48 RGB565 at 0x2480) as RGBA, and the titles
/// (short, long, publisher; UTF-16, the English entry when it says something, else the first).
pub fn smdh(path: &str) -> Result<(image::RgbaImage, Vec<String>), String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if !bytes.starts_with(b"SMDH") {
        return Err(format!("{}: no SMDH header", path));
    }
    let cut = |at: usize, len: usize| -> Option<&[u8]> { bytes.get(at..at + len) };
    let text = |at: usize, len: usize| -> Option<String> {
        let units: Vec<u16> = cut(at, len)?
            .as_chunks::<2>()
            .0
            .iter()
            .map(|chunk| u16::from_le_bytes(*chunk))
            .take_while(|&unit| unit != 0)
            .collect();
        Some(String::from_utf16_lossy(&units))
    };
    // sixteen language entries of 0x200 bytes: short 0x40, long 0x80, publisher 0x40; the English
    // one (entry 1) when it says something, else the first
    let (entry_0, entry_1) = (0x10, 0x10 + 0x200);
    let short = if text(entry_1, 0x40).is_some_and(|t| !t.is_empty()) {
        entry_1
    } else {
        entry_0
    };
    let titles: Vec<String> = [(short, 0x40), (short + 0x40, 0x80), (short + 0xC0, 0x40)]
        .into_iter()
        .map(|(at, len)| text(at, len).unwrap_or_default())
        .collect();
    let mut img = image::RgbaImage::new(48, 48);
    for (i, pixel) in img.pixels_mut().enumerate() {
        let Some(raw) = cut(0x2480 + i * 2, 2) else {
            return Err(format!("{}: the SMDH is cut short", path));
        };
        let p = u16::from_le_bytes([raw[0], raw[1]]);
        let (r, g, b) = (
            u32::from((p >> 11) & 0x1F),
            u32::from((p >> 5) & 0x3F),
            u32::from(p & 0x1F),
        );
        *pixel = image::Rgba([
            (r * 255 / 31) as u8,
            (g * 255 / 63) as u8,
            (b * 255 / 31) as u8,
            255,
        ]);
    }
    Ok((img, titles))
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
