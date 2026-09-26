//! Best-effort background persistence of sanitized events.

use std::{
    fmt,
    io::Write,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
};

use super::{EventStore, ReporterConfig, RetentionPolicy, SafeEvent, StoreError};

const DEFAULT_QUEUE_CAPACITY: usize = 256;

enum Message {
    Event(SafeEvent),
    Flush(mpsc::Sender<()>),
}

#[derive(Default)]
struct Counters {
    accepted: AtomicU64,
    dropped: AtomicU64,
    storage_failures: AtomicU64,
    warned_queue: AtomicBool,
    warned_storage: AtomicBool,
}

/// Counters for accepted, dropped, and failed event writes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ReporterStats {
    /// Events accepted into the bounded queue.
    pub accepted: u64,
    /// Events dropped because the queue was full or the worker stopped.
    pub dropped: u64,
    /// Events that the worker could not persist.
    pub storage_failures: u64,
}

/// Reporter setup failed before any event could be queued.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReporterError {
    /// Queue capacity must be positive.
    InvalidCapacity,
    /// The worker thread could not be started.
    WorkerStart,
    /// The worker stopped unexpectedly before a flush completed.
    WorkerStopped,
}

impl fmt::Display for ReporterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidCapacity => "invalid error-report queue capacity",
            Self::WorkerStart => "error-report worker could not start",
            Self::WorkerStopped => "error-report worker stopped unexpectedly",
        })
    }
}

impl std::error::Error for ReporterError {}

struct Worker {
    sender: SyncSender<Message>,
    handle: JoinHandle<()>,
}

/// Optional background reporter. `report` never performs database I/O or
/// changes the result of the browser operation that observed an error.
pub struct ErrorReporter {
    worker: Option<Worker>,
    counters: Arc<Counters>,
}

impl ErrorReporter {
    /// Starts a bounded worker only when local recording is enabled.
    ///
    /// The database is opened on the worker. Setup and write failures are
    /// counted and diagnosed once, without reporting themselves as events.
    pub fn new(
        config: &ReporterConfig,
        database_path: PathBuf,
        retention: RetentionPolicy,
    ) -> Result<Self, ReporterError> {
        if !config.records_locally() {
            return Ok(Self {
                worker: None,
                counters: Arc::default(),
            });
        }
        Self::with_sink(DEFAULT_QUEUE_CAPACITY, move || {
            let mut store = EventStore::open(database_path, retention)?;
            Ok(move |event: &SafeEvent| store.record(event))
        })
    }

    fn with_sink<S, F>(capacity: usize, create_sink: F) -> Result<Self, ReporterError>
    where
        S: FnMut(&SafeEvent) -> Result<(), StoreError> + Send + 'static,
        F: FnOnce() -> Result<S, StoreError> + Send + 'static,
    {
        if capacity == 0 {
            return Err(ReporterError::InvalidCapacity);
        }
        let (sender, receiver) = mpsc::sync_channel(capacity);
        let counters = Arc::new(Counters::default());
        let worker_counters = Arc::clone(&counters);
        let handle = thread::Builder::new()
            .name("omoikane-error-reporter".into())
            .spawn(move || {
                let mut sink = match create_sink() {
                    Ok(sink) => Some(sink),
                    Err(_) => {
                        warn_storage_once(&worker_counters);
                        None
                    }
                };
                while let Ok(message) = receiver.recv() {
                    match message {
                        Message::Event(event) => {
                            let stored = sink.as_mut().is_some_and(|sink| sink(&event).is_ok());
                            if !stored {
                                worker_counters
                                    .storage_failures
                                    .fetch_add(1, Ordering::Relaxed);
                                warn_storage_once(&worker_counters);
                            }
                        }
                        Message::Flush(reply) => {
                            let _ = reply.send(());
                        }
                    }
                }
            })
            .map_err(|_| ReporterError::WorkerStart)?;
        Ok(Self {
            worker: Some(Worker { sender, handle }),
            counters,
        })
    }

    /// Queues a sanitized event without waiting for SQLite or the worker.
    /// A full or stopped queue drops the event and emits one fixed diagnostic.
    pub fn report(&self, event: SafeEvent) {
        let Some(worker) = &self.worker else {
            return;
        };
        match worker.sender.try_send(Message::Event(event)) {
            Ok(()) => {
                self.counters.accepted.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                self.counters.dropped.fetch_add(1, Ordering::Relaxed);
                if !self.counters.warned_queue.swap(true, Ordering::Relaxed) {
                    write_diagnostic(
                        "Omoikane error reporter: queue unavailable; events may be dropped",
                    );
                }
            }
        }
    }

    /// Waits until all events queued before this call have been processed.
    /// Intended for orderly shutdown and explicit diagnostic workflows.
    pub fn flush(&self) -> Result<(), ReporterError> {
        let Some(worker) = &self.worker else {
            return Ok(());
        };
        let (reply, completed) = mpsc::channel();
        worker
            .sender
            .send(Message::Flush(reply))
            .map_err(|_| ReporterError::WorkerStopped)?;
        completed.recv().map_err(|_| ReporterError::WorkerStopped)
    }

    /// Returns local counters; no event details or credentials are exposed.
    pub fn stats(&self) -> ReporterStats {
        ReporterStats {
            accepted: self.counters.accepted.load(Ordering::Relaxed),
            dropped: self.counters.dropped.load(Ordering::Relaxed),
            storage_failures: self.counters.storage_failures.load(Ordering::Relaxed),
        }
    }

