use cosmic_text::Color;

use crate::engine::dom::{Document, NodeData};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Display {
    Block,
    Inline,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug)]
enum Length {
    Auto,
    Px(f32),
    Em(f32),
    Rem(f32),
    Percent(f32),
    Vh(f32),
    Vw(f32),
}

#[derive(Clone, Copy, Debug)]
enum FontSize {
    Px(f32),
    Em(f32),
    Rem(f32),
    Percent(f32),
}

#[derive(Clone, Copy, Debug, Default)]
struct Edges {
    top: Option<Length>,
    right: Option<Length>,
    bottom: Option<Length>,
    left: Option<Length>,
}

#[derive(Clone, Debug, Default)]
struct Declared {
    color: Option<Color>,
    background: Option<Option<Color>>,
    font_size: Option<FontSize>,
    margin: Edges,
    padding: Edges,
    text_align: Option<TextAlign>,
    display: Option<Display>,
}

#[derive(Clone, Debug)]
struct Rule {
    tags: Vec<String>,
    declared: Declared,
}

#[derive(Clone, Debug)]
pub struct Stylesheet {
    rules: Vec<Rule>,
}

#[derive(Clone, Copy, Debug)]
pub struct Viewport {
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Computed {
    pub display: Display,
    pub color: Color,
    pub background: Option<Color>,
    pub font_size: f32,
    pub margin: [f32; 4],
    pub padding: [f32; 4],
    pub text_align: TextAlign,
}

pub fn stylesheet(doc: &Document) -> Stylesheet {
    let mut css = String::new();
    for node in &doc.nodes {
        let NodeData::Element { tag, .. } = &node.data else {
            continue;
        };
        if tag != "style" {
            continue;
        }
        for child in &node.children {
            if let NodeData::Text(text) = &doc.nodes[child.0].data {
                css.push_str(text);
                css.push('\n');
            }
        }
    }
    Stylesheet {
        rules: parse_stylesheet(&css),
    }
}

pub fn root_style() -> Computed {
    Computed {
        display: Display::Block,
        color: Color::rgb(0x1A, 0x1A, 0x1A),
        background: None,
        font_size: 16.0,
        margin: [0.0; 4],
        padding: [0.0; 4],
        text_align: TextAlign::Left,
    }
}

pub fn compute(
    tag: &str,
    attrs: &[(String, String)],
    parent: &Computed,
    sheet: &Stylesheet,
    viewport: Viewport,
    containing_width: f32,
) -> Computed {
    let mut style = parent.inherited();
    apply_user_agent(&mut style, tag, parent.font_size);
    for rule in sheet.rules.iter().filter(|rule| rule.tags.iter().any(|name| name == tag)) {
        apply_declared(
            &mut style,
            &rule.declared,
            parent.font_size,
            viewport,
            containing_width,
        );
    }
    if let Some(inline) = attrs.iter().find(|(name, _)| name == "style").map(|(_, value)| value) {
        let declared = parse_declarations(inline);
        apply_declared(
            &mut style,
            &declared,
            parent.font_size,
            viewport,
            containing_width,
        );
    }
    style.font_size = style.font_size.max(1.0);
    style
}

impl Computed {
    fn inherited(self) -> Self {
        Self {
            display: Display::Inline,
            color: self.color,
            background: None,
            font_size: self.font_size,
            margin: [0.0; 4],
            padding: [0.0; 4],
            text_align: self.text_align,
        }
    }
}

fn apply_user_agent(style: &mut Computed, tag: &str, parent_font: f32) {
    style.display = default_display(tag);
    let declared = match tag {
        "h1" => ua_heading(2.0, 0.67),
        "h2" => ua_heading(1.5, 0.83),
        "h3" => ua_heading(1.17, 1.0),
        "h4" => ua_heading(1.0, 1.33),
        "h5" => ua_heading(0.83, 1.67),
        "h6" => ua_heading(0.67, 2.33),
        "p" | "div" => {
            let mut declared = Declared::default();
            declared.margin.top = Some(Length::Em(1.0));
            declared.margin.bottom = Some(Length::Em(1.0));
            declared
        }
        "body" => {
            let mut declared = Declared::default();
            declared.margin = Edges {
                top: Some(Length::Px(8.0)),
                right: Some(Length::Px(8.0)),
                bottom: Some(Length::Px(8.0)),
                left: Some(Length::Px(8.0)),
            };
            declared
        }
        "a" => {
            let mut declared = Declared::default();
            declared.color = Some(Color::rgb(0x1A, 0x0D, 0xAB));
            declared
        }
        _ => Declared::default(),
    };
    apply_declared(
        style,
        &declared,
        parent_font,
        Viewport { width: 0.0, height: 0.0 },
        0.0,
    );
}

fn ua_heading(em: f32, margin_em: f32) -> Declared {
    let mut declared = Declared::default();
    declared.font_size = Some(FontSize::Em(em));
    declared.margin.top = Some(Length::Em(margin_em));
    declared.margin.bottom = Some(Length::Em(margin_em));
    declared
}

fn default_display(tag: &str) -> Display {
    if matches!(tag, "head" | "script" | "style" | "template" | "title") {
        return Display::None;
    }
    if is_block_tag(tag) {
        Display::Block
    } else {
        Display::Inline
    }
}

fn is_block_tag(tag: &str) -> bool {
    matches!(
        tag,
        "address"
            | "article"
            | "aside"
            | "blockquote"
            | "body"
            | "div"
            | "dl"
            | "fieldset"
            | "figcaption"
            | "figure"
            | "footer"
            | "form"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "header"
            | "hr"
            | "html"
            | "li"
            | "main"
            | "nav"
            | "ol"
            | "p"
            | "pre"
            | "section"
            | "table"
            | "tr"
            | "ul"
    )
}

fn apply_declared(
    style: &mut Computed,
    declared: &Declared,
    parent_font: f32,
    viewport: Viewport,
    containing_width: f32,
) {
    if let Some(color) = declared.color {
        style.color = color;
    }
    if let Some(background) = declared.background {
        style.background = background;
    }
    if let Some(display) = declared.display {
        style.display = display;
    }
    if let Some(text_align) = declared.text_align {
        style.text_align = text_align;
    }
    if let Some(font_size) = declared.font_size {
        style.font_size = resolve_font_size(font_size, parent_font).max(1.0);
    }
    resolve_edge(&declared.margin.top, &mut style.margin[0], style.font_size, viewport, containing_width, false);
    resolve_edge(&declared.margin.right, &mut style.margin[1], style.font_size, viewport, containing_width, false);
    resolve_edge(&declared.margin.bottom, &mut style.margin[2], style.font_size, viewport, containing_width, false);
    resolve_edge(&declared.margin.left, &mut style.margin[3], style.font_size, viewport, containing_width, false);
    resolve_edge(&declared.padding.top, &mut style.padding[0], style.font_size, viewport, containing_width, true);
    resolve_edge(&declared.padding.right, &mut style.padding[1], style.font_size, viewport, containing_width, true);
    resolve_edge(&declared.padding.bottom, &mut style.padding[2], style.font_size, viewport, containing_width, true);
    resolve_edge(&declared.padding.left, &mut style.padding[3], style.font_size, viewport, containing_width, true);
}

fn resolve_edge(
    length: &Option<Length>,
    slot: &mut f32,
    font_size: f32,
    viewport: Viewport,
    containing_width: f32,
    clamp_negative: bool,
) {
    let Some(length) = length else {
        return;
    };
    let mut value = match length {
        Length::Auto => 0.0,
        Length::Px(n) => *n,
        Length::Em(n) => *n * font_size,
        Length::Rem(n) => *n * 16.0,
        Length::Percent(n) => *n / 100.0 * containing_width,
        Length::Vh(n) => *n / 100.0 * viewport.height,
        Length::Vw(n) => *n / 100.0 * viewport.width,
    };
    if clamp_negative {
        value = value.max(0.0);
    }
    *slot = value;
}

fn resolve_font_size(size: FontSize, parent_font: f32) -> f32 {
    match size {
        FontSize::Px(n) => n,
        FontSize::Em(n) | FontSize::Percent(n) => parent_font * n,
        FontSize::Rem(n) => 16.0 * n,
    }
}

fn parse_stylesheet(css: &str) -> Vec<Rule> {
    let css = strip_comments(css);
    let mut rules = Vec::new();
    for chunk in css.split('}') {
        let Some((selector, body)) = chunk.split_once('{') else {
            continue;
        };
        let tags = selector
            .split(',')
            .filter_map(|part| simple_tag(part.trim()))
            .collect::<Vec<_>>();
        if tags.is_empty() {
            continue;
        }
        rules.push(Rule {
            tags,
            declared: parse_declarations(body),
        });
    }
    rules
}

fn simple_tag(selector: &str) -> Option<String> {
    if selector.is_empty() {
        return None;
    }
    if selector.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-') {
        Some(selector.to_ascii_lowercase())
    } else {
        None
    }
}

