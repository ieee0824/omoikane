//! Rejected endpoints across CDP navigation phases, without retaining old hosts.
use super::*;
use crate::js::worker_startup_tests::{
    FailedStartupProbe, FailedStartupSnapshot, denied_data_worker_probe, failed_startup_count,
};
use crate::test_support::http_fixture::{
    ACCEPT_TIMEOUT, FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback,
    read_request_headers,
};
use std::io::Write;

fn session_with_pending_failure() -> (CdpSession, FailedStartupProbe) {
    let mut session = CdpSession::new().unwrap();
    let listener = bind_loopback().unwrap();
    let address = listener.local_addr().unwrap();
    let server = FixtureWorker::spawn(move || {
        let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
        let request = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
        assert!(request.starts_with("GET /worker-owner HTTP/1.1\r\n"));
        let body = "<html><body><main id=old>old</main></body></html>";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.set_write_timeout(Some(READ_TIMEOUT)).unwrap();
        stream.write_all(response.as_bytes()).unwrap();
    });
    let url = format!("http://127.0.0.1:{}/worker-owner", address.port());
    session
        .dispatch("Page.navigate", json!({"url":url}))
        .unwrap();
    server.join();
    assert_eq!(session.current_url(), url);
    assert_eq!(session.history_entries[session.history_index].url, url);
    let probe = denied_data_worker_probe(&mut session.runtime);
    assert_pending(&session, &probe);
    (session, probe)
}

fn assert_pending(session: &CdpSession, probe: &FailedStartupProbe) {
    assert_eq!(session.runtime.document().identity(), probe.document_id());
    assert_eq!(
        probe.snapshot(),
        FailedStartupSnapshot::Alive {
            marker_document_id: Some(probe.document_id()),
            runtime_present: false,
        }
    );
}

#[test]
fn uncommitted_navigation_keeps_the_failed_endpoint_until_background_collection() {
    let (mut session, probe) = session_with_pending_failure();
    let generation = session.document_generation;
    let fragment = format!("{}#same-document", session.current_url);
    // The prepared fragment path commits only the same-Document URL/jobs;
    // ordinary dispatch additionally drives background tasks below.
    assert!(matches!(
        session
            .prepare_page_navigate(&json!({"url":fragment}))
            .unwrap(),
        PreparedPageNavigation::Complete(_)
    ));
    assert_pending(&session, &probe);
    assert_eq!(session.document_generation, generation);

    assert!(
        session
            .dispatch("Page.navigate", json!({"url":"http://["}))
            .is_err()
    );
    assert_pending(&session, &probe);
    assert_eq!(session.document_generation, generation);

    let prepared = session
        .prepare_page_navigate(&json!({"url":"data:text/html,<main>not committed</main>"}))
        .unwrap();
    assert!(matches!(&prepared, PreparedPageNavigation::Pending { .. }));
    assert_pending(&session, &probe);
    drop(prepared);
    // Dropping a prepared request is not a committed navigation or a disconnect.
    assert_pending(&session, &probe);
    assert_eq!(session.document_generation, generation);

    let pumped_fragment = format!("{}-pump", session.current_url);
    session
        .dispatch("Page.navigate", json!({"url":pumped_fragment}))
        .unwrap();
    // Ordinary same-Document dispatch drives run_until_idle: collection here
    // is a worker background phase, not Document retirement.
    assert_eq!(session.runtime.document().identity(), probe.document_id());
    assert_eq!(session.document_generation, generation);
    assert_eq!(
        probe.snapshot(),
        FailedStartupSnapshot::Alive {
            marker_document_id: None,
            runtime_present: false,
        }
    );
}

#[test]
fn committed_navigation_retires_an_existing_failed_endpoint_across_replacement() {
    let (mut session, probe) = session_with_pending_failure();
    let generation = session.document_generation;
    session
        .dispatch(
            "Page.navigate",
            json!({"url":"data:text/html,<script>globalThis.replacementErrors=0;addEventListener('error',()=>replacementErrors++)</script><main>new</main>"}),
        )
        .unwrap();
    // Dispatch has already pumped the replacement; this is a post-call observation.
    assert_ne!(session.runtime.document().identity(), probe.document_id());
    assert_eq!(session.document_generation, generation + 1);
    match probe.snapshot() {
        // Host destruction proves retirement, not a separately observed clear.
        FailedStartupSnapshot::HostDropped => {}
        FailedStartupSnapshot::Alive {
            marker_document_id,
            runtime_present,
        } => {
            assert_eq!(marker_document_id, None);
            assert!(!runtime_present);
        }
    }
    assert_eq!(failed_startup_count(&session.runtime), 0);
    session.runtime.run_until_idle().unwrap();
    assert_eq!(
        session
            .runtime
            .eval("replacementErrors")
            .unwrap()
            .as_number(),
        Some(0.0)
    );
    assert!(session.runtime.take_task_errors().is_empty());
}
