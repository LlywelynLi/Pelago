mod chrome;
mod dom;
mod html;
mod image;
mod js;
mod layout;
mod net;
mod paint;
mod style;
mod text;
mod urlutil;
mod window;

use window::PageSource;

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
    let source = match args.next().as_deref() {
        Some("--html") => {
            let Some(html) = args.next() else {
                eprintln!("用法: pelago --html <html>");
                std::process::exit(2);
            };
            PageSource::Html(html)
        }
        Some(url) => PageSource::Url(url.to_string()),
        None => PageSource::Html(HOME_HTML.to_string()),
    };

    if let Err(err) = window::open(source) {
        eprintln!("{err}");
        std::process::exit(1);
    }
}
