mod app;
mod engine;
mod net;

use app::PageSource;

const HOME_HTML: &str = r#"<!doctype html>
<html>
<head><meta charset="utf-8"><title>Pelago</title>
<style>
body { font-family: sans-serif; margin: 48px; color: #222; }
h1 { font-size: 28px; margin: 0 0 12px; }
p { margin: 0; color: #555; }
</style>
</head>
<body>
<h1>Pelago</h1>
<p>在上方地址栏输入网址并回车开始浏览。</p>
</body>
</html>"#;

fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("--smoke") => {
            let Some(url) = args.next() else {
                eprintln!("用法: pelago --smoke <url>");
                std::process::exit(2);
            };
            match engine::js::smoke_page(&url) {
                Ok(result) => {
                    std::process::exit(if result.failures == 0 { 0 } else { 1 });
                }
                Err(err) => {
                    eprintln!("smoke 失败：{err}");
                    std::process::exit(1);
                }
            }
        }
        Some("--html") => {
            let Some(html) = args.next() else {
                eprintln!("用法: pelago --html <html>");
                std::process::exit(2);
            };
            if let Err(err) = app::open(PageSource::Html(html)) {
                eprintln!("{err}");
                std::process::exit(1);
            }
        }
        Some(url) => {
            if let Err(err) = app::open(PageSource::Url(url.to_string())) {
                eprintln!("{err}");
                std::process::exit(1);
            }
        }
        None => {
            if let Err(err) = app::open(PageSource::Html(HOME_HTML.to_string())) {
                eprintln!("{err}");
                std::process::exit(1);
            }
        }
    }
}
