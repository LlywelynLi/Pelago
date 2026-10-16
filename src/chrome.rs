use cosmic_text::{Attrs, Buffer, Color, FontSystem, Metrics, Shaping, SwashCache};

pub const BAR_LOGICAL: f32 = 48.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarHit {
    Back,
    Reload,
    Address,
    Page,
}

pub struct Chrome {
    pub address: String,
    pub focused: bool,
    pub can_go_back: bool,
}

pub fn bar_px(scale: f64) -> f32 {
    (BAR_LOGICAL * scale as f32).max(1.0)
}

pub fn hit_test(x: f32, y: f32, bar_height: f32, width: f32, scale: f32) -> BarHit {
    if y >= bar_height {
        return BarHit::Page;
    }
    let metrics = button_metrics(scale, width);
    if contains(x, y, metrics.back) {
        return BarHit::Back;
    }
    if contains(x, y, metrics.reload) {
        return BarHit::Reload;
    }
    if contains(x, y, metrics.address) {
        return BarHit::Address;
    }
    BarHit::Page
}

pub fn paint(
    pixels: &mut [u32],
    width: u32,
    bar_height: f32,
    scale: f32,
    chrome: &Chrome,
    font_system: &mut FontSystem,
    cache: &mut SwashCache,
) {
    fill(pixels, width, 0, 0, width as i32, bar_height.ceil() as i32, 0x00F2_F2_F2);
    fill(pixels, width, 0, bar_height as i32 - 1, width as i32, 1, 0x00D0_D0_D0);
    let metrics = button_metrics(scale, width as f32);
    let back_color = if chrome.can_go_back { 0x00222222 } else { 0x00A0_A0_A0 };
    fill_rect(pixels, width, metrics.back, 0x00FF_FFFF);
    draw_left_arrow(pixels, width, metrics.back, back_color);
    fill_rect(pixels, width, metrics.reload, 0x00FF_FFFF);
    draw_reload(pixels, width, metrics.reload, 0x00222222);
    let border = if chrome.focused { 0x004A_7D_FF } else { 0x00C8_C8_C8 };
    fill_rect(pixels, width, metrics.address, 0x00FF_FFFF);
    stroke_rect(pixels, width, metrics.address, border);
    paint_address(pixels, width, metrics.address, chrome, scale, font_system, cache);
}

#[derive(Clone, Copy)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

struct ButtonMetrics {
    back: Rect,
    reload: Rect,
    address: Rect,
}

fn button_metrics(scale: f32, width: f32) -> ButtonMetrics {
    let margin = 8.0 * scale;
    let button = 32.0 * scale;
    let y = (BAR_LOGICAL * scale - button) / 2.0;
    let back = Rect { x: margin, y, w: button, h: button };
    let reload = Rect {
        x: back.x + button + margin,
        y,
        w: button,
        h: button,
    };
    let address_x = reload.x + button + margin;
    ButtonMetrics {
        back,
        reload,
        address: Rect {
            x: address_x,
            y,
            w: (width - address_x - margin).max(1.0),
            h: button,
        },
    }
}

fn contains(x: f32, y: f32, rect: Rect) -> bool {
    x >= rect.x && y >= rect.y && x < rect.x + rect.w && y < rect.y + rect.h
}

fn paint_address(
    pixels: &mut [u32],
    width: u32,
    field: Rect,
    chrome: &Chrome,
    scale: f32,
    font_system: &mut FontSystem,
    cache: &mut SwashCache,
) {
    let font_size = (15.0 * scale).max(1.0);
    let metrics = Metrics::new(font_size, font_size * 1.3);
    let mut buffer = Buffer::new(font_system, metrics);
    buffer.set_size(Some((field.w - 16.0 * scale).max(1.0)), Some(field.h));
    let attrs = Attrs::new().color(Color::rgb(0x22, 0x22, 0x22)).metrics(metrics);
    buffer.set_text(&chrome.address, &attrs, Shaping::Advanced, None);
    let origin_x = (field.x + 8.0 * scale).round() as i32;
    let origin_y = (field.y + (field.h - metrics.line_height) / 2.0).round() as i32;
    let clip = field;
    buffer.draw(font_system, cache, attrs.color_opt.unwrap_or(Color::rgb(0, 0, 0)), |x, y, w, h, color| {
        blit_clipped(pixels, width, origin_x + x, origin_y + y, w, h, color, clip);
    });
    if chrome.focused {
        let caret_x = origin_x + text_width(&buffer) as i32 + (2.0 * scale) as i32;
        fill(pixels, width, caret_x, origin_y, 2, metrics.line_height as i32, 0x00222222);
    }
}

