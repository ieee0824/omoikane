//! Parser-deferred fetches share workers but retain execution order.
mod diagnostic;

use super::CdpSession;
use crate::http::{HttpParseError, HttpRequest, HttpResponse, HttpTransport};
use crate::resource_worker::{
    ResourceKind, ResourceOutcome, ResourcePriority, ResourceQueue, ResourceScope,
};
use diagnostic::{Diagnostic, OwnerPreparation, PreparationCancelled};
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

struct HeldDeferredScripts {
    diagnostic: Diagnostic,
    entered: mpsc::Sender<usize>,
    finished: mpsc::Sender<usize>,
    gates: [Mutex<mpsc::Receiver<()>>; 2],
}
impl HttpTransport for HeldDeferredScripts {
    fn send(
        &self,
        request: &HttpRequest,
        _: Option<Duration>,
    ) -> Result<HttpResponse, HttpParseError> {
        let url = request.url().to_string();
        assert!(url.ends_with("/one.js") || url.ends_with("/two.js"));
        let index = usize::from(url.ends_with("/two.js"));
        self.diagnostic.stamp(if index == 0 {
            "transport_one_enter"
        } else {
            "transport_two_enter"
        });
        self.entered.send(index).unwrap();
        self.gates[index].lock().unwrap().recv().unwrap();
        let _ = self.finished.send(index);
        Ok(HttpResponse::new(
            200,
            "OK",
            vec![("Content-Type".into(), "text/javascript".into())],
            format!(
                "startupOrder.push('{}')",
                if index == 0 { "one" } else { "two" }
            )
            .into_bytes(),
        ))
    }
}
struct ReleaseDeferredScripts([Option<mpsc::Sender<()>>; 2]);
impl ReleaseDeferredScripts {
    fn release(&mut self, index: usize) {
        if let Some(sender) = self.0[index].take() {
            let _ = sender.send(());
        }
    }
}
impl Drop for ReleaseDeferredScripts {
    fn drop(&mut self) {
        self.release(0);
        self.release(1);
    }
}
fn order(session: &mut CdpSession) -> String {
    session
        .runtime
        .eval("JSON.stringify(startupOrder)")
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped()
}
enum OwnerExit {
    Completed(String),
    CancelledBeforeFetch(PreparationCancelled),
}

fn deferred_startup_owner(
    queue: ResourceQueue,
    transport: Arc<HeldDeferredScripts>,
    commands: mpsc::Receiver<()>,
    progress: mpsc::Sender<(bool, String)>,
    preparation: OwnerPreparation,
) -> OwnerExit {
    let diagnostic = transport.diagnostic.clone();
    diagnostic.stamp("owner_entry");
    diagnostic.stamp("session_new_begin");
    let mut session = CdpSession::new().unwrap();
    diagnostic.stamp("session_new_end");
    session.set_resource_queue(queue);
    session.set_deferred_document_startup(true);
    diagnostic.stamp("install_begin");
    session.install_document("https://example.test/",
        "<head><script defer src='/one.js'></script><script defer src='/two.js'></script></head>\
         <body><script>startupOrder.push('inline')</script></body>", 1, "null").unwrap();
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
    let prepared = preparation.wait(Duration::from_secs(5));
    diagnostic.preparation_result(true, prepared);
    if let Err(reason) = prepared {
        return OwnerExit::CancelledBeforeFetch(reason);
    }
    diagnostic.stamp("go_received");
    diagnostic.stamp("first_frame_begin");
    session.drive_event_loop(0).unwrap();
    diagnostic.stamp("first_frame_end");
    diagnostic.stamp("parser_drive_begin");
    session.drive_event_loop(0).unwrap();
    diagnostic.stamp("parser_drive_end");
    let first_order = order(&mut session);
    progress
        .send((session.has_pending_document_startup(), first_order))
        .unwrap();
    commands.recv().unwrap();
    session.drive_event_loop(0).unwrap();
    let second_order = order(&mut session);
    progress
        .send((session.has_pending_document_startup(), second_order))
        .unwrap();
    commands.recv().unwrap();
    session.settle_for_render().unwrap();
    OwnerExit::Completed(order(&mut session))
}

fn await_preparation(
    prepared: &mpsc::Receiver<std::time::Instant>,
    go: &mpsc::Sender<()>,
    diagnostic: &Diagnostic,
    budget: Duration,
) -> Result<(), PreparationCancelled> {
    diagnostic.stamp("ready_wait_begin");
    let preparation = prepared
        .recv_timeout(budget)
        .map_err(|error| match error {
            mpsc::RecvTimeoutError::Timeout => PreparationCancelled::ReadyTimeout,
            mpsc::RecvTimeoutError::Disconnected => PreparationCancelled::ReadyDisconnected,
        })
        .and_then(|deadline| {
            if std::time::Instant::now() >= deadline {
                return Err(PreparationCancelled::GoTimeout);
            }
            diagnostic.stamp("go_sent");
            go.send(())
                .map_err(|_| PreparationCancelled::GoDisconnected)
        });
    diagnostic.stamp("ready_wait_end");
    diagnostic.preparation_result(false, preparation);
    preparation
}

