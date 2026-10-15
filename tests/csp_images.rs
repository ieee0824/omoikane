//! Image requests retain document CSP, reporting, and completion ownership.
use base64::Engine as _;
use omoikane::{
    html::TreeBuilder,
    http::{HttpParseError, HttpRequest, HttpResponse, HttpTransport},
    js::JsRuntime,
};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct ImageTransport(Arc<Mutex<Vec<String>>>);

impl HttpTransport for ImageTransport {
    fn send(
        &self,
        request: &HttpRequest,
        _: Option<Duration>,
    ) -> Result<HttpResponse, HttpParseError> {
        let url = request.url().to_string();
        self.0.lock().unwrap().push(url.clone());
        if url.ends_with("/redirect.gif") {
            return HttpResponse::parse(&mut &b"HTTP/1.1 302 Found\r\nLocation: https://other.test/final.gif\r\nContent-Length: 0\r\n\r\n"[..]);
        }
        let pixel = base64::engine::general_purpose::STANDARD
            .decode("R0lGODlhAQABAIAAAAAAAP///ywAAAAAAQABAAACAUwAOw==")
            .unwrap();
        let mut bytes = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: image/gif\r\nContent-Length: {}\r\n\r\n",
            pixel.len()
        )
        .into_bytes();
        bytes.extend(pixel);
        HttpResponse::parse(&mut bytes.as_slice())
    }
}

struct Fixture {
    runtime: JsRuntime,
    requests: Arc<Mutex<Vec<String>>>,
}

