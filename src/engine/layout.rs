use cosmic_text::{Align, Attrs, Buffer, Color, FontSystem, Metrics, Shaping, UnderlineStyle, Wrap};

use crate::engine::dom::{Document, NodeData, NodeId};
use crate::engine::image::{Bitmap, ImageStore};
use crate::engine::style::{self, Computed, Display, Stylesheet, TextAlign, Viewport};
use crate::net::url::resolve_url;

const LINE_SCALE: f32 = 1.4;

#[derive(Debug)]
pub struct Fill {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub color: Color,
}

pub struct TextBox {
    pub x: f32,
    pub y: f32,
    pub hit_x: f32,
    pub hit_w: f32,
    pub h: f32,
    pub href: Option<String>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub text: String,
    pub buffer: Buffer,
}

pub struct ImageBox {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub rgba: Vec<u8>,
    pub href: Option<String>,
}

pub struct Scene {
    pub background: Color,
    background_is_default: bool,
    pub height: f32,
    pub fills: Vec<Fill>,
    pub texts: Vec<TextBox>,
    pub images: Vec<ImageBox>,
}

impl Scene {
    pub fn href_at(&self, x: f32, y: f32) -> Option<&str> {
        for image in self.images.iter().rev() {
            if image.href.is_some() && contains(x, y, image.x, image.y, image.w, image.h) {
                return image.href.as_deref();
            }
        }
        for text in self.texts.iter().rev() {
            if text.href.is_some() && contains(x, y, text.hit_x, text.y, text.hit_w, text.h) {
                return text.href.as_deref();
            }
        }
        None
    }
}

fn contains(x: f32, y: f32, left: f32, top: f32, width: f32, height: f32) -> bool {
    x >= left && y >= top && x < left + width && y < top + height
}

pub fn layout(
    doc: &Document,
    viewport_width: f32,
    viewport_height: f32,
    base_url: Option<&str>,
    images: &mut ImageStore,
    font_system: &mut FontSystem,
) -> Scene {
    let sheet = style::stylesheet(doc);
    let viewport = Viewport {
        width: viewport_width,
        height: viewport_height,
    };
    let mut scene = Scene {
        background: Color::rgb(0xFF, 0xFF, 0xFF),
        background_is_default: true,
        height: 0.0,
        fills: Vec::new(),
        texts: Vec::new(),
        images: Vec::new(),
    };
    let mut flow = Flow {
        base_url,
        images,
        link: None,
    };
    let bottom = layout_block_children(
        doc,
        doc.root(),
        0.0,
        0.0,
        viewport_width.max(1.0),
        false,
        &style::root_style(),
        &sheet,
        viewport,
        &mut scene,
        font_system,
        &mut flow,
    );
    scene.height = bottom.max(0.0);
    scene
}

struct Flow<'a> {
    base_url: Option<&'a str>,
    images: &'a mut ImageStore,
    link: Option<String>,
}

pub fn clamp_scroll(scroll: f32, content_height: f32, view_height: f32) -> f32 {
    let max_scroll = (content_height - view_height).max(0.0);
    scroll.clamp(0.0, max_scroll)
}

struct InlineBuilder {
    spans: Vec<Span>,
    pending_space: bool,
}

struct Span {
    text: String,
    color: Color,
    font_size: f32,
}

impl InlineBuilder {
    fn new() -> Self {
        Self {
            spans: Vec::new(),
            pending_space: false,
        }
    }

    fn push_text(&mut self, raw: &str, style: &Computed, preserve: bool) {
        if preserve {
            self.flush_space(style);
            self.append(raw, style);
            return;
        }
        let leading = raw.chars().next().is_some_and(char::is_whitespace);
        let trailing = raw.chars().next_back().is_some_and(char::is_whitespace);
        let collapsed = collapse(raw);
        if collapsed.is_empty() {
            if leading || trailing {
                self.pending_space = true;
            }
            return;
        }
        if leading {
            self.pending_space = true;
        }
        self.flush_space(style);
        self.append(&collapsed, style);
        if trailing {
            self.pending_space = true;
        }
    }

    fn newline(&mut self) {
        self.pending_space = false;
        if let Some(span) = self.spans.last_mut() {
            span.text.push('\n');
        } else {
            self.spans.push(Span {
                text: "\n".to_string(),
                color: Color::rgb(0, 0, 0),
                font_size: 16.0,
            });
        }
    }

