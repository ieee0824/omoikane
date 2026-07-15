//! WPT testharness.js smoke runner.
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use omoikane::html::TreeBuilder;
use omoikane::http::{Client, Url};
use omoikane::js::JsRuntime;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct Manifest { tests: Vec<WptCase> }
#[derive(Deserialize)]
struct WptCase { path: String, expected: String }

#[derive(Serialize)]
struct WptReport { revision: String, results: Vec<WptResult> }
#[derive(Serialize)]
struct WptResult {
    path: String,
    expected: String,
    actual: String,
    script_errors: Vec<String>,
    subtests: serde_json::Value,
}

struct StaticServer { base_url: String }
impl StaticServer {
    fn start(root: PathBuf) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind WPT server");
        let address = listener.local_addr().expect("WPT server address");
        let root = Arc::new(root);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() { serve(stream, &root); }
        });
        Self { base_url: format!("http://{address}") }
    }
}

fn serve(mut stream: TcpStream, root: &Path) {
    let mut request_line = String::new();
    {
        let mut reader = BufReader::new(&stream);
        if reader.read_line(&mut request_line).is_err() { return; }
    }
    let target = request_line.split_whitespace().nth(1).unwrap_or("/");
    let path = target.split("?").next().unwrap_or("/");
    if path == "/resources/testharnessreport.js" {
        let body = br#"
globalThis.__wpt_results = [];
globalThis.__wpt_harness_status = -1;
globalThis.__wpt_complete = false;
add_result_callback(test => globalThis.__wpt_results.push({name:String(test.name),status:Number(test.status),message:String(test.message||"")}));
add_completion_callback((tests,status) => { globalThis.__wpt_harness_status=Number(status.status); globalThis.__wpt_complete=true; });
"#;
        respond(&mut stream, 200, "text/javascript; charset=utf-8", body);
        return;
    }
    let relative = path.trim_start_matches("/");
    if relative.split("/").any(|part| part == "..") {
        respond(&mut stream, 403, "text/plain", b"forbidden"); return;
    }
    let file = root.join(relative);
    match fs::read(&file) {
        Ok(body) => respond(&mut stream, 200, content_type(&file), &body),
        Err(_) => respond(&mut stream, 404, "text/plain", b"not found"),
    }
}