fn text_width(buffer: &Buffer) -> f32 {
    let mut max_x = 0.0_f32;
    for run in buffer.layout_runs() {
        for glyph in run.glyphs {
            max_x = max_x.max(glyph.x + glyph.w);
        }
    }
    max_x
}

fn draw_left_arrow(pixels: &mut [u32], width: u32, rect: Rect, color: u32) {
    let cx = rect.x + rect.w / 2.0;
    let cy = rect.y + rect.h / 2.0;
    let size = rect.h * 0.22;
    for step in 0..=size as i32 {
        let dx = step as f32;
        plot(pixels, width, cx - size + dx, cy - dx, color);
        plot(pixels, width, cx - size + dx, cy + dx, color);
    }
}

fn draw_reload(pixels: &mut [u32], width: u32, rect: Rect, color: u32) {
    let cx = rect.x + rect.w / 2.0;
    let cy = rect.y + rect.h / 2.0;
    let radius = rect.h * 0.28;
    let mut angle = 0.0_f32;
    while angle < 5.4 {
        let x = cx + angle.cos() * radius;
        let y = cy + angle.sin() * radius;
        plot(pixels, width, x, y, color);
        angle += 0.15;
    }
    plot(pixels, width, cx + radius, cy, color);
    plot(pixels, width, cx + radius - 3.0, cy - 4.0, color);
    plot(pixels, width, cx + radius + 1.0, cy - 4.0, color);
}

fn fill_rect(pixels: &mut [u32], width: u32, rect: Rect, color: u32) {
    fill(
        pixels,
        width,
        rect.x.floor() as i32,
        rect.y.floor() as i32,
        rect.w.ceil() as i32,
        rect.h.ceil() as i32,
        color,
    );
}

fn stroke_rect(pixels: &mut [u32], width: u32, rect: Rect, color: u32) {
    let x = rect.x.floor() as i32;
    let y = rect.y.floor() as i32;
    let w = rect.w.ceil() as i32;
    let h = rect.h.ceil() as i32;
    fill(pixels, width, x, y, w, 1, color);
    fill(pixels, width, x, y + h - 1, w, 1, color);
    fill(pixels, width, x, y, 1, h, color);
    fill(pixels, width, x + w - 1, y, 1, h, color);
}

fn fill(pixels: &mut [u32], width: u32, x: i32, y: i32, w: i32, h: i32, color: u32) {
    for row in 0..h {
        for col in 0..w {
            plot(pixels, width, (x + col) as f32, (y + row) as f32, color);
        }
    }
}

fn plot(pixels: &mut [u32], width: u32, x: f32, y: f32, color: u32) {
    let x = x.round() as i32;
    let y = y.round() as i32;
    if x < 0 || y < 0 || x as u32 >= width {
        return;
    }
    let index = y as u32 * width + x as u32;
    if (index as usize) < pixels.len() {
        pixels[index as usize] = color;
    }
}

fn blit_clipped(
    pixels: &mut [u32],
    width: u32,
    origin_x: i32,
    origin_y: i32,
    glyph_width: u32,
    glyph_height: u32,
    color: Color,
    clip: Rect,
) {
    let (r, g, b, a) = color.as_rgba_tuple();
    if a == 0 {
        return;
    }
    for row in 0..glyph_height {
        for col in 0..glyph_width {
            let x = origin_x + col as i32;
            let y = origin_y + row as i32;
            if (x as f32) < clip.x || (y as f32) < clip.y || (x as f32) >= clip.x + clip.w || (y as f32) >= clip.y + clip.h {
                continue;
            }
            plot(pixels, width, x as f32, y as f32, ((r as u32) << 16) | ((g as u32) << 8) | b as u32);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolbar_hits_back_reload_and_address() {
        let scale = 1.0;
        let bar = bar_px(scale as f64);
        assert_eq!(hit_test(20.0, 24.0, bar, 800.0, scale), BarHit::Back);
        assert_eq!(hit_test(60.0, 24.0, bar, 800.0, scale), BarHit::Reload);
        assert_eq!(hit_test(200.0, 24.0, bar, 800.0, scale), BarHit::Address);
        assert_eq!(hit_test(20.0, bar + 2.0, bar, 800.0, scale), BarHit::Page);
    }
}
