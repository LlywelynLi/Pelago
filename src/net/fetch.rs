use std::fmt;
use std::time::Duration;

use encoding_rs::{Encoding, UTF_8};
use ureq::Agent;

const MAX_BODY: u64 = 2 * 1024 * 1024;

#[derive(Debug)]
pub struct Page {
    pub final_url: String,
    pub html: String,
}

#[derive(Debug)]
pub enum FetchError {
    BadUrl(String),
    Http(String),
    NotHtml(String),
    Body(String),
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FetchError::BadUrl(message) => write!(f, "{message}"),
            FetchError::Http(message) => write!(f, "{message}"),
            FetchError::NotHtml(mime) => write!(f, "响应不是 HTML：{mime}"),
            FetchError::Body(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for FetchError {}

pub fn fetch(url: &str) -> Result<Page, FetchError> {
    let response = fetch_bytes(url, MAX_BODY)?;
    if let Some(mime) = response
        .content_type
        .as_deref()
        .and_then(|value| value.split(';').next())
        .map(str::trim)
    {
        if !mime.eq_ignore_ascii_case("text/html") {
            return Err(FetchError::NotHtml(mime.to_string()));
        }
    }
    let html = decode_html(&response.bytes, response.content_type.as_deref());
    Ok(Page {
        final_url: url.trim().to_string(),
        html,
    })
}

pub struct BytesResponse {
    pub content_type: Option<String>,
    pub bytes: Vec<u8>,
}

pub fn fetch_bytes(url: &str, limit: u64) -> Result<BytesResponse, FetchError> {
    let url = url.trim();
    let scheme = url.split(':').next().unwrap_or("");
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return Err(FetchError::BadUrl(
            "只支持 http 和 https 地址".to_string(),
        ));
    }

    let config = Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .max_redirects(5)
        .build();
    let agent: Agent = config.into();
    let mut response = agent
        .get(url)
        .call()
        .map_err(|err| FetchError::Http(err.to_string()))?;

    let status = response.status();
    if !status.is_success() {
        return Err(FetchError::Http(format!("HTTP {status}")));
    }

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .map_err(|err| FetchError::Body(err.to_string()))?;
    Ok(BytesResponse { content_type, bytes })
}

pub fn decode_html(bytes: &[u8], content_type: Option<&str>) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return decode(rest, UTF_8);
    }
    if let Some(label) = content_type.and_then(charset_label) {
        if let Some(encoding) = Encoding::for_label(label.as_bytes()) {
            return decode(bytes, encoding);
        }
    }
    let head_len = bytes.len().min(1024);
    if let Some(encoding) = meta_charset(&bytes[..head_len]) {
        return decode(bytes, encoding);
    }
    decode(bytes, UTF_8)
}

fn decode(bytes: &[u8], encoding: &'static Encoding) -> String {
    let (text, _, _) = encoding.decode(bytes);
    text.into_owned()
}

fn charset_label(content_type: &str) -> Option<String> {
    label_after(content_type, "charset=")
}

fn meta_charset(head: &[u8]) -> Option<&'static Encoding> {
    let text = String::from_utf8_lossy(head);
    let lower = text.to_ascii_lowercase();
    let label = label_after(&lower, "charset=")?;
    Encoding::for_label(label.as_bytes())
}

fn label_after(text: &str, marker: &str) -> Option<String> {
    let index = text.to_ascii_lowercase().find(marker)?;
    let mut rest = text[index + marker.len()..].trim_start();
    rest = rest.trim_start_matches(['"', '\'']);
    let end = rest
        .find(|ch: char| ch == '"' || ch == '\'' || ch == ';' || ch == '>' || ch.is_whitespace())
        .unwrap_or(rest.len());
    let label = rest[..end].trim();
    if label.is_empty() {
        None
    } else {
        Some(label.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_bom_content_type_meta_and_utf8() {
        assert_eq!(
            decode_html(&[0xEF, 0xBB, 0xBF, b'A'], Some("text/html; charset=gbk")),
            "A"
        );
        let gbk = decode_html(&[0xC4, 0xE3], Some("text/html; charset=gbk"));
        assert_eq!(gbk, "你");
        let meta = b"<meta charset=\"gbk\">\xC4\xE3";
        assert_eq!(decode_html(meta, None), "<meta charset=\"gbk\">你");
        assert_eq!(decode_html("你好".as_bytes(), None), "你好");
    }

    #[test]
    fn rejects_non_http_urls() {
        let err = fetch("file:///C:/tmp.html").unwrap_err();
        assert!(err.to_string().contains("http"));
    }

    #[test]
    fn fetches_example_domain() {
        let page = fetch("https://example.com").expect("example.com");
        let runs = crate::engine::text::text_runs(&crate::engine::html::parse(&page.html));
        assert!(
            runs.iter().any(|line| line.contains("documentation examples")),
            "正文里没有页面文字：{runs:?}"
        );
    }
}
