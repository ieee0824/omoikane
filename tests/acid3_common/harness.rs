//! Shared Acid3 test harness: a manifest-driven fixture HTTP server plus a
//! runner that drives the Omoikane engine through the Acid3 page.
//!
//! This file is referenced verbatim (via `#[path = ...]`) by both
//! `examples/acid3.rs` (the CLI runner) and `tests/acid3_harness.rs` (the
//! integration test), so it must stay dependency-free beyond `std`,
//! `serde`/`serde_json`, and the public `omoikane` API.
//!
//! IMPORTANT: nothing here modifies the browser engine. It only calls existing
//! public APIs to obtain an honest baseline of the current engine behaviour.

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use omoikane::html::TreeBuilder;
use omoikane::http::{Client, Url};
use omoikane::js::JsRuntime;

/// Absolute path to `tests/fixtures/acid3`.
pub fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("acid3")
}

// ---------------------------------------------------------------------------
// Manifest
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, serde::Deserialize)]
struct ManifestFile {
    path: String,
    status: u16,
    content_type: String,
}

#[derive(Clone, Debug, serde::Deserialize)]
struct Manifest {
    files: Vec<ManifestFile>,
}

/// Loads the fixture manifest that records the exact HTTP status code and
/// Content-Type acid3.acidtests.org returns for each resource.
fn load_manifest(dir: &Path) -> Manifest {
    let raw = std::fs::read_to_string(dir.join("manifest.json"))
        .expect("tests/fixtures/acid3/manifest.json must exist");
    serde_json::from_str(&raw).expect("manifest.json must be valid JSON")
}

// ---------------------------------------------------------------------------
// Fixture HTTP server
// ---------------------------------------------------------------------------

/// A local HTTP/1.1 server that serves the vendored Acid3 fixtures from
/// `tests/fixtures/acid3` on `127.0.0.1` using the status code and Content-Type
/// declared in `manifest.json`. The root path (`/`) maps to `acid3.html`, just
/// like the canonical origin.
///
/// The worker thread is detached and lives until the process exits, matching
/// the pattern used by the existing HTTP client tests.
pub struct FixtureServer {
    port: u16,
}

impl FixtureServer {
    /// Starts the server on an ephemeral port and returns once it is bound.
    pub fn start() -> Self {
        Self::start_in(fixture_dir())
    }

    /// Starts the server serving `dir`.
    pub fn start_in(dir: PathBuf) -> Self {
        let manifest = Arc::new(load_manifest(&dir));
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        let port = listener.local_addr().unwrap().port();
        let dir = Arc::new(dir);

        std::thread::spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        let manifest = Arc::clone(&manifest);
                        let dir = Arc::clone(&dir);
                        // Handle sequentially; the engine fetches serially anyway.
                        let _ = handle_connection(stream, &manifest, &dir);
                    }
                    Err(_) => break,
                }
            }
        });

        FixtureServer { port }
    }

    /// Base URL, e.g. `http://127.0.0.1:54321`.
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// URL of the main Acid3 page.
    pub fn acid3_url(&self) -> String {
        format!("{}/acid3.html", self.base_url())
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "OK",
    }
}

/// Guesses a Content-Type from a file extension for resources not listed in the
/// manifest. The manifest is authoritative for everything Acid3 actually loads.
fn guess_content_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "png" => "image/png",
        "gif" => "image/gif",
        "jpg" | "jpeg" => "image/jpeg",
        "svg" => "image/svg+xml",
        "xml" => "text/xml",
        "ttf" => "application/x-truetype-font",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn handle_connection(
    mut stream: TcpStream,
    manifest: &Manifest,
    dir: &Path,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);

    // Request line.
    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Ok(());
    }
    // Drain headers.
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 {
            break;
        }
        if header.trim().is_empty() {
            break;
        }
    }

    let raw_path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .to_string();
    // Strip query/fragment and leading slash.
    let path = raw_path
        .split(['?', '#'])
        .next()
        .unwrap_or("/")
        .trim_start_matches('/')
        .to_string();
    let path = if path.is_empty() {
        "acid3.html".to_string()
    } else {
        path
    };

    let (status, content_type) = manifest
        .files
        .iter()
        .find(|f| f.path == path)
        .map(|f| (f.status, f.content_type.clone()))
        .unwrap_or_else(|| (200, guess_content_type(&path).to_string()));

    // Prevent path traversal; only serve files inside the fixture dir.
    let safe = !path.contains("..") && !path.starts_with('/');
    let body = if safe {
        std::fs::read(dir.join(&path)).ok()
    } else {
        None
    };

    let (status, body) = match body {
        Some(bytes) => (status, bytes),
        None => (404, b"not found".to_vec()),
    };

    let header = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        status,
        reason_phrase(status),
        content_type,
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(&body)?;
    stream.flush()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------

