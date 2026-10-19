use std::cell::{Cell, RefCell};
use std::rc::Rc;

use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{js_string, Context, JsArgs, JsResult, JsString, JsValue, NativeFunction, Source};

use crate::engine::dom::{Document, NodeData, NodeId};
use crate::net::url::{normalize_navigation, resolve_url};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigationKind {
    Assign,
    Replace,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Navigation {
    pub url: String,
    pub kind: NavigationKind,
}

struct NavState {
    href: String,
    pending: Option<Navigation>,
}

/// 按文档顺序执行内联和外部 script，返回脚本触发的导航（若有）。
pub fn run_inline_scripts(doc: &mut Document, base_url: &str) -> Option<Navigation> {
    run_scripts_with(doc, base_url, &|url| {
        crate::net::fetch_bytes(url, 2 * 1024 * 1024)
            .map(|response| response.bytes)
            .map_err(|err| err.to_string())
    })
}

pub fn run_scripts_with(
    doc: &mut Document,
    base_url: &str,
    fetch: &dyn Fn(&str) -> Result<Vec<u8>, String>,
) -> Option<Navigation> {
    let scripts = collect_scripts(doc, base_url, fetch);
    if scripts.is_empty() {
        return None;
    }

    let state = Rc::new(RefCell::new(NavState {
        href: base_url.to_string(),
        pending: None,
    }));

    let mut context = Context::default();
    if let Err(err) = install_location(&mut context, state.clone(), base_url) {
        eprintln!("JS location 初始化失败：{err}");
        return None;
    }
    let doc_ptr = doc as *mut Document;
    if let Err(err) = install_document(&mut context, doc_ptr) {
        eprintln!("JS document 初始化失败：{err}");
    }
    if let Err(err) = install_host(&mut context) {
        eprintln!("JS 宿主对象初始化失败：{err}");
    }

    for source in scripts {
        if let Err(err) = context.eval(Source::from_bytes(source.as_bytes())) {
            eprintln!("JS 错误：{err}");
        }
    }

    state.borrow_mut().pending.take()
}

fn collect_scripts(
    doc: &Document,
    base_url: &str,
    fetch: &dyn Fn(&str) -> Result<Vec<u8>, String>,
) -> Vec<String> {
    let mut out = Vec::new();
    walk_scripts(doc, doc.root(), base_url, fetch, &mut out);
    out
}

fn walk_scripts(
    doc: &Document,
    id: NodeId,
    base_url: &str,
    fetch: &dyn Fn(&str) -> Result<Vec<u8>, String>,
    out: &mut Vec<String>,
) {
    let node = &doc.nodes[id.0];
    match &node.data {
        NodeData::Document => {
            for child in &node.children {
                walk_scripts(doc, *child, base_url, fetch, out);
            }
        }
        NodeData::Element { tag, attrs } => {
            if tag.eq_ignore_ascii_case("script") {
                if is_javascript_script(attrs) {
                    if let Some(src) = attrs.iter().find(|(name, value)| name.eq_ignore_ascii_case("src") && !value.trim().is_empty()).map(|(_, value)| value.as_str()) {
                        if let Some(url) = resolve_url(Some(base_url), src) {
                            match fetch(&url) {
                                Ok(bytes) => out.push(String::from_utf8_lossy(&bytes).into_owned()),
                                Err(err) => eprintln!("脚本加载失败 {url}：{err}"),
                            }
                        }
                    } else if !has_src(attrs) {
                        out.push(script_text(doc, id));
                    }
                }
                return;
            }
            for child in &node.children {
                walk_scripts(doc, *child, base_url, fetch, out);
            }
        }
        NodeData::Text(_) => {}
    }
}

fn has_src(attrs: &[(String, String)]) -> bool {
    attrs
        .iter()
        .any(|(name, value)| name.eq_ignore_ascii_case("src") && !value.trim().is_empty())
}

fn is_javascript_script(attrs: &[(String, String)]) -> bool {
    let Some((_, value)) = attrs
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("type"))
    else {
        return true;
    };
    let value = value.trim();
    if value.is_empty() {
        return true;
    }
    let lower = value.to_ascii_lowercase();
    lower == "text/javascript"
        || lower == "application/javascript"
        || lower == "application/ecmascript"
        || lower == "text/ecmascript"
        || lower == "module"
}

