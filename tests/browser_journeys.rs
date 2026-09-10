//! Browser behavior contracts exercised through the same session API as the GUI.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

use omoikane::frame::BrowserFrame;
use omoikane::platform_browser::PlatformBrowser;
use serde_json::{Value, json};

#[derive(Clone, Debug)]
struct Request {
    method: String,
    path: String,
    body: String,
}

struct FixtureServer {
    origin: String,
    stop: Arc<AtomicBool>,
    requests: Arc<Mutex<Vec<Request>>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl FixtureServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let worker_requests = requests.clone();
        let worker = thread::spawn(move || {
            while !worker_stop.load(Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    Err(error) => panic!("fixture accept: {error}"),
                };
                // BSD sockets can inherit the listener's nonblocking flag.
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(&mut stream);
                let mut request = String::new();
                reader.read_line(&mut request).unwrap();
                let mut fields = request.split_whitespace();
                let method = fields.next().unwrap().to_owned();
                let path = fields.next().unwrap().to_owned();
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    assert!(!line.is_empty(), "incomplete fixture request");
                    if let Some((name, value)) = line.split_once(':')
                        && name.eq_ignore_ascii_case("content-length")
                    {
                        length = value.trim().parse::<usize>().unwrap();
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                worker_requests.lock().unwrap().push(Request {
                    method,
                    path: path.clone(),
                    body: String::from_utf8(body).unwrap(),
                });
                let (status, mime, extra, content) = match path.as_str() {
                    "/start" => (
                        "302 Found",
                        "text/plain",
                        "Location: /app/index.html\r\n",
                        "redirect",
                    ),
                    "/app/index.html" => (
                        "200 OK",
                        "text/html; charset=utf-8",
                        "",
                        include_str!("fixtures/anonymized-browser-journey/index.html"),
                    ),
                    "/assets/style.css" => (
                        "200 OK",
                        "text/css",
                        "",
                        include_str!("fixtures/anonymized-browser-journey/style.css"),
                    ),
                    "/style-contract" => (
                        "200 OK",
                        "text/html",
                        "",
                        "<html><head><base href='/assets/'><style>.target {left:12px}</style><link id='sheet' rel='stylesheet' href='composed.css'><style>.target {left:55px}</style><link rel='stylesheet' media='print' href='alternate.css'><link rel='stylesheet' href='not-css'></head><body><div id='target' class='target'></div></body></html>",
                    ),
                    "/style-csp" => (
                        "200 OK",
                        "text/html",
                        "Content-Security-Policy: style-src 'self'\r\n",
                        "<html><head><base href='/assets/'><style>.target {left:900px!important}</style><link rel='stylesheet' href='composed.css'><link rel='stylesheet' href='data:text/css,.target%7Bleft%3A950px%21important%7D'></head><body><div id='target' class='target'></div></body></html>",
                    ),
                    "/assets/composed.css" => (
                        "200 OK",
                        "text/css",
                        "",
                        "@import 'imported.css'; .target {left:44px}",
                    ),
                    "/assets/imported.css" => (
                        "200 OK",
                        "text/css",
                        "",
                        ".target {position:absolute;left:30px;top:50px;width:70px;height:30px;background:rgb(30,90,150)}",
                    ),
                    "/assets/alternate.css" => (
                        "200 OK",
                        "text/css",
                        "",
                        ".target {position:absolute;left:66px;top:70px;width:70px;height:30px;background:rgb(20,150,80)}",
                    ),
                    "/assets/not-css" => {
                        ("200 OK", "text/plain", "", ".target {left:999px!important}")
                    }
                    "/inline-form" => (
                        "200 OK",
                        "text/html",
                        "",
                        "<html><body><form id='form' action='/submit' method='post'><label>Name <input id='name' name='name' style='width:80px;height:20px;padding:3px;border:2px solid;box-sizing:content-box'></label><button id='submit'>Send</button></form><script>document.getElementById('form').addEventListener('submit',()=>localStorage.setItem('journey-name',document.getElementById('name').value));</script></body></html>",
                    ),
                    "/assets/start.js" => (
                        "200 OK",
                        "text/javascript",
                        "",
                        "globalThis.boots = (globalThis.boots || 0) + 1; globalThis.journey = {events: [], errors: []}; addEventListener('error', e => journey.errors.push(e.message)); addEventListener('unhandledrejection', e => journey.errors.push(String(e.reason))); addEventListener('load', () => journey.loaded = true);",
                    ),
                    "/app/main.js" => (
                        "200 OK",
                        "text/javascript",
                        "",
                        include_str!("fixtures/anonymized-browser-journey/main.js"),
                    ),
                    "/app/numbers.js" => {
                        ("200 OK", "text/javascript", "", "export const answer = 42;")
                    }
                    "/api/data" => ("200 OK", "application/json", "", "{\"answer\":42}"),
                    "/worker.js" => (
                        "200 OK",
                        "text/javascript",
                        "",
                        "onmessage = e => postMessage({text: e.data.text, answer: e.data.answer + 1});",
                    ),
                    "/child.html" => (
                        "200 OK",
                        "text/html",
                        "",
                        "<html><body style='background:rgb(245,210,90)'><p id='child'>Child ready</p><script>globalThis.childValue=7;</script></body></html>",
                    ),
                    "/submit" => (
                        "200 OK",
                        "text/html",
                        "",
                        "<html><head><title>Saved</title></head><body><main id='receipt'>Saved</main><script>globalThis.savedName=localStorage.getItem('journey-name');</script></body></html>",
                    ),
                    "/storage" | "/other" => (
                        "200 OK",
                        "text/html",
                        "",
                        "<html><head><title>Storage</title></head><body>Storage page</body></html>",
                    ),
                    _ => ("404 Not Found", "text/plain", "", "unexpected fixture URL"),
                };
                write!(stream, "HTTP/1.1 {status}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n{content}", content.len()).unwrap();
            }
        });
        Self {
            origin,
            stop,
            requests,
            worker: Some(worker),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.origin)
    }

    fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let result = self.worker.take().unwrap().join();
        if !thread::panicking() {
            result.expect("fixture server failed");
        }
    }
}