/// How the runner advances the Acid3 test loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriveMode {
    /// Faithful browser emulation: wire the page's `on*` inline handlers and
    /// dispatch the real `load` event (so `<body onload="update()">` runs
    /// through the engine's own event machinery), then advance the event loop's
    /// virtual clock so scheduled `setTimeout(update, delay)` tasks fire,
    /// exactly as a real page load would. Exposes whatever breaks in the
    /// engine's handler-wiring and timer/callback plumbing.
    Faithful,
    /// Force the loop forward by invoking `update()` directly N times, bypassing
    /// `setTimeout`. Gives an upper-bound baseline of how many individual tests
    /// the engine can pass when the loop itself is not the blocker.
    DirectDrive,
}

/// Outcome of a single Acid3 run.
#[derive(Clone, Debug)]
pub struct Acid3Run {
    pub mode: DriveMode,
    /// HTTP status of the acid3.html fetch.
    pub page_status: u16,
    /// Number of DOM nodes registered after parsing (sanity signal).
    pub html_bytes: usize,
    /// `typeof update` after running all document scripts.
    pub update_typeof: String,
    /// Errors returned by `execute_document_scripts`.
    pub script_errors: Vec<String>,
    /// Errors raised while invoking the load handler / driving the loop.
    pub drive_errors: Vec<String>,
    /// Global `tests.length` (total number of Acid3 subtests), if readable.
    pub total: Option<i64>,
    /// Global `score` after driving, if readable.
    pub score: Option<i64>,
    /// Global `index` (how far the loop advanced), if readable.
    pub index: Option<i64>,
    /// `document.getElementById('score').firstChild.data` after driving.
    pub score_text: Option<String>,
    /// Global `log` accumulated by the harness (per-test failures).
    pub log: Option<String>,
    /// How many times the loop step actually executed.
    pub iterations: usize,
}

