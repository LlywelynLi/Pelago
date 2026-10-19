use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ptr;
use std::rc::Rc;

use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{js_string, Context, JsArgs, JsResult, JsString, JsValue, NativeFunction, Source};

use crate::engine::dom::{Document, NodeData, NodeId};
use crate::net::url::{normalize_navigation, resolve_url};

thread_local! {
    static ACTIVE_CONTEXT: Cell<*mut Context> = const { Cell::new(ptr::null_mut()) };
    static ACTIVE_BASE: RefCell<String> = const { RefCell::new(String::new()) };
    static ACTIVE_DOC: Cell<*mut Document> = const { Cell::new(ptr::null_mut()) };
    static SCRIPT_FAILURES: Cell<u32> = const { Cell::new(0) };
    static SCRIPT_SAMPLES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static API_MISSES: RefCell<HashMap<String, u32>> = RefCell::new(HashMap::new());
}

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

/// 一次页面脚本运行的结果（供窗口导航与 `--smoke` 验收使用）。
#[derive(Debug, Default)]
pub struct ScriptRunResult {
    pub navigation: Option<Navigation>,
    pub failures: u32,
    pub samples: Vec<String>,
    pub api_misses: Vec<(String, u32)>,
}

/// 无窗口拉取页面并执行脚本，用于路线图 Phase 0 验收。
pub fn smoke_page(raw_url: &str) -> Result<ScriptRunResult, String> {
    let url = normalize_navigation(raw_url)?;
    eprintln!("smoke: 拉取 {url}");
    let page = crate::net::fetch(&url).map_err(|err| err.to_string())?;
    eprintln!("smoke: 已加载 {}", page.final_url);
    let mut doc = crate::engine::html::parse(&page.html);
    Ok(run_inline_scripts(&mut doc, &page.final_url))
}

pub fn print_script_report(result: &ScriptRunResult) {
    if result.failures > 0 {
        eprintln!(
            "页面脚本有 {} 处执行失败（宿主 API 尚不完整）。",
            result.failures
        );
        for sample in &result.samples {
            eprintln!("  · {sample}");
        }
    } else {
        eprintln!("页面脚本执行完成，无失败。");
    }
    if !result.api_misses.is_empty() {
        eprintln!("未实现 API 访问 Top {}：", result.api_misses.len().min(15));
        for (name, count) in result.api_misses.iter().take(15) {
            eprintln!("  · {name} × {count}");
        }
    }
}

/// 按文档顺序执行内联和外部 script。
pub fn run_inline_scripts(doc: &mut Document, base_url: &str) -> ScriptRunResult {
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
) -> ScriptRunResult {
    reset_script_stats();
    let scripts = collect_scripts(doc, base_url, fetch);
    if scripts.is_empty() {
        return ScriptRunResult::default();
    }

    let state = Rc::new(RefCell::new(NavState {
        href: base_url.to_string(),
        pending: None,
    }));

    let mut context = Context::default();
    if let Err(err) = install_location(&mut context, state.clone(), base_url) {
        eprintln!("JS location 初始化失败：{err}");
        return ScriptRunResult::default();
    }
    let doc_ptr = doc as *mut Document;
    if let Err(err) = install_document(&mut context, doc_ptr) {
        eprintln!("JS document 初始化失败：{err}");
    }
    if let Err(err) = install_host(&mut context, base_url) {
        eprintln!("JS 宿主对象初始化失败：{err}");
    }

    ACTIVE_CONTEXT.with(|slot| slot.set(&mut context as *mut Context));
    ACTIVE_DOC.with(|slot| slot.set(doc_ptr));
    ACTIVE_BASE.with(|slot| *slot.borrow_mut() = base_url.to_string());

    let script_count = scripts.len();
    for (index, source) in scripts.iter().enumerate() {
        // 单脚本失败不中断后续脚本（失败隔离）。
        if let Err(err) = context.eval(Source::from_bytes(source.as_bytes())) {
            let message = format!("#{}/{}: {err}", index + 1, script_count);
            note_script_failure(&message);
            if std::env::var_os("PELAGO_SCRIPT_CONTEXT").is_some() {
                eprintln!("{}", script_error_context(source, &err.to_string()));
            }
        }
        drain_timers(&mut context);
    }
    let _ = context.eval(Source::from_bytes(
        br#"
        try {
          if (typeof __pelagoInstallSjFallback === "function") __pelagoInstallSjFallback();
          document.readyState = "interactive";
          document.dispatchEvent(new Event("DOMContentLoaded"));
          window.dispatchEvent(new Event("DOMContentLoaded"));
          document.readyState = "complete";
          document.dispatchEvent(new Event("load"));
          window.dispatchEvent(new Event("load"));
        } catch (e) {}
        "#,
    ));
    drain_timers(&mut context);

    ACTIVE_CONTEXT.with(|slot| slot.set(ptr::null_mut()));
    ACTIVE_DOC.with(|slot| slot.set(ptr::null_mut()));

    let result = ScriptRunResult {
        navigation: state.borrow_mut().pending.take(),
        failures: SCRIPT_FAILURES.with(Cell::get),
        samples: SCRIPT_SAMPLES.with(|slot| slot.borrow().clone()),
        api_misses: take_api_misses(),
    };
    print_script_report(&result);
    result
}

fn reset_script_stats() {
    SCRIPT_FAILURES.with(|slot| slot.set(0));
    SCRIPT_SAMPLES.with(|slot| slot.borrow_mut().clear());
    API_MISSES.with(|slot| slot.borrow_mut().clear());
}

fn script_error_context(source: &str, err: &str) -> String {
    let mut out = String::new();
    for token in err.split("at :") {
        let Some(rest) = token.get(..token.find([' ', '\n', ')']).unwrap_or(token.len())) else {
            continue;
        };
        let mut parts = rest.split(':');
        let (Some(line), Some(col)) = (parts.next(), parts.next()) else {
            continue;
        };
        let Ok(line) = line.parse::<usize>() else { continue };
        let Ok(col) = col.parse::<usize>() else { continue };
        let Some(text) = source.lines().nth(line.saturating_sub(1)) else {
            continue;
        };
        let start = col.saturating_sub(80);
        let end = (col + 40).min(text.len());
        if start >= text.len() {
            continue;
        }
        out.push_str(&format!("  ctx L{line}:{col} …{}\n", &text[start..end]));
    }
    out
}

fn note_script_failure(message: &str) {
    SCRIPT_FAILURES.with(|slot| slot.set(slot.get() + 1));
    SCRIPT_SAMPLES.with(|slot| {
        let mut samples = slot.borrow_mut();
        if samples.len() < 8 {
            samples.push(message.to_string());
        }
    });
}

fn note_api_miss(name: &str) {
    if name.is_empty() {
        return;
    }
    API_MISSES.with(|slot| {
        *slot.borrow_mut().entry(name.to_string()).or_insert(0) += 1;
    });
}

fn take_api_misses() -> Vec<(String, u32)> {
    API_MISSES.with(|slot| {
        let mut items: Vec<_> = slot.borrow_mut().drain().collect();
        items.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        items
    })
}

fn drain_timers(context: &mut Context) {
    for _ in 0..12 {
        let Ok(value) = context.eval(Source::from_bytes(
            br#"
            (function () {
              if (typeof __pelagoDrainTimers !== "function") return 0;
              return __pelagoDrainTimers(96);
            })()
            "#,
        )) else {
            break;
        };
        if value.as_number().unwrap_or(0.0) <= 0.0 {
            break;
        }
    }
}

fn eval_script_source(source: &str) {
    ACTIVE_CONTEXT.with(|slot| {
        let ptr = slot.get();
        if ptr.is_null() {
            return;
        }
        unsafe {
            if let Err(err) = (*ptr).eval(Source::from_bytes(source.as_bytes())) {
                note_script_failure(&err.to_string());
            }
            drain_timers(&mut *ptr);
        }
    });
}

