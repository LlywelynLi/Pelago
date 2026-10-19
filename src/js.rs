use std::cell::RefCell;
use std::rc::Rc;

use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{js_string, Context, JsArgs, JsResult, JsString, JsValue, NativeFunction, Source};

use crate::dom::{Document, NodeData, NodeId};
use crate::urlutil::{normalize_navigation, resolve_url};

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

/// 执行文档中的内联 script，返回脚本触发的导航（若有）。
pub fn run_inline_scripts(doc: &Document, base_url: &str) -> Option<Navigation> {
    let scripts = collect_inline_scripts(doc);
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

    for source in scripts {
        if let Err(err) = context.eval(Source::from_bytes(source.as_bytes())) {
            eprintln!("JS 错误：{err}");
        }
    }

    state.borrow_mut().pending.take()
}

fn collect_inline_scripts(doc: &Document) -> Vec<String> {
    let mut out = Vec::new();
    walk_scripts(doc, doc.root(), &mut out);
    out
}

fn walk_scripts(doc: &Document, id: NodeId, out: &mut Vec<String>) {
    let node = &doc.nodes[id.0];
    match &node.data {
        NodeData::Document => {
            for child in &node.children {
                walk_scripts(doc, *child, out);
            }
        }
        NodeData::Element { tag, attrs } => {
            if tag.eq_ignore_ascii_case("script") {
                if is_javascript_script(attrs) && !has_src(attrs) {
                    out.push(script_text(doc, id));
                }
                return;
            }
            for child in &node.children {
                walk_scripts(doc, *child, out);
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
        .build();
    context.register_global_property(js_string!("window"), window, Attribute::all())?;

    Ok(())
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
    use crate::html::parse;

    #[test]
    fn baidu_style_https_to_http_replace() {
        let html = r#"<html><head><script>
            location.replace(location.href.replace("https://","http://"));
        </script></head><body></body></html>"#;
        let doc = parse(html);
        let nav = run_inline_scripts(&doc, "https://www.baidu.com/").expect("navigation");
        assert_eq!(nav.url, "http://www.baidu.com/");
        assert_eq!(nav.kind, NavigationKind::Replace);
    }

    #[test]
    fn location_href_assignment() {
        let html = r#"<script>location.href = "https://example.com/next";</script>"#;
        let doc = parse(html);
        let nav = run_inline_scripts(&doc, "https://example.com/").expect("navigation");
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
        let doc = parse(html);
        let nav = run_inline_scripts(&doc, "https://example.com/").expect("navigation");
        assert_eq!(nav.url, "https://example.com/ok");
    }
}
