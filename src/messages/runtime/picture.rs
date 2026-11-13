//! `bot.image`: a message's picture, the way a vision model takes it.
//!
//! Telegram serves what people sent: webp stickers, png screenshots, photos in
//! whatever size. Models take JPEG or PNG and pay per pixel, so the picture is
//! decoded, scaled down and re-encoded as a small JPEG here, where it is cheap,
//! instead of in a script, which has no bytes to work with.

use base64::Engine;
use image::{DynamicImage, GenericImageView, ImageReader, Rgba, RgbaImage, imageops};
use serde_json::Value;
use std::io::Cursor;

/// The longer side of the picture `image` gives by default. Enough to read a
/// screenshot's text; a vision model shrinks anything bigger anyway.
pub const DEFAULT_SIDE: u32 = 768;
/// The most `image` will give: base64 of a JPEG this size stays well under
/// dog4's 1 MiB string limit.
pub const MAX_SIDE: u32 = 1536;
const QUALITY: u8 = 85;
/// Pictures bigger than this many pixels are refused before decoding, so a
/// small file that expands into a huge image can't eat the bot's memory.
const MAX_PIXELS: u64 = 50_000_000;

/// Which of a message's files is its picture: the photo's preview size, a
/// static sticker or an image file itself, otherwise the thumbnail.
pub fn source(kind: &str, media: Option<&Value>) -> Option<String> {
    let media = media?;
    let field = |name: &str| media.get(name).and_then(Value::as_str).map(str::to_owned);
    match kind {
        "photo" => field("preview_id").or_else(|| field("file_id")),
        "sticker" if media.get("format").and_then(Value::as_str) == Some("static") => {
            field("file_id")
        }
        "document"
            if media
                .get("mime_type")
                .and_then(Value::as_str)
                .is_some_and(|m| m.starts_with("image/")) =>
        {
            field("file_id")
        }
        _ => field("thumb_id"),
    }
}

/// Decodes `bytes`, fits the picture within `side` pixels and gives it as a
/// JPEG data URL. Transparency (stickers have plenty) becomes white.
pub fn data_url(bytes: &[u8], side: u32) -> Result<String, String> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| format!("could not read the picture: {e}"))?;
    let (w, h) = reader
        .into_dimensions()
        .map_err(|e| format!("could not read the picture: {e}"))?;
    if w as u64 * h as u64 > MAX_PIXELS {
        return Err(format!("the picture is too large ({w}×{h})"));
    }
    let picture = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| format!("could not read the picture: {e}"))?
        .decode()
        .map_err(|e| format!("could not decode the picture: {e}"))?;
    let picture = if picture.width().max(picture.height()) > side {
        picture.resize(side, side, imageops::FilterType::Triangle)
    } else {
        picture
    };
    let flat = on_white(&picture);
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, QUALITY)
        .encode_image(&flat)
        .map_err(|e| format!("could not encode the picture: {e}"))?;
    Ok(format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(jpeg)
    ))
}

/// The picture with its transparent parts painted white, since JPEG has no
/// transparency and a black background would hide a dark sticker.
fn on_white(picture: &DynamicImage) -> DynamicImage {
    let (w, h) = picture.dimensions();
    let mut canvas = RgbaImage::from_pixel(w, h, Rgba([255, 255, 255, 255]));
    imageops::overlay(&mut canvas, &picture.to_rgba8(), 0, 0);
    DynamicImage::ImageRgba8(canvas).to_rgb8().into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn encoded(picture: DynamicImage, format: image::ImageFormat) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        picture.write_to(&mut out, format).unwrap();
        out.into_inner()
    }

    fn decode(url: &str) -> DynamicImage {
        let b64 = url
            .strip_prefix("data:image/jpeg;base64,")
            .expect("a jpeg data url");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap();
        image::load_from_memory(&bytes).unwrap()
    }

    #[test]
    fn a_large_png_is_scaled_down_to_a_jpeg() {
        let big = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            2000,
            1000,
            image::Rgb([10, 200, 30]),
        ));
        let url = data_url(&encoded(big, image::ImageFormat::Png), 768).unwrap();
        let out = decode(&url);
        assert_eq!(out.dimensions(), (768, 384));
    }

    #[test]
    fn a_transparent_webp_sticker_gets_a_white_background() {
        let clear = DynamicImage::ImageRgba8(RgbaImage::from_pixel(64, 64, Rgba([0, 0, 0, 0])));
        let url = data_url(&encoded(clear, image::ImageFormat::WebP), 768).unwrap();
        let out = decode(&url).to_rgb8();
        assert_eq!(out.dimensions(), (64, 64));
        let p = out.get_pixel(10, 10);
        assert!(p.0.iter().all(|c| *c > 240), "{p:?}");
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(data_url(b"not a picture", 768).is_err());
    }

    #[test]
    fn the_source_is_the_right_file() {
        let photo = json!({ "file_id": "big", "preview_id": "mid" });
        assert_eq!(source("photo", Some(&photo)).as_deref(), Some("mid"));
        let sticker = json!({ "file_id": "s", "format": "static", "thumb_id": "t" });
        assert_eq!(source("sticker", Some(&sticker)).as_deref(), Some("s"));
        let animated = json!({ "file_id": "s", "format": "animated", "thumb_id": "t" });
        assert_eq!(source("sticker", Some(&animated)).as_deref(), Some("t"));
        let pdf = json!({ "file_id": "d", "mime_type": "application/pdf" });
        assert_eq!(source("document", Some(&pdf)), None);
        let png = json!({ "file_id": "d", "mime_type": "image/png" });
        assert_eq!(source("document", Some(&png)).as_deref(), Some("d"));
        assert_eq!(source("text", None), None);
    }
}