fn script_text(doc: &Document, id: NodeId) -> String {
    let mut text = String::new();
    for child in &doc.nodes[id.0].children {
        if let NodeData::Text(value) = &doc.nodes[child.0].data {
            text.push_str(value);
        }
    }
    text
}

fn install_location(
    context: &mut Context,
    state: Rc<RefCell<NavState>>,
    base_url: &str,
) -> JsResult<()> {
    let base_for_resolve = base_url.to_string();

    // SAFETY: 闭包只捕获不含 JsValue 的 NavState，不会干扰 Boa GC。
    let get_href = {
        let state = state.clone();
        unsafe {
            NativeFunction::from_closure(move |_, _, _| {
                let href = state.borrow().href.clone();
                Ok(JsValue::from(JsString::from(href.as_str())))
            })
        }
    };

    let set_href = {
        let state = state.clone();
        let base = base_for_resolve.clone();
        unsafe {
            NativeFunction::from_closure(move |_, args, context| {
                let value = args.get_or_undefined(0).to_string(context)?;
                let raw = value.to_std_string_escaped();
                queue_navigation(&state, &base, &raw, NavigationKind::Assign);
                Ok(JsValue::undefined())
            })
        }
    };

    let replace = {
        let state = state.clone();
        let base = base_for_resolve.clone();
        unsafe {
            NativeFunction::from_closure(move |_, args, context| {
                let value = args.get_or_undefined(0).to_string(context)?;
                let raw = value.to_std_string_escaped();
                queue_navigation(&state, &base, &raw, NavigationKind::Replace);
                Ok(JsValue::undefined())
            })
        }
    };

    let assign = {
        let state = state.clone();
        let base = base_for_resolve;
        unsafe {
            NativeFunction::from_closure(move |_, args, context| {
                let value = args.get_or_undefined(0).to_string(context)?;
                let raw = value.to_std_string_escaped();
                queue_navigation(&state, &base, &raw, NavigationKind::Assign);
                Ok(JsValue::undefined())
            })
        }
    };

    let get_fn = get_href.to_js_function(context.realm());
    let set_fn = set_href.to_js_function(context.realm());

    let location = ObjectInitializer::new(context)
        .accessor(
            js_string!("href"),
            Some(get_fn),
            Some(set_fn),
            Attribute::WRITABLE | Attribute::CONFIGURABLE | Attribute::ENUMERABLE,
        )
        .function(replace, js_string!("replace"), 1)
        .function(assign, js_string!("assign"), 1)
        .build();

    context.register_global_property(js_string!("location"), location.clone(), Attribute::all())?;

    let window = ObjectInitializer::new(context)
        .property(js_string!("location"), location, Attribute::all())
        .function(noop(), js_string!("addEventListener"), 2)
        .function(noop(), js_string!("removeEventListener"), 2)
        .build();
    context.register_global_property(js_string!("window"), window, Attribute::all())?;

    Ok(())
}