fn parse_declarations(block: &str) -> Declared {
    let mut declared = Declared::default();
    for part in block.split(';') {
        let Some((name, value)) = part.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value
            .trim()
            .trim_end_matches(|ch: char| ch.is_ascii_whitespace())
            .trim_end_matches("!important")
            .trim();
        apply_declaration(&mut declared, &name, value);
    }
    declared
}

fn apply_declaration(declared: &mut Declared, name: &str, value: &str) {
    match name {
        "color" => {
            if let Some(color) = parse_color(value) {
                declared.color = Some(color);
            }
        }
        "background" | "background-color" => {
            if value.eq_ignore_ascii_case("transparent") {
                declared.background = Some(None);
            } else if let Some(color) = parse_color(value) {
                declared.background = Some(Some(color));
            }
        }
        "font-size" => {
            if let Some(size) = parse_font_size(value) {
                declared.font_size = Some(size);
            }
        }
        "margin" => set_shorthand(&mut declared.margin, value),
        "margin-top" => declared.margin.top = parse_length(value),
        "margin-right" => declared.margin.right = parse_length(value),
        "margin-bottom" => declared.margin.bottom = parse_length(value),
        "margin-left" => declared.margin.left = parse_length(value),
        "padding" => set_shorthand(&mut declared.padding, value),
        "padding-top" => declared.padding.top = parse_length(value),
        "padding-right" => declared.padding.right = parse_length(value),
        "padding-bottom" => declared.padding.bottom = parse_length(value),
        "padding-left" => declared.padding.left = parse_length(value),
        "text-align" => {
            declared.text_align = match value.to_ascii_lowercase().as_str() {
                "left" | "start" => Some(TextAlign::Left),
                "center" => Some(TextAlign::Center),
                "right" | "end" => Some(TextAlign::Right),
                _ => None,
            };
        }
        "display" => {
            declared.display = match value.to_ascii_lowercase().as_str() {
                "block" => Some(Display::Block),
                "inline" => Some(Display::Inline),
                "none" => Some(Display::None),
                _ => None,
            };
        }
        _ => {}
    }
}

