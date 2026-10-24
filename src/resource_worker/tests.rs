use super::*;
use std::time::Duration;
const WAIT: Duration = Duration::from_secs(5);

#[test]
fn shared_queue_bounds_concurrency_across_resource_types() {
    let queue = ResourceQueue::new(NonZeroUsize::new(2).unwrap());
    let scope = ResourceScope::default();
    let (started, observed) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let gate = Arc::new(Mutex::new(gate));
    let mut tasks = Vec::new();
    for kind in [
        ResourceKind::Image,
        ResourceKind::Font,
        ResourceKind::Stylesheet,
    ] {
        let started = started.clone();
        let gate = gate.clone();
        tasks.push(
            queue
                .clone()
                .submit(&scope, ResourcePriority::Normal, kind, move |_| {
                    started.send(kind).unwrap();
                    gate.lock().unwrap().recv_timeout(WAIT).unwrap();
                    kind
                })
                .unwrap(),
        );
    }
    observed.recv_timeout(WAIT).unwrap();
    observed.recv_timeout(WAIT).unwrap();
    assert_eq!(observed.try_recv(), Err(mpsc::TryRecvError::Empty));
    release.send(()).unwrap();
    observed.recv_timeout(WAIT).unwrap();
    release.send(()).unwrap();
    release.send(()).unwrap();
    for task in tasks {
        assert!(matches!(task.recv().unwrap(), ResourceOutcome::Complete(_)));
    }
}

#[test]
fn queued_priority_is_stable_and_cancelled_jobs_do_not_execute() {
    let queue = ResourceQueue::new(NonZeroUsize::new(1).unwrap());
    let scope = ResourceScope::default();
    let cancelled = ResourceScope::default();
    let (ready, started) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let first = queue
        .submit(
            &scope,
            ResourcePriority::Normal,
            ResourceKind::Other,
            move |_| {
                ready.send(()).unwrap();
                gate.recv_timeout(WAIT).unwrap();
            },
        )
        .unwrap();
    started.recv_timeout(WAIT).unwrap();
    let order = Arc::new(Mutex::new(Vec::new()));
    let mut tasks = Vec::new();
    for (id, priority) in [
        (1, ResourcePriority::Background),
        (2, ResourcePriority::Critical),
        (3, ResourcePriority::Critical),
    ] {
        let order = order.clone();
        tasks.push(
            queue
                .submit(&scope, priority, ResourceKind::Other, move |_| {
                    order.lock().unwrap().push(id)
                })
                .unwrap(),
        );
    }
    let skipped = queue
        .submit(
            &cancelled,
            ResourcePriority::Critical,
            ResourceKind::Other,
            |_| panic!("cancelled work executed"),
        )
        .unwrap();
    drop(cancelled);
    release.send(()).unwrap();
    first.recv().unwrap();
    for task in tasks {
        assert!(matches!(
            task.recv().unwrap(),
            ResourceOutcome::Complete(())
        ));
    }
    assert!(matches!(
        skipped.recv().unwrap(),
        ResourceOutcome::Cancelled
    ));
    assert_eq!(*order.lock().unwrap(), [2, 3, 1]);
}

#[test]
fn job_panic_does_not_destroy_the_worker() {
    let queue = ResourceQueue::new(NonZeroUsize::new(1).unwrap());
    let scope = ResourceScope::default();
    let broken = queue
        .submit(
            &scope,
            ResourcePriority::Normal,
            ResourceKind::Other,
            |_| panic!("job panic"),
        )
        .unwrap();
    let next = queue
        .submit(
            &scope,
            ResourcePriority::Normal,
            ResourceKind::Other,
            |_| 42,
        )
        .unwrap();
    assert!(matches!(broken.recv().unwrap(), ResourceOutcome::Panicked));
    assert!(matches!(
        next.recv().unwrap(),
        ResourceOutcome::Complete(42)
    ));
}

#[test]
fn last_queue_drop_cancels_queued_work_without_waiting_for_io() {
    let queue = ResourceQueue::new(NonZeroUsize::new(1).unwrap());
    let scope = ResourceScope::default();
    let (ready, started) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let running = queue
        .submit(
            &scope,
            ResourcePriority::Normal,
            ResourceKind::Other,
            move |_| {
                ready.send(()).unwrap();
                gate.recv_timeout(WAIT).unwrap();
            },
        )
        .unwrap();
    started.recv_timeout(WAIT).unwrap();
    let queued = queue
        .submit(
            &scope,
            ResourcePriority::Normal,
            ResourceKind::Other,
            |_| panic!("shutdown job executed"),
        )
        .unwrap();
    drop(queue);
    assert!(matches!(queued.recv().unwrap(), ResourceOutcome::Cancelled));
    release.send(()).unwrap();
    assert!(matches!(
        running.recv().unwrap(),
        ResourceOutcome::Complete(())
    ));
}
struct FencedCompletionWake {
    arrived: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
}