    fn flush_space(&mut self, style: &Computed) {
        if !self.pending_space || self.spans.is_empty() {
            self.pending_space = false;
            return;
        }
        self.pending_space = false;
        if self.spans.last().is_some_and(|span| span.text.ends_with([' ', '\n'])) {
            return;
        }
        self.append(" ", style);
    }

    fn append(&mut self, text: &str, style: &Computed) {
        if text.is_empty() {
            return;
        }
        if let Some(span) = self.spans.last_mut() {
            if span.color.as_rgba() == style.color.as_rgba() && (span.font_size - style.font_size).abs() < 0.1 {
                span.text.push_str(text);
                return;
            }
        }
        self.spans.push(Span {
            text: text.to_string(),
            color: style.color,
            font_size: style.font_size,
        });
    }

    fn finish(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        align: TextAlign,
        href: Option<String>,
        font_system: &mut FontSystem,
        scene: &mut Scene,
    ) -> f32 {
        self.pending_space = false;
        if !self.spans.iter().any(|span| span.text.contains('\n') || span.text.chars().any(|ch| !ch.is_whitespace()))
        {
            self.spans.clear();
            return y;
        }
        let font_size = self.spans.iter().map(|span| span.font_size).fold(16.0_f32, f32::max);
        let metrics = Metrics::new(font_size, (font_size * LINE_SCALE).max(1.0));
        let mut buffer = Buffer::new(font_system, metrics);
        buffer.set_size(Some(width.max(1.0)), None);
        buffer.set_wrap(Wrap::Word);
        let mut default_attrs = Attrs::new().metrics(metrics);
        if href.is_some() {
            default_attrs = default_attrs.underline(UnderlineStyle::Single);
        }
        let alignment = match align {
            TextAlign::Left => Align::Left,
            TextAlign::Center => Align::Center,
            TextAlign::Right => Align::Right,
        };
        let link = href.clone();
        buffer.set_rich_text(
            self.spans.iter().map(|span| {
                let mut attrs = Attrs::new().color(span.color).metrics(Metrics::new(
                    span.font_size,
                    (span.font_size * LINE_SCALE).max(1.0),
                ));
                if link.is_some() {
                    attrs = attrs.underline(UnderlineStyle::Single);
                }
                (span.text.as_str(), attrs)
            }),
            &default_attrs,
            Shaping::Advanced,
            Some(alignment),
        );
        buffer.shape_until_scroll(font_system, false);
        let mut height = 0.0;
        let mut min_x = f32::MAX;
        let mut max_x = 0.0_f32;
        for run in buffer.layout_runs() {
            height += run.line_height;
            for glyph in run.glyphs {
                min_x = min_x.min(glyph.x);
                max_x = max_x.max(glyph.x + glyph.w);
            }
        }
        if height <= 0.0 {
            height = metrics.line_height;
        }
        let (hit_x, hit_w) = if min_x == f32::MAX {
            (x, width)
        } else {
            (x + min_x, (max_x - min_x).max(4.0))
        };
        let text = self.spans.iter().map(|span| span.text.as_str()).collect::<String>();
        self.spans.clear();
        scene.texts.push(TextBox {
            x,
            y,
            hit_x,
            hit_w,
            h: height,
            href,
            text,
            buffer,
        });
        y + height
    }
}

fn collapse(raw: &str) -> String {
    let mut out = String::new();
    let mut pending_space = false;
    for ch in raw.chars() {
        if ch.is_whitespace() {
            pending_space = true;
            continue;
        }
        if pending_space && !out.is_empty() {
            out.push(' ');
        }
        pending_space = false;
        out.push(ch);
    }
    out
}

fn layout_block_children(
    doc: &Document,
    parent: NodeId,
    x: f32,
    mut y: f32,
    width: f32,
    preserve: bool,
    parent_style: &Computed,
    sheet: &Stylesheet,
    viewport: Viewport,
    scene: &mut Scene,
    font_system: &mut FontSystem,
    flow: &mut Flow<'_>,
) -> f32 {
    let mut inline = InlineBuilder::new();
    y = walk(
        doc,
        parent,
        x,
        y,
        width,
        preserve,
        parent_style,
        sheet,
        viewport,
        scene,
        font_system,
        flow,
        &mut inline,
    );
    inline.finish(x, y, width, parent_style.text_align, flow.link.clone(), font_system, scene)
}