fn cancel_preparation(
    prepared: mpsc::Receiver<std::time::Instant>,
    go: mpsc::Sender<()>,
    advance: mpsc::Sender<()>,
    release: &mut ReleaseDeferredScripts,
    owner: std::thread::JoinHandle<OwnerExit>,
    diagnostic: &Diagnostic,
) -> std::thread::Result<OwnerExit> {
    drop(prepared);
    drop(go);
    drop(advance);
    release.release(0);
    release.release(1);
    diagnostic.stamp("join_begin");
    let result = owner.join();
    diagnostic.stamp("join_end");
    diagnostic.emit(&[None, None], result.is_err(), None);
    result
}

#[test]
fn deferred_parser_fetches_overlap_and_execution_waits_for_document_order() {
    let diagnostic = Diagnostic::from_environment();
    let queue = ResourceQueue::new(NonZeroUsize::new(2).unwrap());
    let (entered, observed) = mpsc::channel();
    let (finished, completed) = mpsc::channel();
    let (first, first_gate) = mpsc::channel();
    let (second, second_gate) = mpsc::channel();
    let mut release = ReleaseDeferredScripts([Some(first), Some(second)]);
    let transport = Arc::new(HeldDeferredScripts {
        diagnostic: diagnostic.clone(),
        entered,
        finished,
        gates: [Mutex::new(first_gate), Mutex::new(second_gate)],
    });
    let (advance, commands) = mpsc::channel();
    let (progress, returned) = mpsc::channel();
    let (ready, prepared) = mpsc::channel();
    let (go, start) = mpsc::channel();
    let owner_queue = queue.clone();
    diagnostic.stamp("spawn_begin");
    let owner = std::thread::spawn(move || {
        deferred_startup_owner(
            owner_queue,
            transport,
            commands,
            progress,
            OwnerPreparation { ready, go: start },
        )
    });
    let preparation = await_preparation(&prepared, &go, &diagnostic, Duration::from_secs(5));
    if let Err(reason) = preparation {
        let result = cancel_preparation(prepared, go, advance, &mut release, owner, &diagnostic);
        match result.unwrap() {
            OwnerExit::CancelledBeforeFetch(owner_reason) => {
                panic!("owner preparation failed: {reason:?}; owner: {owner_reason:?}");
            }
            OwnerExit::Completed(_) => {
                panic!("owner completed after preparation failed: {reason:?}")
            }
        }
    }
    let mut received = [None, None];
    let mut requests: Vec<_> = (0..2)
        .filter_map(|index| {
            diagnostic.stamp(if index == 0 {
                "receive_one_begin"
            } else {
                "receive_two_begin"
            });
            let result = observed.recv_timeout(Duration::from_secs(5));
            diagnostic.stamp(if index == 0 {
                "receive_one_end"
            } else {
                "receive_two_end"
            });
            received[index].insert(result).as_ref().ok().copied()
        })
        .collect();
    requests.sort();
    let initial = returned.recv_timeout(Duration::from_secs(5));
    diagnostic.stamp("release_second");
    release.release(1);
    let second_finished = completed.recv_timeout(Duration::from_secs(5));
    // With the first worker held, this barrier can run only after the second
    // worker has committed its source response to the completion channel.
    let scope = ResourceScope::default();
    let barrier = queue
        .submit(
            &scope,
            ResourcePriority::Critical,
            ResourceKind::Other,
            |_| true,
        )
        .unwrap();
    let (barrier_done, barrier_observed) = mpsc::channel();
    let waiter = std::thread::spawn(move || {
        let _ = barrier_done.send(matches!(
            barrier.recv(),
            Ok(ResourceOutcome::Complete(true))
        ));
    });
    let committed = barrier_observed.recv_timeout(Duration::from_secs(5));
    let _ = advance.send(());
    let after_second = returned.recv_timeout(Duration::from_secs(5));
    diagnostic.stamp("release_first");
    release.release(0);
    let _ = advance.send(());
    diagnostic.stamp("join_begin");
    let owner_result = owner.join();
    if owner_result.is_err() {
        // Preserve the original early unwind: do not wait for the barrier waiter.
        diagnostic.emit(&received, true, None);
    }
    let final_order = owner_result.unwrap();
    let waiter_result = waiter.join();
    diagnostic.stamp("join_end");
    diagnostic.emit(&received, false, Some(waiter_result.is_err()));
    waiter_result.unwrap();
    let final_order = match final_order {
        OwnerExit::Completed(order) => order,
        OwnerExit::CancelledBeforeFetch(reason) => {
            panic!("owner cancelled before fetch: {reason:?}")
        }
    };
    for (index, receipt) in received.iter().enumerate() {
        assert!(
            matches!(receipt, Some(Ok(_))),
            "source notification {index}: {receipt:?}"
        );
    }
    assert_eq!(
        requests,
        [0, 1],
        "both deferred sources must start before either response is released"
    );
    assert_eq!(initial, Ok((true, "[\"inline\"]".into())));
    assert_eq!(second_finished, Ok(1));
    assert_eq!(committed, Ok(true));
    assert_eq!(after_second, Ok((true, "[\"inline\"]".into())));
    assert_eq!(final_order, "[\"inline\",\"one\",\"two\",\"dom\",\"load\"]");
}

