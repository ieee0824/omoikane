//! WPT testharness.js smoke runner.
use omoikane::html::TreeBuilder;
use omoikane::http::{Client, Url};
use omoikane::js::JsRuntime;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    tests: Vec<WptCase>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WptCase {
    path: String,
    #[serde(default)]
    known_failure: Option<KnownFailure>,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct KnownFailure {
    status: ActualStatus,
    reason: String,
    issue: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
enum ActualStatus {
    Pass,
    Fail,
    Timeout,
    Error,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Classification {
    Pass,
    KnownFailure,
    Regression,
    Improvement,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct WptReport {
    revision: String,
    summary: WptSummary,
    results: Vec<WptResult>,
}
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
struct WptSummary {
    total: usize,
    pass: usize,
    known_failure: usize,
    regression: usize,
    improvement: usize,
    by_area: BTreeMap<String, AreaSummary>,
}
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
struct AreaSummary {
    total: usize,
    pass: usize,
    known_failure: usize,
    regression: usize,
    improvement: usize,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct WptResult {
    path: String,
    area: String,
    actual: ActualStatus,
    classification: Classification,
    #[serde(skip_serializing_if = "Option::is_none")]
    known_failure: Option<KnownFailure>,
    script_errors: Vec<String>,
    subtests: serde_json::Value,
}

#[derive(Debug, Deserialize, PartialEq, Serialize)]
struct WptAreaReport {
    revision: String,
    area: String,
    summary: AreaSummary,
    results: Vec<WptResult>,
}

#[derive(Debug, PartialEq, Serialize)]
struct WptRevisionDiff {
    previous_revision: String,
    current_revision: String,
    known_failure_delta: i64,
    regression_delta: i64,
    improvement_delta: i64,
    changed: Vec<WptResultChange>,
}

#[derive(Debug, PartialEq, Serialize)]
struct WptResultChange {
    path: String,
    previous: Option<Classification>,
    current: Option<Classification>,
}

struct StaticServer {
    base_url: String,
}
impl StaticServer {
    fn start(root: PathBuf) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind WPT server");
        let address = listener.local_addr().expect("WPT server address");
        let root = Arc::new(root);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                serve(stream, &root);
            }
        });
        Self {
            base_url: format!("http://{address}"),
        }
    }
}

fn serve(mut stream: TcpStream, root: &Path) {
    let mut request_line = String::new();
    {
        let mut reader = BufReader::new(&stream);
        if reader.read_line(&mut request_line).is_err() {
            return;
        }
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
        respond(&mut stream, 403, "text/plain", b"forbidden");
        return;
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
    runtime
        .eval(source)
        .ok()
        .and_then(|value| value.as_boolean())
        .unwrap_or(false)
}

impl ActualStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Fail => "FAIL",
            Self::Timeout => "TIMEOUT",
            Self::Error => "ERROR",
        }
    }
}