impl std::task::Wake for FencedCompletionWake {
    fn wake(self: Arc<Self>) {
        self.arrived.send(()).unwrap();
        self.release.lock().unwrap().recv_timeout(WAIT).unwrap();
    }
}

#[test]
fn every_completion_outcome_is_visible_before_the_owner_is_woken() {
    let queue = ResourceQueue::new(NonZeroUsize::new(1).unwrap());
    for case in 0..3 {
        let scope = ResourceScope::default();
        if case == 1 {
            scope.cancel();
        }
        let (arrived, notified) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let waker = std::task::Waker::from(Arc::new(FencedCompletionWake {
            arrived,
            release: Mutex::new(gate),
        }));
        let task = queue
            .submit_with_waker(
                &scope,
                ResourcePriority::Normal,
                ResourceKind::Font,
                waker,
                move |_| {
                    assert_ne!(case, 1, "cancelled work executed");
                    assert_ne!(case, 2, "injected work panic");
                    7
                },
            )
            .unwrap();
        notified.recv_timeout(WAIT).unwrap();
        // The worker cannot advance beyond wake until the owner releases it.
        // Waking before committing the outcome fails this nonblocking receive.
        let outcome = task.try_recv().unwrap();
        match case {
            0 => assert!(matches!(outcome, ResourceOutcome::Complete(7))),
            1 => assert!(matches!(outcome, ResourceOutcome::Cancelled)),
            _ => assert!(matches!(outcome, ResourceOutcome::Panicked)),
        }
        assert_eq!(notified.try_recv(), Err(mpsc::TryRecvError::Empty));
        release.send(()).unwrap();
    }
}

struct CompletionSignal(mpsc::Sender<()>);
impl std::task::Wake for CompletionSignal {
    fn wake(self: Arc<Self>) {
        let _ = self.0.send(());
    }
}

#[test]
fn dropping_the_queue_notifies_cancelled_queued_work() {
    let queue = ResourceQueue::new(NonZeroUsize::new(1).unwrap());
    let scope = ResourceScope::default();
    let (ready, started) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let running = queue
        .submit(
            &scope,
            ResourcePriority::Normal,
            ResourceKind::Other,
            move |_| {
                ready.send(()).unwrap();
                gate.recv_timeout(WAIT).unwrap();
            },
        )
        .unwrap();
    started.recv_timeout(WAIT).unwrap();
    let (signal, notified) = mpsc::channel();
    let pending = queue
        .submit_with_waker(
            &scope,
            ResourcePriority::Normal,
            ResourceKind::Font,
            std::task::Waker::from(Arc::new(CompletionSignal(signal))),
            |_| -> () { panic!("queued work ran during shutdown") },
        )
        .unwrap();
    drop(queue);
    notified.recv_timeout(WAIT).unwrap();
    assert!(matches!(
        pending.try_recv().unwrap(),
        ResourceOutcome::Cancelled
    ));
    release.send(()).unwrap();
    assert!(matches!(
        running.receiver.recv_timeout(WAIT).unwrap(),
        ResourceOutcome::Complete(())
    ));
}

struct PanickingCompletionWake;
impl std::task::Wake for PanickingCompletionWake {
    fn wake(self: Arc<Self>) {
        panic!("injected notification panic")
    }
}

#[test]
fn a_notification_panic_does_not_destroy_the_worker() {
    let queue = ResourceQueue::new(NonZeroUsize::new(1).unwrap());
    let scope = ResourceScope::default();
    let first = queue
        .submit_with_waker(
            &scope,
            ResourcePriority::Normal,
            ResourceKind::Font,
            std::task::Waker::from(Arc::new(PanickingCompletionWake)),
            |_| 7,
        )
        .unwrap();
    assert!(matches!(
        first.receiver.recv_timeout(WAIT).unwrap(),
        ResourceOutcome::Complete(7)
    ));
    let next = queue
        .submit(&scope, ResourcePriority::Normal, ResourceKind::Other, |_| 9)
        .unwrap();
    assert!(matches!(
        next.receiver.recv_timeout(WAIT).unwrap(),
        ResourceOutcome::Complete(9)
    ));
}