fn set_shorthand(edges: &mut Edges, value: &str) {
    let parts = value
        .split_whitespace()
        .filter_map(parse_length)
        .collect::<Vec<_>>();
    let tokens = value.split_whitespace().count();
    if parts.len() != tokens || parts.is_empty() || parts.len() > 4 {
        return;
    }
    let (top, right, bottom, left) = match parts.as_slice() {
        [all] => (*all, *all, *all, *all),
        [vertical, horizontal] => (*vertical, *horizontal, *vertical, *horizontal),
        [top, horizontal, bottom] => (*top, *horizontal, *bottom, *horizontal),
        [top, right, bottom, left] => (*top, *right, *bottom, *left),
        _ => return,
    };
    edges.top = Some(top);
    edges.right = Some(right);
    edges.bottom = Some(bottom);
    edges.left = Some(left);
}

fn parse_font_size(value: &str) -> Option<FontSize> {
    match value.to_ascii_lowercase().as_str() {
        "xx-small" => return Some(FontSize::Em(0.6)),
        "x-small" => return Some(FontSize::Em(0.75)),
        "small" => return Some(FontSize::Em(0.875)),
        "medium" => return Some(FontSize::Px(16.0)),
        "large" => return Some(FontSize::Em(1.25)),
        "x-large" => return Some(FontSize::Em(1.5)),
        "xx-large" => return Some(FontSize::Em(2.0)),
        _ => {}
    }
    let length = parse_length(value)?;
    match length {
        Length::Px(n) => Some(FontSize::Px(n)),
        Length::Em(n) => Some(FontSize::Em(n)),
        Length::Rem(n) => Some(FontSize::Rem(n)),
        Length::Percent(n) => Some(FontSize::Percent(n / 100.0)),
        _ => None,
    }
}

fn parse_length(value: &str) -> Option<Length> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("auto") {
        return Some(Length::Auto);
    }
    let mut index = 0;
    let bytes = value.as_bytes();
    if matches!(bytes.first(), Some(b'+' | b'-')) {
        index = 1;
    }
    let number_start = index;
    while index < bytes.len() && (bytes[index].is_ascii_digit() || bytes[index] == b'.') {
        index += 1;
    }
    if index == number_start {
        return None;
    }
    let number: f32 = value[..index].parse().ok()?;
    match &value[index..] {
        "" | "px" => Some(Length::Px(number)),
        "em" => Some(Length::Em(number)),
        "rem" => Some(Length::Rem(number)),
        "%" => Some(Length::Percent(number)),
        "vh" => Some(Length::Vh(number)),
        "vw" => Some(Length::Vw(number)),
        _ => None,
    }
}

