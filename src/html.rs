use crate::dom::{Document, NodeData, NodeId};

const VOID_TAGS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

pub fn parse(input: &str) -> Document {
    let mut parser = Parser {
        src: input,
        pos: 0,
        doc: Document::new(),
        stack: vec![NodeId(0)],
    };
    parser.run();
    parser.doc
}

struct Parser<'a> {
    src: &'a str,
    pos: usize,
    doc: Document,
    stack: Vec<NodeId>,
}

impl<'a> Parser<'a> {
    fn run(&mut self) {
        while self.pos < self.src.len() {
            if self.starts_with("<!--") {
                self.skip_comment();
                continue;
            }
            if self.starts_with("<!") || self.starts_with("<?") {
                self.skip_until('>');
                continue;
            }
            if self.starts_with("</") {
                self.parse_end_tag();
                continue;
            }
            if self.starts_with("<") && self.next_is_tag_name() {
                self.parse_start_tag();
                continue;
            }
            self.parse_text();
        }
    }

    fn parse_start_tag(&mut self) {
        self.bump("<".len());
        let name = self.read_name();
        if name.is_empty() {
            self.append_text("<".to_string());
            return;
        }
        let mut attrs = Vec::new();
        let mut self_closing = false;
        loop {
            self.skip_ascii_ws();
            match self.peek_char() {
                Some('/') => {
                    self.consume_char();
                    self_closing = true;
                }
                Some('>') => {
                    self.consume_char();
                    break;
                }
                Some(ch) if is_name_char(ch) => {
                    let attr_name = self.read_name();
                    self.skip_ascii_ws();
                    let value = if self.peek_char() == Some('=') {
                        self.consume_char();
                        self.skip_ascii_ws();
                        self.read_attr_value()
                    } else {
                        String::new()
                    };
                    attrs.push((attr_name, decode_entities(&value)));
                }
                Some(_) => {
                    self.consume_char();
                }
                None => break,
            }
        }

        let id = self.doc.push(
            NodeData::Element {
                tag: name.clone(),
                attrs,
            },
            self.parent(),
        );
        let raw = is_raw(&name);
        let void = self_closing || is_void(&name);
        if !void {
            self.stack.push(id);
        }
        if raw {
            let text = self.read_raw_text(&name);
            if !text.is_empty() {
                self.doc.push(NodeData::Text(text), id);
            }
            self.stack.pop();
        }
    }

    fn parse_end_tag(&mut self) {
        self.bump("</".len());
        let name = self.read_name();
        self.skip_until('>');
        if let Some(index) = self.stack.iter().rposition(|id| element_tag(&self.doc, *id) == Some(name.as_str()))
        {
            self.stack.truncate(index);
        }
    }

    fn parse_text(&mut self) {
        let start = self.pos;
        while self.pos < self.src.len() && !self.starts_with("<") {
            let ch = self.consume_char().unwrap();
            let _ = ch;
        }
        let raw = &self.src[start..self.pos];
        let decoded = decode_entities(raw);
        if !decoded.is_empty() {
            self.append_text(decoded);
        }
    }

    fn read_raw_text(&mut self, tag: &str) -> String {
        let rest = &self.src[self.pos..];
        let lower = rest.to_ascii_lowercase();
        let marker = format!("</{tag}");
        let end = lower.find(&marker).unwrap_or(rest.len());
        let text = rest[..end].to_string();
        self.pos += end;
        if self.starts_with("</") {
            self.skip_until('>');
        }
        text
    }

    fn read_name(&mut self) -> String {
        let start = self.pos;
        while let Some(ch) = self.peek_char() {
            if is_name_char(ch) {
                self.consume_char();
            } else {
                break;
            }
        }
        self.src[start..self.pos].to_ascii_lowercase()
    }

    fn read_attr_value(&mut self) -> String {
        match self.peek_char() {
            Some(quote @ ('"' | '\'')) => {
                self.consume_char();
                let start = self.pos;
                while let Some(ch) = self.peek_char() {
                    if ch == quote {
                        break;
                    }
                    self.consume_char();
                }
                let value = self.src[start..self.pos].to_string();
                if self.peek_char() == Some(quote) {
                    self.consume_char();
                }
                value
            }
            _ => {
                let start = self.pos;
                while let Some(ch) = self.peek_char() {
                    if ch.is_ascii_whitespace() || ch == '>' {
                        break;
                    }
                    self.consume_char();
                }
                self.src[start..self.pos].to_string()
            }
        }
    }

    fn append_text(&mut self, text: String) {
        self.doc.push(NodeData::Text(text), self.parent());
    }