fn load_and_eval_script(url: &str) {
    let base = ACTIVE_BASE.with(|slot| slot.borrow().clone());
    let resolved = resolve_url(Some(&base), url).unwrap_or_else(|| url.to_string());
    match crate::net::fetch_bytes(&resolved, 2 * 1024 * 1024) {
        Ok(response) => {
            let source = String::from_utf8_lossy(&response.bytes);
            eval_script_source(&source);
        }
        Err(err) => eprintln!("脚本加载失败 {resolved}：{err}"),
    }
}

fn maybe_run_script_node(doc: *mut Document, id: NodeId) {
    let Some(tag) = (unsafe { (*doc).tag_name(id) }) else {
        return;
    };
    if !tag.eq_ignore_ascii_case("script") {
        return;
    }
    if let Some(src) = unsafe { (*doc).attr(id, "src") } {
        if !src.trim().is_empty() {
            load_and_eval_script(src);
            return;
        }
    }
    let source = unsafe { (*doc).text_content(id) };
    if !source.trim().is_empty() {
        eval_script_source(&source);
    }
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
    let mut part_getters = Vec::new();
    for part in ["protocol", "host", "hostname", "port", "pathname", "search", "hash", "origin"] {
        let state = state.clone();
        let part_name = part.to_string();
        let getter = unsafe {
            NativeFunction::from_closure(move |_, _, _| {
                let href = state.borrow().href.clone();
                Ok(JsValue::from(JsString::from(location_part(&href, &part_name).as_str())))
            })
        };
        part_getters.push((JsString::from(part), getter.to_js_function(context.realm())));
    }

    let mut location_builder = ObjectInitializer::new(context);
    location_builder.accessor(
        js_string!("href"),
        Some(get_fn),
        Some(set_fn),
        Attribute::WRITABLE | Attribute::CONFIGURABLE | Attribute::ENUMERABLE,
    );
    location_builder.function(replace, js_string!("replace"), 1);
    location_builder.function(assign, js_string!("assign"), 1);
    for (name, getter) in part_getters {
        location_builder.accessor(
            name,
            Some(getter),
            None,
            Attribute::CONFIGURABLE | Attribute::ENUMERABLE,
        );
    }
    let location = location_builder.build();

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
    let create_text = NativeFunction::from_copy_closure(move |_, args, context| {
        let value = args.get_or_undefined(0).to_string(context)?;
        let node = unsafe { (*doc).create(NodeData::Text(value.to_std_string_escaped())) };
        element_value(node, doc, context)
    });
    let create_comment = NativeFunction::from_copy_closure(move |_, args, context| {
        let value = args.get_or_undefined(0).to_string(context)?;
        let node = unsafe {
            (*doc).create(NodeData::Element {
                tag: "!--".to_string(),
                attrs: vec![("data".into(), value.to_std_string_escaped())],
            })
        };
        element_value(node, doc, context)
    });
    let create_fragment = NativeFunction::from_copy_closure(move |_, _, context| {
        let node = unsafe {
            (*doc).create(NodeData::Element {
                tag: "#document-fragment".to_string(),
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
        .function(create_text, js_string!("createTextNode"), 1)
        .function(create_comment, js_string!("createComment"), 1)
        .function(create_fragment, js_string!("createDocumentFragment"), 0)
        .function(noop(), js_string!("addEventListener"), 2)
        .function(noop(), js_string!("removeEventListener"), 2)
        .function(noop(), js_string!("dispatchEvent"), 1)
        .accessor(js_string!("body"), Some(get_body), None, Attribute::all())
        .accessor(js_string!("documentElement"), Some(get_root), None, Attribute::all())
        .property(js_string!("readyState"), js_string!("loading"), Attribute::all())
        .property(js_string!("cookie"), js_string!(""), Attribute::all())
        .property(js_string!("nodeType"), 9, Attribute::all())
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
    let get_html = NativeFunction::from_copy_closure(move |this, _, context| {
        let Some(node) = node_id(this, context)? else {
            return Ok(JsValue::undefined());
        };
        let html = unsafe { serialize_children(&*doc, node) };
        Ok(JsValue::from(JsString::from(html.as_str())))
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
        maybe_run_script_node(doc, child);
        Ok(args.get_or_undefined(0).clone())
    });
    let insert_before = NativeFunction::from_copy_closure(move |this, args, context| {
        let Some(parent) = node_id(this, context)? else {
            return Ok(JsValue::undefined());
        };
        let Some(child) = node_id(&args.get_or_undefined(0).clone(), context)? else {
            return Ok(JsValue::undefined());
        };
        let before = node_id(&args.get_or_undefined(1).clone(), context)?;
        unsafe { (*doc).insert_before(parent, child, before) };
        maybe_run_script_node(doc, child);
        Ok(args.get_or_undefined(0).clone())
    });
    let remove_child = NativeFunction::from_copy_closure(move |this, args, context| {
        let Some(parent) = node_id(this, context)? else {
            return Ok(JsValue::undefined());
        };
        let Some(child) = node_id(&args.get_or_undefined(0).clone(), context)? else {
            return Ok(JsValue::undefined());
        };
        unsafe { (*doc).remove_child(parent, child) };
        Ok(args.get_or_undefined(0).clone())
    });
    let remove_self = NativeFunction::from_copy_closure(move |this, _, context| {
        let Some(node) = node_id(this, context)? else {
            return Ok(JsValue::undefined());
        };
        unsafe { (*doc).remove(node) };
        Ok(JsValue::undefined())
    });
    let clone_node = NativeFunction::from_copy_closure(move |this, args, context| {
        let Some(node) = node_id(this, context)? else {
            return Ok(JsValue::null());
        };
        let deep = args.get_or_undefined(0).to_boolean();
        match unsafe { (*doc).clone_node(node, deep) } {
            Some(cloned) => element_value(cloned, doc, context),
            None => Ok(JsValue::null()),
        }
    });
    let get_parent = NativeFunction::from_copy_closure(move |this, _, context| {
        let Some(node) = node_id(this, context)? else {
            return Ok(JsValue::null());
        };
        match unsafe { (*doc).parent_of(node) } {
            Some(parent) => element_value(parent, doc, context),
            None => Ok(JsValue::null()),
        }
    });
    let get_children = NativeFunction::from_copy_closure(move |this, _, context| {
        let Some(node) = node_id(this, context)? else {
            return Ok(boa_engine::object::builtins::JsArray::from_iter([], context).into());
        };
        let children = unsafe { (*doc).element_children(node) };
        let values = children
            .into_iter()
            .map(|child| element_value(child, doc, context))
            .collect::<JsResult<Vec<_>>>()?;
        Ok(boa_engine::object::builtins::JsArray::from_iter(values, context).into())
    });
    let query_one_local = NativeFunction::from_copy_closure(move |this, args, context| {
        let Some(node) = node_id(this, context)? else {
            return Ok(JsValue::null());
        };
        let value = args.get_or_undefined(0).to_string(context)?;
        let selector = value.to_std_string_escaped();
        let found = unsafe { query_under(&*doc, node, &selector) };
        match found {
            Some(found) => element_value(found, doc, context),
            None => Ok(JsValue::null()),
        }
    });
    let get_text = get_text.to_js_function(context.realm());
    let set_text = set_text.to_js_function(context.realm());
    let get_class = get_class.to_js_function(context.realm());
    let set_class = set_class.to_js_function(context.realm());
    let get_html = get_html.to_js_function(context.realm());
    let set_html = set_html.to_js_function(context.realm());
    let get_parent_fn = get_parent.to_js_function(context.realm());
    let get_children_fn = get_children.to_js_function(context.realm());
    let (get_src, set_src) = attr_accessor(doc, "src", context);
    let (get_id, set_id) = attr_accessor(doc, "id", context);
    let (get_href, set_href) = attr_accessor(doc, "href", context);
    let (get_width, set_width) = attr_accessor(doc, "width", context);
    let (get_height, set_height) = attr_accessor(doc, "height", context);
    let style_obj = ObjectInitializer::new(context).build();
    let class_add = NativeFunction::from_copy_closure(move |_, args, context| {
        let class = args.get_or_undefined(0).to_string(context)?.to_std_string_escaped();
        unsafe { class_list_add(&mut *doc, id, &class) };
        Ok(JsValue::undefined())
    });
    let class_remove = NativeFunction::from_copy_closure(move |_, args, context| {
        let class = args.get_or_undefined(0).to_string(context)?.to_std_string_escaped();
        unsafe { class_list_remove(&mut *doc, id, &class) };
        Ok(JsValue::undefined())
    });
    let class_toggle = NativeFunction::from_copy_closure(move |_, args, context| {
        let class = args.get_or_undefined(0).to_string(context)?.to_std_string_escaped();
        Ok(JsValue::from(unsafe { class_list_toggle(&mut *doc, id, &class) }))
    });
    let class_contains = NativeFunction::from_copy_closure(move |_, args, context| {
        let class = args.get_or_undefined(0).to_string(context)?.to_std_string_escaped();
        Ok(JsValue::from(unsafe { class_list_contains(&*doc, id, &class) }))
    });
    let class_list = ObjectInitializer::new(context)
        .function(class_add, js_string!("add"), 1)
        .function(class_remove, js_string!("remove"), 1)
        .function(class_toggle, js_string!("toggle"), 1)
        .function(class_contains, js_string!("contains"), 1)
        .build();
    let object = ObjectInitializer::new(context)
        .property(js_string!("pelagoId"), id.0 as u32, Attribute::READONLY)
        .accessor(js_string!("textContent"), Some(get_text), Some(set_text), Attribute::all())
        .accessor(js_string!("className"), Some(get_class), Some(set_class), Attribute::all())
        .accessor(js_string!("innerHTML"), Some(get_html), Some(set_html), Attribute::all())
        .accessor(js_string!("src"), Some(get_src), Some(set_src), Attribute::all())
        .accessor(js_string!("id"), Some(get_id), Some(set_id), Attribute::all())
        .accessor(js_string!("href"), Some(get_href), Some(set_href), Attribute::all())
        .accessor(js_string!("width"), Some(get_width), Some(set_width), Attribute::all())
        .accessor(js_string!("height"), Some(get_height), Some(set_height), Attribute::all())
        .function(append_child, js_string!("appendChild"), 1)
        .function(insert_before, js_string!("insertBefore"), 2)
        .function(remove_child, js_string!("removeChild"), 1)
        .function(remove_self, js_string!("remove"), 0)
        .function(clone_node, js_string!("cloneNode"), 1)
        .function(query_one_local, js_string!("querySelector"), 1)
        .accessor(js_string!("parentNode"), Some(get_parent_fn.clone()), None, Attribute::all())
        .accessor(js_string!("parentElement"), Some(get_parent_fn), None, Attribute::all())
        .accessor(js_string!("children"), Some(get_children_fn.clone()), None, Attribute::all())
        .accessor(js_string!("childNodes"), Some(get_children_fn), None, Attribute::all())
        .property(js_string!("nodeType"), 1, Attribute::all())
        .property(
            js_string!("nodeName"),
            JsString::from(unsafe { (*doc).tag_name(id).unwrap_or("#text") }),
            Attribute::all(),
        )
        .property(
            js_string!("tagName"),
            JsString::from(unsafe { (*doc).tag_name(id).unwrap_or("").to_ascii_uppercase() }),
            Attribute::all(),
        )
        .function(noop(), js_string!("addEventListener"), 2)
        .function(noop(), js_string!("removeEventListener"), 2)
        .function(noop(), js_string!("dispatchEvent"), 1)
        .function(noop(), js_string!("focus"), 0)
        .function(noop(), js_string!("blur"), 0)
        .function(noop(), js_string!("click"), 0)
        .function(
            NativeFunction::from_copy_closure(move |this, args, context| {
                let Some(parent) = node_id(this, context)? else {
                    return Ok(JsValue::undefined());
                };
                let value = args.get_or_undefined(0).to_string(context)?;
                let name = value.to_std_string_escaped();
                let val = args.get_or_undefined(1).to_string(context)?.to_std_string_escaped();
                unsafe { (*doc).set_attr(parent, &name, val) };
                Ok(JsValue::undefined())
            }),
            js_string!("setAttribute"),
            2,
        )
        .function(
            NativeFunction::from_copy_closure(move |this, args, context| {
                let Some(parent) = node_id(this, context)? else {
                    return Ok(JsValue::undefined());
                };
                let value = args.get_or_undefined(0).to_string(context)?;
                let name = value.to_std_string_escaped();
                let attr = unsafe { (*doc).attr(parent, &name).unwrap_or("").to_string() };
                Ok(JsValue::from(JsString::from(attr.as_str())))
            }),
            js_string!("getAttribute"),
            1,
        )
        .function(
            NativeFunction::from_copy_closure(move |this, args, context| {
                let Some(parent) = node_id(this, context)? else {
                    return Ok(JsValue::undefined());
                };
                let value = args.get_or_undefined(0).to_string(context)?;
                let name = value.to_std_string_escaped();
                unsafe { (*doc).remove_attr(parent, &name) };
                Ok(JsValue::undefined())
            }),
            js_string!("removeAttribute"),
            1,
        )
        .function(
            NativeFunction::from_copy_closure(move |this, args, context| {
                let Some(parent) = node_id(this, context)? else {
                    return Ok(JsValue::from(false));
                };
                let value = args.get_or_undefined(0).to_string(context)?;
                let name = value.to_std_string_escaped();
                Ok(JsValue::from(unsafe { (*doc).has_attr(parent, &name) }))
            }),
            js_string!("hasAttribute"),
            1,
        )
        .function(
            NativeFunction::from_copy_closure(move |_, args, context| {
                let value = args.get_or_undefined(0).to_string(context)?;
                elements_by_tag(doc, &value.to_std_string_escaped(), context)
            }),
            js_string!("getElementsByTagName"),
            1,
        )
        .property(js_string!("style"), style_obj, Attribute::all())
        .property(js_string!("classList"), class_list, Attribute::all())
        .build();
    Ok(JsValue::from(object))
}

fn attr_accessor(doc: *mut Document, name: &'static str, context: &mut Context) -> (boa_engine::object::builtins::JsFunction, boa_engine::object::builtins::JsFunction) {
    let get = NativeFunction::from_copy_closure(move |this, _, context| {
        let Some(node) = node_id(this, context)? else {
            return Ok(JsValue::undefined());
        };
        let value = unsafe { (*doc).attr(node, name).unwrap_or("").to_string() };
        Ok(JsValue::from(JsString::from(value.as_str())))
    });
    let set = NativeFunction::from_copy_closure(move |this, args, context| {
        let Some(node) = node_id(this, context)? else {
            return Ok(JsValue::undefined());
        };
        let value = args.get_or_undefined(0).to_string(context)?;
        unsafe { (*doc).set_attr(node, name, value.to_std_string_escaped()) };
        Ok(JsValue::undefined())
    });
    (get.to_js_function(context.realm()), set.to_js_function(context.realm()))
}

fn noop() -> NativeFunction {
    NativeFunction::from_copy_closure(|_, _, _| Ok(JsValue::undefined()))
}

fn install_host(context: &mut Context, base_url: &str) -> JsResult<()> {
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
    let miss = NativeFunction::from_copy_closure(|_, args, context| {
        let name = args.get_or_undefined(0).to_string(context)?.to_std_string_escaped();
        note_api_miss(&name);
        Ok(JsValue::undefined())
    });
    context.register_global_property(js_string!("console"), console.clone(), Attribute::all())?;
    context.register_global_property(js_string!("navigator"), navigator.clone(), Attribute::all())?;
    // 定时器由 HOST_SHIM 实现队列；这里先占位，随后被 shim 覆盖。
    let placeholder = noop().to_js_function(context.realm());
    context.register_global_property(js_string!("setTimeout"), placeholder.clone(), Attribute::all())?;
    context.register_global_property(js_string!("setInterval"), placeholder.clone(), Attribute::all())?;
    context.register_global_property(js_string!("clearTimeout"), placeholder.clone(), Attribute::all())?;
    context.register_global_property(js_string!("clearInterval"), placeholder.clone(), Attribute::all())?;
    let base = base_url.to_string();
    let http = unsafe {
        NativeFunction::from_closure(move |_, args, context| {
            let method = args.get_or_undefined(0).to_string(context)?.to_std_string_escaped();
            let raw = args.get_or_undefined(1).to_string(context)?.to_std_string_escaped();
            let payload = args.get_or_undefined(2).to_string(context)?.to_std_string_escaped();
            let url = resolve_url(Some(&base), &raw).unwrap_or(raw);
            let result = crate::net::exchange(&method, &url, Some(&payload));
            let (status, text) = match result {
                Ok(response) => (response.status as i32, response.text),
                Err(err) => (0, err.to_string()),
            };
            let object = ObjectInitializer::new(context)
                .property(js_string!("status"), status, Attribute::all())
                .property(js_string!("body"), JsString::from(text.as_str()), Attribute::all())
                .build();
            Ok(JsValue::from(object))
        })
    };
    context.register_global_property(js_string!("__pelagoHttp"), http.to_js_function(context.realm()), Attribute::all())?;
    context.register_global_property(
        js_string!("__pelagoMiss"),
        miss.to_js_function(context.realm()),
        Attribute::all(),
    )?;
    let load_script = NativeFunction::from_copy_closure(|_, args, context| {
        let value = args.get_or_undefined(0).to_string(context)?;
        load_and_eval_script(&value.to_std_string_escaped());
        Ok(JsValue::undefined())
    });
    context.register_global_property(
        js_string!("__pelagoLoadScript"),
        load_script.to_js_function(context.realm()),
        Attribute::all(),
    )?;
    if let Some(window) = context.global_object().get(js_string!("window"), context)?.as_object() {
        window.set(js_string!("console"), console, false, context)?;
        window.set(js_string!("navigator"), navigator, false, context)?;
        window.set(js_string!("document"), context.global_object().get(js_string!("document"), context)?, false, context)?;
    }
    context.eval(Source::from_bytes(HOST_SHIM))?;
    Ok(())
}

const HOST_SHIM: &str = r#"
(function (global) {
  var window = global.window || global;
  function noop() { return undefined; }
  function miss(name) {
    try { if (typeof __pelagoMiss === "function") __pelagoMiss(String(name)); } catch (e) {}
  }
  var __pelagoTimerSeq = 1;
  var __pelagoTimers = [];
  var __pelagoCancelled = Object.create(null);
  function queueTimer(cb, ms, interval) {
    var id = __pelagoTimerSeq++;
    __pelagoTimers.push({ id: id, cb: cb, ms: ms | 0, interval: !!interval });
    return id;
  }
  function clearTimer(id) {
    __pelagoCancelled[id | 0] = true;
  }
  global.setTimeout = window.setTimeout = function (cb, ms) { return queueTimer(cb, ms, false); };
  global.setInterval = window.setInterval = function (cb, ms) { return queueTimer(cb, ms, true); };
  global.clearTimeout = window.clearTimeout = clearTimer;
  global.clearInterval = window.clearInterval = clearTimer;
  global.__pelagoDrainTimers = function (budget) {
    budget = budget || 64;
    var ran = 0;
    var pending = 0;
    while (ran < budget && __pelagoTimers.length) {
      var job = __pelagoTimers.shift();
      if (__pelagoCancelled[job.id]) continue;
      ran++;
      try {
        if (typeof job.cb === "function") job.cb();
        else if (typeof job.cb === "string") (0, eval)(job.cb);
      } catch (e) {}
      if (job.interval && !__pelagoCancelled[job.id]) {
        __pelagoTimers.push(job);
        pending++;
      }
    }
    return __pelagoTimers.length + pending > 0 ? __pelagoTimers.length : 0;
  };
  function soft(value, label) {
    if (value == null || (typeof value !== "object" && typeof value !== "function")) return value;
    try {
      if (value.__pelagoSoft) return value;
    } catch (e) {}
    label = label || "soft";
    try {
      return new Proxy(value, {
        get: function (target, prop, receiver) {
          if (prop === "__pelagoSoft") return true;
          if (typeof prop === "symbol") return Reflect.get(target, prop, receiver);
          var name = String(prop);
          var got = Reflect.get(target, prop, receiver);
          var own = Object.prototype.hasOwnProperty.call(target, prop);
          var proto = Object.getPrototypeOf(target);
          var onProto = !!(proto && name in proto);
          if (own || onProto || got !== undefined) {
            return typeof got === "function" ? got.bind(target) : soft(got, label + "." + name);
          }
          if (name.indexOf("__pelago") === 0) return undefined;
          miss(label + "." + name);
          return function () { return soft({}, label + "." + name + "()"); };
        },
        set: function (target, prop, next) {
          try { target[prop] = next; } catch (e) {}
          return true;
        },
        has: function () { return true; }
      });
    } catch (e) {
      return value;
    }
  }
  function mirrorGlobal(prop, next) {
    if (typeof prop !== "string") return;
    if (prop === "window" || prop === "document" || prop === "globalThis") return;
    // window.foo = x 时同步到全局绑定，供后续脚本使用裸标识符 foo。
    try { global[prop] = next; } catch (e) {}
  }
  function probe(obj, label) {
    if (!obj || typeof obj !== "object") return obj;
    try {
      return new Proxy(obj, {
        get: function (target, prop, receiver) {
          if (typeof prop === "symbol") return Reflect.get(target, prop, receiver);
          if (!(prop in target)) miss(label + "." + String(prop));
          var got = Reflect.get(target, prop, receiver);
          return typeof got === "function" ? got.bind(target) : got;
        },
        set: function (target, prop, next) {
          try { target[prop] = next; } catch (e) {}
          mirrorGlobal(prop, next);
          return true;
        }
      });
    } catch (e) {
      return obj;
    }
  }
  function wrapNode(node) {
    if (node == null) {
      return soft({
        nodeType: 1,
        tagName: "DIV",
        style: soft({}),
        classList: soft({ add: noop, remove: noop, contains: function () { return false; }, toggle: noop }),
        appendChild: function (child) { return child; },
        setAttribute: noop,
        getAttribute: function () { return ""; },
        addEventListener: noop,
        removeEventListener: noop,
        querySelector: function () { return wrapNode(null); },
        querySelectorAll: function () { return []; }
      });
    }
    return installTarget(soft(node));
  }
  function patch(obj, name) {
    if (!obj || typeof obj[name] !== "function") return;
    var original = obj[name].bind(obj);
    obj[name] = function () {
      var result = original.apply(obj, arguments);
      if (name === "querySelectorAll" || name === "getElementsByTagName") {
        return soft(Array.prototype.slice.call(result || []));
      }
      return wrapNode(result);
    };
  }
  function installTarget(obj) {
    if (!obj) return obj;
    try {
      if (Object.prototype.hasOwnProperty.call(obj, "__pelagoEvents")) return obj;
    } catch (e) {
      if (obj.__pelagoEvents) return obj;
    }
    var map = Object.create(null);
    obj.__pelagoEvents = map;
    obj.addEventListener = function (type, fn) {
      if (typeof fn !== "function") return;
      type = String(type);
      (map[type] = map[type] || []).push(fn);
    };
    obj.removeEventListener = function (type, fn) {
      type = String(type);
      map[type] = (map[type] || []).filter(function (item) { return item !== fn; });
    };
    obj.dispatchEvent = function (event) {
      event = event || {};
      if (typeof event === "string") event = { type: event };
      var type = String(event.type || "");
      event.target = event.target || obj;
      event.currentTarget = obj;
      var list = (map[type] || []).slice();
      for (var i = 0; i < list.length; i++) {
        try { list[i].call(obj, event); } catch (e) {}
      }
      var key = "on" + type;
      if (typeof obj[key] === "function") {
        try { obj[key].call(obj, event); } catch (e) {}
      }
      return !event.defaultPrevented;
    };
    return obj;
  }

  try {
    ["getElementById", "querySelector", "createElement", "createTextNode", "createDocumentFragment"].forEach(function (name) { patch(document, name); });
    ["querySelectorAll", "getElementsByTagName"].forEach(function (name) { patch(document, name); });
  } catch (e) {}
  installTarget(document);
  installTarget(window);

  function Image() { return wrapNode(document.createElement("img")); }
  global.Image = window.Image = Image;

  function Event(type, init) {
    this.type = String(type || "");
    this.bubbles = !!(init && init.bubbles);
    this.cancelable = !!(init && init.cancelable);
    this.defaultPrevented = false;
    this.target = null;
    this.currentTarget = null;
  }
  Event.prototype.preventDefault = function () { this.defaultPrevented = true; };
  Event.prototype.stopPropagation = noop;
  Event.prototype.stopImmediatePropagation = noop;
  global.Event = window.Event = Event;
  global.CustomEvent = window.CustomEvent = function (type, init) {
    Event.call(this, type);
    this.detail = init && init.detail;
  };
  global.MutationObserver = window.MutationObserver = function (cb) { this._cb = cb; };
  window.MutationObserver.prototype.observe = noop;
  window.MutationObserver.prototype.disconnect = noop;
  window.MutationObserver.prototype.takeRecords = function () { return []; };
  global.ResizeObserver = window.ResizeObserver = window.MutationObserver;
  global.IntersectionObserver = window.IntersectionObserver = window.MutationObserver;

  function Storage() { this._data = Object.create(null); }
  Storage.prototype.getItem = function (key) { return Object.prototype.hasOwnProperty.call(this._data, key) ? this._data[key] : null; };
  Storage.prototype.setItem = function (key, value) { this._data[key] = String(value); };
  Storage.prototype.removeItem = function (key) { delete this._data[key]; };
  Storage.prototype.clear = function () { this._data = Object.create(null); };
  Storage.prototype.key = function (index) { return Object.keys(this._data)[index] || null; };
  global.localStorage = window.localStorage = soft(new Storage());
  global.sessionStorage = window.sessionStorage = soft(new Storage());
  global.history = window.history = soft({ pushState: noop, replaceState: noop, back: noop, forward: noop, go: noop, length: 1, state: null });
  global.screen = window.screen = soft({ width: 1920, height: 1080, availWidth: 1920, availHeight: 1040, colorDepth: 24, pixelDepth: 24 });
  var __navStart = Date.now() - 120;
  global.performance = window.performance = soft({
    now: function () { return Date.now() - __navStart; },
    timing: {
      navigationStart: __navStart,
      fetchStart: __navStart + 10,
      domainLookupStart: __navStart + 12,
      domainLookupEnd: __navStart + 14,
      connectStart: __navStart + 15,
      connectEnd: __navStart + 20,
      requestStart: __navStart + 25,
      responseStart: __navStart + 40,
      responseEnd: __navStart + 55,
      domLoading: __navStart + 60,
      domInteractive: __navStart + 80,
      domContentLoadedEventStart: __navStart + 90,
      domContentLoadedEventEnd: __navStart + 95,
      domComplete: __navStart + 110,
      loadEventStart: __navStart + 112,
      loadEventEnd: __navStart + 115
    },
    navigation: { type: 0, redirectCount: 0 }
  });
  try {
    var cookieBox = { value: "" };
    Object.defineProperty(document, "cookie", {
      configurable: true,
      enumerable: true,
      get: function () { return cookieBox.value; },
      set: function (next) {
        next = String(next || "");
        var part = next.split(";")[0];
        var eq = part.indexOf("=");
        if (eq <= 0) return;
        var key = part.slice(0, eq).trim();
        var val = part.slice(eq + 1);
        var pieces = cookieBox.value ? cookieBox.value.split("; ") : [];
        var found = false;
        for (var i = 0; i < pieces.length; i++) {
          if (pieces[i].slice(0, key.length + 1) === key + "=") {
            pieces[i] = key + "=" + val;
            found = true;
            break;
          }
        }
        if (!found) pieces.push(key + "=" + val);
        cookieBox.value = pieces.join("; ");
      }
    });
  } catch (e) {}
  global.matchMedia = window.matchMedia = function () { return soft({ matches: false, media: "", addListener: noop, removeListener: noop, addEventListener: noop, removeEventListener: noop }); };
  global.getComputedStyle = window.getComputedStyle = function () { return soft({ getPropertyValue: function () { return ""; } }); };
  global.requestAnimationFrame = window.requestAnimationFrame = function (cb) {
    return setTimeout(function () { if (typeof cb === "function") cb(Date.now()); }, 0);
  };
  global.cancelAnimationFrame = window.cancelAnimationFrame = function (id) { clearTimeout(id); };
  global.fetch = window.fetch = function (url, init) {
    init = init || {};
    var method = (init.method || "GET").toUpperCase();
    var body = init.body == null ? "" : String(init.body);
    var result = __pelagoHttp(method, String(url), body);
    var response = soft({
      ok: result.status >= 200 && result.status < 300,
      status: result.status,
      statusText: "",
      url: String(url),
      headers: soft({ get: function () { return null; } }),
      text: function () { return Promise.resolve(result.body); },
      json: function () {
        try { return Promise.resolve(JSON.parse(result.body)); }
        catch (e) { return Promise.reject(e); }
      },
      arrayBuffer: function () { return Promise.resolve(new ArrayBuffer(0)); },
      blob: function () { return Promise.resolve(soft({})); }
    });
    return Promise.resolve(response);
  };
  global.FormData = window.FormData = function () { this.append = noop; this.set = noop; this.get = function () { return null; }; this.delete = noop; };
  global.Blob = window.Blob = function () { return soft({}); };
  global.URL = window.URL = window.URL || soft({ createObjectURL: function () { return ""; }, revokeObjectURL: noop });
  global.HTMLElement = window.HTMLElement = function () {};
  global.Node = window.Node = function () {};
  global.Element = window.Element = window.HTMLElement;
  window.self = window;
  window.top = window;
  window.parent = window;
  window.frames = window;
  window.name = window.name || "";
  window.devicePixelRatio = 1;
  window.innerWidth = 1280;
  window.innerHeight = 720;
  window.outerWidth = 1280;
  window.outerHeight = 720;
  window.scrollX = 0;
  window.scrollY = 0;
  window.pageXOffset = 0;
  window.pageYOffset = 0;
  window.scrollTo = noop;
  window.scrollBy = noop;
  window.open = function () { return null; };
  window.close = noop;
  window.alert = noop;
  window.confirm = function () { return false; };
  window.prompt = function () { return null; };
  window.getSelection = function () { return soft({ toString: function () { return ""; }, removeAllRanges: noop }); };
  if (typeof global.atob !== "function") {
    global.atob = window.atob = function (value) { return String(value || ""); };
  }
  if (typeof global.btoa !== "function") {
    global.btoa = window.btoa = function (value) { return String(value || ""); };
  }

  var __pelagoModules = Object.create(null);
  function isScriptUrl(name) {
    return /^(https?:)?\/\//.test(name) || name.indexOf("/") === 0 || /\.js(\?|#|$)/.test(name);
  }
  function require(id, callback) {
    if (typeof id === "function") { id(); return soft({}); }
    if (Array.isArray(id)) {
      var loaded = id.map(function (name) {
        if (isScriptUrl(String(name))) {
          try { __pelagoLoadScript(String(name)); } catch (e) {}
          return soft(__pelagoModules[name] || {});
        }
        return require(name);
      });
      if (typeof callback === "function") {
        try { callback.apply(null, loaded); } catch (e) {}
      }
      return loaded[0];
    }
    id = String(id);
    if (Object.prototype.hasOwnProperty.call(__pelagoModules, id)) return __pelagoModules[id];
    if (isScriptUrl(id)) {
      try { __pelagoLoadScript(id); } catch (e) {}
      return soft(__pelagoModules[id] || {});
    }
    return soft({});
  }
  require.addPaths = noop;
  require.config = noop;
  require.async = require;
  function define(name, deps, factory) {
    if (typeof name !== "string") { factory = deps; deps = name; name = ""; }
    if (!Array.isArray(deps)) { factory = deps; deps = []; }
    var exports = {};
    var module = { exports: exports };
    var result = factory;
    if (typeof factory === "function") {
      var args = deps.map(function (dep) {
        if (dep === "require") return require;
        if (dep === "exports") return exports;
        if (dep === "module") return module;
        return require(dep);
      });
      try { result = factory.apply(null, args); } catch (e) { result = module.exports; }
      if (result === undefined) result = module.exports;
    }
    __pelagoModules[name || ("anon-" + Object.keys(__pelagoModules).length)] = soft(result || {});
  }
  define.amd = {};
  global.define = window.define = define;
  global.require = window.require = require;

  function XMLHttpRequest() {
    this.readyState = 0;
    this.status = 0;
    this.responseText = "";
    this.response = "";
    this.responseType = "";
    this._method = "GET";
    this._url = "";
  }
  XMLHttpRequest.UNSENT = 0;
  XMLHttpRequest.OPENED = 1;
  XMLHttpRequest.HEADERS_RECEIVED = 2;
  XMLHttpRequest.LOADING = 3;
  XMLHttpRequest.DONE = 4;
  XMLHttpRequest.prototype.open = function (method, url) {
    this._method = method || "GET";
    this._url = url || "";
    this.readyState = 1;
  };
  XMLHttpRequest.prototype.setRequestHeader = noop;
  XMLHttpRequest.prototype.getResponseHeader = function () { return ""; };
  XMLHttpRequest.prototype.getAllResponseHeaders = function () { return ""; };
  XMLHttpRequest.prototype.abort = noop;
  XMLHttpRequest.prototype.overrideMimeType = noop;
  XMLHttpRequest.prototype.send = function (body) {
    var result = __pelagoHttp(String(this._method), String(this._url), body == null ? "" : String(body));
    this.status = result.status;
    this.responseText = result.body;
    this.response = result.body;
    this.readyState = 4;
    if (typeof this.onreadystatechange === "function") this.onreadystatechange();
    if (this.status >= 200 && this.status < 300) {
      if (typeof this.onload === "function") this.onload();
    } else if (typeof this.onerror === "function") {
      this.onerror();
    }
  };
  global.XMLHttpRequest = window.XMLHttpRequest = XMLHttpRequest;

  function jQuery(sel) {
    if (typeof sel === "function") { try { sel(jQuery); } catch (e) {} return jQuery; }
    var nodes = [];
    if (sel == null) nodes = [];
    else if (typeof sel === "string") {
      if (sel.charAt(0) === "<") nodes = [document.createElement("div")];
      else {
        var one = document.querySelector(sel);
        nodes = one ? [one] : [];
      }
    } else if (sel.nodeType) nodes = [sel];
    else if (sel.length != null) nodes = Array.prototype.slice.call(sel);
    var api = soft({
      length: nodes.length,
      0: nodes[0],
      each: function (fn) { nodes.forEach(function (node, index) { fn.call(node, index, node); }); return api; },
      find: function (q) { return jQuery(nodes[0] ? nodes[0].querySelector && nodes[0].querySelector(q) : null); },
      on: function () { return api; },
      off: function () { return api; },
      ready: function (fn) { if (typeof fn === "function") fn(jQuery); return api; },
      html: function (value) { if (value == null) return nodes[0] ? nodes[0].innerHTML : ""; if (nodes[0]) nodes[0].innerHTML = value; return api; },
      text: function (value) { if (value == null) return nodes[0] ? nodes[0].textContent : ""; if (nodes[0]) nodes[0].textContent = value; return api; },
      attr: function (name, value) { if (value == null) return nodes[0] ? nodes[0].getAttribute(name) : undefined; if (nodes[0]) nodes[0].setAttribute(name, value); return api; },
      addClass: function () { return api; },
      removeClass: function () { return api; },
      css: function () { return api; },
      append: function () { return api; },
      appendTo: function () { return api; },
      hide: function () { return api; },
      show: function () { return api; },
      ajax: jQuery.ajax
    });
    return api;
  }
  jQuery.fn = jQuery.prototype = soft({});
  jQuery.ajax = function () {
    return soft({
      done: function (fn) { if (typeof fn === "function") try { fn({}, "success", soft({})); } catch (e) {} return this; },
      fail: function () { return this; },
      always: function (fn) { if (typeof fn === "function") try { fn(); } catch (e) {} return this; },
      then: function (fn) { return this.done(fn); }
    });
  };
  jQuery.get = jQuery.ajax;
  jQuery.post = jQuery.ajax;
  jQuery.extend = function () {
    var target = arguments[0] || {};
    for (var i = 1; i < arguments.length; i++) {
      var src = arguments[i] || {};
      for (var key in src) if (Object.prototype.hasOwnProperty.call(src, key)) target[key] = src[key];
    }
    return target;
  };
  global.$ = global.jQuery = window.$ = window.jQuery = jQuery;
  global.bds = window.bds = window.bds || soft({ util: soft({}), conf: soft({}), event: soft({ on: noop, un: noop, trigger: noop }) });
  global.F = window.F = window.F || soft({});

  // 预置常见 window 事件属性，避免 Boa「uninitialized global property」赋值错误。
  ["onload", "onerror", "onunhandledrejection", "onresize", "onscroll", "onclick", "onfocus", "onblur"].forEach(function (name) {
    try {
      if (typeof global[name] === "undefined") global[name] = null;
      if (typeof window[name] === "undefined") window[name] = null;
    } catch (e) {}
  });

  // 必应等站点把 window/document 缩写成 _w/_d；先挂上避免引导脚本中途 ReferenceError。
  global._w = window._w = window;
  global._d = window._d = document;
  // 预声明 _G，避免外链在 `window._G=...` 之前用 `null==_G` 触发 ReferenceError。
  // （不可过早提供 sj_be：必应用 `"u">typeof sj_be` 判断，假 stub 会走进依赖 _G 的分支。）
  if (typeof global._G === "undefined") {
    global._G = window._G = {};
  }
  global.sb_st = window.sb_st = window.sb_st || setTimeout;
  global.sb_ct = window.sb_ct = window.sb_ct || clearTimeout;
  global.sb_si = window.sb_si = window.sb_si || setInterval;
  global.sb_ci = window.sb_ci = window.sb_ci || clearInterval;
  if (typeof global._ge !== "function") {
    global._ge = window._ge = function (id) { return document.getElementById(id); };
  }
  if (typeof global.sj_gx !== "function") {
    global.sj_gx = window.sj_gx = function () { return new XMLHttpRequest(); };
  }
  // sj_* 在外链真正提供前保持未定义；仅在页面跑完后若仍缺失再补最小实现。
  global.__pelagoInstallSjFallback = function () {
    if (typeof global.sj_ce !== "function") {
      global.sj_ce = window.sj_ce = function (tag, id) {
        var el = document.createElement(tag || "div");
        if (id) el.id = id;
        return el;
      };
    }
    if (typeof global.sj_be !== "function") {
      global.sj_be = window.sj_be = function (el, type, fn) {
        if (el && typeof el.addEventListener === "function" && typeof fn === "function") {
          el.addEventListener(String(type), fn);
        }
      };
    }
    if (typeof global.sj_ue !== "function") {
      global.sj_ue = window.sj_ue = function (el, type, fn) {
        if (el && typeof el.removeEventListener === "function" && typeof fn === "function") {
          el.removeEventListener(String(type), fn);
        }
      };
    }
    if (typeof global.sj_evt === "undefined") {
      global.sj_evt = window.sj_evt = soft({
        bind: function (type, fn) { window.addEventListener(String(type), fn); },
        fire: function (type) { window.dispatchEvent(new Event(String(type))); },
        unbind: function (type, fn) { window.removeEventListener(String(type), fn); }
      });
    }
  };

  // 探针放在 shim 末尾，避免初始化阶段读未定义属性时误伤。
  try {
    var probedDocument = probe(document, "document");
    var probedWindow = probe(window, "window");
    global.document = probedDocument;
    global.window = probedWindow;
    probedWindow.document = probedDocument;
    probedWindow.window = probedWindow;
    probedWindow.self = probedWindow;
    probedWindow.top = probedWindow;
    probedWindow.parent = probedWindow;
    global._w = probedWindow._w = probedWindow;
    global._d = probedWindow._d = probedDocument;
  } catch (e) {}
})(typeof globalThis !== "undefined" ? globalThis : (0, eval)("this"));
"#;

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
            let node_id = NodeId(index);
            if !doc.is_connected(node_id) {
                return None;
            }
            let NodeData::Element { tag: node_tag, .. } = &node.data else {
                return None;
            };
            node_tag.eq_ignore_ascii_case(tag).then_some(node_id)
        })
        .collect()
}

fn query_many(doc: &Document, selector: &str) -> Vec<NodeId> {
    doc.nodes
        .iter()
        .enumerate()
        .filter_map(|(index, _)| {
            let id = NodeId(index);
            (doc.is_connected(id) && query_one_node(doc, id, selector)).then_some(id)
        })
        .collect()
}

fn query_under(doc: &Document, root: NodeId, selector: &str) -> Option<NodeId> {
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        if id != root && query_one_node(doc, id, selector) {
            return Some(id);
        }
        stack.extend(doc.nodes[id.0].children.iter().copied().rev());
    }
    None
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
        let node_id = NodeId(index);
        if !doc.is_connected(node_id) {
            return None;
        }
        let NodeData::Element { attrs, .. } = &node.data else {
            return None;
        };
        attrs.iter().any(|(key, value)| key.eq_ignore_ascii_case("id") && value == id).then_some(node_id)
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

fn serialize_children(doc: &Document, id: NodeId) -> String {
    let mut out = String::new();
    for child in &doc.nodes[id.0].children {
        serialize_node(doc, *child, &mut out);
    }
    out
}

fn serialize_node(doc: &Document, id: NodeId, out: &mut String) {
    match &doc.nodes[id.0].data {
        NodeData::Text(text) => out.push_str(&escape_html_text(text)),
        NodeData::Element { tag, attrs } => {
            if tag == "#document-fragment" || tag == "!--" {
                for child in &doc.nodes[id.0].children {
                    serialize_node(doc, *child, out);
                }
                return;
            }
            out.push('<');
            out.push_str(tag);
            for (name, value) in attrs {
                out.push(' ');
                out.push_str(name);
                out.push_str("=\"");
                out.push_str(&escape_html_attr(value));
                out.push('"');
            }
            if is_void_tag(tag) {
                out.push_str(" />");
                return;
            }
            out.push('>');
            for child in &doc.nodes[id.0].children {
                serialize_node(doc, *child, out);
            }
            out.push_str("</");
            out.push_str(tag);
            out.push('>');
        }
        NodeData::Document => {
            for child in &doc.nodes[id.0].children {
                serialize_node(doc, *child, out);
            }
        }
    }
}

fn escape_html_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_html_attr(text: &str) -> String {
    escape_html_text(text).replace('"', "&quot;")
}

fn is_void_tag(tag: &str) -> bool {
    matches!(
        tag.to_ascii_lowercase().as_str(),
        "area" | "base" | "br" | "col" | "embed" | "hr" | "img" | "input" | "link" | "meta"
            | "param" | "source" | "track" | "wbr"
    )
}

fn class_tokens(doc: &Document, id: NodeId) -> Vec<String> {
    doc.attr(id, "class")
        .unwrap_or("")
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

fn class_list_contains(doc: &Document, id: NodeId, class: &str) -> bool {
    class_tokens(doc, id).iter().any(|item| item == class)
}

fn class_list_add(doc: &mut Document, id: NodeId, class: &str) {
    if class.is_empty() || class_list_contains(doc, id, class) {
        return;
    }
    let mut tokens = class_tokens(doc, id);
    tokens.push(class.to_string());
    doc.set_attr(id, "class", tokens.join(" "));
}

fn class_list_remove(doc: &mut Document, id: NodeId, class: &str) {
    let tokens: Vec<_> = class_tokens(doc, id)
        .into_iter()
        .filter(|item| item != class)
        .collect();
    doc.set_attr(id, "class", tokens.join(" "));
}

fn class_list_toggle(doc: &mut Document, id: NodeId, class: &str) -> bool {
    if class_list_contains(doc, id, class) {
        class_list_remove(doc, id, class);
        false
    } else {
        class_list_add(doc, id, class);
        true
    }
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

fn location_part(href: &str, part: &str) -> String {
    let Ok(parsed) = url::Url::parse(href) else {
        return String::new();
    };
    match part {
        "protocol" => format!("{}:", parsed.scheme()),
        "hostname" => parsed.host_str().unwrap_or("").to_string(),
        "port" => parsed.port().map(|port| port.to_string()).unwrap_or_default(),
        "host" => match (parsed.host_str(), parsed.port()) {
            (Some(host), Some(port)) => format!("{host}:{port}"),
            (Some(host), None) => host.to_string(),
            _ => String::new(),
        },
        "pathname" => parsed.path().to_string(),
        "search" => parsed.query().map(|query| format!("?{query}")).unwrap_or_default(),
        "hash" => parsed.fragment().map(|fragment| format!("#{fragment}")).unwrap_or_default(),
        "origin" => parsed.origin().ascii_serialization(),
        _ => String::new(),
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
        let nav = run_inline_scripts(&mut doc, "https://www.baidu.com/")
            .navigation
            .expect("navigation");
        assert_eq!(nav.url, "http://www.baidu.com/");
        assert_eq!(nav.kind, NavigationKind::Replace);
    }

    #[test]
    fn location_exposes_url_parts() {
        let mut doc = parse(
            r#"<body><p id="msg"></p><script>
            var path = location.pathname;
            var host = location.hostname;
            document.getElementById("msg").textContent =
              location.protocol + "|" + host + "|" + path + "|" + (host.match(/bing/) ? "ok" : "no") + "|" + path.indexOf("/");
        </script></body>"#,
        );
        let result = run_scripts_with(&mut doc, "https://www.bing.com/search?q=1", &|_| Err("无外部脚本".into()));
        assert_eq!(result.failures, 0, "samples={:?}", result.samples);
        assert_eq!(
            doc.text_content(find_id(&doc, "msg").unwrap()),
            "https:|www.bing.com|/search|ok|0"
        );
    }

    #[test]
    fn location_href_assignment() {
        let html = r#"<script>location.href = "https://example.com/next";</script>"#;
        let mut doc = parse(html);
        let nav = run_inline_scripts(&mut doc, "https://example.com/")
            .navigation
            .expect("navigation");
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
        let nav = run_scripts_with(&mut doc, "https://example.com/", &|_| Err("跳过外部脚本".into()))
            .navigation
            .expect("navigation");
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
        let result = run_scripts_with(&mut doc, "https://example.com/", &|_| Err("无外部脚本".into()));
        assert!(result.navigation.is_none());
        let text = doc.text_content(doc.root());
        assert!(text.contains("Win32"), "{text}");
        assert!(!text.contains("old"), "{text}");
    }

    #[test]
    fn image_and_define_require() {
        let mut doc = parse(r#"<body><p id="msg"></p><script>
            var img = new Image();
            img.src = "https://example.com/a.png";
            document.body.appendChild(img);
            define("mod", [], function () { return { ok: "ready" }; });
            require(["mod"], function (mod) {
                document.getElementById("msg").textContent = mod.ok + img.src;
            });
        </script></body>"#);
        assert!(run_scripts_with(&mut doc, "https://example.com/", &|_| Err("无外部脚本".into()))
            .navigation
            .is_none());
        let text = doc.text_content(doc.root());
        assert!(text.contains("readyhttps://example.com/a.png"), "{text}");
        assert!(doc.nodes.iter().any(|node| matches!(&node.data, NodeData::Element { tag, attrs } if tag == "img" && attrs.iter().any(|(key, value)| key == "src" && value == "https://example.com/a.png"))));
    }

    #[test]
    fn xml_http_request_reports_status_and_body() {
        let mut doc = parse(r#"<body><p id="msg"></p><script>
            __pelagoHttp = function (method, url) {
                return { status: 200, body: method + " " + url };
            };
            var xhr = new XMLHttpRequest();
            xhr.open("GET", "/ping");
            xhr.send();
            document.getElementById("msg").textContent = xhr.status + " " + xhr.responseText;
        </script></body>"#);
        assert!(run_scripts_with(&mut doc, "https://example.com/app/", &|_| Err("无外部脚本".into()))
            .navigation
            .is_none());
        let text = doc.text_content(doc.root());
        assert!(text.contains("200 GET /ping"), "{text}");
    }

    #[test]
    fn dom_events_and_dynamic_script() {
        let mut doc = parse(r#"<body><p id="msg">x</p><script>
            var seen = [];
            document.addEventListener("DOMContentLoaded", function () { seen.push("ready"); });
            window.addEventListener("load", function () { seen.push("load"); });
            var box = document.createElement("div");
            var child = document.createElement("span");
            child.textContent = "n";
            box.appendChild(child);
            document.body.appendChild(box);
            var script = document.createElement("script");
            script.src = "extra.js";
            document.body.appendChild(script);
            document.getElementById("msg").textContent = seen.join(",") + child.textContent;
        </script></body>"#);
        run_scripts_with(&mut doc, "https://example.com/", &|url| {
            if url.ends_with("extra.js") {
                Ok(br#"document.getElementById("msg").textContent += "|dyn";"#.to_vec())
            } else {
                Err("无此脚本".into())
            }
        });
        // dynamic script uses net fetch via ACTIVE_BASE, not the test fetch callback.
        // So we only assert event wiring + DOM mutation here.
        let text = doc.text_content(doc.root());
        assert!(text.contains("n"), "{text}");
        assert!(doc.nodes.iter().any(|node| matches!(&node.data, NodeData::Element { tag, .. } if tag == "div")));
    }

    #[test]
    fn insert_before_and_remove_child() {
        let mut doc = parse(r#"<body><div id="box"><span id="b">B</span></div><script>
            var box = document.getElementById("box");
            var a = document.createElement("span");
            a.id = "a";
            a.textContent = "A";
            box.insertBefore(a, document.getElementById("b"));
            box.removeChild(document.getElementById("b"));
        </script></body>"#);
        assert!(run_scripts_with(&mut doc, "https://example.com/", &|_| Err("无外部脚本".into()))
            .navigation
            .is_none());
        let box_id = find_id(&doc, "box").expect("box");
        let kids = doc.element_children(box_id);
        assert_eq!(kids.len(), 1, "kids={kids:?}");
        assert_eq!(doc.attr(kids[0], "id"), Some("a"));
        assert_eq!(doc.text_content(kids[0]), "A");
        assert!(find_id(&doc, "b").is_none());
    }

    #[test]
    fn timer_queue_runs_after_script() {
        let mut doc = parse(
            r#"<body><p id="msg">0</p><script>
            setTimeout(function () {
              document.getElementById("msg").textContent = "1";
              setTimeout(function () {
                document.getElementById("msg").textContent = "2";
              }, 0);
            }, 0);
        </script></body>"#,
        );
        let result = run_scripts_with(&mut doc, "https://example.com/", &|_| Err("无外部脚本".into()));
        assert_eq!(result.failures, 0);
        assert_eq!(doc.text_content(find_id(&doc, "msg").unwrap()), "2");
    }

    #[test]
    fn script_failure_isolation_continues() {
        let mut doc = parse(
            r#"<body><p id="msg">x</p>
            <script>throw new Error("boom");</script>
            <script>document.getElementById("msg").textContent = "ok";</script>
            </body>"#,
        );
        let result = run_scripts_with(&mut doc, "https://example.com/", &|_| Err("无外部脚本".into()));
        assert!(result.failures >= 1);
        assert_eq!(doc.text_content(find_id(&doc, "msg").unwrap()), "ok");
    }

    #[test]
    fn top_level_var_persists_across_scripts() {
        let mut doc = parse(
            r#"<body><p id="msg"></p>
            <script>"use strict"; var _d; typeof _d=="undefined"&&(_d=document);</script>
            <script>"use strict"; document.getElementById("msg").textContent = typeof _d;</script>
            </body>"#,
        );
        let result = run_scripts_with(&mut doc, "https://example.com/", &|_| Err("无外部脚本".into()));
        assert_eq!(result.failures, 0, "samples={:?}", result.samples);
        assert_eq!(doc.text_content(find_id(&doc, "msg").unwrap()), "object");
    }

    #[test]
    fn top_level_var_survives_later_throw() {
        let mut doc = parse(
            r#"<body><p id="msg"></p>
            <script>"use strict"; var marker; marker=document; notDefinedAtAll.x = 1;</script>
            <script>"use strict"; document.getElementById("msg").textContent = typeof marker;</script>
            </body>"#,
        );
        let result = run_scripts_with(&mut doc, "https://example.com/", &|_| Err("无外部脚本".into()));
        assert!(result.failures >= 1, "samples={:?}", result.samples);
        assert_eq!(doc.text_content(find_id(&doc, "msg").unwrap()), "object");
    }

    #[test]
    fn api_miss_probe_records_unknown_props() {
        let mut doc = parse(
            r#"<body><script>
            void document.pelagoDefinitelyMissing;
            void window.pelagoAlsoMissing;
        </script></body>"#,
        );
        let result = run_scripts_with(&mut doc, "https://example.com/", &|_| Err("无外部脚本".into()));
        assert!(
            result
                .api_misses
                .iter()
                .any(|(name, _)| name.contains("pelagoDefinitelyMissing") || name.contains("pelagoAlsoMissing")),
            "misses={:?}",
            result.api_misses
        );
    }

    #[test]
    fn inner_html_get_and_class_list() {
        let mut doc = parse(
            r#"<body><div id="box" class="a"><span>x</span></div><script>
            var box = document.getElementById("box");
            box.classList.add("b");
            box.classList.remove("a");
            document.body.setAttribute("data-html", box.innerHTML);
            document.body.setAttribute("data-has", String(box.classList.contains("b")));
        </script></body>"#,
        );
        let result = run_scripts_with(&mut doc, "https://example.com/", &|_| Err("无外部脚本".into()));
        assert_eq!(result.failures, 0, "samples={:?}", result.samples);
        let body = find_tag(&doc, "body").expect("body");
        assert!(doc.attr(body, "data-html").unwrap_or("").contains("<span>"), "{:?}", doc.attr(body, "data-html"));
        assert_eq!(doc.attr(body, "data-has"), Some("true"));
        assert_eq!(doc.attr(find_id(&doc, "box").unwrap(), "class"), Some("b"));
    }

    #[test]
    fn bing_style_aliases_exist() {
        let mut doc = parse(
            r#"<body><p id="msg"></p><script>
            document.getElementById("msg").textContent =
              (typeof _w !== "undefined") + "," + (typeof _d !== "undefined") + "," + (typeof _G !== "undefined");
        </script></body>"#,
        );
        let result = run_scripts_with(&mut doc, "https://example.com/", &|_| Err("无外部脚本".into()));
        assert_eq!(result.failures, 0, "samples={:?}", result.samples);
        assert_eq!(doc.text_content(find_id(&doc, "msg").unwrap()), "true,true,true");
    }

    #[test]
    fn window_property_mirrors_to_global() {
        let mut doc = parse(
            r#"<body><p id="msg"></p>
            <script>window._G = { ok: 1 };</script>
            <script>document.getElementById("msg").textContent = String(_G.ok);</script>
            </body>"#,
        );
        let result = run_scripts_with(&mut doc, "https://example.com/", &|_| Err("无外部脚本".into()));
        assert_eq!(result.failures, 0, "samples={:?}", result.samples);
        assert_eq!(doc.text_content(find_id(&doc, "msg").unwrap()), "1");
    }
}