impl Fixture {
    fn new(policy: &str, report_only: bool) -> Self {
        let document = TreeBuilder::parse("<!doctype html><body></body>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/page").unwrap();
        runtime.install_document_response_headers(&[(
            if report_only {
                "Content-Security-Policy-Report-Only"
            } else {
                "Content-Security-Policy"
            }
            .into(),
            format!("{policy}; report-to images"),
        )]);
        let requests = Arc::new(Mutex::new(Vec::new()));
        runtime.set_http_resource_transport(Some(Arc::new(ImageTransport(Arc::clone(&requests)))));
        runtime.eval(r#"
            globalThis.imageEvents = [];
            globalThis.imageViolations = [];
            globalThis.imageReports = [];
            document.addEventListener('securitypolicyviolation', event => imageViolations.push([event.effectiveDirective, event.disposition, event.blockedURI]));
            new ReportingObserver(reports => imageReports.push(...reports), {types:['csp-violation']}).observe();
            globalThis.makeImage = (source, attach = true) => {
                const image = new Image();
                image.onload = () => imageEvents.push('load');
                image.onerror = () => imageEvents.push('error');
                image.src = source;
                if (attach) document.body.appendChild(image);
                return image;
            };
        "#).unwrap();
        Self { runtime, requests }
    }

    fn run(&mut self, script: &str) {
        self.runtime.eval(script).unwrap();
        self.runtime.run_until_idle().unwrap();
    }

    fn value(&mut self, expression: &str) -> Value {
        let value = self
            .runtime
            .eval(&format!("JSON.stringify({expression})"))
            .unwrap();
        serde_json::from_str(&value.as_string().unwrap().to_std_string_escaped()).unwrap()
    }
}

#[test]
fn enforced_image_csp_blocks_before_transport_and_delivers_real_error_and_reports() {
    let mut fixture = Fixture::new("img-src 'none'", false);
    fixture.run("globalThis.image = makeImage('/blocked.gif');");
    assert!(fixture.requests.lock().unwrap().is_empty());
    assert_eq!(fixture.value("imageEvents"), serde_json::json!(["error"]));
    assert_eq!(
        fixture.value("imageViolations"),
        serde_json::json!([["img-src", "enforce", "https://example.test/blocked.gif"]])
    );
    assert_eq!(fixture.value("imageReports.length"), 1);
}

#[test]
fn report_only_image_csp_loads_and_layout_reuses_the_request_without_duplicate_reports() {
    let mut fixture = Fixture::new("img-src 'none'", true);
    fixture.run("globalThis.image = makeImage('/allowed.gif');");
    assert_eq!(fixture.value("imageEvents"), serde_json::json!(["load"]));
    assert_eq!(fixture.value("imageViolations.length"), 1);
    fixture.run("image.getBoundingClientRect(); image.getBoundingClientRect();");
    assert_eq!(fixture.requests.lock().unwrap().len(), 1);
    assert_eq!(fixture.value("imageViolations.length"), 1);
    assert_eq!(fixture.value("imageReports.length"), 1);
}

#[test]
fn image_redirect_checks_the_target_before_sending_it() {
    let mut fixture = Fixture::new("img-src 'self'", false);
    fixture.run("globalThis.image = makeImage('/redirect.gif');");
    assert_eq!(
        *fixture.requests.lock().unwrap(),
        vec!["https://example.test/redirect.gif"]
    );
    assert_eq!(fixture.value("imageEvents"), serde_json::json!(["error"]));
    assert_eq!(fixture.value("imageViolations.length"), 1);
}

#[test]
fn detached_and_removed_images_load_under_their_active_owner_document() {
    let mut fixture = Fixture::new("img-src 'self'", false);
    fixture.run("globalThis.detached = makeImage('/detached.gif', false); globalThis.removed = makeImage('/removed.gif'); removed.remove();");
    assert_eq!(fixture.requests.lock().unwrap().len(), 2);
    assert_eq!(
        fixture.value("imageEvents"),
        serde_json::json!(["load", "load"])
    );
}

#[test]
fn superseded_image_source_is_not_fetched_or_completed() {
    let mut fixture = Fixture::new("img-src 'self'", false);
    fixture.run("globalThis.image = makeImage('/old.gif'); image.src = '/new.gif';");
    assert_eq!(
        *fixture.requests.lock().unwrap(),
        vec!["https://example.test/new.gif"]
    );
    assert_eq!(fixture.value("imageEvents"), serde_json::json!(["load"]));
}

#[test]
fn cached_image_cannot_bypass_a_new_document_policy() {
    let mut fixture = Fixture::new("img-src 'self'", false);
    fixture.run("globalThis.image = makeImage('/cached.gif');");
    fixture.runtime.install_document_response_headers(&[(
        "Content-Security-Policy".into(),
        "img-src 'none'".into(),
    )]);
    fixture.run("image.src = '/cached.gif';");
    assert_eq!(fixture.requests.lock().unwrap().len(), 1);
    assert_eq!(
        fixture.value("imageEvents"),
        serde_json::json!(["load", "error"])
    );
    assert_eq!(fixture.value("imageViolations.length"), 1);
}

#[test]
fn retiring_an_image_document_discards_its_queued_completion() {
    let mut fixture = Fixture::new("img-src 'self'", false);
    fixture.run("globalThis.frame = document.createElement('iframe'); frame.srcdoc = '<body></body>'; document.body.append(frame);");
    fixture.runtime.eval("globalThis.oldDoc = frame.contentDocument; globalThis.oldImage = oldDoc.createElement('img'); oldImage.onload = () => imageEvents.push('stale'); oldImage.src = '/retired.gif'; oldDoc.body.append(oldImage);").unwrap();
    fixture
        .runtime
        .set_task_pump_limit(std::num::NonZeroUsize::new(1));
    fixture.runtime.run_until_idle().unwrap();
    assert_eq!(fixture.requests.lock().unwrap().len(), 1);
    fixture
        .runtime
        .eval("frame.srcdoc = '<body>replacement</body>'; void frame.contentDocument;")
        .unwrap();
    fixture.runtime.set_task_pump_limit(None);
    fixture.runtime.run_until_idle().unwrap();
    assert_eq!(fixture.value("imageEvents"), serde_json::json!([]));
}

#[test]
fn pending_image_request_keeps_its_completion_handler_alive_through_gc() {
    let mut fixture = Fixture::new("img-src 'self'", false);
    fixture
        .runtime
        .eval("(() => { makeImage('/gc.gif', false); })();")
        .unwrap();
    boa_gc::force_collect();
    fixture.runtime.run_until_idle().unwrap();
    assert_eq!(fixture.requests.lock().unwrap().len(), 1);
    assert_eq!(fixture.value("imageEvents"), serde_json::json!(["load"]));
}

#[test]
fn image_completion_retains_the_requested_url_after_base_changes() {
    let mut fixture = Fixture::new("img-src 'self'", false);
    fixture.runtime.eval("globalThis.base = document.createElement('base'); base.setAttribute('href', '/first/'); document.head.append(base); globalThis.image = makeImage('pixel.gif');").unwrap();
    fixture
        .runtime
        .set_task_pump_limit(std::num::NonZeroUsize::new(1));
    fixture.runtime.run_until_idle().unwrap();
    fixture
        .runtime
        .eval("base.setAttribute('href', '/second/');")
        .unwrap();
    fixture.runtime.set_task_pump_limit(None);
    fixture.runtime.run_until_idle().unwrap();
    assert_eq!(
        *fixture.requests.lock().unwrap(),
        vec!["https://example.test/first/pixel.gif"]
    );
    assert_eq!(
        fixture.value("performance.getEntriesByType('resource').map(entry => entry.name)"),
        serde_json::json!(["https://example.test/first/pixel.gif"])
    );
}

#[test]
fn child_image_requests_do_not_evict_parent_assets_or_repeat_parent_reports() {
    let mut fixture = Fixture::new("img-src 'none'", true);
    fixture.run("globalThis.rootImage = makeImage('/root.gif');");
    fixture.run("globalThis.frame = document.createElement('iframe'); frame.srcdoc = '<body><img src=\"/child.gif\"></body>'; document.body.append(frame);");
    fixture.run("rootImage.style.width = '4px'; rootImage.getBoundingClientRect();");
    assert_eq!(
        fixture
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|url| url.ends_with("/root.gif"))
            .count(),
        1
    );
    assert_eq!(
        fixture.value("imageReports.map(report => report.body.blockedURL)"),
        serde_json::json!(["https://example.test/root.gif"])
    );
    fixture.run("globalThis.childReports = []; globalThis.childObserver = new frame.contentWindow.ReportingObserver(reports => childReports.push(...reports), {types:['csp-violation'], buffered:true}); childObserver.observe();");
    assert_eq!(
        fixture.value("childReports.map(report => report.body.blockedURL)"),
        serde_json::json!(["https://example.test/child.gif"])
    );
}