    fn parent(&self) -> NodeId {
        *self.stack.last().unwrap()
    }

    fn skip_comment(&mut self) {
        self.bump("<!--".len());
        if let Some(rel) = self.src[self.pos..].find("-->") {
            self.pos += rel + "-->".len();
        } else {
            self.pos = self.src.len();
        }
    }

    fn skip_until(&mut self, needle: char) {
        while let Some(ch) = self.consume_char() {
            if ch == needle {
                break;
            }
        }
    }

    fn skip_ascii_ws(&mut self) {
        while let Some(ch) = self.peek_char() {
            if ch.is_ascii_whitespace() {
                self.consume_char();
            } else {
                break;
            }
        }
    }

    fn next_is_tag_name(&self) -> bool {
        self.src[self.pos..]
            .chars()
            .nth(1)
            .is_some_and(is_name_char)
    }

    fn starts_with(&self, prefix: &str) -> bool {
        self.src[self.pos..].starts_with(prefix)
    }

    fn bump(&mut self, len: usize) {
        self.pos += len;
    }

    fn peek_char(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn consume_char(&mut self) -> Option<char> {
        let ch = self.peek_char()?;
        self.pos += ch.len_utf8();
        Some(ch)
    }
}

fn element_tag<'a>(doc: &'a Document, id: NodeId) -> Option<&'a str> {
    match &doc.nodes[id.0].data {
        NodeData::Element { tag, .. } => Some(tag.as_str()),
        _ => None,
    }
}

fn is_name_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '-' || ch == ':'
}

fn is_void(tag: &str) -> bool {
    VOID_TAGS.contains(&tag)
}

fn is_raw(tag: &str) -> bool {
    tag == "script" || tag == "style"
}

fn decode_entities(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(index) = rest.find('&') {
        out.push_str(&rest[..index]);
        rest = &rest[index..];
        if let Some((ch, consumed)) = parse_reference(rest) {
            out.push(ch);
            rest = &rest[consumed..];
        } else {
            out.push('&');
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    out
}

fn parse_reference(input: &str) -> Option<(char, usize)> {
    let end = input.find(';')?;
    let body = &input[1..end];
    let ch = if let Some(hex) = body.strip_prefix("#x").or_else(|| body.strip_prefix("#X")) {
        char::from_u32(u32::from_str_radix(hex, 16).ok()?)?
    } else if let Some(decimal) = body.strip_prefix('#') {
        char::from_u32(decimal.parse().ok()?)?
    } else {
        match body {
            "lt" => '<',
            "gt" => '>',
            "amp" => '&',
            "quot" => '"',
            "apos" => '\'',
            "nbsp" => '\u{00A0}',
            _ => return None,
        }
    };
    Some((ch, end + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element<'a>(doc: &'a Document, id: NodeId) -> Option<(&'a str, &'a [(String, String)])> {
        match &doc.nodes[id.0].data {
            NodeData::Element { tag, attrs } => Some((tag.as_str(), attrs.as_slice())),
            _ => None,
        }
    }

    fn texts(doc: &Document) -> Vec<&str> {
        doc.nodes
            .iter()
            .filter_map(|node| match &node.data {
                NodeData::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn nested_tags_attributes_and_void_elements() {
        let doc = parse(r#"<div id="main"><p class="a">Hi<br>there</p><img src="x.png"></div>"#);
        let root_children = &doc.nodes[0].children;
        let (tag, attrs) = element(&doc, root_children[0]).unwrap();
        assert_eq!(tag, "div");
        assert_eq!(attrs, &[("id".into(), "main".into())]);
        assert_eq!(texts(&doc), vec!["Hi", "there"]);
        let html = r#"<img src="x.png">"#;
        let images = parse(html);
        let img = images.nodes[0].children[0];
        assert!(images.nodes[img.0].children.is_empty());
    }

    #[test]
    fn comments_entities_and_raw_script() {
        let doc = parse("<!-- hidden --><p>a&amp;b &lt;c&gt; &#65; &#x42; &nbsp;</p><script>if (a < b) { }</script><p>ok</p>");
        let joined = texts(&doc).join("|");
        assert!(joined.contains("a&b <c> A B \u{00A0}"));
        assert!(joined.contains("if (a < b) { }"));
        assert!(joined.contains("ok"));
        assert!(!joined.contains("hidden"));
    }

    #[test]
    fn mismatched_end_tag_keeps_following_text() {
        let doc = parse("</div><p>x</p>");
        assert_eq!(texts(&doc), vec!["x"]);
    }
}
