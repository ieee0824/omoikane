//! Test-only preparation protocol and opt-in wall-time observations.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PreparationCancelled {
    ReadyDisconnected,
    ReadyTimeout,
    GoDisconnected,
    GoTimeout,
}

/// Owns the preparation handshake; no runtime state crosses the thread boundary.
pub(super) struct OwnerPreparation {
    pub ready: Sender<Instant>,
    pub go: Receiver<()>,
}

impl OwnerPreparation {
    pub(super) fn wait(self, budget: Duration) -> Result<(), PreparationCancelled> {
        let deadline = Instant::now() + budget;
        self.ready
            .send(deadline)
            .map_err(|_| PreparationCancelled::ReadyDisconnected)?;
        receive_go_until(&self.go, deadline)
    }
}

fn receive_go_until(go: &Receiver<()>, deadline: Instant) -> Result<(), PreparationCancelled> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or(PreparationCancelled::GoTimeout)?;
    check_go_result(go.recv_timeout(remaining), Instant::now(), deadline)
}

fn check_go_result(
    received: Result<(), RecvTimeoutError>,
    now: Instant,
    deadline: Instant,
) -> Result<(), PreparationCancelled> {
    match received {
        Ok(()) if now < deadline => Ok(()),
        Ok(()) | Err(RecvTimeoutError::Timeout) => Err(PreparationCancelled::GoTimeout),
        Err(RecvTimeoutError::Disconnected) => Err(PreparationCancelled::GoDisconnected),
    }
}

#[test]
fn expired_go_is_rejected_even_when_already_queued() {
    let (send, receive) = std::sync::mpsc::channel();
    send.send(()).unwrap();
    assert_eq!(
        receive_go_until(&receive, Instant::now()),
        Err(PreparationCancelled::GoTimeout)
    );
}

#[test]
fn disconnected_go_is_distinct_from_timeout() {
    let (send, receive) = std::sync::mpsc::channel();
    drop(send);
    assert_eq!(
        receive_go_until(&receive, Instant::now() + Duration::from_secs(5)),
        Err(PreparationCancelled::GoDisconnected)
    );
}

#[test]
fn unavailable_ready_receiver_cancels_before_go() {
    let (ready, observed) = std::sync::mpsc::channel();
    let (_send, go) = std::sync::mpsc::channel();
    drop(observed);
    assert_eq!(
        OwnerPreparation { ready, go }.wait(Duration::from_secs(5)),
        Err(PreparationCancelled::ReadyDisconnected)
    );
}

const PHASES: [&str; 27] = [
    "ready_wait_begin",
    "ready_wait_end",
    "go_sent",
    "go_received",
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
    "transport_one_enter",
    "transport_two_enter",
    "receive_one_begin",
    "receive_one_end",
    "receive_two_begin",
    "receive_two_end",
    "release_second",
    "release_first",
    "join_begin",
    "join_end",
];

#[derive(Clone, Default)]
pub(super) struct Diagnostic(Option<Arc<State>>);

struct State {
    started: Instant,
    slots: [AtomicU64; PHASES.len()],
    duplicate: AtomicBool,
    overflow: AtomicBool,
    preparation_main: AtomicU8,
    preparation_owner: AtomicU8,
}

impl Diagnostic {
    pub(super) fn from_environment() -> Self {
        Self::new(
            std::env::var_os("OMOIKANE_GATE_DIAGNOSTICS").as_deref()
                == Some(std::ffi::OsStr::new("1")),
        )
    }

    fn new(enabled: bool) -> Self {
        Self(enabled.then(|| {
            Arc::new(State {
                started: Instant::now(),
                slots: std::array::from_fn(|_| AtomicU64::new(0)),
                duplicate: AtomicBool::new(false),
                overflow: AtomicBool::new(false),
                preparation_main: AtomicU8::new(0),
                preparation_owner: AtomicU8::new(0),
            })
        }))
    }

    pub(super) fn preparation_result(&self, owner: bool, result: Result<(), PreparationCancelled>) {
        let Some(state) = &self.0 else { return };
        let code = match result {
            Ok(()) => 1,
            Err(PreparationCancelled::ReadyDisconnected) => 2,
            Err(PreparationCancelled::ReadyTimeout) => 3,
            Err(PreparationCancelled::GoDisconnected) => 4,
            Err(PreparationCancelled::GoTimeout) => 5,
        };
        let slot = if owner {
            &state.preparation_owner
        } else {
            &state.preparation_main
        };
        slot.store(code, Ordering::Relaxed);
    }

    pub(super) fn stamp(&self, phase: &'static str) {
        let Some(state) = &self.0 else { return };
        let index = PHASES.iter().position(|name| *name == phase).unwrap();
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
        let phases: serde_json::Map<String, serde_json::Value> = PHASES
            .iter()
            .zip(&state.slots)
            .map(|(name, slot)| {
                let time = slot.load(Ordering::Relaxed).checked_sub(1);
                ((*name).to_owned(), serde_json::json!(time))
            })
            .collect();
        Some(serde_json::json!({
            "phases_ns": phases,
            "preparation_main": preparation_record(state.preparation_main.load(Ordering::Relaxed)),
            "preparation_owner": preparation_record(state.preparation_owner.load(Ordering::Relaxed)),
            "duplicate": state.duplicate.load(Ordering::Relaxed),
            "overflow": state.overflow.load(Ordering::Relaxed),
            "scope": "inclusive wall phases; parser drive to transport includes queue and HTTP; not CPU or inner JS deadline",
        }))
    }