fn install_document(context: &mut Context, doc: *mut Document) -> JsResult<()> {
    let get_element_by_id = NativeFunction::from_copy_closure(move |_, args, context| {
        let value = args.get_or_undefined(0).to_string(context)?;
        let id = value.to_std_string_escaped();
        let found = unsafe { find_id(&*doc, &id) };
        match found {
            Some(node) => element_value(node, doc, context),
            None => Ok(JsValue::null()),
        }
    });
    let query_selector = NativeFunction::from_copy_closure(move |_, args, context| {
        let value = args.get_or_undefined(0).to_string(context)?;
        let selector = value.to_std_string_escaped();
        let found = unsafe { query_one(&*doc, &selector) };
        match found {
            Some(node) => element_value(node, doc, context),
            None => Ok(JsValue::null()),
        }
    });
    let create_element = NativeFunction::from_copy_closure(move |_, args, context| {
        let value = args.get_or_undefined(0).to_string(context)?;
        let tag = value.to_std_string_escaped().to_ascii_lowercase();
        if tag.is_empty() {
            return Ok(JsValue::null());
        }
        let node = unsafe {
            (*doc).create(NodeData::Element {
                tag,
                attrs: Vec::new(),
            })
        };
        element_value(node, doc, context)
    });
    let get_body = NativeFunction::from_copy_closure(move |_, _, context| match unsafe { find_tag(&*doc, "body") } {
        Some(node) => element_value(node, doc, context),
        None => Ok(JsValue::null()),
    });
    let get_root = NativeFunction::from_copy_closure(move |_, _, context| match unsafe { find_tag(&*doc, "html") } {
        Some(node) => element_value(node, doc, context),
        None => Ok(JsValue::null()),
    });
    let by_tag = NativeFunction::from_copy_closure(move |_, args, context| {
        let value = args.get_or_undefined(0).to_string(context)?;
        let tag = value.to_std_string_escaped();
        elements_by_tag(doc, &tag, context)
    });
    let query_all = NativeFunction::from_copy_closure(move |_, args, context| {
        let value = args.get_or_undefined(0).to_string(context)?;
        let selector = value.to_std_string_escaped();
        let nodes = unsafe { query_many(&*doc, &selector) };
        let values = nodes
            .into_iter()
            .map(|node| element_value(node, doc, context))
            .collect::<JsResult<Vec<_>>>()?;
        Ok(boa_engine::object::builtins::JsArray::from_iter(values, context).into())
    });
    let get_body = get_body.to_js_function(context.realm());
    let get_root = get_root.to_js_function(context.realm());
    let document = ObjectInitializer::new(context)
        .function(get_element_by_id, js_string!("getElementById"), 1)
        .function(query_selector, js_string!("querySelector"), 1)
        .function(query_all, js_string!("querySelectorAll"), 1)
        .function(by_tag, js_string!("getElementsByTagName"), 1)
        .function(create_element, js_string!("createElement"), 1)
        .function(noop(), js_string!("addEventListener"), 2)
        .function(noop(), js_string!("removeEventListener"), 2)
        .accessor(js_string!("body"), Some(get_body), None, Attribute::all())
        .accessor(js_string!("documentElement"), Some(get_root), None, Attribute::all())
        .build();
    context.register_global_property(js_string!("document"), document, Attribute::all())?;
    Ok(())
}

fn element_value(id: NodeId, doc: *mut Document, context: &mut Context) -> JsResult<JsValue> {
    let get_text = NativeFunction::from_copy_closure(move |this, _, context| {
        let Some(node) = node_id(this, context)? else {
            return Ok(JsValue::undefined());
        };
        let text = unsafe { (*doc).text_content(node) };
        Ok(JsValue::from(JsString::from(text.as_str())))
    });
    let set_text = NativeFunction::from_copy_closure(move |this, args, context| {
        let Some(node) = node_id(this, context)? else {
            return Ok(JsValue::undefined());
        };
        let value = args.get_or_undefined(0).to_string(context)?;
        unsafe { (*doc).set_text(node, value.to_std_string_escaped()) };
        Ok(JsValue::undefined())
    });
    let get_class = NativeFunction::from_copy_closure(move |this, _, context| {
        let Some(node) = node_id(this, context)? else {
            return Ok(JsValue::undefined());
        };
        let class = unsafe { (*doc).attr(node, "class").unwrap_or("").to_string() };
        Ok(JsValue::from(JsString::from(class.as_str())))
    });
    let set_class = NativeFunction::from_copy_closure(move |this, args, context| {
        let Some(node) = node_id(this, context)? else {
            return Ok(JsValue::undefined());
        };
        let value = args.get_or_undefined(0).to_string(context)?;
        unsafe { (*doc).set_attr(node, "class", value.to_std_string_escaped()) };
        Ok(JsValue::undefined())
    });
    let set_html = NativeFunction::from_copy_closure(move |this, args, context| {
        let Some(node) = node_id(this, context)? else {
            return Ok(JsValue::undefined());
        };
        let value = args.get_or_undefined(0).to_string(context)?;
        unsafe { set_inner_html(&mut *doc, node, &value.to_std_string_escaped()) };
        Ok(JsValue::undefined())
    });
    let append_child = NativeFunction::from_copy_closure(move |this, args, context| {
        let Some(parent) = node_id(this, context)? else {
            return Ok(JsValue::undefined());
        };
        let Some(child) = node_id(&args.get_or_undefined(0).clone(), context)? else {
            return Ok(JsValue::undefined());
        };
        unsafe { (*doc).append_child(parent, child) };
        Ok(args.get_or_undefined(0).clone())
    });
    let get_text = get_text.to_js_function(context.realm());
    let set_text = set_text.to_js_function(context.realm());
    let get_class = get_class.to_js_function(context.realm());
    let set_class = set_class.to_js_function(context.realm());
    let set_html = set_html.to_js_function(context.realm());
    let object = ObjectInitializer::new(context)
        .property(js_string!("pelagoId"), id.0 as u32, Attribute::READONLY)
        .accessor(js_string!("textContent"), Some(get_text), Some(set_text), Attribute::all())
        .accessor(js_string!("className"), Some(get_class), Some(set_class), Attribute::all())
        .accessor(js_string!("innerHTML"), None, Some(set_html), Attribute::all())
        .function(append_child, js_string!("appendChild"), 1)
        .function(noop(), js_string!("addEventListener"), 2)
        .function(noop(), js_string!("removeEventListener"), 2)
        .function(
            NativeFunction::from_copy_closure(move |_, args, context| {
                let value = args.get_or_undefined(0).to_string(context)?;
                elements_by_tag(doc, &value.to_std_string_escaped(), context)
            }),
            js_string!("getElementsByTagName"),
            1,
        )
        .build();
    Ok(JsValue::from(object))
}

