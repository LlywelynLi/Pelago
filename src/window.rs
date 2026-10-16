use std::num::NonZeroU32;
use std::sync::Arc;

use cosmic_text::{FontSystem, SwashCache};
use softbuffer::{Context, Surface};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{CursorIcon, Window, WindowId};

use crate::chrome::{self, BarHit, Chrome};
use crate::dom::Document;
use crate::html::parse;
use crate::image::ImageStore;
use crate::layout::{clamp_scroll, layout, Scene};
use crate::net::fetch;
use crate::paint::paint_scene;
use crate::urlutil::normalize_navigation;

const WHEEL_STEP: f32 = 48.0;

pub enum PageSource {
    Url(String),
    Html(String),
}

pub fn open(source: PageSource) -> Result<(), Box<dyn std::error::Error>> {
    let event_loop = EventLoop::new()?;
    let mut app = App {
        source,
        window: None,
        gfx: None,
        loaded: false,
        document: None,
        current_url: None,
        address: String::new(),
        address_focused: false,
        history: Vec::new(),
        history_index: 0,
        scene: None,
        images: ImageStore::new(),
        layout_size: (0, 0),
        scroll: 0.0,
        scale: 1.0,
        cursor: PhysicalPosition::new(0.0, 0.0),
        modifiers: ModifiersState::default(),
        font_system: FontSystem::new(),
        swash_cache: SwashCache::new(),
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}

struct Gfx {
    surface: Surface<Arc<Window>, Arc<Window>>,
}

struct App {
    source: PageSource,
    window: Option<Arc<Window>>,
    gfx: Option<Gfx>,
    loaded: bool,
    document: Option<Document>,
    current_url: Option<String>,
    address: String,
    address_focused: bool,
    history: Vec<String>,
    history_index: usize,
    scene: Option<Scene>,
    images: ImageStore,
    layout_size: (u32, u32),
    scroll: f32,
    scale: f64,
    cursor: PhysicalPosition<f64>,
    modifiers: ModifiersState,
    font_system: FontSystem,
    swash_cache: SwashCache,
}

impl App {
    fn redraw(&mut self) {
        let Some(window) = self.window.clone() else {
            return;
        };
        self.scale = window.scale_factor();
        let size = window.inner_size();
        let (Some(width), Some(height)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return;
        };
        let resized = self
            .gfx
            .as_mut()
            .map(|gfx| gfx.surface.resize(width, height))
            .transpose();
        if resized.is_err() {
            return;
        }
        if !self.loaded {
            self.load_initial();
        }
        let bar = chrome::bar_px(self.scale);
        let view_height = (height.get() as f32 - bar).max(1.0);
        let view_size = (width.get(), view_height.round() as u32);
        if self.scene.is_none() || self.layout_size != view_size {
            if let Some(document) = &self.document {
                let scene = layout(
                    document,
                    width.get() as f32,
                    view_height,
                    self.current_url.as_deref(),
                    &mut self.images,
                    &mut self.font_system,
                );
                self.scroll = clamp_scroll(self.scroll, scene.height, view_height);
                self.scene = Some(scene);
                self.layout_size = view_size;
            }
        }
        let mut pixels = vec![0; (width.get() as usize) * (height.get() as usize)];
        paint_scene(
            self.scene.as_mut().expect("page is laid out"),
            self.scroll,
            bar,
            &mut pixels,
            width.get(),
            height.get(),
            &mut self.font_system,
            &mut self.swash_cache,
        );
        chrome::paint(
            &mut pixels,
            width.get(),
            bar,
            self.scale as f32,
            &Chrome {
                address: self.address.clone(),
                focused: self.address_focused,
                can_go_back: self.history_index > 0,
            },
            &mut self.font_system,
            &mut self.swash_cache,
        );
        let Some(gfx) = self.gfx.as_mut() else {
            return;
        };
        let mut buffer = match gfx.surface.buffer_mut() {
            Ok(buffer) => buffer,
            Err(err) => {
                eprintln!("{err}");
                return;
            }
        };
        buffer.copy_from_slice(&pixels);
        if let Err(err) = buffer.present() {
            eprintln!("{err}");
        }
    }

    fn load_initial(&mut self) {
        match &self.source {
            PageSource::Html(html) => {
                self.document = Some(parse(html));
                self.current_url = None;
                self.address.clear();
            }
            PageSource::Url(url) => {
                let url = url.clone();
                self.open_url(&url, true);
            }
        }
        self.loaded = true;
        self.scene = None;
        self.scroll = 0.0;
    }

    fn open_url(&mut self, raw: &str, record: bool) {
        let url = match normalize_navigation(raw) {
            Ok(url) => url,
            Err(message) => {
                eprintln!("{message}");
                self.document = Some(error_document(&message));
                self.scene = None;
                self.scroll = 0.0;
                self.address_focused = false;
                return;
            }
        };
        self.document = Some(load_document(&url));
        self.current_url = Some(url.clone());
        self.address = url.clone();
        self.address_focused = false;
        self.scene = None;
        self.scroll = 0.0;
        if record {
            if self.history_index + 1 < self.history.len() {
                self.history.truncate(self.history_index + 1);
            }
            self.history.push(url);
            self.history_index = self.history.len() - 1;
        }
    }

    fn go_back(&mut self) {
        if self.history_index == 0 || self.history.is_empty() {
            return;
        }
        self.history_index -= 1;
        let url = self.history[self.history_index].clone();
        self.open_url(&url, false);
    }

    fn reload(&mut self) {
        let Some(url) = self.current_url.clone() else {
            return;
        };
        self.open_url(&url, false);
    }

    fn scroll_by(&mut self, delta: f32) {
        let Some(scene) = &self.scene else {
            return;
        };
        let view = self.layout_size.1 as f32;
        self.scroll = clamp_scroll(self.scroll + delta, scene.height, view);
    }

    fn on_click(&mut self) {
        let x = self.cursor.x as f32;
        let y = self.cursor.y as f32;
        let bar = chrome::bar_px(self.scale);
        match chrome::hit_test(x, y, bar, self.layout_width(), self.scale as f32) {
            BarHit::Back => self.go_back(),
            BarHit::Reload => self.reload(),
            BarHit::Address => {
                self.address_focused = true;
                if self.address.is_empty() {
                    if let Some(url) = &self.current_url {
                        self.address = url.clone();
                    }
                }
            }
            BarHit::Page => {
                self.address_focused = false;
                if let Some(url) = &self.current_url {
                    self.address = url.clone();
                }
                let document_x = x;
                let document_y = y - bar + self.scroll;
                if let Some(href) = self.scene.as_ref().and_then(|scene| scene.href_at(document_x, document_y)) {
                    let href = href.to_string();
                    self.open_url(&href, true);
                }
            }
        }
    }

    fn on_key(&mut self, logical: &Key, text: Option<&str>) {
        if self.modifiers.control_key() {
            match logical {
                Key::Character(value) if value.eq_ignore_ascii_case("l") => {
                    self.address_focused = true;
                }
                Key::Character(value) if value.eq_ignore_ascii_case("r") => self.reload(),
                _ => {}
            }
            return;
        }
        if self.modifiers.alt_key() && matches!(logical, Key::Named(NamedKey::ArrowLeft)) {
            self.go_back();
            return;
        }
        if !self.address_focused {
            return;
        }
        match logical {
            Key::Named(NamedKey::Enter) => {
                let address = self.address.clone();
                self.open_url(&address, true);
            }
            Key::Named(NamedKey::Backspace) => {
                self.address.pop();
            }
            Key::Named(NamedKey::Escape) => {
                self.address_focused = false;
                if let Some(url) = &self.current_url {
                    self.address = url.clone();
                }
            }
            _ => {
                if let Some(text) = text {
                    if !text.chars().any(|ch| ch.is_control()) {
                        self.address.push_str(text);
                    }
                }
            }
        }
    }

    fn update_cursor(&self) {
        let Some(window) = &self.window else {
            return;
        };
        let x = self.cursor.x as f32;
        let y = self.cursor.y as f32;
        let bar = chrome::bar_px(self.scale);
        let icon = match chrome::hit_test(x, y, bar, self.layout_width(), self.scale as f32) {
            BarHit::Address => CursorIcon::Text,
            BarHit::Back | BarHit::Reload => CursorIcon::Pointer,
            BarHit::Page => {
                let href = self.scene.as_ref().and_then(|scene| scene.href_at(x, y - bar + self.scroll));
                if href.is_some() { CursorIcon::Pointer } else { CursorIcon::Default }
            }
        };
        window.set_cursor(icon);
    }

    fn layout_width(&self) -> f32 {
        self.window.as_ref().map(|window| window.inner_size().width as f32).unwrap_or(800.0)
    }

    fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

fn load_document(url: &str) -> Document {
    match fetch(url) {
        Ok(page) => {
            eprintln!("{}", page.final_url);
            parse(&page.html)
        }
        Err(err) => {
            eprintln!("{err}");
            error_document(&err.to_string())
        }
    }
}

fn error_document(message: &str) -> Document {
    let escaped = message.replace('&', "&amp;").replace('<', "&lt;");
    parse(&format!("<p style=\"color:#9b1c1c\">{escaped}</p>"))
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("Pelago")
            .with_inner_size(LogicalSize::new(960.0, 720.0));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(err) => {
                eprintln!("{err}");
                event_loop.exit();
                return;
            }
        };
        let context = match Context::new(window.clone()) {
            Ok(context) => context,
            Err(err) => {
                eprintln!("{err}");
                event_loop.exit();
                return;
            }
        };
        let surface = match Surface::new(&context, window.clone()) {
            Ok(surface) => surface,
            Err(err) => {
                eprintln!("{err}");
                event_loop.exit();
                return;
            }
        };
        self.scale = window.scale_factor();
        self.window = Some(window.clone());
        self.gfx = Some(Gfx { surface });
        let _ = context;
        window.request_redraw();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _window_id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => self.request_redraw(),
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = position;
                self.update_cursor();
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => {
                self.on_click();
                self.request_redraw();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let delta_y = match delta {
                    MouseScrollDelta::LineDelta(_, y) => -y * WHEEL_STEP,
                    MouseScrollDelta::PixelDelta(position) => -position.y as f32,
                };
                self.scroll_by(delta_y);
                self.request_redraw();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    self.on_key(&event.logical_key, event.text.as_deref());
                    self.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }
}
