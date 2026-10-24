//! Parser-deferred fetches share workers but retain execution order.
use super::CdpSession;
use crate::http::{HttpParseError, HttpRequest, HttpResponse, HttpTransport};
use crate::resource_worker::{
    ResourceKind, ResourceOutcome, ResourcePriority, ResourceQueue, ResourceScope,
};
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

struct HeldDeferredScripts {
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
fn deferred_startup_owner(
    queue: ResourceQueue,
    transport: Arc<HeldDeferredScripts>,
    commands: mpsc::Receiver<()>,
    progress: mpsc::Sender<(bool, String)>,
) -> String {
    let mut session = CdpSession::new().unwrap();
    session.set_resource_queue(queue);
    session.set_deferred_document_startup(true);
    session.install_document("https://example.test/",
        "<head><script defer src='/one.js'></script><script defer src='/two.js'></script></head>\
         <body><script>startupOrder.push('inline')</script></body>", 1, "null").unwrap();
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
    session.drive_event_loop(0).unwrap();
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
    order(&mut session)
}

#[test]
fn deferred_parser_fetches_overlap_and_execution_waits_for_document_order() {
    let queue = ResourceQueue::new(NonZeroUsize::new(2).unwrap());
    let (entered, observed) = mpsc::channel();
    let (finished, completed) = mpsc::channel();
    let (first, first_gate) = mpsc::channel();
    let (second, second_gate) = mpsc::channel();
    let mut release = ReleaseDeferredScripts([Some(first), Some(second)]);
    let transport = Arc::new(HeldDeferredScripts {
        entered,
        finished,
        gates: [Mutex::new(first_gate), Mutex::new(second_gate)],
    });
    let (advance, commands) = mpsc::channel();
    let (progress, returned) = mpsc::channel();
    let owner_queue = queue.clone();
    let owner = std::thread::spawn(move || {
        deferred_startup_owner(owner_queue, transport, commands, progress)
    });
    let mut requests: Vec<_> = (0..2)
        .filter_map(|_| observed.recv_timeout(Duration::from_secs(5)).ok())
        .collect();
    requests.sort();
    let initial = returned.recv_timeout(Duration::from_secs(5));
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
    release.release(0);
    let _ = advance.send(());
    let final_order = owner.join().unwrap();
    waiter.join().unwrap();
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