fn dispatch(browser: &mut PlatformBrowser, method: &str, params: Value) -> Value {
    browser
        .active_session_mut()
        .unwrap()
        .dispatch(method, params)
        .unwrap_or_else(|error| panic!("{method}: {error:?}"))
}

fn evaluate(browser: &mut PlatformBrowser, expression: &str) -> Value {
    let result = dispatch(
        browser,
        "Runtime.evaluate",
        json!({"expression": expression, "returnByValue": true}),
    );
    assert!(
        result.get("exceptionDetails").is_none(),
        "{expression}: {result}"
    );
    result["result"]["value"].clone()
}

fn click(browser: &mut PlatformBrowser, selector: &str) {
    let expression = format!(
        "JSON.stringify((() => {{const e=document.querySelector({}); const r=e.getBoundingClientRect(); return [r.x+r.width/2,r.y+r.height/2];}})())",
        serde_json::to_string(selector).unwrap()
    );
    let coordinates = evaluate(browser, &expression);
    let coordinates: Vec<f64> = serde_json::from_str(coordinates.as_str().unwrap()).unwrap();
    eprintln!("Click {selector} at {coordinates:?}");
    for kind in ["mousePressed", "mouseReleased"] {
        let response = dispatch(
            browser,
            "Input.dispatchMouseEvent",
            json!({"type":kind,"x":coordinates[0],"y":coordinates[1],"button":"left","clickCount":1}),
        );
        // Mouse release may commit navigation and invalidate the old node IDs.
        eprintln!("{kind}: {response}");
    }
}

