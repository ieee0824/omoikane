//! Legacy-encoded pages follow the same navigation and paint path as the GUI.

use std::io::Write;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use omoikane::font::load_default_text_fonts;
use omoikane::frame::BrowserFrame;
use omoikane::platform_browser::PlatformBrowser;
use serde_json::{Value, json};

#[path = "support/http_fixture.rs"]
mod http_fixture;
use http_fixture::{
    FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback, read_request_headers,
};

const TITLE: &str = "日本語のホームページ";
const TEXT: &str = "阿部寛の紹介です。";
const NEXT_TITLE: &str = "次の日本語ページ";
const NEXT_TEXT: &str = "出演作品のお知らせ。";

struct Server {
    origin: String,
    stop: Arc<AtomicBool>,
    worker: Option<FixtureWorker<()>>,
}

impl Server {
    fn start() -> Self {
        let listener = bind_loopback().unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker = FixtureWorker::spawn(move || {
            while !worker_stop.load(Ordering::Relaxed) {
                let mut stream = match accept_with_timeout(&listener, Duration::from_millis(50)) {
                    Ok(stream) => stream,
                    Err(error) if error.kind() == std::io::ErrorKind::TimedOut => continue,
                    Err(error) => panic!("fixture accept: {error}"),
                };
                stream.set_write_timeout(Some(READ_TIMEOUT)).unwrap();
                let request = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
                let path = request.split_whitespace().nth(1).unwrap();
                let (content_type, body) = response(path);
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
                stream.write_all(body).unwrap();
            }
        });
        Self {
            origin,
            stop,
            worker: Some(worker),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.origin)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.worker.take().unwrap().join();
    }
}

fn response(path: &str) -> (&'static str, &'static [u8]) {
    let body: &[u8] = match path {
        "/sjis/index.html" => include_bytes!("fixtures/legacy-encoding/index.sjis.html"),
        "/sjis/page.html" => include_bytes!("fixtures/legacy-encoding/page.sjis.html"),
        "/sjis/next.html" => include_bytes!("fixtures/legacy-encoding/next.sjis.html"),
        "/utf8/index.html" => include_bytes!("fixtures/legacy-encoding/index.utf8.html"),
        "/utf8/page.html" => include_bytes!("fixtures/legacy-encoding/page.utf8.html"),
        "/utf8/next.html" => include_bytes!("fixtures/legacy-encoding/next.utf8.html"),
        // The explicit header must override the conflicting UTF-8 meta declaration.
        "/header/page.html" => include_bytes!("fixtures/legacy-encoding/header.sjis.html"),
        _ => panic!("unexpected fixture request {path}"),
    };
    let content_type = if path.starts_with("/header/") {
        "text/html; charset=Shift_JIS"
    } else {
        "text/html"
    };
    (content_type, body)
}

fn evaluate(browser: &mut PlatformBrowser, expression: &str) -> Value {
    let result = browser
        .active_session_mut()
        .unwrap()
        .dispatch(
            "Runtime.evaluate",
            json!({"expression": expression, "returnByValue": true}),
        )
        .unwrap();
    assert!(
        result.get("exceptionDetails").is_none(),
        "{expression}: {result}"
    );
    result["result"]["value"].clone()
}

fn document_text(browser: &mut PlatformBrowser, document: &str) -> Value {
    let result = evaluate(
        browser,
        &format!(
            "JSON.stringify((() => {{const d={document};return [d.title,d.querySelector('#text').textContent];}})())"
        ),
    );
    serde_json::from_str(result.as_str().unwrap()).unwrap()
}

fn save_frame(name: &str, frame: &BrowserFrame) {
    let Some(root) = std::env::var_os("OMOIKANE_BROWSER_REPORT_DIR") else {
        return;
    };
    let root = std::path::PathBuf::from(root).join("legacy-encoding");
    std::fs::create_dir_all(&root).unwrap();
    let file = std::fs::File::create(root.join(format!("{name}.png"))).unwrap();
    let mut encoder = png::Encoder::new(file, frame.width(), frame.height());
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(frame.pixels())
        .unwrap();
}