thread_local! {
    static TIMER_DEPTH: Cell<u32> = Cell::new(0);
}

fn noop() -> NativeFunction {
    NativeFunction::from_copy_closure(|_, _, _| Ok(JsValue::undefined()))
}

fn install_host(context: &mut Context) -> JsResult<()> {
    let print = |label: &'static str| {
        NativeFunction::from_copy_closure(move |_, args, context| {
            let mut parts = Vec::new();
            for arg in args {
                let text = arg.to_string(context)?;
                parts.push(text.to_std_string_escaped());
            }
            eprintln!("console.{label}: {}", parts.join(" "));
            Ok(JsValue::undefined())
        })
    };
    let console = ObjectInitializer::new(context)
        .function(print("log"), js_string!("log"), 0)
        .function(print("info"), js_string!("info"), 0)
        .function(print("warn"), js_string!("warn"), 0)
        .function(print("error"), js_string!("error"), 0)
        .function(print("debug"), js_string!("debug"), 0)
        .build();
    let navigator = ObjectInitializer::new(context)
        .property(js_string!("userAgent"), js_string!("Mozilla/5.0 Pelago"), Attribute::all())
        .property(js_string!("platform"), js_string!("Win32"), Attribute::all())
        .property(js_string!("language"), js_string!("zh-CN"), Attribute::all())
        .property(js_string!("cookieEnabled"), false, Attribute::all())
        .property(js_string!("onLine"), true, Attribute::all())
        .build();
    let set_timeout = NativeFunction::from_copy_closure(|_, args, context| invoke_timer(args, context));
    let clear_timeout = noop();
    let timeout = set_timeout.clone().to_js_function(context.realm());
    let interval = set_timeout.to_js_function(context.realm());
    let clear = clear_timeout.clone().to_js_function(context.realm());
    let clear_interval = clear_timeout.to_js_function(context.realm());
    context.register_global_property(js_string!("console"), console.clone(), Attribute::all())?;
    context.register_global_property(js_string!("navigator"), navigator.clone(), Attribute::all())?;
    context.register_global_property(js_string!("setTimeout"), timeout.clone(), Attribute::all())?;
    context.register_global_property(js_string!("setInterval"), interval.clone(), Attribute::all())?;
    context.register_global_property(js_string!("clearTimeout"), clear, Attribute::all())?;
    context.register_global_property(js_string!("clearInterval"), clear_interval, Attribute::all())?;
    if let Some(window) = context.global_object().get(js_string!("window"), context)?.as_object() {
        window.set(js_string!("console"), console, false, context)?;
        window.set(js_string!("navigator"), navigator, false, context)?;
        window.set(js_string!("setTimeout"), timeout, false, context)?;
        window.set(js_string!("setInterval"), interval, false, context)?;
        window.set(js_string!("document"), context.global_object().get(js_string!("document"), context)?, false, context)?;
    }
    Ok(())
}

