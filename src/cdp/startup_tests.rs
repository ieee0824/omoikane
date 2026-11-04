//! Deferred startup must leave the page owner available during resource waits.
use super::CdpSession;
use crate::http::{HttpParseError, HttpRequest, HttpResponse, HttpTransport};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const STARTUP_PHASES: [&str; 18] = [
    "spawn_begin",
    "owner_entry",
    "session_new_begin",
    "session_new_end",
    "install_begin",
    "install_end",
    "transport_ready",
    "listeners_begin",
    "listeners_end",
    "first_frame_begin",
    "first_frame_end",
    "parser_drive_begin",
    "parser_drive_end",
    "transport_enter",
    "receive_begin",
    "receive_end",
    "join_begin",
    "join_end",
];

#[derive(Clone, Default)]
struct StartupDiagnostic(Option<Arc<StartupDiagnosticState>>);

struct StartupDiagnosticState {
    started: Instant,
    slots: [AtomicU64; STARTUP_PHASES.len()],
    duplicate: AtomicBool,
    overflow: AtomicBool,
}

impl StartupDiagnostic {
    fn from_environment() -> Self {
        Self::new(
            std::env::var_os("OMOIKANE_GATE_DIAGNOSTICS").as_deref()
                == Some(std::ffi::OsStr::new("1")),
        )
    }

    fn new(enabled: bool) -> Self {
        Self(enabled.then(|| {
            Arc::new(StartupDiagnosticState {
                started: Instant::now(),
                slots: std::array::from_fn(|_| AtomicU64::new(0)),
                duplicate: AtomicBool::new(false),
                overflow: AtomicBool::new(false),
            })
        }))
    }

    // First-write slots have no synchronization role in the tested protocol.
    fn stamp(&self, phase: &'static str) {
        let Some(state) = &self.0 else { return };
        let index = STARTUP_PHASES
            .iter()
            .position(|name| *name == phase)
            .unwrap();
        let Some(value) = u64::try_from(state.started.elapsed().as_nanos())
            .ok()
            .and_then(|value| value.checked_add(1))
        else {
            state.overflow.store(true, Ordering::Relaxed);
            return;
        };
        if state.slots[index]
            .compare_exchange(0, value, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            state.duplicate.store(true, Ordering::Relaxed);
        }
    }

    fn snapshot(&self) -> Option<serde_json::Value> {
        let state = self.0.as_ref()?;
        let phases: serde_json::Map<String, serde_json::Value> = STARTUP_PHASES
            .iter()
            .zip(&state.slots)
            .map(|(name, slot)| {
                let time = slot.load(Ordering::Relaxed).checked_sub(1);
                ((*name).to_owned(), serde_json::json!(time))
            })
            .collect();
        Some(serde_json::json!({
            "phases_ns": phases,
            "duplicate": state.duplicate.load(Ordering::Relaxed),
            "overflow": state.overflow.load(Ordering::Relaxed),
            "scope": "owner inclusive phases; parser_drive to transport_enter includes queue and HTTP; wall only",
        }))
    }

    // Serialize only after the original release/continuation/join protocol.
    fn emit(&self, received: &Result<(), mpsc::RecvTimeoutError>, owner_panicked: bool) {
        let Some(mut report) = self.snapshot() else {
            return;
        };
        report["receive"] = serde_json::json!(match received {
            Ok(()) => "ok",
            Err(mpsc::RecvTimeoutError::Timeout) => "timeout",
            Err(mpsc::RecvTimeoutError::Disconnected) => "disconnected",
        });
        report["owner_panicked"] = serde_json::json!(owner_panicked);
        report["case"] = serde_json::json!(std::thread::current().name());
        report["schema"] = serde_json::json!("cdp-startup-owner-v1");
        eprintln!("CDP_STARTUP_DIAGNOSTIC {report}");
    }
}

struct HeldStartupScript {
    diagnostic: StartupDiagnostic,
    entered: mpsc::Sender<()>,
    finished: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
    other_requests: AtomicUsize,
}