fn respond(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) {
    let reason = if status == 200 { "OK" } else { "Error" };
    let header = format!("HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
}
fn content_type(path: &Path) -> &str {
    match path.extension().and_then(|value| value.to_str()) {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    }
}
fn js_bool(runtime: &mut JsRuntime, source: &str) -> bool {
    runtime.eval(source).ok().and_then(|value| value.as_boolean()).unwrap_or(false)
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn junit_xml(report: &WptReport) -> String {
    let failures = report.results.iter()
        .filter(|result| result.actual != result.expected)
        .count();
    let mut xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuite name=\"wpt-smoke\" tests=\"{}\" failures=\"{}\">\n",
        report.results.len(), failures
    );
    for result in &report.results {
        let details = serde_json::to_string(&result.subtests).expect("serialize WPT subtests");
        xml.push_str(&format!(
            "  <testcase classname=\"wpt\" name=\"{}\">\n",
            escape_xml(&result.path)
        ));
        if result.actual != result.expected {
            xml.push_str(&format!(
                "    <failure message=\"expected {}, got {}\">{}</failure>\n",
                escape_xml(&result.expected),
                escape_xml(&result.actual),
                escape_xml(&details)
            ));
        }
        xml.push_str(&format!(
            "    <system-out>{}</system-out>\n  </testcase>\n",
            escape_xml(&details)
        ));
    }
    xml.push_str("</testsuite>\n");
    xml
}

#[test]
fn junit_report_escapes_xml_and_reports_mismatches() {
    let report = WptReport {
        revision: "test".to_string(),
        results: vec![WptResult {
            path: "a<&\"'".to_string(),
            expected: "PASS".to_string(),
            actual: "FAIL".to_string(),
            script_errors: vec![],
            subtests: serde_json::json!({"message": "boom <x>"}),
        }],
    };
    let xml = junit_xml(&report);
    assert!(xml.contains("tests=\"1\" failures=\"1\""));
    assert!(xml.contains("name=\"a&lt;&amp;&quot;&apos;\""));
    assert!(xml.contains("boom &lt;x&gt;"));
}

#[test]
fn selected_wpt_testharness_cases_match_expectations() {
    let root = std::env::var("WPT_ROOT").map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("target/wpt"));
    if !root.join("resources/testharness.js").is_file() {
        if std::env::var_os("WPT_REQUIRED").is_some() {
            panic!("WPT checkout required but missing (WPT_ROOT={})", root.display());
        }
        eprintln!(
            "WPT checkout missing; run scripts/fetch-wpt.sh (WPT_ROOT={})",
            root.display()
        );
        return;
    }
    let manifest: Manifest = serde_json::from_slice(
        &fs::read("tests/wpt/manifest.json").expect("read WPT manifest"))
        .expect("parse WPT manifest");
    let revision = fs::read_to_string("tests/wpt/revision.txt")
        .expect("read WPT revision").trim().to_string();
    let server = StaticServer::start(root);
    let mut results = Vec::new();
    let mut mismatches = Vec::new();
    for case in manifest.tests {
        let url = format!("{}/{}", server.base_url, case.path);
        let mut client = Client::new();
        let response = client.get(&url).unwrap_or_else(|error| panic!("GET {url}: {error}"));
        assert_eq!(response.status_code(), 200, "WPT resource missing: {}", case.path);
        let document = TreeBuilder::parse(&String::from_utf8_lossy(response.body())).document();
        let base: Url = url.parse().expect("parse WPT URL");
        let mut runtime = JsRuntime::with_document(document).expect("create WPT runtime");
        let errors = runtime.execute_document_scripts(Some(&base));
        runtime.wire_inline_event_handlers().expect("wire WPT handlers");
        runtime.fire_load().expect("fire WPT load");
        runtime.run_timers(5_000, 10, 2_000);
        runtime.run_jobs().expect("drain WPT jobs");
        let complete = js_bool(&mut runtime, "globalThis.__wpt_complete === true");
        let passed = js_bool(&mut runtime, "__wpt_complete===true && __wpt_harness_status===0 && __wpt_results.length>0 && __wpt_results.every(test=>test.status===0)");
        let actual = if passed { "PASS" } else if complete { "FAIL" } else { "TIMEOUT" };
        let details = runtime.eval("JSON.stringify(globalThis.__wpt_results||[])").ok()
            .and_then(|value| value.as_string().map(|text| text.to_std_string_escaped()))
            .unwrap_or_else(|| "[]".to_string());
        println!("WPT {}: expected={} actual={}", case.path, case.expected, actual);
        if actual != case.expected {
            mismatches.push(format!(
                "WPT {} mismatch: expected={} actual={}; script errors={errors:?}; results={details}",
                case.path, case.expected, actual
            ));
        }
        results.push(WptResult {
            path: case.path,
            expected: case.expected,
            actual: actual.to_string(),
            script_errors: errors,
            subtests: serde_json::from_str(&details).unwrap_or(serde_json::Value::Null),
        });
    }
    let report = WptReport { revision, results };
    if let Ok(path) = std::env::var("WPT_REPORT") {
        let path = PathBuf::from(path);
        if let Some(parent) = path.parent() { fs::create_dir_all(parent).expect("create report directory"); }
        fs::write(&path, serde_json::to_vec_pretty(&report).expect("serialize WPT report"))
            .expect("write WPT report");
    }
    if let Ok(path) = std::env::var("WPT_JUNIT") {
        let path = PathBuf::from(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create JUnit directory");
        }
        fs::write(&path, junit_xml(&report)).expect("write WPT JUnit report");
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}
