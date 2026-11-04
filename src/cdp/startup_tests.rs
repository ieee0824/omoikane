//! Deferred startup must leave the page owner available during resource waits.
use super::CdpSession;
use crate::http::{HttpParseError, HttpRequest, HttpResponse, HttpTransport};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PreparationCancelled {
    ReadyDisconnected,
    ReadyTimeout,
    GoDisconnected,
    GoTimeout,
}

struct OwnerPreparation {
    ready: mpsc::Sender<Instant>,
    go: mpsc::Receiver<()>,
}

impl OwnerPreparation {
    fn wait(self, budget: Duration) -> Result<(), PreparationCancelled> {
        let deadline = Instant::now() + budget;
        self.ready
            .send(deadline)
            .map_err(|_| PreparationCancelled::ReadyDisconnected)?;
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|duration| !duration.is_zero())
            .ok_or(PreparationCancelled::GoTimeout)?;
        check_go_result(self.go.recv_timeout(remaining), Instant::now(), deadline)
    }
}

fn check_go_result(
    result: Result<(), mpsc::RecvTimeoutError>,
    now: Instant,
    deadline: Instant,
) -> Result<(), PreparationCancelled> {
    match result {
        Ok(()) if now < deadline => Ok(()),
        Ok(()) | Err(mpsc::RecvTimeoutError::Timeout) => Err(PreparationCancelled::GoTimeout),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(PreparationCancelled::GoDisconnected),
    }
}

fn await_preparation(
    ready: mpsc::Receiver<Instant>,
    go: mpsc::Sender<()>,
    budget: Duration,
) -> Result<(), PreparationCancelled> {
    let deadline = ready.recv_timeout(budget).map_err(|error| match error {
        mpsc::RecvTimeoutError::Timeout => PreparationCancelled::ReadyTimeout,
        mpsc::RecvTimeoutError::Disconnected => PreparationCancelled::ReadyDisconnected,
    })?;
    if Instant::now() >= deadline {
        return Err(PreparationCancelled::GoTimeout);
    }
    go.send(())
        .map_err(|_| PreparationCancelled::GoDisconnected)
}

fn preparation_channels() -> (OwnerPreparation, mpsc::Receiver<Instant>, mpsc::Sender<()>) {
    let (ready, observed) = mpsc::channel();
    let (resume, go) = mpsc::channel();
    (OwnerPreparation { ready, go }, observed, resume)
}

fn cancel_preparation<T>(
    command: mpsc::Sender<()>,
    release: &mut ReleaseStartupScript,
    owner: std::thread::JoinHandle<T>,
) -> T {
    drop(command);
    release.release();
    owner.join().unwrap()
}

fn startup_session(
    transport: Arc<HeldStartupScript>,
    late_connect_policy: bool,
    preparation: OwnerPreparation,
    preparation_budget: Duration,
) -> Result<CdpSession, PreparationCancelled> {
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
    // No deferred-script drive starts before the independent preparation handshake.
    preparation.wait(preparation_budget)?;
    session.drive_event_loop(0).unwrap();
    Ok(session)
}

fn check_held_startup(late_connect_policy: bool) {
    let (transport, observed, _, mut release) = held_startup_transport();
    let owner_transport = transport.clone();
    let (progress, returned) = mpsc::channel();
    let (resume, continuation) = mpsc::channel();
    let (preparation, ready, go) = preparation_channels();
    let owner = std::thread::spawn(move || {
        let mut session = startup_session(
            owner_transport,
            late_connect_policy,
            preparation,
            Duration::from_secs(5),
        )?;
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
        Ok::<_, PreparationCancelled>(
            session
                .runtime
                .eval("JSON.stringify(startupOrder)")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
        )
    });
    if let Err(error) = await_preparation(ready, go, Duration::from_secs(5)) {
        let outcome = cancel_preparation(resume, &mut release, owner);
        panic!("startup preparation failed: {error:?}; owner: {outcome:?}");
    }
    let request_started = observed.recv_timeout(Duration::from_secs(5));
    let early = returned.recv_timeout(Duration::from_secs(5));
    // Release and join before assertions, including on the old blocking path.
    release.release();
    let _ = resume.send(());
    let order = owner.join().unwrap().unwrap();
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
    let (preparation, ready, go) = preparation_channels();
    let owner = std::thread::spawn(move || {
        let mut session = startup_session(transport, false, preparation, Duration::from_secs(5))?;
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
        Ok::<_, PreparationCancelled>(
            session
                .runtime
                .eval("localStorage.getItem('heldScriptRan')===null")
                .unwrap()
                .to_boolean(),
        )
    });
    if let Err(error) = await_preparation(ready, go, Duration::from_secs(5)) {
        let outcome = cancel_preparation(advance, &mut release, owner);
        panic!("startup preparation failed: {error:?}; owner: {outcome:?}");
    }
    let request_started = observed.recv_timeout(Duration::from_secs(5));
    let waiting = returned.recv_timeout(Duration::from_secs(5));
    let _ = advance.send(());
    let replacement = returned.recv_timeout(Duration::from_secs(5));
    release.release();
    let finished = completed.recv_timeout(Duration::from_secs(5));
    let _ = advance.send(());
    let retired_did_not_run = owner.join().unwrap().unwrap();
    assert_eq!(request_started, Ok(()));
    assert_eq!(waiting, Ok(true));
    assert_eq!(replacement, Ok(true));
    assert_eq!(finished, Ok(()));
    assert!(retired_did_not_run);
}

