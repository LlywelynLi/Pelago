mod chrome;
mod dom;
mod html;
mod image;
mod layout;
mod net;
mod paint;
mod style;
mod text;
mod urlutil;
mod window;

use window::PageSource;

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
        None => {
            eprintln!("用法: pelago <url>");
            eprintln!("      pelago --html <html>");
            std::process::exit(2);
        }
    };

    if let Err(err) = window::open(source) {
        eprintln!("{err}");
        std::process::exit(1);
    }
}