fn walk(
    doc: &Document,
    parent: NodeId,
    x: f32,
    mut y: f32,
    width: f32,
    preserve: bool,
    parent_style: &Computed,
    sheet: &Stylesheet,
    viewport: Viewport,
    scene: &mut Scene,
    font_system: &mut FontSystem,
    flow: &mut Flow<'_>,
    inline: &mut InlineBuilder,
) -> f32 {
    for child in doc.nodes[parent.0].children.clone() {
        match &doc.nodes[child.0].data {
            NodeData::Text(text) => inline.push_text(text, parent_style, preserve),
            NodeData::Document => {}
            NodeData::Element { tag, attrs } => {
                let tag = tag.clone();
                let attrs = attrs.clone();
                if tag == "br" {
                    inline.newline();
                    continue;
                }
                let style = style::compute(&tag, &attrs, parent_style, sheet, viewport, width);
                if style.display == Display::None {
                    continue;
                }
                if tag == "img" {
                    y = inline.finish(x, y, width, parent_style.text_align, flow.link.clone(), font_system, scene);
                    y = place_image(x, y, width, &attrs, &style, scene, flow);
                    continue;
                }
                if tag == "a" {
                    y = inline.finish(x, y, width, parent_style.text_align, flow.link.clone(), font_system, scene);
                    let previous = flow.link.clone();
                    flow.link = attr(&attrs, "href").and_then(|href| resolve_url(flow.base_url, href));
                    let mut inner = InlineBuilder::new();
                    y = walk(
                        doc,
                        child,
                        x,
                        y,
                        width,
                        preserve,
                        &style,
                        sheet,
                        viewport,
                        scene,
                        font_system,
                        flow,
                        &mut inner,
                    );
                    y = inner.finish(x, y, width, style.text_align, flow.link.clone(), font_system, scene);
                    flow.link = previous;
                    continue;
                }
                match style.display {
                    Display::None => {}
                    Display::Inline => {
                        y = walk(
                            doc,
                            child,
                            x,
                            y,
                            width,
                            preserve,
                            &style,
                            sheet,
                            viewport,
                            scene,
                            font_system,
                            flow,
                            inline,
                        );
                    }
                    Display::Block => {
                        y = inline.finish(x, y, width, parent_style.text_align, flow.link.clone(), font_system, scene);
                        y = layout_block(
                            doc,
                            child,
                            &tag,
                            &style,
                            x,
                            y,
                            width,
                            preserve,
                            sheet,
                            viewport,
                            scene,
                            font_system,
                            flow,
                        );
                    }
                }
            }
        }
    }
    y
}

fn layout_block(
    doc: &Document,
    id: NodeId,
    tag: &str,
    style: &Computed,
    x: f32,
    y: f32,
    available_width: f32,
    preserve: bool,
    sheet: &Stylesheet,
    viewport: Viewport,
    scene: &mut Scene,
    font_system: &mut FontSystem,
    flow: &mut Flow<'_>,
) -> f32 {
    let [mt, mr, mb, ml] = style.margin;
    let [pt, pr, pb, pl] = style.padding;
    let border_x = x + ml;
    let border_w = (available_width - ml - mr).max(1.0);
    let content_x = border_x + pl;
    let content_w = (border_w - pl - pr).max(1.0);
    let top = y + mt;
    let content_y = top + pt;
    let fill_index = style.background.map(|color| {
        scene.fills.push(Fill {
            x: border_x,
            y: top,
            w: border_w,
            h: 0.0,
            color,
        });
        scene.fills.len() - 1
    });
    if matches!(tag, "html" | "body") {
        if let Some(color) = style.background {
            if tag == "html" || scene.background_is_default {
                scene.background = color;
                scene.background_is_default = false;
            }
        }
    }
    let content_bottom = layout_block_children(
        doc,
        id,
        content_x,
        content_y,
        content_w,
        preserve || tag == "pre",
        style,
        sheet,
        viewport,
        scene,
        font_system,
        flow,
    );
    let border_bottom = content_bottom + pb;
    if let Some(index) = fill_index {
        scene.fills[index].h = (border_bottom - top).max(0.0);
    }
    border_bottom + mb
}

fn place_image(x: f32, y: f32, width: f32, attrs: &[(String, String)], style: &Computed, scene: &mut Scene, flow: &mut Flow<'_>) -> f32 {
    let gap = 8.0;
    let Some(src) = attr(attrs, "src") else {
        return y;
    };
    let Some(url) = resolve_url(flow.base_url, src) else {
        return y;
    };
    let Some(bitmap) = flow.images.load(&url) else {
        let box_w = 160.0_f32.min(width);
        scene.fills.push(Fill {
            x,
            y,
            w: box_w,
            h: 36.0,
            color: Color::rgb(0xE6, 0xE6, 0xE6),
        });
        return y + 36.0 + gap;
    };
    let (draw_w, draw_h) = image_size(&bitmap, attr(attrs, "width"), attr(attrs, "height"), width - style.margin[1] - style.margin[3]);
    scene.images.push(ImageBox {
        x,
        y,
        w: draw_w,
        h: draw_h,
        pixel_width: bitmap.width,
        pixel_height: bitmap.height,
        rgba: bitmap.rgba,
        href: flow.link.clone(),
    });
    y + draw_h + gap
}