/// Fetches, parses, scripts, and drives the Acid3 page, returning an honest
/// snapshot of the current engine behaviour. Never panics on engine failure.
pub fn run_acid3(base_url: &str, mode: DriveMode) -> Acid3Run {
    let acid3_url = format!("{}/acid3.html", base_url.trim_end_matches('/'));

    // 1. Fetch over HTTP.
    let mut client = Client::new();
    let response = client.get(&acid3_url).expect("fetch acid3.html");
    let page_status = response.status_code();
    let html = String::from_utf8_lossy(response.body()).to_string();
    let html_bytes = html.len();

    // 2. Parse + build runtime.
    let document = TreeBuilder::parse(&html).document();
    let base: Url = acid3_url.parse().expect("parse base url");
    let mut runtime = JsRuntime::with_document(document).expect("create runtime");

    // 3. Execute all inline / external <script>s (fires DOMContentLoaded).
    let script_errors = runtime.execute_document_scripts(Some(&base));

    let update_typeof = eval_string(&mut runtime, "typeof update").unwrap_or_default();

    let mut drive_errors = Vec::new();
    let mut iterations = 0usize;

    match mode {
        DriveMode::Faithful => {
            // Wire the page's on* inline attributes and dispatch the real load
            // event so <body onload="update()"> starts the Acid3 driver through
            // the engine's own event pipeline -- no manual update() call.
            if let Err(e) = runtime.wire_inline_event_handlers() {
                drive_errors.push(format!("wire inline handlers: {e}"));
            }
            if let Err(e) = runtime.fire_load() {
                drive_errors.push(format!("fire load: {e}"));
            }
            // Advance virtual time so setTimeout(update, delay) tasks fire.
            // Cap: 60 virtual seconds at the page's 10ms delay = 6000 ticks.
            let delay_ms = 10u64;
            let max_ticks = 6000usize;
            let mut last_index = read_int(&mut runtime, "index").unwrap_or(-1);
            let mut stalled = 0usize;
            for _ in 0..max_ticks {
                iterations += 1;
                if let Err(e) = runtime.tick(delay_ms) {
                    drive_errors.push(format!("tick: {e}"));
                    break;
                }
                let idx = read_int(&mut runtime, "index").unwrap_or(last_index);
                let total = read_int(&mut runtime, "tests.length");
                if let Some(t) = total {
                    if idx >= t {
                        break;
                    }
                }
                if idx == last_index {
                    stalled += 1;
                    // The event loop produced no forward progress; the timer
                    // queue has drained. Stop instead of spinning.
                    if stalled >= 3 {
                        break;
                    }
                } else {
                    stalled = 0;
                    last_index = idx;
                }
            }
        }
        DriveMode::DirectDrive => {
            // Direct-drive bypasses the page's timer chain, but connected
            // iframe/object loads are browser macrotasks rather than part of
            // that bypass. Flush the already queued zero-delay resource tasks
            // once so preparation tests such as Acid3 test 65 complete before
            // their later assertions are invoked directly.
            if let Err(e) = runtime.tick(0) {
                drive_errors.push(format!("initial resource tasks: {e}"));
            }
            // Invoke update() directly, once per subtest, bypassing setTimeout.
            let total = read_int(&mut runtime, "tests.length").unwrap_or(0);
            // Budget: one call per test plus a bounded retry allowance.
            let max_calls = if total > 0 {
                (total as usize) + 4000
            } else {
                4000
            };
            let mut last_index = read_int(&mut runtime, "index").unwrap_or(-1);
            let mut stall = 0usize;
            for _ in 0..max_calls {
                iterations += 1;
                if let Err(e) = runtime.eval_safe("if (typeof update === 'function') update();") {
                    drive_errors.push(format!("update(): {e}"));
                    break;
                }
                let idx = read_int(&mut runtime, "index").unwrap_or(last_index);
                if total > 0 && idx >= total {
                    break;
                }
                if idx == last_index {
                    stall += 1;
                    // A test stuck on "retry" self-resolves after 500 attempts;
                    // give it room but do not spin forever.
                    if stall > 600 {
                        break;
                    }
                } else {
                    stall = 0;
                    last_index = idx;
                }
            }
        }
    }

    Acid3Run {
        mode,
        page_status,
        html_bytes,
        update_typeof,
        script_errors,
        drive_errors,
        total: read_int(&mut runtime, "tests.length"),
        score: read_int(&mut runtime, "score"),
        index: read_int(&mut runtime, "index"),
        score_text: eval_string(
            &mut runtime,
            "(document.getElementById('score') && document.getElementById('score').firstChild) ? String(document.getElementById('score').firstChild.data) : null",
        ),
        log: eval_string(&mut runtime, "typeof log !== 'undefined' ? String(log) : null"),
        iterations,
    }
}

/// Evaluates `expr` and returns it coerced to a Rust `String`. Any thrown error
/// is captured as `<<eval-error: ...>>` rather than propagated.
fn eval_string(runtime: &mut JsRuntime, expr: &str) -> Option<String> {
    let wrapped = format!(
        "(function(){{ try {{ var __v = ({expr}); return (__v === null || __v === undefined) ? '' : String(__v); }} catch (e) {{ return '<<eval-error: ' + e + '>>'; }} }})()"
    );
    match runtime.eval(&wrapped) {
        Ok(value) => value.as_string().map(|s| s.to_std_string_escaped()),
        Err(_) => None,
    }
}

/// Reads a numeric global as an `i64`, returning `None` if unreadable/NaN.
fn read_int(runtime: &mut JsRuntime, expr: &str) -> Option<i64> {
    let s = eval_string(
        runtime,
        &format!("(function(){{ var v = ({expr}); return (typeof v === 'number' && isFinite(v)) ? v : NaN; }})()"),
    )?;
    let trimmed = s.trim();
    if trimmed.is_empty() || trimmed == "NaN" || trimmed.starts_with("<<") {
        return None;
    }
    trimmed.parse::<f64>().ok().map(|f| f as i64)
}