fn pixel(frame: &BrowserFrame, x: u32, y: u32) -> &[u8] {
    let start = ((y * frame.width() + x) * 4) as usize;
    &frame.pixels()[start..start + 4]
}

fn save_frame(case: &str, frame: &BrowserFrame) {
    let Some(root) = std::env::var_os("OMOIKANE_BROWSER_REPORT_DIR") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    std::fs::create_dir_all(&root).unwrap();
    let file =
        std::fs::File::create(root.join(format!("anonymized-browser-journey.{case}.actual.png")))
            .unwrap();
    let mut encoder = png::Encoder::new(file, frame.width(), frame.height());
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(frame.pixels())
        .unwrap();
}

fn record(case: &str, details: Value) {
    let report = json!({"case":case,"status":"passed","details":details});
    println!("Browser journey: {report}");
    if let Some(root) = std::env::var_os("OMOIKANE_BROWSER_REPORT_DIR") {
        let root = std::path::PathBuf::from(root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join(format!("{case}.json")),
            serde_json::to_vec_pretty(&report).unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn http_page_input_fetch_and_dom_changes_reach_the_painted_frame() {
    let server = FixtureServer::start();
    let mut browser = PlatformBrowser::with_tab(Some(&server.url("/start"))).unwrap();
    assert_eq!(
        browser.active_session_mut().unwrap().current_url(),
        server.url("/app/index.html")
    );
    let first = browser.render_active(640, 480, 16).unwrap();
    save_frame("page-before-input", &first);
    assert_eq!(pixel(&first, 300, 290), [30, 90, 150, 255]);
    assert_eq!(evaluate(&mut browser, "boots"), 1);
    assert_eq!(
        evaluate(&mut browser, "journey.loaded && journey.module"),
        true
    );
    click(&mut browser, "#name");
    for key in ["A", "d", "x", "Backspace", "a"] {
        let params = if key == "Backspace" {
            json!({"type":"keyDown","key":key})
        } else {
            json!({"type":"keyDown","key":key,"text":key})
        };
        dispatch(&mut browser, "Input.dispatchKeyEvent", params);
    }
    assert_eq!(
        evaluate(&mut browser, "document.getElementById('name').value"),
        "Ada"
    );
    click(&mut browser, "#apply");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut elapsed = 16;
    while evaluate(&mut browser, "journey.updated === true") != true {
        assert!(
            Instant::now() < deadline,
            "page update did not settle: {}",
            evaluate(&mut browser, "JSON.stringify(journey)")
        );
        elapsed += 16;
        browser
            .active_session_mut()
            .unwrap()
            .drive_event_loop(elapsed)
            .unwrap();
        thread::sleep(Duration::from_millis(1));
    }
    let after = browser.render_active(640, 480, elapsed + 16).unwrap();
    save_frame("page-after-input", &after);
    assert_eq!(pixel(&after, 300, 290), [20, 150, 80, 255]);
    let changed_input_pixels = (154..179)
        .flat_map(|y| (25..100).map(move |x| (x, y)))
        .filter(|&(x, y)| pixel(&first, x, y) != pixel(&after, x, y))
        .count();
    assert!(
        changed_input_pixels > 20,
        "the typed value must be visible inside the input, changed pixels: {changed_input_pixels}"
    );
    assert_eq!(
        evaluate(
            &mut browser,
            "document.getElementById('status').textContent"
        ),
        "Ada: 42"
    );
    assert_eq!(
        evaluate(&mut browser, "boots"),
        1,
        "redraw must not re-run startup scripts"
    );
    assert_eq!(
        evaluate(&mut browser, "JSON.stringify(journey.errors)"),
        "[]"
    );
    let requests = server.requests();
    for path in [
        "/start",
        "/app/index.html",
        "/assets/style.css",
        "/assets/start.js",
        "/app/main.js",
        "/app/numbers.js",
        "/api/data",
    ] {
        assert!(
            requests.iter().any(|r| r.path == path),
            "missing resource: {path}"
        );
    }
    record(
        "input-fetch-paint",
        json!({"name":"Ada","answer":42,"startup_executions":1,"initial_pixel":[30,90,150,255],"updated_pixel":[20,150,80,255]}),
    );
}

#[test]
fn form_submission_and_back_navigation_preserve_saved_state() {
    let server = FixtureServer::start();
    let mut browser = PlatformBrowser::with_tab(Some(&server.url("/app/index.html"))).unwrap();
    browser.render_active(640, 480, 16).unwrap();
    click(&mut browser, "#name");
    dispatch(
        &mut browser,
        "Input.insertText",
        json!({"text":"Ada Lovelace"}),
    );
    click(&mut browser, "#submit");
    // The frontend commits navigation during the next event-loop/frame tick.
    let submitted = browser.render_active(640, 480, 32).unwrap();
    save_frame("page-after-submit", &submitted);
    assert_eq!(
        browser.active_session_mut().unwrap().current_url(),
        server.url("/submit")
    );
    assert_eq!(
        evaluate(
            &mut browser,
            "document.getElementById('receipt').textContent"
        ),
        "Saved"
    );
    assert_eq!(evaluate(&mut browser, "savedName"), "Ada Lovelace");
    assert!(
        server
            .requests()
            .iter()
            .any(|r| r.method == "POST" && r.path == "/submit" && r.body == "name=Ada+Lovelace")
    );
    browser.traverse_history(-1).unwrap();
    assert_eq!(
        browser.active_session_mut().unwrap().current_url(),
        server.url("/app/index.html")
    );
    assert_eq!(
        evaluate(&mut browser, "localStorage.getItem('journey-name')"),
        "Ada Lovelace"
    );
    let frame = browser.render_active(640, 480, 32).unwrap();
    save_frame("page-after-back", &frame);
    record(
        "form-history",
        json!({"submitted":"name=Ada+Lovelace","restored_origin_storage":"Ada Lovelace"}),
    );
}

#[test]
fn tabs_share_local_storage_but_keep_session_storage_separate() {
    let server = FixtureServer::start();
    let mut browser = PlatformBrowser::with_tab(Some(&server.url("/storage"))).unwrap();
    let first = browser.active_tab().unwrap();
    evaluate(
        &mut browser,
        "localStorage.setItem('shared','first'); sessionStorage.setItem('tab','one')",
    );
    let second = browser.open_tab(Some(&server.url("/storage"))).unwrap();
    assert_eq!(
        evaluate(&mut browser, "localStorage.getItem('shared')"),
        "first",
        "same-origin tabs must share localStorage"
    );
    assert_eq!(
        evaluate(&mut browser, "sessionStorage.getItem('tab')"),
        Value::Null
    );
    evaluate(
        &mut browser,
        "localStorage.setItem('shared','second'); sessionStorage.setItem('tab','two')",
    );
    browser.activate_tab(first).unwrap();
    assert_eq!(
        evaluate(&mut browser, "localStorage.getItem('shared')"),
        "second"
    );
    assert_eq!(
        evaluate(&mut browser, "sessionStorage.getItem('tab')"),
        "one"
    );
    browser.close_tab(second).unwrap();
    let reopened = browser.open_tab(Some(&server.url("/other"))).unwrap();
    assert_eq!(
        evaluate(&mut browser, "localStorage.getItem('shared')"),
        "second"
    );
    assert_eq!(
        evaluate(&mut browser, "sessionStorage.getItem('tab')"),
        Value::Null
    );
    browser.close_tab(reopened).unwrap();
    let other_origin = FixtureServer::start();
    let cross_origin = browser
        .open_tab(Some(&other_origin.url("/storage")))
        .unwrap();
    assert_eq!(
        evaluate(&mut browser, "localStorage.getItem('shared')"),
        Value::Null
    );
    browser.close_tab(cross_origin).unwrap();
    browser.close_tab(first).unwrap();
    assert!(browser.active_tab().is_none());
    browser.open_tab(Some(&server.url("/storage"))).unwrap();
    assert_eq!(
        evaluate(&mut browser, "localStorage.getItem('shared')"),
        "second"
    );
    assert_eq!(
        evaluate(&mut browser, "sessionStorage.getItem('tab')"),
        Value::Null
    );
    let mut other_browser = PlatformBrowser::with_tab(Some(&server.url("/storage"))).unwrap();
    assert_eq!(
        evaluate(&mut other_browser, "localStorage.getItem('shared')"),
        Value::Null,
        "independent browser profiles must remain isolated"
    );
    record(
        "tab-storage",
        json!({"local_storage_shared":true,"session_storage_separate":true,"browser_profiles_separate":true}),
    );
}

#[test]
fn worker_clone_and_child_realm_complete_through_the_page_event_loop() {
    let server = FixtureServer::start();
    let mut browser = PlatformBrowser::with_tab(Some(&server.url("/app/index.html"))).unwrap();
    browser.render_active(640, 480, 16).unwrap();
    evaluate(
        &mut browser,
        r#"globalThis.workerResults=[];
        globalThis.worker=new Worker('/worker.js');
        worker.onmessage=e=>workerResults.push(e.data);
        globalThis.payload={text:'original',answer:41};
        worker.postMessage(payload); payload.text='changed';
        globalThis.childFrame=document.createElement('iframe');
        childFrame.srcdoc='<html><body><p id="child">Child ready</p></body></html>';
        document.body.appendChild(childFrame);
        globalThis.parentObject=Object;
        globalThis.eventOrder=[];
        Promise.resolve().then(()=>eventOrder.push('microtask'));
        setTimeout(()=>eventOrder.push('timer'),0);
        requestAnimationFrame(()=>eventOrder.push('frame'));"#,
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut elapsed = 16;
    loop {
        elapsed += 16;
        browser
            .active_session_mut()
            .unwrap()
            .drive_event_loop(elapsed)
            .unwrap();
        if evaluate(
            &mut browser,
            "workerResults.length === 1 && eventOrder.includes('frame')",
        ) == true
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "worker or animation frame did not settle"
        );
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        evaluate(&mut browser, "JSON.stringify(workerResults)"),
        r#"[{"text":"original","answer":42}]"#
    );
    assert_eq!(
        evaluate(&mut browser, "JSON.stringify(eventOrder)"),
        r#"["microtask","timer","frame"]"#
    );
    assert_eq!(
        evaluate(
            &mut browser,
            "childFrame.contentWindow.Object !== parentObject && childFrame.contentDocument.getElementById('child').textContent === 'Child ready'"
        ),
        true
    );
    evaluate(&mut browser, "worker.terminate(); childFrame.remove()");
    assert_eq!(
        evaluate(&mut browser, "JSON.stringify(journey.errors)"),
        "[]"
    );
    record(
        "worker-realm-event-loop",
        json!({"cloned_payload":"original","worker_answer":42,"order":["microtask","timer","frame"],"realm_isolation":true}),
    );
}

#[test]
fn linked_stylesheets_share_cascade_geometry_and_paint_across_updates() {
    let server = FixtureServer::start();
    let mut browser = PlatformBrowser::with_tab(Some(&server.url("/style-contract"))).unwrap();
    browser.active_session_mut().unwrap().set_viewport(640, 480);
    let rect = "JSON.stringify((()=>{const r=document.getElementById('target').getBoundingClientRect(); return [r.x,r.y,r.width,r.height]})())";
    assert_eq!(evaluate(&mut browser, rect), "[55,50,70,30]");
    let before = browser.render_active(640, 480, 16).unwrap();
    assert_eq!(pixel(&before, 80, 60), [30, 90, 150, 255]);
    evaluate(
        &mut browser,
        "document.getElementById('target').style.backgroundColor='rgb(20,150,80)'",
    );
    let changed = browser.render_active(640, 480, 32).unwrap();
    assert_eq!(pixel(&changed, 80, 60), [20, 150, 80, 255]);
    evaluate(
        &mut browser,
        "document.getElementById('sheet').setAttribute('href','alternate.css')",
    );
    assert_eq!(evaluate(&mut browser, rect), "[55,70,70,30]");
    evaluate(
        &mut browser,
        "document.getElementById('sheet').setAttribute('media','print')",
    );
    assert_eq!(
        evaluate(
            &mut browser,
            "getComputedStyle(document.getElementById('target')).position"
        ),
        "static"
    );
    evaluate(
        &mut browser,
        "document.getElementById('sheet').setAttribute('media','screen')",
    );
    assert_eq!(evaluate(&mut browser, rect), "[55,70,70,30]");
    let after = browser.render_active(640, 480, 48).unwrap();
    assert_eq!(pixel(&after, 80, 80), [20, 150, 80, 255]);
    for path in [
        "/assets/composed.css",
        "/assets/imported.css",
        "/assets/alternate.css",
    ] {
        assert_eq!(
            server
                .requests()
                .iter()
                .filter(|request| request.path == path)
                .count(),
            1,
            "style invalidation and repaint must retain loaded resource: {path}"
        );
    }
    save_frame("stylesheet-updates", &after);
    record(
        "stylesheet-updates",
        json!({"cascade_order":true,"imports":true,"media":true,"geometry_matches_paint":true,"resources_retained":true}),
    );
}

#[test]
fn stylesheet_policy_is_shared_by_geometry_and_paint() {
    let server = FixtureServer::start();
    let mut browser = PlatformBrowser::with_tab(Some(&server.url("/style-csp"))).unwrap();
    let frame = browser.render_active(640, 480, 16).unwrap();
    assert_eq!(
        evaluate(
            &mut browser,
            "document.getElementById('target').getBoundingClientRect().x"
        ),
        44
    );
    assert_eq!(pixel(&frame, 80, 60), [30, 90, 150, 255]);
    record(
        "stylesheet-policy",
        json!({"same_origin_styles":true,"inline_and_data_styles_blocked":true,"geometry_matches_paint":true}),
    );
}

#[test]
fn normal_flow_form_controls_accept_input_and_submit_using_painted_coordinates() {
    let server = FixtureServer::start();
    let mut browser = PlatformBrowser::with_tab(Some(&server.url("/inline-form"))).unwrap();
    let first = browser.render_active(320, 240, 16).unwrap();
    assert_eq!(
        evaluate(
            &mut browser,
            "JSON.stringify((()=>{const r=document.getElementById('name').getBoundingClientRect();return [r.width,r.height]})())"
        ),
        "[90,30]"
    );
    click(&mut browser, "#name");
    assert_eq!(evaluate(&mut browser, "document.activeElement.id"), "name");
    dispatch(&mut browser, "Input.insertText", json!({"text":"Lin"}));
    assert_eq!(
        evaluate(&mut browser, "document.getElementById('name').value"),
        "Lin"
    );
    let typed = browser.render_active(320, 240, 32).unwrap();
    assert_ne!(
        first.pixels(),
        typed.pixels(),
        "typed text must update the frame"
    );
    save_frame("inline-form-typed", &typed);
    click(&mut browser, "#submit");
    browser.render_active(320, 240, 48).unwrap();
    assert_eq!(
        browser.active_session_mut().unwrap().current_url(),
        server.url("/submit")
    );
    assert_eq!(evaluate(&mut browser, "savedName"), "Lin");
    assert!(
        server
            .requests()
            .iter()
            .any(|r| r.method == "POST" && r.path == "/submit" && r.body == "name=Lin")
    );
    record(
        "inline-form",
        json!({"input_border_box":[90,30],"focused_input":true,"submitted":"name=Lin"}),
    );
}