    /// Whether this reporter has a local recording worker.
    pub fn records_locally(&self) -> bool {
        self.worker.is_some()
    }
}

impl Drop for ErrorReporter {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            // Closing the channel drains queued events before the worker exits.
            drop(worker.sender);
            let _ = worker.handle.join();
        }
    }
}

fn warn_storage_once(counters: &Counters) {
    if !counters.warned_storage.swap(true, Ordering::Relaxed) {
        write_diagnostic("Omoikane error reporter: local storage unavailable");
    }
}

fn write_diagnostic(message: &str) {
    let _ = writeln!(std::io::stderr().lock(), "{message}");
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::{Arc, Barrier, atomic::AtomicUsize},
    };

    use super::*;
    use crate::error_reporting::{
        ErrorCategory, ErrorCode, ErrorSeverity, ExecutionSurface, RawEvent,
    };

    fn event() -> SafeEvent {
        RawEvent::new(
            ErrorCategory::Internal,
            ErrorSeverity::Error,
            ErrorCode::new("WORKER_TEST").unwrap(),
            ExecutionSurface::Background,
            "opaque details",
            &[],
        )
        .sanitize()
    }

    #[test]
    fn bounded_queue_drops_without_blocking_and_flushes_remaining_events() {
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let entered_worker = Arc::clone(&entered);
        let release_worker = Arc::clone(&release);
        let reporter = ErrorReporter::with_sink(1, move || {
            Ok(move |_: &SafeEvent| {
                entered_worker.wait();
                release_worker.wait();
                Ok(())
            })
        })
        .unwrap();
        reporter.report(event());
        entered.wait();
        reporter.report(event());
        reporter.report(event());
        assert_eq!(reporter.stats().accepted, 2);
        assert_eq!(reporter.stats().dropped, 1);
        release.wait();
        // The second event reaches the worker after release. Its two barriers
        // need another matching participant to complete the shutdown flush.
        entered.wait();
        release.wait();
        reporter.flush().unwrap();
        assert_eq!(reporter.stats().storage_failures, 0);
    }

    #[test]
    fn storage_failure_is_isolated_and_not_recursively_queued() {
        let reporter =
            ErrorReporter::with_sink(2, || Ok(|_: &SafeEvent| Err(StoreError::Database))).unwrap();
        reporter.report(event());
        reporter.flush().unwrap();
        assert_eq!(reporter.stats().storage_failures, 1);
        assert_eq!(reporter.stats().accepted, 1);
        assert!(reporter.counters.warned_storage.load(Ordering::Relaxed));
    }

    #[test]
    fn shutdown_drains_the_queue() {
        let written = Arc::new(AtomicUsize::new(0));
        let written_by_worker = Arc::clone(&written);
        let reporter = ErrorReporter::with_sink(2, move || {
            Ok(move |_: &SafeEvent| {
                written_by_worker.fetch_add(1, Ordering::Relaxed);
                Ok(())
            })
        })
        .unwrap();
        reporter.report(event());
        drop(reporter);
        assert_eq!(written.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn off_mode_creates_no_database_or_worker() {
        let path =
            std::env::temp_dir().join(format!("omoikane-reporter-off-{}", std::process::id()));
        let _ = fs::remove_file(&path);
        let config = ReporterConfig::from_values(None, None).unwrap();
        let reporter =
            ErrorReporter::new(&config, path.clone(), RetentionPolicy::default()).unwrap();
        reporter.report(event());
        reporter.flush().unwrap();
        assert_eq!(reporter.stats(), ReporterStats::default());
        assert!(!path.exists());
    }

    #[test]
    fn corrupt_database_does_not_change_the_callers_result() {
        let path =
            std::env::temp_dir().join(format!("omoikane-reporter-corrupt-{}", std::process::id()));
        fs::write(&path, b"not a sqlite database").unwrap();
        let config = ReporterConfig::from_values(Some("record-only"), None).unwrap();
        let reporter =
            ErrorReporter::new(&config, path.clone(), RetentionPolicy::default()).unwrap();
        let browser_result: Result<u32, &'static str> = Ok(42);
        reporter.report(event());
        reporter.flush().unwrap();
        assert_eq!(browser_result, Ok(42));
        assert_eq!(reporter.stats().storage_failures, 1);
        drop(reporter);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn locked_database_does_not_change_the_callers_result() {
        let path =
            std::env::temp_dir().join(format!("omoikane-reporter-locked-{}", std::process::id()));
        let _ = fs::remove_file(&path);
        drop(EventStore::open(&path, RetentionPolicy::default()).unwrap());
        let blocker = rusqlite::Connection::open(&path).unwrap();
        blocker
            .pragma_update(None, "journal_mode", "DELETE")
            .unwrap();
        blocker.execute_batch("BEGIN EXCLUSIVE").unwrap();

        let config = ReporterConfig::from_values(Some("record-only"), None).unwrap();
        let reporter =
            ErrorReporter::new(&config, path.clone(), RetentionPolicy::default()).unwrap();
        let browser_result: Result<u32, &'static str> = Ok(42);
        reporter.report(event());
        reporter.flush().unwrap();
        assert_eq!(browser_result, Ok(42));
        assert_eq!(reporter.stats().storage_failures, 1);
        drop(reporter);

        blocker.execute_batch("ROLLBACK").unwrap();
        drop(blocker);
        fs::remove_file(path).unwrap();
    }
}