fn invoke_timer(args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(function) = args.get_or_undefined(0).as_callable() else {
        return Ok(JsValue::from(0));
    };
    let depth = TIMER_DEPTH.with(Cell::get);
    if depth >= 4 {
        return Ok(JsValue::from(0));
    }
    TIMER_DEPTH.with(|slot| slot.set(depth + 1));
    let result = function.call(&JsValue::undefined(), &[], context);
    TIMER_DEPTH.with(|slot| slot.set(depth));
    if let Err(err) = result {
        eprintln!("JS 错误：{err}");
    }
    Ok(JsValue::from(1))
}

fn elements_by_tag(doc: *mut Document, tag: &str, context: &mut Context) -> JsResult<JsValue> {
    let nodes = unsafe { find_tags(&*doc, tag) };
    let values = nodes
        .into_iter()
        .map(|node| element_value(node, doc, context))
        .collect::<JsResult<Vec<_>>>()?;
    Ok(boa_engine::object::builtins::JsArray::from_iter(values, context).into())
}

fn node_id(this: &JsValue, context: &mut Context) -> JsResult<Option<NodeId>> {
    let Some(object) = this.as_object() else {
        return Ok(None);
    };
    let value = object.get(js_string!("pelagoId"), context)?;
    Ok(value.as_number().map(|number| NodeId(number as usize)))
}

fn find_tag(doc: &Document, tag: &str) -> Option<NodeId> {
    find_tags(doc, tag).into_iter().next()
}

fn find_tags(doc: &Document, tag: &str) -> Vec<NodeId> {
    doc.nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| {
            let NodeData::Element { tag: node_tag, .. } = &node.data else {
                return None;
            };
            node_tag.eq_ignore_ascii_case(tag).then_some(NodeId(index))
        })
        .collect()
}

fn query_many(doc: &Document, selector: &str) -> Vec<NodeId> {
    doc.nodes
        .iter()
        .enumerate()
        .filter_map(|(index, _)| {
            let id = NodeId(index);
            query_one_node(doc, id, selector).then_some(id)
        })
        .collect()
}

fn query_one_node(doc: &Document, id: NodeId, selector: &str) -> bool {
    let NodeData::Element { tag: node_tag, attrs } = &doc.nodes[id.0].data else {
        return false;
    };
    let selector = selector.trim();
    if let Some(wanted) = selector.strip_prefix('#') {
        return !wanted.contains([' ', '.']) && attrs.iter().any(|(key, value)| key.eq_ignore_ascii_case("id") && value == wanted);
    }
    let (tag, class) = if let Some(class) = selector.strip_prefix('.') {
        (None, Some(class))
    } else if let Some((tag, class)) = selector.split_once('.') {
        (Some(tag), Some(class))
    } else if !selector.contains(' ') {
        (Some(selector), None)
    } else {
        return false;
    };
    if let Some(tag) = tag {
        if !node_tag.eq_ignore_ascii_case(tag) {
            return false;
        }
    }
    if let Some(class) = class {
        return attrs.iter().any(|(key, value)| {
            key.eq_ignore_ascii_case("class") && value.split_whitespace().any(|part| part == class)
        });
    }
    true
}

fn find_id(doc: &Document, id: &str) -> Option<NodeId> {
    doc.nodes.iter().enumerate().find_map(|(index, node)| {
        let NodeData::Element { attrs, .. } = &node.data else {
            return None;
        };
        attrs.iter().any(|(key, value)| key.eq_ignore_ascii_case("id") && value == id).then_some(NodeId(index))
    })
}

fn query_one(doc: &Document, selector: &str) -> Option<NodeId> {
    let selector = selector.trim();
    if let Some(id) = selector.strip_prefix('#') {
        if !id.contains([' ', '.']) {
            return find_id(doc, id);
        }
    }
    let (tag, class) = if let Some(class) = selector.strip_prefix('.') {
        (None, Some(class))
    } else if let Some((tag, class)) = selector.split_once('.') {
        (Some(tag), Some(class))
    } else if !selector.contains(' ') {
        (Some(selector), None)
    } else {
        return None;
    };
    doc.nodes.iter().enumerate().find_map(|(index, node)| {
        let NodeData::Element { tag: node_tag, attrs } = &node.data else {
            return None;
        };
        if let Some(tag) = tag {
            if !node_tag.eq_ignore_ascii_case(tag) {
                return None;
            }
        }
        if let Some(class) = class {
            let matched = attrs.iter().any(|(key, value)| {
                key.eq_ignore_ascii_case("class") && value.split_whitespace().any(|part| part == class)
            });
            if !matched {
                return None;
            }
        }
        Some(NodeId(index))
    })
}

