//! Server Timing on Navigation and Resource Timing entries (Issue #1293).

use std::io::Write;

#[path = "support/http_fixture.rs"]
mod http_fixture;

use http_fixture::{
    ACCEPT_TIMEOUT, FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback,
    read_request_headers,
};
use omoikane::html::TreeBuilder;
use omoikane::js::JsRuntime;

fn runtime(url: &str) -> JsRuntime {
    JsRuntime::with_document_and_url(
        TreeBuilder::parse("<!doctype html><body></body>").document(),
        url,
    )
    .unwrap()
}

/// Evaluates `script`, which returns `true` or a failure description.
fn check(runtime: &mut JsRuntime, script: &str) {
    let result = runtime.eval(&format!("({script})()")).unwrap();
    assert!(result.as_boolean() == Some(true), "{}", result.display());
}

fn serve_once(response: String) -> (String, FixtureWorker<()>) {
    let listener = bind_loopback().unwrap();
    let address = listener.local_addr().unwrap();
    let handle = FixtureWorker::spawn(move || {
        let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
        read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
        stream.write_all(response.as_bytes()).unwrap();
    });
    (format!("http://{address}"), handle)
}

fn response(extra_headers: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\
         Server-Timing: db;dur=12.5;desc=\"Database\", cache;desc=hit\r\n\
         {extra_headers}Connection: close\r\n\r\nok"
    )
}

/// Fetches `url` and stores the resulting entry's `serverTiming` as JSON.
fn fetch_server_timing(runtime: &mut JsRuntime, url: &str) -> String {
    runtime
        .eval(&format!(
            "globalThis.result = null; fetch({url:?}).then(() => {{ \
               const [entry] = performance.getEntriesByName({url:?}); \
               globalThis.result = JSON.stringify(entry.serverTiming); \
             }}, error => {{ globalThis.result = 'error ' + error; }});"
        ))
        .unwrap();
    for _ in 0..200 {
        runtime.tick(5).unwrap();
        let result = runtime.eval("globalThis.result").unwrap();
        if !result.is_null() {
            return result.display().to_string().trim_matches('"').to_string();
        }
    }
    panic!("fetch did not settle");
}

#[test]
fn navigation_entry_exposes_the_document_server_timing() {
    let mut runtime = runtime("https://example.com/");
    runtime.install_document_response_headers(&[
        ("Content-Type".to_string(), "text/html".to_string()),
        (
            "Server-Timing".to_string(),
            "db;dur=53.2;desc=\"Database\", cache;desc=hit".to_string(),
        ),
        ("server-timing".to_string(), "total;dur=60".to_string()),
    ]);
    check(
        &mut runtime,
        r#"() => {
          const [navigation] = performance.getEntriesByType('navigation');
          const metrics = navigation.serverTiming;
          if (!Object.isFrozen(metrics) || metrics.length !== 3) return 'metrics ' + metrics.length;
          if (!(metrics[0] instanceof PerformanceServerTiming)) return 'interface';
          const text = metrics.map(m => `${m.name}:${m.duration}:${m.description}`).join();
          if (text !== 'db:53.2:Database,cache:0:hit,total:60:') return text;
          try { new PerformanceServerTiming(); return 'constructible'; } catch (e) {}
          try {
            Object.getOwnPropertyDescriptor(PerformanceServerTiming.prototype, 'name').get.call({});
            return 'unbranded getter';
          } catch (e) {}
          if (Object.prototype.toString.call(metrics[0]) !== '[object PerformanceServerTiming]')
            return 'class string';
          const json = navigation.toJSON().serverTiming;
          if (JSON.stringify(json[0]) !== '{"name":"db","duration":53.2,"description":"Database"}')
            return 'toJSON ' + JSON.stringify(json);
          return Object.keys(navigation).includes('serverTiming') === false ||
            'serverTiming is an own property';
        }"#,
    );
}

#[test]
fn same_origin_fetch_entries_expose_server_timing() {
    let (base, server) = serve_once(response(""));
    let mut runtime = runtime(&format!("{base}/page"));
    let metrics = fetch_server_timing(&mut runtime, &format!("{base}/data"));
    server.join();
    assert_eq!(
        metrics,
        r#"[{"name":"db","duration":12.5,"description":"Database"},{"name":"cache","duration":0,"description":"hit"}]"#
            .replace('"', "\\\"")
    );
}

#[test]
fn cross_origin_fetch_needs_timing_allow_origin() {
    let (base, server) = serve_once(response("Access-Control-Allow-Origin: *\r\n"));
    let mut runtime = runtime("http://page.test/");
    let hidden = fetch_server_timing(&mut runtime, &format!("{base}/data"));
    server.join();
    assert_eq!(hidden, "[]");

    let (base, server) = serve_once(response(
        "Access-Control-Allow-Origin: *\r\nTiming-Allow-Origin: http://other.test, http://page.test\r\n",
    ));
    let allowed = fetch_server_timing(&mut runtime, &format!("{base}/data"));
    server.join();
    assert!(allowed.contains("Database"), "{allowed}");
}