fn parse_color(value: &str) -> Option<Color> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix('#') {
        return parse_hex(hex);
    }
    let lower = value.to_ascii_lowercase();
    if let Some(inner) = lower.strip_prefix("rgb(").and_then(|body| body.strip_suffix(')')) {
        let parts = inner
            .split(|ch: char| ch == ',' || ch.is_ascii_whitespace())
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        if parts.len() >= 3 {
            let r = parts[0].trim_end_matches('%').parse::<f32>().ok()?;
            let g = parts[1].trim_end_matches('%').parse::<f32>().ok()?;
            let b = parts[2].trim_end_matches('%').parse::<f32>().ok()?;
            return Some(Color::rgb(channel(r), channel(g), channel(b)));
        }
        return None;
    }
    Some(match lower.as_str() {
        "black" => Color::rgb(0, 0, 0),
        "white" => Color::rgb(255, 255, 255),
        "red" => Color::rgb(255, 0, 0),
        "green" => Color::rgb(0, 128, 0),
        "blue" => Color::rgb(0, 0, 255),
        "gray" | "grey" => Color::rgb(128, 128, 128),
        "silver" => Color::rgb(192, 192, 192),
        "navy" => Color::rgb(0, 0, 128),
        "maroon" => Color::rgb(128, 0, 0),
        "purple" => Color::rgb(128, 0, 128),
        "orange" => Color::rgb(255, 165, 0),
        "yellow" => Color::rgb(255, 255, 0),
        "transparent" => Color::rgba(0, 0, 0, 0),
        _ => return None,
    })
}

fn channel(value: f32) -> u8 {
    value.clamp(0.0, 255.0) as u8
}

fn parse_hex(hex: &str) -> Option<Color> {
    let digit = |byte: u8| -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    };
    let expand = |high: u8| high * 16 + high;
    let bytes = hex.as_bytes();
    match bytes.len() {
        3 | 4 => {
            let r = expand(digit(bytes[0])?);
            let g = expand(digit(bytes[1])?);
            let b = expand(digit(bytes[2])?);
            let a = if bytes.len() == 4 { expand(digit(bytes[3])?) } else { 255 };
            Some(Color::rgba(r, g, b, a))
        }
        6 | 8 => {
            let pair = |index: usize| -> Option<u8> {
                Some(digit(bytes[index])? * 16 + digit(bytes[index + 1])?)
            };
            let a = if bytes.len() == 8 { pair(6)? } else { 255 };
            Some(Color::rgba(pair(0)?, pair(2)?, pair(4)?, a))
        }
        _ => None,
    }
}

fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        rest = &rest[start + 2..];
        if let Some(end) = rest.find("*/") {
            rest = &rest[end + 2..];
        } else {
            rest = "";
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::html::parse;

    fn body_style(html: &str, viewport: Viewport) -> Computed {
        let doc = parse(html);
        let sheet = stylesheet(&doc);
        let body = doc.nodes.iter().find_map(|node| match &node.data {
            NodeData::Element { tag, attrs } if tag == "body" => Some(attrs.clone()),
            _ => None,
        });
        compute("body", body.as_deref().unwrap_or(&[]), &root_style(), &sheet, viewport, 800.0)
    }

    #[test]
    fn author_style_and_inline_override() {
        let viewport = Viewport { width: 800.0, height: 600.0 };
        let style = body_style(
            r#"<style>body { color: #abc; text-align: center; padding: 25vh 2em 2em; margin: auto }</style>
               <body style="color: rgb(1, 2, 3)"></body>"#,
            viewport,
        );
        assert_eq!(style.color.as_rgba(), [1, 2, 3, 255]);
        assert_eq!(style.text_align, TextAlign::Center);
        assert!((style.padding[0] - 150.0).abs() < 0.1);
        assert!((style.padding[1] - 32.0).abs() < 0.1);
        assert_eq!(style.margin, [0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn headings_are_larger_than_body_text() {
        let doc = parse("<h1>Title</h1>");
        let attrs = match &doc.nodes[doc.nodes[0].children[0].0].data {
            NodeData::Element { attrs, .. } => attrs.clone(),
            _ => panic!("missing h1"),
        };
        let style = compute("h1", &attrs, &root_style(), &stylesheet(&doc), Viewport { width: 800.0, height: 600.0 }, 800.0);
        assert!((style.font_size - 32.0).abs() < 0.1);
        assert_eq!(style.display, Display::Block);
    }

    #[test]
    fn display_none_and_link_color() {
        let parent = root_style();
        let hidden = compute(
            "p",
            &[("style".into(), "display:none".into())],
            &parent,
            &Stylesheet { rules: Vec::new() },
            Viewport { width: 100.0, height: 100.0 },
            100.0,
        );
        assert_eq!(hidden.display, Display::None);
        let link = compute(
            "a",
            &[],
            &parent,
            &Stylesheet { rules: Vec::new() },
            Viewport { width: 100.0, height: 100.0 },
            100.0,
        );
        assert_eq!(link.color.as_rgba(), [0x1A, 0x0D, 0xAB, 255]);
    }
}