fn image_size(bitmap: &Bitmap, width_attr: Option<&str>, height_attr: Option<&str>, max_width: f32) -> (f32, f32) {
    let intrinsic_w = bitmap.width as f32;
    let intrinsic_h = bitmap.height as f32;
    let attr_w = width_attr.and_then(parse_px);
    let attr_h = height_attr.and_then(parse_px);
    let (mut w, mut h) = match (attr_w, attr_h) {
        (Some(w), Some(h)) => (w, h),
        (Some(w), None) => (w, intrinsic_h * w / intrinsic_w.max(1.0)),
        (None, Some(h)) => (intrinsic_w * h / intrinsic_h.max(1.0), h),
        (None, None) => (intrinsic_w, intrinsic_h),
    };
    if w > max_width && w > 0.0 {
        h *= max_width / w;
        w = max_width;
    }
    (w.max(1.0), h.max(1.0))
}

fn parse_px(value: &str) -> Option<f32> {
    let number: f32 = value.trim().trim_end_matches("px").trim().parse().ok()?;
    if number > 0.0 { Some(number) } else { None }
}

fn attr<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::html::parse;
    use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};

    fn lay_out(html: &str, width: f32, height: f32, base: Option<&str>) -> Scene {
        let doc = parse(html);
        let mut images = ImageStore::new();
        let mut font_system = FontSystem::new();
        layout(&doc, width, height, base, &mut images, &mut font_system)
    }

    #[test]
    fn headings_and_paragraphs_stack() {
        let scene = lay_out("<h1>Title</h1><p>Body <a href=\"#\">link</a></p><p style=\"display:none\">hidden</p>", 640.0, 480.0, None);
        assert!(scene.texts.iter().any(|text| (text.buffer.metrics().font_size - 32.0).abs() < 0.1 && text.text.contains("Title")));
        let body = scene.texts.iter().find(|text| text.text.contains("Body")).unwrap();
        let link = scene.texts.iter().find(|text| text.text.contains("link")).unwrap();
        assert!(link.y >= body.y);
        assert!(scene.texts.iter().all(|text| !text.text.contains("hidden")));
    }

    #[test]
    fn example_stylesheet_pads_and_keeps_text() {
        let scene = lay_out(
            r#"<style>body{margin:auto;padding:25vh 2em 2em;text-align:center}</style><body><p>Hi</p></body>"#,
            800.0,
            600.0,
            None,
        );
        let text = scene.texts.iter().find(|text| text.text.contains("Hi")).unwrap();
        assert!(text.y > 150.0, "y={}", text.y);
        assert!(text.y < 220.0, "y={}", text.y);
    }

    #[test]
    fn link_hit_target_uses_the_resolved_url() {
        let scene = lay_out(
            r#"<p>before <a href="/next">go</a> after</p>"#,
            640.0,
            200.0,
            Some("https://example.com/dir/page"),
        );
        let link = scene.texts.iter().find(|text| text.text.contains("go")).unwrap();
        assert_eq!(link.href.as_deref(), Some("https://example.com/next"));
        assert_eq!(scene.href_at(link.hit_x + 1.0, link.y + 1.0), Some("https://example.com/next"));
    }

    #[test]
    fn png_data_url_becomes_an_image_box() {
        let image = RgbaImage::from_pixel(8, 4, Rgba([1, 2, 3, 255]));
        let mut bytes = Vec::new();
        DynamicImage::ImageRgba8(image)
            .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes);
        let scene = lay_out(&format!(r#"<img src="data:image/png;base64,{encoded}" width="8" height="4">"#), 200.0, 200.0, None);
        assert_eq!(scene.images.len(), 1);
        assert_eq!((scene.images[0].w, scene.images[0].h), (8.0, 4.0));
    }

    #[test]
    fn scroll_stops_at_the_document_ends() {
        assert_eq!(clamp_scroll(-10.0, 1000.0, 200.0), 0.0);
        assert_eq!(clamp_scroll(900.0, 1000.0, 200.0), 800.0);
        assert_eq!(clamp_scroll(40.0, 100.0, 200.0), 0.0);
    }
}
