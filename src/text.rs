#![cfg_attr(not(test), allow(dead_code))]

use crate::dom::{Document, NodeData, NodeId};

pub fn text_runs(doc: &Document) -> Vec<String> {
    let mut collector = Collector {
        runs: Vec::new(),
        current: String::new(),
        pre: 0,
    };
    collector.walk(doc, doc.root());
    collector.flush();
    collector.runs
}

struct Collector {
    runs: Vec<String>,
    current: String,
    pre: u32,
}

impl Collector {
    fn walk(&mut self, doc: &Document, id: NodeId) {
        let node = &doc.nodes[id.0];
        match &node.data {
            NodeData::Document => {
                for child in node.children.clone() {
                    self.walk(doc, child);
                }
            }
            NodeData::Text(text) => {
                append_inline(&mut self.current, text, self.pre > 0);
            }
            NodeData::Element { tag, .. } => {
                if matches!(tag.as_str(), "head" | "script" | "style" | "template") {
                    return;
                }
                let block = tag == "br" || is_block(tag);
                if block {
                    self.flush();
                }
                if tag == "pre" {
                    self.pre += 1;
                }
                if tag != "br" {
                    for child in node.children.clone() {
                        self.walk(doc, child);
                    }
                }
                if tag == "pre" {
                    self.pre -= 1;
                }
                if block {
                    self.flush();
                }
            }
        }
    }

    fn flush(&mut self) {
        let line = self.current.trim().to_string();
        self.current.clear();
        if !line.is_empty() {
            self.runs.push(line);
        }
    }
}

fn is_block(tag: &str) -> bool {
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

fn append_inline(current: &mut String, raw: &str, preserve: bool) {
    if preserve {
        current.push_str(raw);
        return;
    }
    let leading = raw.chars().next().is_some_and(char::is_whitespace);
    let trailing = raw.chars().next_back().is_some_and(char::is_whitespace);
    let collapsed = collapse_inner(raw);
    if collapsed.is_empty() {
        if (leading || trailing) && !current.is_empty() && !current.ends_with(' ') {
            current.push(' ');
        }
        return;
    }
    if !current.is_empty() && !current.ends_with(' ') && leading {
        current.push(' ');
    }
    current.push_str(&collapsed);
    if trailing {
        current.push(' ');
    }
}

fn collapse_inner(raw: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::parse;

    #[test]
    fn block_breaks_skip_head_and_collapse_space() {
        let doc = parse(
            r#"<!DOCTYPE html>
            <html><head><title>Secret</title><script>if (a < b) {}</script></head>
            <body>
              <h1>Example &amp; Domain</h1>
              <p>Hello <b>world</b></p>
              <p>你好</p>
            </body></html>"#,
        );
        assert_eq!(
            text_runs(&doc),
            vec![
                "Example & Domain".to_string(),
                "Hello world".to_string(),
                "你好".to_string(),
            ]
        );
    }

    #[test]
    fn br_splits_a_line() {
        let doc = parse("<p>a<br>b</p>");
        assert_eq!(text_runs(&doc), vec!["a".to_string(), "b".to_string()]);
    }
}