#[test]
fn local_image_timing_waits_for_real_decode_completion() {
    let mut fixture = Fixture::new("img-src data:", false);
    fixture.runtime.eval("globalThis.image = makeImage('data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///ywAAAAAAQABAAACAUwAOw==');").unwrap();
    assert_eq!(
        fixture.value("performance.getEntriesByType('resource').length"),
        0
    );
    fixture.runtime.run_until_idle().unwrap();
    assert!(fixture.requests.lock().unwrap().is_empty());
    assert_eq!(fixture.value("imageEvents"), serde_json::json!(["load"]));
}

#[test]
fn csp_blocked_local_image_never_publishes_a_successful_timing() {
    let mut fixture = Fixture::new("img-src 'none'", false);
    fixture.run("globalThis.image = makeImage('data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///ywAAAAAAQABAAACAUwAOw==');");
    assert!(fixture.requests.lock().unwrap().is_empty());
    assert_eq!(fixture.value("imageEvents"), serde_json::json!(["error"]));
    assert_eq!(
        fixture.value(
            "performance.getEntriesByType('resource').every(entry => entry.responseStatus === 0)"
        ),
        true
    );
    assert_eq!(fixture.value("imageViolations[0][2]"), "data");
}

#[test]
fn blob_images_require_explicit_permission_and_use_real_decode_completion() {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode("R0lGODlhAQABAIAAAAAAAP///ywAAAAAAQABAAACAUwAOw==")
        .unwrap();
    for (policy, expected) in [("img-src blob:", "load"), ("img-src 'none'", "error")] {
        let mut fixture = Fixture::new(policy, false);
        fixture.runtime.eval(&format!(
            "globalThis.blobSource = URL.createObjectURL(new Blob([new Uint8Array({bytes:?})], {{type:'image/gif'}})); globalThis.image = makeImage(blobSource);"
        )).unwrap();
        assert_eq!(fixture.value("imageEvents.length"), 0);
        fixture.runtime.run_until_idle().unwrap();
        assert!(fixture.requests.lock().unwrap().is_empty());
        assert_eq!(fixture.value("imageEvents"), serde_json::json!([expected]));
        assert_eq!(
            fixture.value("performance.getEntriesByName(blobSource).length"),
            1
        );
        assert_eq!(
            fixture.value("performance.getEntriesByName(blobSource)[0].responseStatus"),
            if expected == "load" { 200 } else { 0 }
        );
        assert_eq!(
            fixture.value("imageViolations.length"),
            if expected == "load" { 0 } else { 1 }
        );
    }
}
