use std::collections::HashMap;

use base64::Engine;
use image::DynamicImage;

use crate::net;

#[derive(Clone)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Default)]
pub struct ImageStore {
    cache: HashMap<String, Option<Bitmap>>,
}

impl ImageStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load(&mut self, url: &str) -> Option<Bitmap> {
        if let Some(cached) = self.cache.get(url) {
            return cached.clone();
        }
        let decoded = decode_url(url).ok();
        self.cache.insert(url.to_string(), decoded.clone());
        decoded
    }
}

fn decode_url(url: &str) -> Result<Bitmap, String> {
    if let Some(bitmap) = decode_data_url(url) {
        return Ok(bitmap);
    }
    let response = net::fetch_bytes(url, 8 * 1024 * 1024).map_err(|err| err.to_string())?;
    if !is_raster(&response.bytes, response.content_type.as_deref()) {
        return Err("不是 PNG 或 JPEG".to_string());
    }
    decode_raster(&response.bytes)
}

fn is_raster(bytes: &[u8], content_type: Option<&str>) -> bool {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") || bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return true;
    }
    content_type.is_some_and(|mime| {
        let mime = mime.split(';').next().unwrap_or("").trim();
        mime.eq_ignore_ascii_case("image/png") || mime.eq_ignore_ascii_case("image/jpeg")
    })
}

pub fn decode_raster(bytes: &[u8]) -> Result<Bitmap, String> {
    let image = image::load_from_memory(bytes).map_err(|err| err.to_string())?;
    if image.width() == 0 || image.height() == 0 || image.width() > 4096 || image.height() > 4096 {
        return Err("图片尺寸无法显示".to_string());
    }
    let rgba = match image {
        DynamicImage::ImageRgba8(buffer) => buffer,
        other => other.to_rgba8(),
    };
    Ok(Bitmap {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

fn decode_data_url(url: &str) -> Option<Bitmap> {
    let rest = url.trim().strip_prefix("data:")?;
    let (meta, data) = rest.split_once(',')?;
    let meta = meta.to_ascii_lowercase();
    if !meta.starts_with("image/png") && !meta.starts_with("image/jpeg") && !meta.starts_with("image/jpg") {
        return None;
    }
    let bytes = if meta.contains(";base64") {
        base64::engine::general_purpose::STANDARD.decode(data.trim()).ok()?
    } else {
        return None;
    };
    decode_raster(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, Rgba, RgbaImage};

    #[test]
    fn decodes_a_png_data_url() {
        let image = RgbaImage::from_pixel(2, 1, Rgba([10, 20, 30, 255]));
        let mut bytes = Vec::new();
        DynamicImage::ImageRgba8(image)
            .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        let bitmap = decode_data_url(&format!("data:image/png;base64,{encoded}")).unwrap();
        assert_eq!((bitmap.width, bitmap.height), (2, 1));
        assert_eq!(&bitmap.rgba[..4], &[10, 20, 30, 255]);
    }
}