#[test]
fn preparation_timeout_and_expired_ready_do_not_send_go() {
    let (_ready, prepared) = mpsc::channel();
    let (go, start) = mpsc::channel();
    assert_eq!(
        await_preparation(&prepared, &go, &Diagnostic::default(), Duration::ZERO),
        Err(PreparationCancelled::ReadyTimeout)
    );
    assert!(matches!(start.try_recv(), Err(mpsc::TryRecvError::Empty)));
    let (ready, prepared) = mpsc::channel();
    ready.send(std::time::Instant::now()).unwrap();
    assert_eq!(
        await_preparation(
            &prepared,
            &go,
            &Diagnostic::default(),
            Duration::from_secs(5)
        ),
        Err(PreparationCancelled::GoTimeout)
    );
    assert!(matches!(start.try_recv(), Err(mpsc::TryRecvError::Empty)));
}

#[test]
fn preparation_cancellation_releases_gates_and_joins_owner_without_fetch() {
    assert_cancelled_owner(
        Duration::from_secs(5),
        false,
        &[PreparationCancelled::GoDisconnected],
    );
}

#[test]
fn expired_go_cancels_owner_without_fetch_and_releases_gates_before_join() {
    assert_cancelled_owner(Duration::ZERO, false, &[PreparationCancelled::GoTimeout]);
}

#[test]
fn ready_timeout_cancels_owner_without_fetch_and_releases_gates_before_join() {
    assert_cancelled_owner(
        Duration::from_secs(5),
        true,
        &[
            PreparationCancelled::ReadyDisconnected,
            PreparationCancelled::GoDisconnected,
        ],
    );
}

fn assert_cancelled_owner(
    budget: Duration,
    ready_timeout: bool,
    expected: &[PreparationCancelled],
) {
    use std::sync::atomic::{AtomicBool, Ordering};
    let (ready, prepared) = mpsc::channel();
    let (go, start) = mpsc::channel();
    let (advance, commands) = mpsc::channel();
    let (prepare, prepare_now) = mpsc::channel();
    let (one, first) = mpsc::channel();
    let (two, second) = mpsc::channel();
    let mut release = ReleaseDeferredScripts([Some(one), Some(two)]);
    let fetched = Arc::new(AtomicBool::new(false));
    let owner_fetched = Arc::clone(&fetched);
    let owner = std::thread::spawn(move || {
        prepare_now.recv().unwrap();
        let result = OwnerPreparation { ready, go: start }.wait(budget);
        if result.is_ok() {
            owner_fetched.store(true, Ordering::SeqCst);
        }
        assert!(commands.recv().is_err());
        match result {
            Err(reason) => OwnerExit::CancelledBeforeFetch(reason),
            Ok(()) => OwnerExit::Completed("unexpected fetch".into()),
        }
    });
    if ready_timeout {
        assert_eq!(
            await_preparation(&prepared, &go, &Diagnostic::default(), Duration::ZERO),
            Err(PreparationCancelled::ReadyTimeout)
        );
        prepare.send(()).unwrap();
    } else {
        prepare.send(()).unwrap();
        let deadline = prepared.recv_timeout(Duration::from_secs(5)).unwrap();
        if budget.is_zero() {
            assert!(std::time::Instant::now() >= deadline);
            let _ = go.send(()); // Deliberately late; owner must never enter fetch.
        }
    }
    let result = cancel_preparation(
        prepared,
        go,
        advance,
        &mut release,
        owner,
        &Diagnostic::default(),
    );
    let reason = match result.unwrap() {
        OwnerExit::CancelledBeforeFetch(reason) => reason,
        OwnerExit::Completed(_) => panic!("cancelled owner executed fetch"),
    };
    assert!(
        expected.contains(&reason),
        "unexpected cancellation: {reason:?}"
    );
    assert!(!fetched.load(Ordering::SeqCst));
    assert_eq!(first.recv(), Ok(()));
    assert_eq!(second.recv(), Ok(()));
}

#[test]
fn preparation_owner_panic_is_returned_after_releasing_gates() {
    let (ready, prepared) = mpsc::channel();
    let (go, _start) = mpsc::channel();
    let (advance, _commands) = mpsc::channel();
    let (one, first) = mpsc::channel();
    let (two, second) = mpsc::channel();
    let mut release = ReleaseDeferredScripts([Some(one), Some(two)]);
    let owner = std::thread::spawn(move || {
        drop(ready);
        panic!("owner preparation panic");
    });
    assert_eq!(
        await_preparation(
            &prepared,
            &go,
            &Diagnostic::default(),
            Duration::from_secs(5)
        ),
        Err(PreparationCancelled::ReadyDisconnected)
    );
    let result = cancel_preparation(
        prepared,
        go,
        advance,
        &mut release,
        owner,
        &Diagnostic::default(),
    );
    assert!(result.is_err());
    assert_eq!(first.recv(), Ok(()));
    assert_eq!(second.recv(), Ok(()));
}
