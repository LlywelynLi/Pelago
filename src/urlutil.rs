use url::Url;

pub fn normalize_navigation(input: &str) -> Result<String, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("地址是空的".to_string());
    }
    if let Ok(url) = Url::parse(input) {
        if url.scheme() == "http" || url.scheme() == "https" {
            return Ok(url.to_string());
        }
    }
    if let Some(rest) = input.strip_prefix("//") {
        return normalize_navigation(&format!("https://{rest}"));
    }
    Url::parse(&format!("https://{input}"))
        .map(|url| url.to_string())
        .map_err(|_| "无法识别这个地址".to_string())
}

pub fn resolve_url(base: Option<&str>, reference: &str) -> Option<String> {
    let reference = reference.trim();
    if reference.is_empty()
        || reference.starts_with('#')
        || reference.to_ascii_lowercase().starts_with("javascript:")
        || reference.to_ascii_lowercase().starts_with("mailto:")
    {
        return None;
    }
    if reference.to_ascii_lowercase().starts_with("data:image/") {
        return Some(reference.to_string());
    }
    if let Some(base) = base.and_then(|value| Url::parse(value).ok()) {
        if let Ok(joined) = base.join(reference) {
            if joined.scheme() == "http" || joined.scheme() == "https" {
                return Some(joined.to_string());
            }
        }
    }
    normalize_navigation(reference).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_https_and_joins_relative_links() {
        assert_eq!(
            normalize_navigation("example.com/docs").unwrap(),
            "https://example.com/docs"
        );
        assert_eq!(
            resolve_url(Some("https://example.com/dir/page"), "/next").as_deref(),
            Some("https://example.com/next")
        );
        assert_eq!(
            resolve_url(Some("https://example.com/dir/page"), "pic.png").as_deref(),
            Some("https://example.com/dir/pic.png")
        );
        assert!(resolve_url(Some("https://example.com/"), "#top").is_none());
    }
}