#[test]
fn preparation_accepts_go_only_before_its_deadline() {
    let deadline = Instant::now();
    assert_eq!(
        check_go_result(Ok(()), deadline - Duration::from_nanos(1), deadline),
        Ok(())
    );
    for now in [deadline, deadline + Duration::from_nanos(1)] {
        assert_eq!(
            check_go_result(Ok(()), now, deadline),
            Err(PreparationCancelled::GoTimeout)
        );
    }
}

#[test]
fn preparation_rejects_expired_ready_without_go() {
    let (ready, observed) = mpsc::channel();
    let (resume, go) = mpsc::channel();
    ready.send(Instant::now()).unwrap();
    assert_eq!(
        await_preparation(observed, resume, Duration::ZERO),
        Err(PreparationCancelled::GoTimeout)
    );
    assert_eq!(go.try_recv(), Err(mpsc::TryRecvError::Disconnected));
}

#[test]
fn preparation_cancellation_releases_and_joins_without_fetch() {
    use std::sync::atomic::AtomicBool;
    for ready_timeout in [true, false] {
        let (transport, _observed, _completed, mut release) = held_startup_transport();
        let (preparation, ready, go) = preparation_channels();
        let fetched = Arc::new(AtomicBool::new(false));
        let owner_fetched = fetched.clone();
        let (start, started) = mpsc::channel();
        let (command, commands) = mpsc::channel();
        let owner = std::thread::spawn(move || {
            started.recv().unwrap();
            preparation.wait(if ready_timeout {
                Duration::from_secs(5)
            } else {
                Duration::ZERO
            })?;
            owner_fetched.store(true, Ordering::SeqCst);
            Ok::<_, PreparationCancelled>(())
        });
        if ready_timeout {
            assert_eq!(
                await_preparation(ready, go, Duration::ZERO),
                Err(PreparationCancelled::ReadyTimeout)
            );
            start.send(()).unwrap();
        } else {
            start.send(()).unwrap();
            let deadline = ready.recv().unwrap();
            // The actual owner uses a zero budget: even a queued GO cannot start fetch.
            go.send(()).ok();
            assert!(deadline <= Instant::now());
            drop(ready);
            drop(go);
        }
        let outcome = cancel_preparation(command, &mut release, owner);
        assert_eq!(
            outcome,
            Err(if ready_timeout {
                PreparationCancelled::ReadyDisconnected
            } else {
                PreparationCancelled::GoTimeout
            })
        );
        assert!(!fetched.load(Ordering::SeqCst));
        assert_eq!(commands.try_recv(), Err(mpsc::TryRecvError::Disconnected));
        assert_eq!(transport.release.lock().unwrap().try_recv(), Ok(()));
    }
}

#[test]
fn preparation_cleanup_preserves_owner_panic() {
    let (transport, _, _, mut release) = held_startup_transport();
    let (command, commands) = mpsc::channel();
    let owner = std::thread::spawn(|| panic!("preparation owner panic"));
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        cancel_preparation(command, &mut release, owner);
    }));
    assert!(outcome.is_err());
    assert_eq!(commands.try_recv(), Err(mpsc::TryRecvError::Disconnected));
    assert_eq!(transport.release.lock().unwrap().try_recv(), Ok(()));
}

#[test]
fn preparation_disconnected_channels_are_reported() {
    let (preparation, ready, go) = preparation_channels();
    drop(preparation);
    assert_eq!(
        await_preparation(ready, go, Duration::from_secs(5)),
        Err(PreparationCancelled::ReadyDisconnected)
    );
    let (preparation, ready, go) = preparation_channels();
    drop(go);
    assert_eq!(
        preparation.wait(Duration::from_secs(5)),
        Err(PreparationCancelled::GoDisconnected)
    );
    assert!(ready.recv().is_ok());
}

#[test]
fn startup_session_preparation_cancellation_never_notifies_transport() {
    for ready_timeout in [true, false] {
        let (transport, observed, completed, mut release) = held_startup_transport();
        let owner_transport = transport.clone();
        let (preparation, ready, go) = preparation_channels();
        let (start, started) = mpsc::channel();
        let (command, commands) = mpsc::channel();
        let owner = std::thread::spawn(move || {
            started.recv().unwrap();
            startup_session(
                owner_transport,
                false,
                preparation,
                if ready_timeout {
                    Duration::from_secs(5)
                } else {
                    Duration::ZERO
                },
            )
            .map(|_| ())
        });
        if ready_timeout {
            assert_eq!(
                await_preparation(ready, go, Duration::ZERO),
                Err(PreparationCancelled::ReadyTimeout)
            );
            start.send(()).unwrap();
        } else {
            // Queue GO before initialization: the expired owner deadline must still reject it.
            go.send(()).unwrap();
            start.send(()).unwrap();
            assert!(ready.recv().unwrap() <= Instant::now());
            drop(ready);
            drop(go);
        }
        let outcome = cancel_preparation(command, &mut release, owner);
        assert_eq!(
            outcome,
            Err(if ready_timeout {
                PreparationCancelled::ReadyDisconnected
            } else {
                PreparationCancelled::GoTimeout
            })
        );
        assert_eq!(observed.try_recv(), Err(mpsc::TryRecvError::Empty));
        assert_eq!(completed.try_recv(), Err(mpsc::TryRecvError::Empty));
        assert_eq!(transport.other_requests.load(Ordering::SeqCst), 0);
        assert_eq!(commands.try_recv(), Err(mpsc::TryRecvError::Disconnected));
        assert_eq!(transport.release.lock().unwrap().try_recv(), Ok(()));
    }
}