fn assert_same_frame(actual: &BrowserFrame, reference: &BrowserFrame) {
    assert_eq!(
        (actual.width(), actual.height()),
        (reference.width(), reference.height())
    );
    let mismatches = actual
        .pixels()
        .iter()
        .zip(reference.pixels())
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(
        mismatches, 0,
        "Shift_JIS must paint identically to the UTF-8 reference"
    );
}

#[test]
fn shift_jis_meta_frames_and_direct_navigation_preserve_text_and_paint() {
    let server = Server::start();
    let mut actual = PlatformBrowser::with_tab(Some(&server.url("/sjis/index.html"))).unwrap();
    let mut reference = PlatformBrowser::with_tab(Some(&server.url("/utf8/index.html"))).unwrap();
    let frame = actual.render_active(600, 200, 0).unwrap();
    let expected = reference.render_active(600, 200, 0).unwrap();
    save_frame("frames-shift-jis", &frame);
    save_frame("frames-utf8", &expected);
    assert_eq!(actual.tabs().next().unwrap().title, "日本語のフレーム");
    assert_eq!(
        document_text(&mut actual, "frames[0].document"),
        json!([TITLE, TEXT])
    );
    assert_same_frame(&frame, &expected);

    // A normal link default action replaces only the named child context.
    for browser in [&mut actual, &mut reference] {
        evaluate(browser, "frames[0].document.querySelector('#next').click()");
    }
    let next = actual.render_active(600, 200, 16).unwrap();
    let next_reference = reference.render_active(600, 200, 16).unwrap();
    save_frame("frames-after-link", &next);
    assert_eq!(
        document_text(&mut actual, "frames[0].document"),
        json!([NEXT_TITLE, NEXT_TEXT])
    );
    assert_eq!(
        evaluate(&mut actual, "location.pathname"),
        "/sjis/index.html"
    );
    assert_same_frame(&next, &next_reference);

    for (browser, path) in [
        (&mut actual, "/sjis/page.html"),
        (&mut reference, "/utf8/page.html"),
    ] {
        let tab = browser.active_tab().unwrap();
        browser.navigate(tab, &server.url(path)).unwrap();
    }
    let direct = actual.render_active(600, 200, 0).unwrap();
    let direct_reference = reference.render_active(600, 200, 0).unwrap();
    save_frame("direct-shift-jis", &direct);
    assert_eq!(actual.tabs().next().unwrap().title, TITLE);
    assert_eq!(document_text(&mut actual, "document"), json!([TITLE, TEXT]));
    assert_same_frame(&direct, &direct_reference);
}

#[test]
fn iframe_and_object_honor_meta_and_http_charset() {
    let server = Server::start();
    let mut browser = PlatformBrowser::with_tab(Some(&server.url("/utf8/page.html"))).unwrap();
    evaluate(
        &mut browser,
        r#"
        for (const tag of ['iframe', 'object']) {
            for (const path of ['/sjis/page.html', '/header/page.html']) {
                const e = document.createElement(tag);
                e.setAttribute(tag === 'object' ? 'data' : 'src', path);
                document.body.appendChild(e);
            }
        }
    "#,
    );
    browser.render_active(600, 300, 0).unwrap();
    for index in 0..4 {
        assert_eq!(
            document_text(
                &mut browser,
                &format!("document.querySelectorAll('iframe,object')[{index}].contentDocument")
            ),
            json!([TITLE, TEXT])
        );
    }
}

#[test]
fn installed_japanese_fallbacks_have_visible_glyphs() {
    let fonts = load_default_text_fonts();
    let mut selected = Vec::new();
    for ch in format!("{TITLE}{TEXT}{NEXT_TITLE}{NEXT_TEXT}").chars() {
        let font = fonts
            .iter()
            .find(|font| font.has_glyph(ch))
            .unwrap_or_else(|| panic!("no installed Japanese fallback for {ch}"));
        let glyph = font.rasterize(ch, 24.0).unwrap();
        assert!(
            glyph.bitmap.iter().any(|coverage| *coverage != 0),
            "empty glyph {ch}"
        );
        selected.push(json!({"character":ch.to_string(), "face":font.system_face()}));
    }
    if let Some(root) = std::env::var_os("OMOIKANE_BROWSER_REPORT_DIR") {
        let root = std::path::PathBuf::from(root).join("legacy-encoding");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("japanese-fonts.json"),
            serde_json::to_vec_pretty(&selected).unwrap(),
        )
        .unwrap();
    }
}
