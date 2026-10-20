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