fn validate_manifest(manifest: &Manifest) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    let mut paths = HashSet::new();
    for case in &manifest.tests {
        if case.path.trim().is_empty() {
            errors.push("test path must not be empty".to_string());
        } else if !paths.insert(case.path.as_str()) {
            errors.push(format!("duplicate test path: {}", case.path));
        }
        if let Some(known) = &case.known_failure {
            if known.status == ActualStatus::Pass {
                errors.push(format!(
                    "{}: known failure status must not be PASS",
                    case.path
                ));
            }
            if known.reason.trim().is_empty() {
                errors.push(format!(
                    "{}: known failure reason must not be empty",
                    case.path
                ));
            }
            if known.issue.trim().is_empty() {
                errors.push(format!(
                    "{}: known failure issue must not be empty",
                    case.path
                ));
            }
            if known
                .expires
                .as_ref()
                .is_some_and(|value| value.trim().is_empty())
            {
                errors.push(format!(
                    "{}: known failure expires must not be empty",
                    case.path
                ));
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn classify(actual: ActualStatus, known: Option<&KnownFailure>) -> Classification {
    match (actual, known) {
        (ActualStatus::Pass, Some(_)) => Classification::Improvement,
        (ActualStatus::Pass, None) => Classification::Pass,
        (status, Some(known)) if status == known.status => Classification::KnownFailure,
        _ => Classification::Regression,
    }
}

fn area_for_path(path: &str) -> String {
    path.split('/').next().unwrap_or("unknown").to_string()
}

fn summarize(results: &[WptResult]) -> WptSummary {
    let mut summary = WptSummary {
        total: results.len(),
        ..WptSummary::default()
    };
    for result in results {
        let area = summary.by_area.entry(result.area.clone()).or_default();
        area.total += 1;
        match result.classification {
            Classification::Pass => {
                summary.pass += 1;
                area.pass += 1;
            }
            Classification::KnownFailure => {
                summary.known_failure += 1;
                area.known_failure += 1;
            }
            Classification::Regression => {
                summary.regression += 1;
                area.regression += 1;
            }
            Classification::Improvement => {
                summary.improvement += 1;
                area.improvement += 1;
            }
        }
    }
    summary
}

fn escape_xml(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

fn junit_xml(report: &WptReport) -> String {
    let mut xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuite name=\"wpt-smoke\" tests=\"{}\" failures=\"{}\">\n",
        report.results.len(), report.summary.regression
    );
    for result in &report.results {
        let details = serde_json::to_string(&result.subtests).expect("serialize WPT subtests");
        let escaped_details = escape_xml(&details);
        let status = match result.classification {
            Classification::KnownFailure => " known-failure=\"true\"",
            Classification::Improvement => " improvement=\"true\"",
            _ => "",
        };
        xml.push_str(&format!(
            "  <testcase classname=\"wpt.{}\" name=\"{}\"{}>\n",
            escape_xml(&result.area),
            escape_xml(&result.path),
            status
        ));
        match result.classification {
            Classification::Regression => {
                let expected = result
                    .known_failure
                    .as_ref()
                    .map(|known| known.status.as_str())
                    .unwrap_or("PASS");
                xml.push_str(&format!(
                    "    <failure message=\"expected {}, got {}\">{}</failure>\n",
                    escape_xml(expected),
                    result.actual.as_str(),
                    escaped_details
                ));
            }
            Classification::KnownFailure => {
                let known = result
                    .known_failure
                    .as_ref()
                    .expect("known failure metadata");
                xml.push_str(&format!(
                    "    <system-out>KNOWN FAILURE [{}] {}: {}\n{}</system-out>\n",
                    result.actual.as_str(),
                    escape_xml(&known.issue),
                    escape_xml(&known.reason),
                    escaped_details
                ));
            }
            Classification::Improvement => {
                let known = result.known_failure.as_ref().expect("improvement metadata");
                xml.push_str(&format!(
                    "    <system-out>IMPROVEMENT: passed despite known {} failure ({})\n{}</system-out>\n",
                    known.status.as_str(), escape_xml(&known.issue), escaped_details
                ));
            }
            Classification::Pass => {}
        }
        if matches!(
            result.classification,
            Classification::Pass | Classification::Regression
        ) {
            xml.push_str(&format!(
                "    <system-out>{}</system-out>\n",
                escaped_details
            ));
        }
        xml.push_str("  </testcase>\n");
    }
    xml.push_str("</testsuite>\n");
    xml
}

fn write_revision_reports(root: &Path, report: &WptReport) -> std::io::Result<()> {
    let revision_dir = root.join(&report.revision);
    fs::create_dir_all(&revision_dir)?;
    fs::write(
        revision_dir.join("report.json"),
        serde_json::to_vec_pretty(report).map_err(std::io::Error::other)?,
    )?;
    for (area, summary) in &report.summary.by_area {
        let area_report = WptAreaReport {
            revision: report.revision.clone(),
            area: area.clone(),
            summary: summary.clone(),
            results: report
                .results
                .iter()
                .filter(|result| result.area == *area)
                .cloned()
                .collect(),
        };
        let filename = area
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                    character
                } else {
                    '_'
                }
            })
            .collect::<String>();
        fs::write(
            revision_dir.join(format!("{filename}.json")),
            serde_json::to_vec_pretty(&area_report).map_err(std::io::Error::other)?,
        )?;
    }
    Ok(())
}

fn read_revision_report(root: &Path, revision: &str) -> std::io::Result<WptReport> {
    let bytes = fs::read(root.join(revision).join("report.json"))?;
    serde_json::from_slice(&bytes).map_err(std::io::Error::other)
}

