use cosmic_text::{Color, FontSystem, SwashCache};

use crate::engine::layout::{ImageBox, Scene};

pub fn paint_scene(
    scene: &mut Scene,
    scroll: f32,
    chrome: f32,
    pixels: &mut [u32],
    width: u32,
    height: u32,
    font_system: &mut FontSystem,
    cache: &mut SwashCache,
) {
    pixels.fill(pack(scene.background));
    for fill in &scene.fills {
        fill_rect(pixels, width, height, fill.x, fill.y - scroll + chrome, fill.w, fill.h, pack(fill.color));
    }
    for image in &scene.images {
        blit_image(pixels, width, height, image, scroll, chrome);
    }
    for text in &mut scene.texts {
        let origin_x = text.x.round() as i32;
        let origin_y = (text.y - scroll + chrome).round() as i32;
        text.buffer.draw(
            font_system,
            cache,
            Color::rgb(0, 0, 0),
            |x, y, glyph_width, glyph_height, pixel| {
                blit(
                    pixels,
                    width,
                    height,
                    origin_x + x,
                    origin_y + y,
                    glyph_width,
                    glyph_height,
                    pixel,
                );
            },
        );
    }
}

fn blit_image(pixels: &mut [u32], width: u32, height: u32, image: &ImageBox, scroll: f32, chrome: f32) {
    if image.pixel_width == 0 || image.pixel_height == 0 || image.w < 1.0 || image.h < 1.0 {
        return;
    }
    let dest_w = image.w.round().max(1.0) as i32;
    let dest_h = image.h.round().max(1.0) as i32;
    let origin_x = image.x.round() as i32;
    let origin_y = (image.y - scroll + chrome).round() as i32;
    for row in 0..dest_h {
        for col in 0..dest_w {
            let x = origin_x + col;
            let y = origin_y + row;
            if x < 0 || y < 0 || x as u32 >= width || y as u32 >= height {
                continue;
            }
            let src_x = (col as u32 * image.pixel_width) / dest_w as u32;
            let src_y = (row as u32 * image.pixel_height) / dest_h as u32;
            let index = ((src_y * image.pixel_width + src_x) * 4) as usize;
            if index + 3 >= image.rgba.len() {
                continue;
            }
            let color = Color::rgba(image.rgba[index], image.rgba[index + 1], image.rgba[index + 2], image.rgba[index + 3]);
            let pixel_index = (y as u32 * width + x as u32) as usize;
            pixels[pixel_index] = blend(pixels[pixel_index], color);
        }
    }
}

fn fill_rect(pixels: &mut [u32], width: u32, height: u32, x: f32, y: f32, w: f32, h: f32, pixel: u32) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let x0 = x.floor().max(0.0) as i32;
    let y0 = y.floor().max(0.0) as i32;
    let x1 = ((x + w).ceil() as i32).min(width as i32);
    let y1 = ((y + h).ceil() as i32).min(height as i32);
    for row in y0..y1 {
        for col in x0..x1 {
            pixels[(row as u32 * width + col as u32) as usize] = pixel;
        }
    }
}

fn blit(
    pixels: &mut [u32],
    width: u32,
    height: u32,
    origin_x: i32,
    origin_y: i32,
    glyph_width: u32,
    glyph_height: u32,
    color: Color,
) {
    for row in 0..glyph_height {
        for col in 0..glyph_width {
            let x = origin_x + col as i32;
            let y = origin_y + row as i32;
            if x < 0 || y < 0 || x as u32 >= width || y as u32 >= height {
                continue;
            }
            let index = (y as u32 * width + x as u32) as usize;
            pixels[index] = blend(pixels[index], color);
        }
    }
}

fn pack(color: Color) -> u32 {
    let (r, g, b, _) = color.as_rgba_tuple();
    ((r as u32) << 16) | ((g as u32) << 8) | b as u32
}

fn blend(dst: u32, src: Color) -> u32 {
    let (sr, sg, sb, sa) = src.as_rgba_tuple();
    if sa == 0 {
        return dst;
    }
    if sa == 255 {
        return ((sr as u32) << 16) | ((sg as u32) << 8) | sb as u32;
    }
    let dr = (dst >> 16) & 0xff;
    let dg = (dst >> 8) & 0xff;
    let db = dst & 0xff;
    let alpha = sa as u32;
    let inverse = 255 - alpha;
    let r = (sr as u32 * alpha + dr * inverse + 127) / 255;
    let g = (sg as u32 * alpha + dg * inverse + 127) / 255;
    let b = (sb as u32 * alpha + db * inverse + 127) / 255;
    (r << 16) | (g << 8) | b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::html::parse;
    use crate::engine::layout::layout;

    #[test]
    fn draws_styled_text_and_scrolls_it_away() {
        let doc = parse("<h1 style=\"color:#000\">Hi 你好</h1>");
        let mut font_system = FontSystem::new();
        let mut cache = SwashCache::new();
        let mut images = crate::engine::image::ImageStore::new();
        let mut scene = layout(&doc, 320.0, 200.0, None, &mut images, &mut font_system);
        let width = 320;
        let height = 120;
        let mut pixels = vec![0; width * height];
        paint_scene(&mut scene, 0.0, 0.0, &mut pixels, width as u32, height as u32, &mut font_system, &mut cache);
        assert!(pixels.iter().any(|pixel| *pixel != 0x00FF_FFFF));

        pixels.fill(0);
        paint_scene(&mut scene, 4000.0, 0.0, &mut pixels, width as u32, height as u32, &mut font_system, &mut cache);
        assert!(pixels.iter().all(|pixel| *pixel == 0x00FF_FFFF));
    }
}
