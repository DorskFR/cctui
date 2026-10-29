use base64::Engine;
use cctui_proto::media::sniff_media_type;
use image::ImageFormat;

pub const REDACTED: &str = "redacted";
pub const NOT_BASE64: &str = "not_base64";
pub const BAD_MAGIC: &str = "bad_magic";
pub const UNDECODABLE: &str = "undecodable";

pub const PLACEHOLDER: &str = "[REDACTED:";

const CHECKED: [&str; 5] = ["image/png", "image/jpeg", "image/jpg", "image/gif", "image/webp"];

#[derive(Debug, PartialEq, Eq)]
pub struct Valid {
    pub bytes: Vec<u8>,
    pub media_type: Option<String>,
}

fn format_of(bytes: &[u8]) -> Option<(ImageFormat, &'static str)> {
    match sniff_media_type("", bytes) {
        "image/png" => Some((ImageFormat::Png, "image/png")),
        "image/jpeg" => Some((ImageFormat::Jpeg, "image/jpeg")),
        "image/gif" => Some((ImageFormat::Gif, "image/gif")),
        "image/webp" => Some((ImageFormat::WebP, "image/webp")),
        _ => None,
    }
}

/// Bytes that claim to be a png/jpeg/gif/webp must be one and fully decode;
/// other media types only need to be base64.
pub fn check(declared: Option<&str>, data: &str) -> Result<Valid, &'static str> {
    if data.contains(PLACEHOLDER) {
        return Err(REDACTED);
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(|_| NOT_BASE64)?;
    check_bytes(declared, bytes)
}

pub fn check_bytes(declared: Option<&str>, bytes: Vec<u8>) -> Result<Valid, &'static str> {
    let Some((format, sniffed)) = format_of(&bytes) else {
        let claims_image =
            declared.is_some_and(|d| CHECKED.contains(&d.trim().to_ascii_lowercase().as_str()));
        if claims_image {
            return Err(BAD_MAGIC);
        }
        return Ok(Valid { bytes, media_type: declared.map(str::to_owned) });
    };
    image::load_from_memory_with_format(&bytes, format).map_err(|_| UNDECODABLE)?;
    Ok(Valid { bytes, media_type: Some(sniffed.to_owned()) })
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::image_blobs::tests::{photo, screenshot};

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn encoded(format: ImageFormat) -> Vec<u8> {
        let px = |v: u32| u8::try_from(v * 10).unwrap();
        let img = image::RgbaImage::from_fn(24, 24, |x, y| image::Rgba([px(x), px(y), 7, 255]));
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, format).unwrap();
        out.into_inner()
    }

    #[test]
    fn each_supported_format_is_valid_under_its_own_type() {
        let cases = [
            (screenshot(1, 5000), "image/png"),
            (photo(2, 5000), "image/jpeg"),
            (encoded(ImageFormat::Gif), "image/gif"),
            (encoded(ImageFormat::WebP), "image/webp"),
        ];
        for (bytes, mt) in cases {
            let v = check(Some(mt), &b64(&bytes)).unwrap_or_else(|e| panic!("{mt}: {e}"));
            assert_eq!(v.bytes, bytes);
            assert_eq!(v.media_type.as_deref(), Some(mt));
        }
    }

    #[test]
    fn a_mislabelled_image_is_valid_under_what_it_is() {
        let v = check(Some("image/png"), &b64(&photo(3, 5000))).unwrap();
        assert_eq!(v.media_type.as_deref(), Some("image/jpeg"));
    }

    #[test]
    fn corrupt_sources_say_why() {
        let png = screenshot(4, 5000);
        let mut redacted = b64(&png);
        redacted.replace_range(100..140, "[REDACTED:square_token:9f2a]");
        assert_eq!(check(Some("image/png"), &redacted), Err(REDACTED));
        assert_eq!(check(Some("image/png"), "not base64 at all!"), Err(NOT_BASE64));
        assert_eq!(check(Some("image/png"), &b64(&[7u8; 2048])), Err(BAD_MAGIC));
        let mut header_only = png[..8].to_vec();
        header_only.extend_from_slice(&[0x5a; 4000]);
        assert_eq!(check(Some("image/png"), &b64(&header_only)), Err(UNDECODABLE));
        assert_eq!(check(Some("image/png"), &b64(&png[..png.len() / 2])), Err(UNDECODABLE));
        let mut flipped = png.clone();
        flipped[png.len() / 2] ^= 0xff;
        assert_eq!(check(Some("image/png"), &b64(&flipped)), Err(UNDECODABLE), "a bad CRC");
    }

    #[test]
    fn other_media_only_need_to_be_base64() {
        let doc = b"%PDF-1.7 not really".repeat(100);
        let v = check(Some("application/pdf"), &b64(&doc)).unwrap();
        assert_eq!(v.media_type.as_deref(), Some("application/pdf"));
        assert!(check(None, &b64(&[1u8; 2048])).is_ok());
    }
}