fn diff_revision_reports(previous: &WptReport, current: &WptReport) -> WptRevisionDiff {
    let previous_results = previous
        .results
        .iter()
        .map(|result| (result.path.as_str(), result.classification))
        .collect::<BTreeMap<_, _>>();
    let current_results = current
        .results
        .iter()
        .map(|result| (result.path.as_str(), result.classification))
        .collect::<BTreeMap<_, _>>();
    let mut paths = previous_results.keys().chain(current_results.keys()).copied().collect::<Vec<_>>();
    paths.sort_unstable();
    paths.dedup();
    let changed = paths
        .into_iter()
        .filter_map(|path| {
            let previous = previous_results.get(path).copied();
            let current = current_results.get(path).copied();
            (previous != current).then(|| WptResultChange {
                path: path.to_string(),
                previous,
                current,
            })
        })
        .collect();
    WptRevisionDiff {
        previous_revision: previous.revision.clone(),
        current_revision: current.revision.clone(),
        known_failure_delta: current.summary.known_failure as i64
            - previous.summary.known_failure as i64,
        regression_delta: current.summary.regression as i64 - previous.summary.regression as i64,
        improvement_delta: current.summary.improvement as i64 - previous.summary.improvement as i64,
        changed,
    }
}

#[test]
fn revision_reports_round_trip_and_split_by_area() {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock must be after Unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "omoikane-wpt-results-{}-{unique}",
        std::process::id(),
    ));
    let results = vec![
        WptResult {
            path: "dom/a.html".to_string(),
            area: "dom".to_string(),
            actual: ActualStatus::Pass,
            classification: Classification::Pass,
            known_failure: None,
            script_errors: vec![],
            subtests: serde_json::json!([]),
        },
        WptResult {
            path: "css/b.html".to_string(),
            area: "css".to_string(),
            actual: ActualStatus::Timeout,
            classification: Classification::KnownFailure,
            known_failure: Some(known_failure(ActualStatus::Timeout)),
            script_errors: vec![],
            subtests: serde_json::json!([]),
        },
    ];
    let report = WptReport {
        revision: "abc123".to_string(),
        summary: summarize(&results),
        results,
    };

    write_revision_reports(&root, &report).unwrap();
    assert_eq!(read_revision_report(&root, "abc123").unwrap(), report);
    let area: WptAreaReport = serde_json::from_slice(
        &fs::read(root.join("abc123/css.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(area.area, "css");
    assert_eq!(area.summary.known_failure, 1);
    assert_eq!(area.results.len(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn revision_diff_reports_known_failure_changes() {
    let result = |classification| WptResult {
        path: "dom/a.html".to_string(),
        area: "dom".to_string(),
        actual: ActualStatus::Pass,
        classification,
        known_failure: None,
        script_errors: vec![],
        subtests: serde_json::json!([]),
    };
    let previous_results = vec![result(Classification::KnownFailure)];
    let current_results = vec![result(Classification::Pass)];
    let previous = WptReport {
        revision: "old".to_string(),
        summary: summarize(&previous_results),
        results: previous_results,
    };
    let current = WptReport {
        revision: "new".to_string(),
        summary: summarize(&current_results),
        results: current_results,
    };

    let diff = diff_revision_reports(&previous, &current);
    assert_eq!(diff.known_failure_delta, -1);
    assert_eq!(diff.regression_delta, 0);
    assert_eq!(diff.changed.len(), 1);
    assert_eq!(diff.changed[0].previous, Some(Classification::KnownFailure));
    assert_eq!(diff.changed[0].current, Some(Classification::Pass));
}

#[test]
fn junit_report_escapes_xml_and_reports_mismatches() {
    let results = vec![WptResult {
        path: "a<&\"'".to_string(),
        area: "dom".to_string(),
        actual: ActualStatus::Fail,
        classification: Classification::Regression,
        known_failure: None,
        script_errors: vec![],
        subtests: serde_json::json!({"message": "boom <x>"}),
    }];
    let report = WptReport {
        revision: "test".to_string(),
        summary: summarize(&results),
        results,
    };
    let xml = junit_xml(&report);
    assert!(xml.contains("tests=\"1\" failures=\"1\""));
    assert!(xml.contains("name=\"a&lt;&amp;&quot;&apos;\""));
    assert!(xml.contains("boom &lt;x&gt;"));
}

fn known_failure(status: ActualStatus) -> KnownFailure {
    KnownFailure {
        status,
        reason: "not implemented <yet>".to_string(),
        issue: "#123&tracking".to_string(),
        expires: None,
    }
}

#[test]
fn classifications_distinguish_known_failures_regressions_and_improvements() {
    assert_eq!(
        classify(ActualStatus::Fail, Some(&known_failure(ActualStatus::Fail))),
        Classification::KnownFailure
    );
    assert_eq!(
        classify(
            ActualStatus::Timeout,
            Some(&known_failure(ActualStatus::Timeout))
        ),
        Classification::KnownFailure
    );
    assert_eq!(
        classify(
            ActualStatus::Error,
            Some(&known_failure(ActualStatus::Timeout))
        ),
        Classification::Regression
    );
    assert_eq!(
        classify(ActualStatus::Fail, None),
        Classification::Regression
    );
    assert_eq!(
        classify(ActualStatus::Pass, Some(&known_failure(ActualStatus::Fail))),
        Classification::Improvement
    );
    assert_eq!(classify(ActualStatus::Pass, None), Classification::Pass);
}

#[test]
fn junit_marks_known_failures_without_skipping_and_reports_improvements() {
    let results = vec![
        WptResult {
            path: "dom/known.html".to_string(),
            area: "dom".to_string(),
            actual: ActualStatus::Timeout,
            classification: Classification::KnownFailure,
            known_failure: Some(known_failure(ActualStatus::Timeout)),
            script_errors: vec![],
            subtests: serde_json::json!([]),
        },
        WptResult {
            path: "css/improved.html".to_string(),
            area: "css".to_string(),
            actual: ActualStatus::Pass,
            classification: Classification::Improvement,
            known_failure: Some(known_failure(ActualStatus::Fail)),
            script_errors: vec![],
            subtests: serde_json::json!([]),
        },
    ];
    let report = WptReport {
        revision: "test".to_string(),
        summary: summarize(&results),
        results,
    };
    let xml = junit_xml(&report);
    assert!(xml.contains("failures=\"0\""));
    assert!(xml.contains("known-failure=\"true\""));
    assert!(xml.contains("KNOWN FAILURE [TIMEOUT] #123&amp;tracking: not implemented &lt;yet&gt;"));
    assert!(xml.contains("improvement=\"true\""));
    assert!(xml.contains("IMPROVEMENT: passed despite known FAIL failure"));
    assert!(!xml.contains("<skipped"));
}

#[test]
fn manifest_validation_rejects_duplicates_and_invalid_known_failures() {
    let manifest: Manifest = serde_json::from_value(serde_json::json!({"tests": [
        {"path": "dom/a.html"},
        {"path": "dom/a.html"},
        {"path": "dom/b.html", "known_failure": {
            "status": "PASS", "reason": "", "issue": "", "expires": ""
        }}
    ]}))
    .unwrap();
    let errors = validate_manifest(&manifest).unwrap_err().join("\n");
    assert!(errors.contains("duplicate test path"));
    assert!(errors.contains("status must not be PASS"));
    assert!(errors.contains("reason must not be empty"));
    assert!(errors.contains("issue must not be empty"));
    assert!(errors.contains("expires must not be empty"));
    assert!(
        serde_json::from_value::<Manifest>(serde_json::json!({
            "tests": [{"path": "dom/a.html", "expected": "PASS"}]
        }))
        .is_err(),
        "legacy expected metadata must be rejected"
    );
}

#[test]
fn selected_wpt_testharness_cases_match_expectations() {
    let root = std::env::var("WPT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("target/wpt"));
    if !root.join("resources/testharness.js").is_file() {
        if std::env::var_os("WPT_REQUIRED").is_some() {
            panic!(
                "WPT checkout required but missing (WPT_ROOT={})",
                root.display()
            );
        }
        eprintln!(
            "WPT checkout missing; run scripts/fetch-wpt.sh (WPT_ROOT={})",
            root.display()
        );
        return;
    }
    let manifest: Manifest =
        serde_json::from_slice(&fs::read("tests/wpt/manifest.json").expect("read WPT manifest"))
            .expect("parse WPT manifest");
    if let Err(errors) = validate_manifest(&manifest) {
        panic!("invalid WPT manifest:\n{}", errors.join("\n"));
    }
    let revision = fs::read_to_string("tests/wpt/revision.txt")
        .expect("read WPT revision")
        .trim()
        .to_string();
    let server = StaticServer::start(root);
    let mut results = Vec::new();
    let mut regressions = Vec::new();
    for case in manifest.tests {
        let url = format!("{}/{}", server.base_url, case.path);
        let mut client = Client::new();
        let response = client
            .get(&url)
            .unwrap_or_else(|error| panic!("GET {url}: {error}"));
        assert_eq!(
            response.status_code(),
            200,
            "WPT resource missing: {}",
            case.path
        );
        let document = TreeBuilder::parse(&String::from_utf8_lossy(response.body())).document();
        let base: Url = url.parse().expect("parse WPT URL");
        let mut runtime = JsRuntime::with_document(document).expect("create WPT runtime");
        let errors = runtime.execute_document_scripts(Some(&base));
        runtime
            .wire_inline_event_handlers()
            .expect("wire WPT handlers");
        runtime.fire_load().expect("fire WPT load");
        runtime.run_timers(5_000, 10, 2_000);
        runtime.run_jobs().expect("drain WPT jobs");
        let complete = js_bool(&mut runtime, "globalThis.__wpt_complete === true");
        let passed = js_bool(&mut runtime, "__wpt_complete===true && __wpt_harness_status===0 && __wpt_results.length>0 && __wpt_results.every(test=>test.status===0)");
        let actual = if !errors.is_empty() {
            ActualStatus::Error
        } else if passed {
            ActualStatus::Pass
        } else if complete {
            ActualStatus::Fail
        } else {
            ActualStatus::Timeout
        };
        let classification = classify(actual, case.known_failure.as_ref());
        let details = runtime
            .eval("JSON.stringify(globalThis.__wpt_results||[])")
            .ok()
            .and_then(|value| value.as_string().map(|text| text.to_std_string_escaped()))
            .unwrap_or_else(|| "[]".to_string());
        println!(
            "WPT {}: actual={} classification={classification:?}",
            case.path,
            actual.as_str()
        );
        if classification == Classification::Regression {
            let expected = case
                .known_failure
                .as_ref()
                .map(|known| known.status.as_str())
                .unwrap_or("PASS");
            regressions.push(format!(
                "WPT {} regression: expected={} actual={}; script errors={errors:?}; results={details}",
                case.path, expected, actual.as_str()
            ));
        }
        results.push(WptResult {
            area: area_for_path(&case.path),
            path: case.path,
            actual,
            classification,
            known_failure: case.known_failure,
            script_errors: errors,
            subtests: serde_json::from_str(&details).unwrap_or(serde_json::Value::Null),
        });
    }
    let summary = summarize(&results);
    println!(
        "WPT summary: total={} pass={} known-failure={} regression={} improvement={}",
        summary.total, summary.pass, summary.known_failure, summary.regression, summary.improvement
    );
    for (area, counts) in &summary.by_area {
        println!(
            "WPT area {area}: total={} pass={} known-failure={} regression={} improvement={}",
            counts.total, counts.pass, counts.known_failure, counts.regression, counts.improvement
        );
    }
    let report = WptReport {
        revision,
        summary,
        results,
    };
    let results_root = std::env::var("WPT_RESULTS_DIR")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("GITHUB_ACTIONS")
                .is_some()
                .then(|| PathBuf::from(".artifacts/wpt/results"))
        });
    if let Some(root) = results_root {
        write_revision_reports(&root, &report).expect("write revision-scoped WPT reports");
        if let Ok(previous_revision) = std::env::var("WPT_COMPARE_REVISION") {
            let previous = read_revision_report(&root, &previous_revision)
                .expect("read previous WPT revision report");
            println!(
                "WPT revision diff: {}",
                serde_json::to_string_pretty(&diff_revision_reports(&previous, &report))
                    .expect("serialize WPT revision diff")
            );
        }
    }
    if let Ok(path) = std::env::var("WPT_REPORT") {
        let path = PathBuf::from(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create report directory");
        }
        fs::write(
            &path,
            serde_json::to_vec_pretty(&report).expect("serialize WPT report"),
        )
        .expect("write WPT report");
    }
    if let Ok(path) = std::env::var("WPT_JUNIT") {
        let path = PathBuf::from(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create JUnit directory");
        }
        fs::write(&path, junit_xml(&report)).expect("write WPT JUnit report");
    }
    assert!(regressions.is_empty(), "{}", regressions.join("\n"));
}