impl HttpTransport for HeldStartupScript {
    fn send(
        &self,
        request: &HttpRequest,
        _: Option<Duration>,
    ) -> Result<HttpResponse, HttpParseError> {
        let held = request.url().to_string() == "https://example.test/held.js";
        if held {
            self.diagnostic.stamp("transport_enter");
            self.entered.send(()).unwrap();
            self.release.lock().unwrap().recv().unwrap();
            let _ = self.finished.send(());
        } else {
            self.other_requests.fetch_add(1, Ordering::SeqCst);
        }
        Ok(HttpResponse::new(
            200,
            "OK",
            vec![
                ("Content-Type".into(), "text/javascript".into()),
                ("Access-Control-Allow-Origin".into(), "*".into()),
            ],
            if held {
                b"localStorage.setItem('heldScriptRan','yes');startupOrder.push('external');"
                    .to_vec()
            } else {
                Vec::new()
            },
        ))
    }
}

struct ReleaseStartupScript(Option<mpsc::Sender<()>>);
impl ReleaseStartupScript {
    fn release(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}
impl Drop for ReleaseStartupScript {
    fn drop(&mut self) {
        self.release();
    }
}

fn held_startup_transport() -> (
    Arc<HeldStartupScript>,
    mpsc::Receiver<()>,
    mpsc::Receiver<()>,
    ReleaseStartupScript,
) {
    let (entered, observed) = mpsc::channel();
    let (finished, completed) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    (
        Arc::new(HeldStartupScript {
            diagnostic: StartupDiagnostic::from_environment(),
            entered,
            finished,
            release: Mutex::new(gate),
            other_requests: AtomicUsize::new(0),
        }),
        observed,
        completed,
        ReleaseStartupScript(Some(release)),
    )
}

fn startup_session(transport: Arc<HeldStartupScript>, late_connect_policy: bool) -> CdpSession {
    let diagnostic = transport.diagnostic.clone();
    diagnostic.stamp("session_new_begin");
    let mut session = CdpSession::new().unwrap();
    diagnostic.stamp("session_new_end");
    session.set_deferred_document_startup(true);
    let meta = if late_connect_policy {
        "<meta http-equiv='Content-Security-Policy' content=\"connect-src 'none'\">"
    } else {
        ""
    };
    diagnostic.stamp("install_begin");
    session.install_document("https://example.test/", &format!(
        "<head><script src='/held.js'></script>{meta}</head><body><script>startupOrder.push('inline')</script></body>"
    ), 1, "null").unwrap();
    diagnostic.stamp("install_end");
    session.runtime.set_http_resource_transport(Some(transport));
    diagnostic.stamp("transport_ready");
    diagnostic.stamp("listeners_begin");
    session
        .runtime
        .eval(
            "globalThis.startupOrder=[]; \
        addEventListener('DOMContentLoaded',()=>startupOrder.push('dom')); \
        addEventListener('load',()=>startupOrder.push('load'));",
        )
        .unwrap();
    diagnostic.stamp("listeners_end");
    diagnostic.stamp("first_frame_begin");
    session.drive_event_loop(0).unwrap();
    diagnostic.stamp("first_frame_end");
    session
}

fn check_held_startup(late_connect_policy: bool) {
    let (transport, observed, _, mut release) = held_startup_transport();
    let owner_transport = transport.clone();
    let (progress, returned) = mpsc::channel();
    let (resume, continuation) = mpsc::channel();
    let diagnostic = transport.diagnostic.clone();
    let owner_diagnostic = diagnostic.clone();
    diagnostic.stamp("spawn_begin");
    let owner = std::thread::spawn(move || {
        owner_diagnostic.stamp("owner_entry");
        let mut session = startup_session(owner_transport, late_connect_policy);
        owner_diagnostic.stamp("parser_drive_begin");
        session.drive_event_loop(0).unwrap();
        owner_diagnostic.stamp("parser_drive_end");
        let policy_ok = if late_connect_policy {
            session
                .runtime
                .eval(
                    "globalThis.connectPolicyResult='pending'; \
                fetch('https://example.test/not-allowed').then( \
                ()=>connectPolicyResult='allowed',()=>connectPolicyResult='blocked');",
                )
                .unwrap();
            session.runtime.run_jobs().unwrap();
            session
                .runtime
                .eval("connectPolicyResult==='blocked'")
                .unwrap()
                .to_boolean()
        } else {
            true
        };
        progress
            .send((
                session.has_pending_document_startup(),
                session
                    .runtime
                    .eval("startupOrder.length===0 && document.readyState==='loading'")
                    .unwrap()
                    .to_boolean(),
                policy_ok,
            ))
            .unwrap();
        continuation.recv().unwrap();
        session.settle_for_render().unwrap();
        session
            .runtime
            .eval("JSON.stringify(startupOrder)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped()
    });
    diagnostic.stamp("receive_begin");
    let request_started = observed.recv_timeout(Duration::from_secs(5));
    diagnostic.stamp("receive_end");
    let early = returned.recv_timeout(Duration::from_secs(5));
    // Release and join before assertions, including on the old blocking path.
    release.release();
    let _ = resume.send(());
    diagnostic.stamp("join_begin");
    let owner_result = owner.join();
    diagnostic.stamp("join_end");
    diagnostic.emit(&request_started, owner_result.is_err());
    let order = owner_result.unwrap();
    assert_eq!(request_started, Ok(()));
    assert_eq!(order, "[\"external\",\"inline\",\"dom\",\"load\"]");
    assert_eq!(
        early,
        Ok((true, true, true)),
        "startup must yield while its response is held"
    );
    assert_eq!(transport.other_requests.load(Ordering::SeqCst), 0);
}

#[test]
fn deferred_startup_yields_while_external_script_response_is_held() {
    check_held_startup(false);
}

#[test]
fn deferred_startup_uses_full_csp_policy_while_a_parser_script_is_waiting() {
    check_held_startup(true);
}

#[test]
fn replacement_cancels_waiting_parser_startup_without_executing_retired_source() {
    let (transport, observed, completed, mut release) = held_startup_transport();
    let (progress, returned) = mpsc::channel();
    let (advance, commands) = mpsc::channel();
    let diagnostic = transport.diagnostic.clone();
    let owner_diagnostic = diagnostic.clone();
    diagnostic.stamp("spawn_begin");
    let owner = std::thread::spawn(move || {
        owner_diagnostic.stamp("owner_entry");
        let mut session = startup_session(transport, false);
        owner_diagnostic.stamp("parser_drive_begin");
        session.drive_event_loop(0).unwrap();
        owner_diagnostic.stamp("parser_drive_end");
        progress
            .send(session.has_pending_document_startup())
            .unwrap();
        commands.recv().unwrap();
        session
            .install_document(
                "https://example.test/replacement",
                "<script>globalThis.replacementRan=true</script>",
                1,
                "null",
            )
            .unwrap();
        session.drive_event_loop(0).unwrap();
        session.drive_event_loop(0).unwrap();
        progress
            .send(
                !session.has_pending_document_startup()
                    && session
                        .runtime
                        .eval("replacementRan && localStorage.getItem('heldScriptRan')===null")
                        .unwrap()
                        .to_boolean(),
            )
            .unwrap();
        commands.recv().unwrap();
        session.drive_event_loop(16).unwrap();
        session
            .runtime
            .eval("localStorage.getItem('heldScriptRan')===null")
            .unwrap()
            .to_boolean()
    });
    diagnostic.stamp("receive_begin");
    let request_started = observed.recv_timeout(Duration::from_secs(5));
    diagnostic.stamp("receive_end");
    let waiting = returned.recv_timeout(Duration::from_secs(5));
    let _ = advance.send(());
    let replacement = returned.recv_timeout(Duration::from_secs(5));
    release.release();
    let finished = completed.recv_timeout(Duration::from_secs(5));
    let _ = advance.send(());
    diagnostic.stamp("join_begin");
    let owner_result = owner.join();
    diagnostic.stamp("join_end");
    diagnostic.emit(&request_started, owner_result.is_err());
    let retired_did_not_run = owner_result.unwrap();
    assert_eq!(request_started, Ok(()));
    assert_eq!(waiting, Ok(true));
    assert_eq!(replacement, Ok(true));
    assert_eq!(finished, Ok(()));
    assert!(retired_did_not_run);
}

#[test]
fn startup_diagnostic_preserves_first_timestamp_and_missing_phases() {
    let diagnostic = StartupDiagnostic::new(true);
    diagnostic.stamp("owner_entry");
    let first = diagnostic.snapshot().unwrap()["phases_ns"]["owner_entry"].clone();
    diagnostic.stamp("owner_entry");
    let report = diagnostic.snapshot().unwrap();
    assert_eq!(report["phases_ns"]["owner_entry"], first);
    assert!(first.is_u64());
    assert!(report["phases_ns"]["transport_enter"].is_null());
    assert_eq!(report["duplicate"], true);
    assert_eq!(report["overflow"], false);
}

#[test]
fn startup_diagnostic_disabled_has_no_record() {
    let diagnostic = StartupDiagnostic::new(false);
    diagnostic.stamp("owner_entry");
    assert!(diagnostic.0.is_none());
    assert!(diagnostic.snapshot().is_none());
}
