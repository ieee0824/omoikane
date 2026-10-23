//! Deferred startup must leave the page owner available during resource waits.
use super::CdpSession;
use crate::http::{HttpParseError, HttpRequest, HttpResponse, HttpTransport};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

struct HeldStartupScript {
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
    let mut session = CdpSession::new().unwrap();
    session.set_deferred_document_startup(true);
    let meta = if late_connect_policy {
        "<meta http-equiv='Content-Security-Policy' content=\"connect-src 'none'\">"
    } else {
        ""
    };
    session.install_document("https://example.test/", &format!(
        "<head><script src='/held.js'></script>{meta}</head><body><script>startupOrder.push('inline')</script></body>"
    ), 1, "null").unwrap();
    session.runtime.set_http_resource_transport(Some(transport));
    session
        .runtime
        .eval(
            "globalThis.startupOrder=[]; \
        addEventListener('DOMContentLoaded',()=>startupOrder.push('dom')); \
        addEventListener('load',()=>startupOrder.push('load'));",
        )
        .unwrap();
    session.drive_event_loop(0).unwrap();
    session
}

fn check_held_startup(late_connect_policy: bool) {
    let (transport, observed, _, mut release) = held_startup_transport();
    let owner_transport = transport.clone();
    let (progress, returned) = mpsc::channel();
    let (resume, continuation) = mpsc::channel();
    let owner = std::thread::spawn(move || {
        let mut session = startup_session(owner_transport, late_connect_policy);
        session.drive_event_loop(0).unwrap();
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
    let request_started = observed.recv_timeout(Duration::from_secs(5));
    let early = returned.recv_timeout(Duration::from_secs(5));
    // Release and join before assertions, including on the old blocking path.
    release.release();
    let _ = resume.send(());
    let order = owner.join().unwrap();
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
    let owner = std::thread::spawn(move || {
        let mut session = startup_session(transport, false);
        session.drive_event_loop(0).unwrap();
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
    let request_started = observed.recv_timeout(Duration::from_secs(5));
    let waiting = returned.recv_timeout(Duration::from_secs(5));
    let _ = advance.send(());
    let replacement = returned.recv_timeout(Duration::from_secs(5));
    release.release();
    let finished = completed.recv_timeout(Duration::from_secs(5));
    let _ = advance.send(());
    let retired_did_not_run = owner.join().unwrap();
    assert_eq!(request_started, Ok(()));
    assert_eq!(waiting, Ok(true));
    assert_eq!(replacement, Ok(true));
    assert_eq!(finished, Ok(()));
    assert!(retired_did_not_run);
}
