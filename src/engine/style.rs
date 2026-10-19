use cosmic_text::Color;

use crate::engine::dom::{Document, NodeData, NodeId};
use crate::net::url::resolve_url;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Display {
    Block,
    Inline,
    InlineBlock,
    Flex,
    Grid,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlexDirection {
    Row,
    Column,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Justify {
    Start,
    Center,
    End,
    SpaceBetween,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlignItems {
    Stretch,
    Start,
    Center,
    End,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloatSide {
    None,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Position {
    Static,
    Relative,
    Absolute,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overflow {
    Visible,
    Hidden,
    Scroll,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoxSizing {
    ContentBox,
    BorderBox,
}

#[derive(Clone, Copy, Debug)]
pub enum Track {
    Px(f32),
    Percent(f32),
    Fr(f32),
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
    width: Option<Length>,
    height: Option<Length>,
    flex_direction: Option<FlexDirection>,
    justify_content: Option<Justify>,
    align_items: Option<AlignItems>,
    gap: Option<Length>,
    flex_grow: Option<f32>,
    float: Option<FloatSide>,
    position: Option<Position>,
    overflow: Option<Overflow>,
    opacity: Option<f32>,
    z_index: Option<i32>,
    min_width: Option<Length>,
    max_width: Option<Length>,
    min_height: Option<Length>,
    max_height: Option<Length>,
    visibility_hidden: Option<bool>,
    background_image: Option<Option<String>>,
    line_height: Option<Length>,
    box_sizing: Option<BoxSizing>,
    inset: Edges,
    grid_columns: Option<[Option<Track>; 6]>,
    grid_column_count: Option<u8>,
}

#[derive(Clone, Debug)]
struct Selector {
    steps: Vec<Compound>,
    /// `direct[i]` 为真表示 steps[i] 与 steps[i + 1] 之间是子代选择器。
    direct: Vec<bool>,
    specificity: (u32, u32, u32),
}

#[derive(Clone, Debug)]
struct Compound {
    simples: Vec<SimpleSel>,
}

#[derive(Clone, Debug)]
enum SimpleSel {
    Tag(String),
    Class(String),
    Id(String),
    Universal,
}

#[derive(Clone, Debug)]
struct Rule {
    selector: Selector,
    declared: Declared,
    order: u32,
}

#[derive(Clone, Debug)]
pub struct ElemSnap {
    pub tag: String,
    pub attrs: Vec<(String, String)>,
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

#[derive(Clone, Debug)]
pub struct Computed {
    pub display: Display,
    pub color: Color,
    pub background: Option<Color>,
    pub font_size: f32,
    pub margin: [f32; 4],
    pub padding: [f32; 4],
    pub text_align: TextAlign,
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub flex_direction: FlexDirection,
    pub justify_content: Justify,
    pub align_items: AlignItems,
    pub gap: f32,
    pub flex_grow: f32,
    pub float: FloatSide,
    pub position: Position,
    pub overflow: Overflow,
    pub opacity: f32,
    pub z_index: Option<i32>,
    pub min_width: Option<f32>,
    pub max_width: Option<f32>,
    pub min_height: Option<f32>,
    pub max_height: Option<f32>,
    pub visibility_hidden: bool,
    pub background_image: Option<String>,
    pub line_height: f32,
    pub box_sizing: BoxSizing,
    pub top: Option<f32>,
    pub right: Option<f32>,
    pub bottom: Option<f32>,
    pub left: Option<f32>,
    pub grid_columns: [Option<Track>; 6],
    pub grid_column_count: u8,
}

pub fn stylesheet(doc: &Document, base_url: Option<&str>) -> Stylesheet {
    stylesheet_with(doc, base_url, &|url| {
        crate::net::fetch_bytes(url, 1024 * 1024)
            .map(|response| String::from_utf8_lossy(&response.bytes).into_owned())
            .map_err(|err| err.to_string())
    })
}

pub fn stylesheet_with(
    doc: &Document,
    base_url: Option<&str>,
    fetch: &dyn Fn(&str) -> Result<String, String>,
) -> Stylesheet {
    let mut css = String::new();
    collect_css(doc, doc.root(), base_url, fetch, &mut css, 0);
    Stylesheet {
        rules: parse_stylesheet(&css),
    }
}

fn collect_css(
    doc: &Document,
    id: NodeId,
    base_url: Option<&str>,
    fetch: &dyn Fn(&str) -> Result<String, String>,
    css: &mut String,
    depth: u32,
) {
    let node = &doc.nodes[id.0];
    match &node.data {
        NodeData::Element { tag, attrs } if tag.eq_ignore_ascii_case("style") => {
            for child in &node.children {
                if let NodeData::Text(text) = &doc.nodes[child.0].data {
                    css.push_str(text);
                    css.push('\n');
                }
            }
        }
        NodeData::Element { tag, attrs } if tag.eq_ignore_ascii_case("link") && depth < 4 => {
            let rel = attr_value(attrs, "rel").unwrap_or("");
            let is_sheet = rel.split_whitespace().any(|part| part.eq_ignore_ascii_case("stylesheet"));
            if is_sheet {
                if let Some(href) = attr_value(attrs, "href") {
                    if let Some(url) = resolve_url(base_url, href) {
                        match fetch(&url) {
                            Ok(text) => append_css_imports(&text, Some(&url), fetch, css, depth + 1),
                            Err(err) => eprintln!("样式表加载失败 {url}：{err}"),
                        }
                    }
                }
            }
        }
        _ => {}
    }
    for child in node.children.clone() {
        collect_css(doc, child, base_url, fetch, css, depth);
    }
}

fn append_css_imports(
    text: &str,
    base_url: Option<&str>,
    fetch: &dyn Fn(&str) -> Result<String, String>,
    css: &mut String,
    depth: u32,
) {
    if depth >= 4 {
        css.push_str(text);
        css.push('\n');
        return;
    }
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(url) = import_url(trimmed) {
            if let Some(resolved) = resolve_url(base_url, url) {
                match fetch(&resolved) {
                    Ok(imported) => append_css_imports(&imported, Some(&resolved), fetch, css, depth + 1),
                    Err(err) => eprintln!("样式表加载失败 {resolved}：{err}"),
                }
            }
        }
    }
    css.push_str(text);
    css.push('\n');
}

fn import_url(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("@import")?.trim();
    let rest = rest.strip_suffix(';').unwrap_or(rest).trim();
    if let Some(inner) = rest.strip_prefix("url(") {
        let inner = inner.trim().trim_end_matches(')').trim();
        return Some(inner.trim_matches(['"', '\'']));
    }
    if rest.starts_with('"') || rest.starts_with('\'') {
        let quote = rest.chars().next()?;
        let body = rest[1..].split(quote).next()?;
        return Some(body);
    }
    None
}

fn attr_value<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.as_str())
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
        width: None,
        height: None,
        flex_direction: FlexDirection::Row,
        justify_content: Justify::Start,
        align_items: AlignItems::Stretch,
        gap: 0.0,
        flex_grow: 0.0,
        float: FloatSide::None,
        position: Position::Static,
        overflow: Overflow::Visible,
        opacity: 1.0,
        z_index: None,
        min_width: None,
        max_width: None,
        min_height: None,
        max_height: None,
        visibility_hidden: false,
        background_image: None,
        line_height: 22.4,
        box_sizing: BoxSizing::ContentBox,
        top: None,
        right: None,
        bottom: None,
        left: None,
        grid_columns: [None; 6],
        grid_column_count: 0,
    }
}

pub fn compute(
    tag: &str,
    attrs: &[(String, String)],
    ancestors: &[ElemSnap],
    parent: &Computed,
    sheet: &Stylesheet,
    viewport: Viewport,
    containing_width: f32,
) -> Computed {
    let mut style = parent.inherited();
    apply_user_agent(&mut style, tag, parent.font_size);
    for rule in &sheet.rules {
        if selector_matches(&rule.selector, tag, attrs, ancestors) {
            apply_declared(
                &mut style,
                &rule.declared,
                parent.font_size,
                viewport,
                containing_width,
            );
        }
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
    pub fn inherited(&self) -> Self {
        Self {
            display: Display::Inline,
            color: self.color,
            background: None,
            font_size: self.font_size,
            margin: [0.0; 4],
            padding: [0.0; 4],
            text_align: self.text_align,
            width: None,
            height: None,
            flex_direction: FlexDirection::Row,
            justify_content: Justify::Start,
            align_items: AlignItems::Stretch,
            gap: 0.0,
            flex_grow: 0.0,
            float: FloatSide::None,
            position: Position::Static,
            overflow: Overflow::Visible,
            opacity: self.opacity,
            z_index: None,
            min_width: None,
            max_width: None,
            min_height: None,
            max_height: None,
            visibility_hidden: self.visibility_hidden,
            background_image: None,
            line_height: self.line_height,
            box_sizing: BoxSizing::ContentBox,
            top: None,
            right: None,
            bottom: None,
            left: None,
            grid_columns: [None; 6],
            grid_column_count: 0,
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
    if let Some(image) = &declared.background_image {
        style.background_image = image.clone();
    }
    if let Some(display) = declared.display {
        style.display = display;
    }
    if let Some(width) = declared.width {
        style.width = resolve_used_length(width, style.font_size, viewport, containing_width);
    }
    if let Some(height) = declared.height {
        style.height = resolve_used_length(height, style.font_size, viewport, viewport.height);
    }
    if let Some(direction) = declared.flex_direction {
        style.flex_direction = direction;
    }
    if let Some(justify) = declared.justify_content {
        style.justify_content = justify;
    }
    if let Some(align) = declared.align_items {
        style.align_items = align;
    }
    if let Some(gap) = declared.gap {
        style.gap = resolve_used_length(gap, style.font_size, viewport, containing_width).unwrap_or(0.0);
    }
    if let Some(grow) = declared.flex_grow {
        style.flex_grow = grow;
    }
    if let Some(float) = declared.float {
        style.float = float;
    }
    if let Some(position) = declared.position {
        style.position = position;
    }
    if let Some(overflow) = declared.overflow {
        style.overflow = overflow;
    }
    if let Some(opacity) = declared.opacity {
        style.opacity = opacity.clamp(0.0, 1.0);
    }
    if let Some(z_index) = declared.z_index {
        style.z_index = Some(z_index);
    }
    if let Some(hidden) = declared.visibility_hidden {
        style.visibility_hidden = hidden;
    }
    if let Some(sizing) = declared.box_sizing {
        style.box_sizing = sizing;
    }
    style.min_width = declared
        .min_width
        .and_then(|length| resolve_used_length(length, style.font_size, viewport, containing_width))
        .or(style.min_width);
    style.max_width = declared
        .max_width
        .and_then(|length| resolve_used_length(length, style.font_size, viewport, containing_width))
        .or(style.max_width);
    style.min_height = declared
        .min_height
        .and_then(|length| resolve_used_length(length, style.font_size, viewport, viewport.height))
        .or(style.min_height);
    style.max_height = declared
        .max_height
        .and_then(|length| resolve_used_length(length, style.font_size, viewport, viewport.height))
        .or(style.max_height);
    style.top = declared.inset.top.and_then(|length| resolve_used_length(length, style.font_size, viewport, viewport.height)).or(style.top);
    style.right = declared.inset.right.and_then(|length| resolve_used_length(length, style.font_size, viewport, containing_width)).or(style.right);
    style.bottom = declared.inset.bottom.and_then(|length| resolve_used_length(length, style.font_size, viewport, viewport.height)).or(style.bottom);
    style.left = declared.inset.left.and_then(|length| resolve_used_length(length, style.font_size, viewport, containing_width)).or(style.left);
    if let Some(count) = declared.grid_column_count {
        style.grid_column_count = count;
        style.grid_columns = declared.grid_columns.unwrap_or([None; 6]);
    }
    if let Some(text_align) = declared.text_align {
        style.text_align = text_align;
    }
    if let Some(font_size) = declared.font_size {
        style.font_size = resolve_font_size(font_size, parent_font).max(1.0);
        if declared.line_height.is_none() {
            style.line_height = style.font_size * 1.4;
        }
    }
    if let Some(line) = declared.line_height {
        style.line_height = resolve_used_length(line, style.font_size, viewport, containing_width).unwrap_or(style.line_height);
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

fn resolve_used_length(length: Length, font_size: f32, viewport: Viewport, containing: f32) -> Option<f32> {
    match length {
        Length::Auto => None,
        Length::Px(n) => Some(n),
        Length::Em(n) => Some(n * font_size),
        Length::Rem(n) => Some(n * 16.0),
        Length::Percent(n) => Some(n / 100.0 * containing),
        Length::Vh(n) => Some(n / 100.0 * viewport.height),
        Length::Vw(n) => Some(n / 100.0 * viewport.width),
    }
}

fn parse_stylesheet(css: &str) -> Vec<Rule> {
    let css = strip_comments(css);
    let mut rules = Vec::new();
    let mut order = 0u32;
    for chunk in css.split('}') {
        let Some((selector, body)) = chunk.split_once('{') else {
            continue;
        };
        let declared = parse_declarations(body);
        for part in split_selector_list(selector) {
            let Some(selector) = parse_selector(part.trim()) else {
                continue;
            };
            rules.push(Rule {
                selector,
                declared: declared.clone(),
                order,
            });
            order += 1;
        }
    }
    rules.sort_by_key(|rule| (rule.selector.specificity, rule.order));
    rules
}

fn split_selector_list(selector: &str) -> Vec<&str> {
    selector.split(',').map(str::trim).filter(|part| !part.is_empty()).collect()
}

fn parse_selector(selector: &str) -> Option<Selector> {
    if selector.chars().any(|ch| matches!(ch, '[' | ':' | '+' | '~')) {
        return None;
    }
    let mut steps = Vec::new();
    let mut direct = Vec::new();
    let mut current = String::new();
    let mut chars = selector.chars().peekable();
    let mut next_direct = false;
    while let Some(ch) = chars.next() {
        if ch == '>' {
            push_compound(&mut steps, &mut direct, &mut current, next_direct)?;
            next_direct = true;
            continue;
        }
        if ch.is_whitespace() {
            if !current.trim().is_empty() {
                push_compound(&mut steps, &mut direct, &mut current, next_direct)?;
                next_direct = false;
            }
            continue;
        }
        current.push(ch);
    }
    push_compound(&mut steps, &mut direct, &mut current, next_direct)?;
    if steps.is_empty() {
        return None;
    }
    let mut ids = 0;
    let mut classes = 0;
    let mut tags = 0;
    for step in &steps {
        for simple in &step.simples {
            match simple {
                SimpleSel::Id(_) => ids += 1,
                SimpleSel::Class(_) => classes += 1,
                SimpleSel::Tag(_) => tags += 1,
                SimpleSel::Universal => {}
            }
        }
    }
    Some(Selector {
        steps,
        direct,
        specificity: (ids, classes, tags),
    })
}

fn push_compound(steps: &mut Vec<Compound>, direct: &mut Vec<bool>, current: &mut String, is_direct: bool) -> Option<()> {
    let text = current.trim();
    if text.is_empty() {
        return Some(());
    }
    let compound = parse_compound(text)?;
    if !steps.is_empty() {
        direct.push(is_direct);
    }
    steps.push(compound);
    current.clear();
    Some(())
}

fn parse_compound(raw: &str) -> Option<Compound> {
    let mut simples = Vec::new();
    let bytes = raw.as_bytes();
    let mut index = 0;
    if bytes.first().is_some_and(|ch| *ch != b'.' && *ch != b'#') {
        let start = index;
        while index < bytes.len() && bytes[index] != b'.' && bytes[index] != b'#' {
            index += 1;
        }
        let tag = &raw[start..index];
        if tag == "*" {
            simples.push(SimpleSel::Universal);
        } else if tag.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-') {
            simples.push(SimpleSel::Tag(tag.to_ascii_lowercase()));
        } else {
            return None;
        }
    }
    while index < raw.len() {
        let kind = bytes[index];
        if kind != b'.' && kind != b'#' {
            return None;
        }
        index += 1;
        let start = index;
        while index < raw.len() && bytes[index] != b'.' && bytes[index] != b'#' {
            index += 1;
        }
        let name = &raw[start..index];
        if name.is_empty() {
            return None;
        }
        if kind == b'.' {
            simples.push(SimpleSel::Class(name.to_string()));
        } else {
            simples.push(SimpleSel::Id(name.to_string()));
        }
    }
    if simples.is_empty() { None } else { Some(Compound { simples }) }
}

fn selector_matches(selector: &Selector, tag: &str, attrs: &[(String, String)], ancestors: &[ElemSnap]) -> bool {
    let Some((subject, rest)) = selector.steps.split_last() else {
        return false;
    };
    if !compound_matches(subject, tag, attrs) {
        return false;
    }
    if rest.is_empty() {
        return true;
    }
    let mut available = ancestors.len();
    for index in (0..rest.len()).rev() {
        let direct = selector.direct.get(index).copied().unwrap_or(false);
        if direct {
            if available == 0 {
                return false;
            }
            available -= 1;
            let ancestor = &ancestors[available];
            if !compound_matches(&rest[index], &ancestor.tag, &ancestor.attrs) {
                return false;
            }
        } else {
            let mut found = false;
            while available > 0 {
                available -= 1;
                let ancestor = &ancestors[available];
                if compound_matches(&rest[index], &ancestor.tag, &ancestor.attrs) {
                    found = true;
                    break;
                }
            }
            if !found {
                return false;
            }
        }
    }
    true
}

fn compound_matches(compound: &Compound, tag: &str, attrs: &[(String, String)]) -> bool {
    compound.simples.iter().all(|simple| match simple {
        SimpleSel::Universal => true,
        SimpleSel::Tag(name) => tag.eq_ignore_ascii_case(name),
        SimpleSel::Id(name) => attr_value(attrs, "id").is_some_and(|id| id == name),
        SimpleSel::Class(name) => attr_value(attrs, "class")
            .is_some_and(|class| class.split_whitespace().any(|part| part == name)),
    })
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

fn css_image_url(value: &str) -> Option<String> {
    if value.eq_ignore_ascii_case("none") {
        return None;
    }
    let lower = value.to_ascii_lowercase();
    let start = lower.find("url(")?;
    let mut rest = value[start + 4..].trim_start();
    if rest.is_empty() {
        return None;
    }
    let body = if rest.starts_with('"') || rest.starts_with('\'') {
        let quote = rest.chars().next()?;
        rest = &rest[1..];
        let end = rest.find(quote)?;
        &rest[..end]
    } else {
        let end = rest.find(')')?;
        rest[..end].trim()
    };
    let body = body.trim();
    if body.is_empty() || body.to_ascii_lowercase().starts_with("data:") {
        None
    } else {
        Some(body.to_string())
    }
}

fn apply_declaration(declared: &mut Declared, name: &str, value: &str) {
    match name {
        "color" => {
            if let Some(color) = parse_color(value) {
                declared.color = Some(color);
            }
        }
        "background-color" => {
            if value.eq_ignore_ascii_case("transparent") {
                declared.background = Some(None);
            } else if let Some(color) = parse_color(value) {
                declared.background = Some(Some(color));
            }
        }
        "background-image" => {
            declared.background_image = Some(css_image_url(value));
        }
        "background" => {
            if value.eq_ignore_ascii_case("transparent") || value.eq_ignore_ascii_case("none") {
                declared.background = Some(None);
            } else if let Some(color) = parse_color(value) {
                declared.background = Some(Some(color));
            }
            if value.to_ascii_lowercase().contains("url(") || value.eq_ignore_ascii_case("none") {
                declared.background_image = Some(css_image_url(value));
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
                "inline-block" => Some(Display::InlineBlock),
                "flex" | "inline-flex" => Some(Display::Flex),
                "grid" | "inline-grid" => Some(Display::Grid),
                "none" => Some(Display::None),
                _ => None,
            };
        }
        "width" => declared.width = parse_length(value),
        "height" => declared.height = parse_length(value),
        "flex-direction" => {
            declared.flex_direction = match value.to_ascii_lowercase().as_str() {
                "row" | "row-reverse" => Some(FlexDirection::Row),
                "column" | "column-reverse" => Some(FlexDirection::Column),
                _ => None,
            };
        }
        "justify-content" => {
            declared.justify_content = match value.to_ascii_lowercase().as_str() {
                "center" => Some(Justify::Center),
                "flex-end" | "end" => Some(Justify::End),
                "space-between" => Some(Justify::SpaceBetween),
                "flex-start" | "start" | "normal" => Some(Justify::Start),
                _ => None,
            };
        }
        "align-items" => {
            declared.align_items = match value.to_ascii_lowercase().as_str() {
                "center" => Some(AlignItems::Center),
                "flex-start" | "start" => Some(AlignItems::Start),
                "flex-end" | "end" => Some(AlignItems::End),
                "stretch" => Some(AlignItems::Stretch),
                _ => None,
            };
        }
        "gap" | "column-gap" | "row-gap" => declared.gap = parse_length(value),
        "flex-grow" => declared.flex_grow = value.trim().parse().ok(),
        "float" => {
            declared.float = match value.to_ascii_lowercase().as_str() {
                "left" => Some(FloatSide::Left),
                "right" => Some(FloatSide::Right),
                "none" => Some(FloatSide::None),
                _ => None,
            };
        }
        "position" => {
            declared.position = match value.to_ascii_lowercase().as_str() {
                "absolute" | "fixed" => Some(Position::Absolute),
                "relative" | "sticky" => Some(Position::Relative),
                "static" => Some(Position::Static),
                _ => None,
            };
        }
        "overflow" | "overflow-y" | "overflow-x" => {
            declared.overflow = match value.to_ascii_lowercase().as_str() {
                "hidden" => Some(Overflow::Hidden),
                "scroll" | "auto" => Some(Overflow::Scroll),
                "visible" => Some(Overflow::Visible),
                _ => None,
            };
        }
        "opacity" => declared.opacity = value.trim().parse::<f32>().ok(),
        "z-index" => declared.z_index = value.trim().parse().ok(),
        "min-width" => declared.min_width = parse_length(value),
        "max-width" => declared.max_width = parse_length(value),
        "min-height" => declared.min_height = parse_length(value),
        "max-height" => declared.max_height = parse_length(value),
        "visibility" => {
            declared.visibility_hidden = Some(value.eq_ignore_ascii_case("hidden") || value.eq_ignore_ascii_case("collapse"));
        }
        "line-height" => {
            declared.line_height = parse_length(value).or_else(|| value.trim().parse::<f32>().ok().map(Length::Em));
        }
        "box-sizing" => {
            declared.box_sizing = match value.to_ascii_lowercase().as_str() {
                "border-box" => Some(BoxSizing::BorderBox),
                "content-box" => Some(BoxSizing::ContentBox),
                _ => None,
            };
        }
        "top" => declared.inset.top = parse_length(value),
        "right" => declared.inset.right = parse_length(value),
        "bottom" => declared.inset.bottom = parse_length(value),
        "left" => declared.inset.left = parse_length(value),
        "grid-template-columns" => {
            let (tracks, count) = parse_tracks(value);
            declared.grid_columns = Some(tracks);
            declared.grid_column_count = Some(count);
        }
        _ => {}
    }
}

fn parse_tracks(value: &str) -> ([Option<Track>; 6], u8) {
    let expanded = expand_repeat(value);
    let mut tracks = [None; 6];
    let mut count = 0u8;
    for token in expanded.split_whitespace() {
        if count as usize >= tracks.len() {
            break;
        }
        let Some(track) = parse_track(token) else {
            continue;
        };
        tracks[count as usize] = Some(track);
        count += 1;
    }
    (tracks, count)
}

fn expand_repeat(value: &str) -> String {
    let Some(start) = value.find("repeat(") else {
        return value.to_string();
    };
    let after = &value[start + "repeat(".len()..];
    let Some(end) = after.find(')') else {
        return value.to_string();
    };
    let inner = &after[..end];
    let Some((count, track)) = inner.split_once(',') else {
        return value.to_string();
    };
    let Ok(count) = count.trim().parse::<usize>() else {
        return value.to_string();
    };
    let track = track.trim();
    let repeated = std::iter::repeat_n(track, count.min(6)).collect::<Vec<_>>().join(" ");
    format!("{}{repeated}{}", &value[..start], &after[end + 1..])
}

fn parse_track(token: &str) -> Option<Track> {
    if let Some(number) = token.strip_suffix("fr") {
        return Some(Track::Fr(number.trim().parse().ok()?));
    }
    match parse_length(token)? {
        Length::Px(n) => Some(Track::Px(n)),
        Length::Percent(n) => Some(Track::Percent(n)),
        _ => None,
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
        let sheet = stylesheet(&doc, None);
        let body = doc.nodes.iter().find_map(|node| match &node.data {
            NodeData::Element { tag, attrs } if tag == "body" => Some(attrs.clone()),
            _ => None,
        });
        compute("body", body.as_deref().unwrap_or(&[]), &[], &root_style(), &sheet, viewport, 800.0)
    }

    #[test]
    fn background_image_url_and_none() {
        let viewport = Viewport { width: 800.0, height: 600.0 };
        let style = body_style(
            r#"<style>body { height: 100vh; background-image: url("https://example.com/a.jpg"); }</style><body></body>"#,
            viewport,
        );
        assert_eq!(style.background_image.as_deref(), Some("https://example.com/a.jpg"));
        assert_eq!(style.height, Some(600.0));
        let cleared = body_style(
            r#"<style>
                body { background-image: url("https://example.com/a.jpg"); }
                body { background-image: none; }
            </style><body></body>"#,
            viewport,
        );
        assert!(cleared.background_image.is_none());
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
        let style = compute("h1", &attrs, &[], &root_style(), &stylesheet(&doc, None), Viewport { width: 800.0, height: 600.0 }, 800.0);
        assert!((style.font_size - 32.0).abs() < 0.1);
        assert_eq!(style.display, Display::Block);
    }

    #[test]
    fn display_none_and_link_color() {
        let parent = root_style();
        let hidden = compute(
            "p",
            &[("style".into(), "display:none".into())],
            &[],
            &parent,
            &Stylesheet { rules: Vec::new() },
            Viewport { width: 100.0, height: 100.0 },
            100.0,
        );
        assert_eq!(hidden.display, Display::None);
        let link = compute(
            "a",
            &[],
            &[],
            &parent,
            &Stylesheet { rules: Vec::new() },
            Viewport { width: 100.0, height: 100.0 },
            100.0,
        );
        assert_eq!(link.color.as_rgba(), [0x1A, 0x0D, 0xAB, 255]);
    }

    #[test]
    fn class_selector_and_linked_stylesheet() {
        let doc = parse(r#"<link rel="stylesheet" href="site.css"><p class="title">Hi</p>"#);
        let sheet = stylesheet_with(&doc, Some("https://example.com/"), &|url| {
            assert_eq!(url, "https://example.com/site.css");
            Ok(".title { color: #010203; display: flex }".to_string())
        });
        let attrs = match &doc.nodes.iter().find(|node| matches!(&node.data, NodeData::Element { tag, .. } if tag == "p")).unwrap().data {
            NodeData::Element { attrs, .. } => attrs.clone(),
            _ => unreachable!(),
        };
        let style = compute("p", &attrs, &[], &root_style(), &sheet, Viewport { width: 800.0, height: 600.0 }, 800.0);
        assert_eq!(style.color.as_rgba(), [1, 2, 3, 255]);
        assert_eq!(style.display, Display::Flex);
    }
}