    fn report(
        &self,
        received: &[Option<Result<usize, RecvTimeoutError>>; 2],
        owner_panicked: bool,
        waiter_panicked: Option<bool>,
    ) -> Option<serde_json::Value> {
        let mut report = self.snapshot()?;
        report["receive"] = serde_json::json!(received.each_ref().map(receive_record));
        report["owner_panicked"] = serde_json::json!(owner_panicked);
        report["waiter_panicked"] = serde_json::json!(waiter_panicked);
        report["case"] = serde_json::json!(std::thread::current().name());
        report["schema"] = serde_json::json!("cdp-parallel-startup-v2");
        Some(report)
    }

    // A partial owner-panic report must not require waiting for the barrier thread.
    pub(super) fn emit(
        &self,
        received: &[Option<Result<usize, RecvTimeoutError>>; 2],
        owner_panicked: bool,
        waiter_panicked: Option<bool>,
    ) {
        if let Some(report) = self.report(received, owner_panicked, waiter_panicked) {
            eprintln!("CDP_PARALLEL_STARTUP_DIAGNOSTIC {report}");
        }
    }
}

fn receive_record(result: &Option<Result<usize, RecvTimeoutError>>) -> serde_json::Value {
    match result {
        Some(Ok(index)) => serde_json::json!({"outcome": "ok", "index": index}),
        Some(Err(RecvTimeoutError::Timeout)) => serde_json::json!({"outcome": "timeout"}),
        Some(Err(RecvTimeoutError::Disconnected)) => serde_json::json!({"outcome": "disconnected"}),
        None => serde_json::json!({"outcome": "unobserved"}),
    }
}

#[test]
fn first_timestamp_and_missing_transport_are_preserved() {
    let diagnostic = Diagnostic::new(true);
    diagnostic.stamp("owner_entry");
    let first = diagnostic.snapshot().unwrap()["phases_ns"]["owner_entry"].clone();
    diagnostic.stamp("owner_entry");
    let report = diagnostic.snapshot().unwrap();
    assert_eq!(report["phases_ns"]["owner_entry"], first);
    assert!(first.is_u64());
    assert!(report["phases_ns"]["transport_one_enter"].is_null());
    assert!(report["phases_ns"]["transport_two_enter"].is_null());
    assert_eq!(report["duplicate"], true);
    assert_eq!(report["overflow"], false);
}

#[test]
fn disabled_diagnostic_has_no_clock_or_record() {
    let diagnostic = Diagnostic::new(false);
    diagnostic.stamp("owner_entry");
    assert!(diagnostic.0.is_none());
    assert!(diagnostic.snapshot().is_none());
}

#[test]
fn owner_panic_does_not_claim_a_waiter_outcome_or_completed_joins() {
    let diagnostic = Diagnostic::new(true);
    diagnostic.stamp("join_begin");
    let report = diagnostic.report(&[None, None], true, None).unwrap();
    assert_eq!(report["owner_panicked"], true);
    assert!(report["waiter_panicked"].is_null());
    assert!(report["phases_ns"]["join_end"].is_null());
    assert_eq!(report["receive"][0]["outcome"], "unobserved");
}

#[test]
fn receive_records_distinguish_missing_notifications_from_source_indices() {
    assert_eq!(
        receive_record(&Some(Ok(0))),
        serde_json::json!({"outcome":"ok","index":0})
    );
    assert_eq!(
        receive_record(&Some(Ok(1))),
        serde_json::json!({"outcome":"ok","index":1})
    );
    assert_eq!(
        receive_record(&Some(Err(RecvTimeoutError::Timeout))),
        serde_json::json!({"outcome":"timeout"})
    );
    assert_eq!(
        receive_record(&Some(Err(RecvTimeoutError::Disconnected))),
        serde_json::json!({"outcome":"disconnected"})
    );
    assert_eq!(
        receive_record(&None),
        serde_json::json!({"outcome":"unobserved"})
    );
}

fn preparation_record(code: u8) -> &'static str {
    match code {
        0 => "unobserved",
        1 => "ok",
        2 => "ready_disconnected",
        3 => "ready_timeout",
        4 => "go_disconnected",
        5 => "go_timeout",
        _ => unreachable!("preparation code is private"),
    }
}

#[test]
fn queued_go_before_deadline_is_accepted() {
    let (send, receive) = std::sync::mpsc::channel();
    send.send(()).unwrap();
    assert_eq!(
        receive_go_until(&receive, Instant::now() + Duration::from_secs(5)),
        Ok(())
    );
}

#[test]
fn successful_receive_at_or_after_deadline_is_rejected() {
    let deadline = Instant::now();
    for now in [deadline, deadline + Duration::from_nanos(1)] {
        assert_eq!(
            check_go_result(Ok(()), now, deadline),
            Err(PreparationCancelled::GoTimeout)
        );
    }
    assert_eq!(
        check_go_result(Err(RecvTimeoutError::Timeout), deadline, deadline),
        Err(PreparationCancelled::GoTimeout)
    );
}

#[test]
fn preparation_records_keep_both_sides_and_failure_reasons() {
    let diagnostic = Diagnostic::new(true);
    assert_eq!(
        diagnostic.snapshot().unwrap()["preparation_owner"],
        "unobserved"
    );
    diagnostic.preparation_result(false, Err(PreparationCancelled::ReadyTimeout));
    diagnostic.preparation_result(true, Err(PreparationCancelled::GoDisconnected));
    let record = diagnostic.snapshot().unwrap();
    assert_eq!(record["preparation_main"], "ready_timeout");
    assert_eq!(record["preparation_owner"], "go_disconnected");
}