fn set_inner_html(doc: &mut Document, id: NodeId, html: &str) {
    doc.clear_children(id);
    let fragment = crate::engine::html::parse(html);
    copy_children(&fragment, fragment.root(), doc, id);
}

fn copy_children(src: &Document, src_parent: NodeId, dst: &mut Document, dst_parent: NodeId) {
    for child in src.nodes[src_parent.0].children.clone() {
        copy_node(src, child, dst, dst_parent);
    }
}

fn copy_node(src: &Document, id: NodeId, dst: &mut Document, parent: NodeId) {
    match &src.nodes[id.0].data {
        NodeData::Text(text) => {
            dst.push(NodeData::Text(text.clone()), parent);
        }
        NodeData::Element { tag, attrs } => {
            let created = dst.push(
                NodeData::Element {
                    tag: tag.clone(),
                    attrs: attrs.clone(),
                },
                parent,
            );
            copy_children(src, id, dst, created);
        }
        NodeData::Document => {}
    }
}

fn queue_navigation(state: &Rc<RefCell<NavState>>, base: &str, raw: &str, kind: NavigationKind) {
    let url = match resolve_url(Some(base), raw).or_else(|| normalize_navigation(raw).ok()) {
        Some(url) => url,
        None => {
            eprintln!("JS 导航地址无效：{raw}");
            return;
        }
    };
    let mut state = state.borrow_mut();
    state.href = url.clone();
    state.pending = Some(Navigation { url, kind });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::html::parse;

    #[test]
    fn baidu_style_https_to_http_replace() {
        let html = r#"<html><head><script>
            location.replace(location.href.replace("https://","http://"));
        </script></head><body></body></html>"#;
        let mut doc = parse(html);
        let nav = run_inline_scripts(&mut doc, "https://www.baidu.com/").expect("navigation");
        assert_eq!(nav.url, "http://www.baidu.com/");
        assert_eq!(nav.kind, NavigationKind::Replace);
    }

    #[test]
    fn location_href_assignment() {
        let html = r#"<script>location.href = "https://example.com/next";</script>"#;
        let mut doc = parse(html);
        let nav = run_inline_scripts(&mut doc, "https://example.com/").expect("navigation");
        assert_eq!(nav.url, "https://example.com/next");
        assert_eq!(nav.kind, NavigationKind::Assign);
    }

    #[test]
    fn skips_external_and_non_js_scripts() {
        let html = r#"
            <script src="app.js">should ignore</script>
            <script type="application/json">{"a":1}</script>
            <script>location.replace("https://example.com/ok");</script>
        "#;
        let mut doc = parse(html);
        let nav = run_scripts_with(&mut doc, "https://example.com/", &|_| Err("跳过外部脚本".into())).expect("navigation");
        assert_eq!(nav.url, "https://example.com/ok");
    }

    #[test]
    fn external_script_and_dom_update() {
        let mut doc = parse(r#"<p id="msg">old</p><div class="box"></div><script src="app.js"></script>"#);
        run_scripts_with(&mut doc, "https://example.com/dir/page", &|url| {
            assert_eq!(url, "https://example.com/dir/app.js");
            Ok(r#"
                document.getElementById("msg").textContent = "new";
                var el = document.createElement("span");
                el.className = "item";
                el.textContent = "hi";
                document.querySelector(".box").appendChild(el);
            "#.as_bytes().to_vec())
        });
        assert!(doc.text_content(doc.root()).contains("new"));
        assert!(doc.text_content(doc.root()).contains("hi"));
        assert!(!doc.text_content(doc.root()).contains("old"));
    }

    #[test]
    fn host_globals_do_not_throw() {
        let mut doc = parse(r#"<body><p id="msg">old</p><script>
            console.log(navigator.userAgent);
            window.setTimeout(function () {
                document.getElementById("msg").textContent = navigator.platform;
            }, 0);
            document.body.addEventListener("click", function () {});
        </script></body>"#);
        let nav = run_scripts_with(&mut doc, "https://example.com/", &|_| Err("无外部脚本".into()));
        assert!(nav.is_none());
        let text = doc.text_content(doc.root());
        assert!(text.contains("Win32"), "{text}");
        assert!(!text.contains("old"), "{text}");
    }
}
