//! Shared workers for owned resource I/O and decoding jobs.
//!
//! Clone a queue to share its concurrency limit across resource types and
//! documents. A document owns a scope; dropping it cancels queued work and lets
//! running jobs observe cancellation between blocking operations. Workers never
//! receive DOM handles or JavaScript values. Applying results belongs to the
//! caller, which can poll completion without blocking its event loop.

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::io;
use std::num::NonZeroUsize;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, Condvar, Mutex, Weak, mpsc};

#[cfg(test)]
mod tests;

/// Scheduling priority; jobs at the same priority retain submission order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResourcePriority {
    /// Speculative work which does not block currently visible content.
    Background,
    /// Visible images and other ordinary resources.
    Normal,
    /// Resources needed to lay out or display the current document.
    Critical,
}

/// Resource category retained on completion handles for diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    /// An author stylesheet or imported stylesheet.
    Stylesheet,
    /// A font resource and its decoding work.
    Font,
    /// An image resource and its decoding work.
    Image,
    /// Script fetching; script execution stays with the owning runtime.
    Script,
    /// Other owned resource work.
    Other,
}

/// A document's cancellation lifetime, independent of worker lifetimes.
#[derive(Debug)]
pub struct ResourceScope {
    live: Arc<AtomicBool>,
}

impl Default for ResourceScope {
    fn default() -> Self {
        Self {
            live: Arc::new(AtomicBool::new(true)),
        }
    }
}

impl ResourceScope {
    pub(crate) fn weak_liveness(&self) -> Weak<AtomicBool> {
        Arc::downgrade(&self.live)
    }

    /// Cancels queued jobs and requests cooperative cancellation of running jobs.
    pub fn cancel(&self) {
        self.live.store(false, AtomicOrdering::Release);
    }

    /// Produces a token which does not keep this scope alive.
    pub fn token(&self) -> CancellationToken {
        CancellationToken {
            live: Arc::downgrade(&self.live),
        }
    }
}

impl Drop for ResourceScope {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// A worker can check this token before I/O, redirects and expensive decoding.
#[derive(Debug, Clone)]
pub struct CancellationToken {
    live: Weak<AtomicBool>,
}

impl CancellationToken {
    /// Reports cancellation, including destruction of the owning document scope.
    pub fn is_cancelled(&self) -> bool {
        self.live
            .upgrade()
            .is_none_or(|live| !live.load(AtomicOrdering::Acquire))
    }
}

/// Terminal outcome of one submitted job; application errors remain inside `T`.
#[derive(Debug)]
pub enum ResourceOutcome<T> {
    /// Work completed while its document scope remained live.
    Complete(T),
    /// Work was cancelled before execution or before delivering its result.
    Cancelled,
    /// The job panicked; the worker remains available for subsequent jobs.
    Panicked,
}

/// Typed completion channel; polling never executes work on the calling thread.
#[derive(Debug)]
pub struct ResourceTask<T> {
    id: u64,
    kind: ResourceKind,
    receiver: mpsc::Receiver<ResourceOutcome<T>>,
}

impl<T> ResourceTask<T> {
    /// Returns the submission identity within its queue.
    pub fn id(&self) -> u64 {
        self.id
    }
    /// Returns the submitted resource category.
    pub fn kind(&self) -> ResourceKind {
        self.kind
    }
    /// Polls completion without blocking the owner thread.
    pub fn try_recv(&self) -> Result<ResourceOutcome<T>, mpsc::TryRecvError> {
        self.receiver.try_recv()
    }
    /// Waits for completion, for noninteractive callers and synchronization tests.
    pub fn recv(&self) -> Result<ResourceOutcome<T>, mpsc::RecvError> {
        self.receiver.recv()
    }
}

type Work = Box<dyn FnOnce(bool) + Send>;
struct Queued {
    priority: ResourcePriority,
    id: u64,
    work: Work,
}
impl PartialEq for Queued {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl Eq for Queued {}
impl PartialOrd for Queued {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Queued {
    fn cmp(&self, other: &Self) -> Ordering {
        self.priority
            .cmp(&other.priority)
            .then_with(|| other.id.cmp(&self.id))
    }
}

#[derive(Default)]
struct State {
    jobs: BinaryHeap<Queued>,
    next_id: u64,
    started: bool,
    stopped: bool,
}
struct Shared {
    state: Mutex<State>,
    ready: Condvar,
}
struct Pool {
    shared: Arc<Shared>,
    workers: NonZeroUsize,
}

impl Drop for Pool {
    fn drop(&mut self) {
        let jobs = {
            let mut state = self.shared.state.lock().unwrap();
            state.stopped = true;
            std::mem::take(&mut state.jobs)
        };
        self.shared.ready.notify_all();
        for job in jobs {
            (job.work)(false);
        }
        // Running blocking I/O is not joined here. Resource scopes provide
        // cooperative cancellation; transports retain their own timeouts.
    }
}

/// Lazily started worker pool shared by cloning this handle.
#[derive(Clone)]
pub struct ResourceQueue {
    pool: Arc<Pool>,
}

impl std::fmt::Debug for ResourceQueue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResourceQueue")
            .field("workers", &self.pool.workers)
            .finish_non_exhaustive()
    }
}

impl Default for ResourceQueue {
    fn default() -> Self {
        Self::new(NonZeroUsize::new(4).unwrap())
    }
}

impl ResourceQueue {
    /// Returns the maximum number of jobs executing concurrently in this queue.
    pub fn worker_count(&self) -> NonZeroUsize {
        self.pool.workers
    }

    /// Configures a concurrency limit without starting threads or performing I/O.
    pub fn new(workers: NonZeroUsize) -> Self {
        Self {
            pool: Arc::new(Pool {
                shared: Arc::new(Shared {
                    state: Mutex::new(State::default()),
                    ready: Condvar::new(),
                }),
                workers,
            }),
        }
    }

    /// Queues owned work and returns its typed completion handle.
    ///
    /// Thread creation failures are returned to the caller. The queue itself is
    /// unbounded; callers retain responsibility for limiting outstanding resource
    /// requests. Only `workers` jobs execute concurrently across all clones.
    pub fn submit<T: Send + 'static>(
        &self,
        scope: &ResourceScope,
        priority: ResourcePriority,
        kind: ResourceKind,
        work: impl FnOnce(CancellationToken) -> T + Send + 'static,
    ) -> io::Result<ResourceTask<T>> {
        self.submit_inner(scope, priority, kind, None, work)
    }

    /// Queues owned work and wakes the owner after its outcome is committed.
    ///
    /// Completion, cancellation and a caught work panic all notify the owner.
    /// A failed submission does not notify. Panics in the waker are isolated
    /// from the worker, just like work panics.
    pub fn submit_with_waker<T: Send + 'static>(
        &self,
        scope: &ResourceScope,
        priority: ResourcePriority,
        kind: ResourceKind,
        waker: std::task::Waker,
        work: impl FnOnce(CancellationToken) -> T + Send + 'static,
    ) -> io::Result<ResourceTask<T>> {
        self.submit_inner(scope, priority, kind, Some(waker), work)
    }

    fn submit_inner<T: Send + 'static>(
        &self,
        scope: &ResourceScope,
        priority: ResourcePriority,
        kind: ResourceKind,
        waker: Option<std::task::Waker>,
        work: impl FnOnce(CancellationToken) -> T + Send + 'static,
    ) -> io::Result<ResourceTask<T>> {
        let mut state = self.pool.shared.state.lock().unwrap();
        if state.stopped {
            return Err(io::Error::other("resource queue is stopped"));
        }
        if !state.started {
            state.started = true;
            for index in 0..self.pool.workers.get() {
                let shared = Arc::clone(&self.pool.shared);
                if let Err(error) = std::thread::Builder::new()
                    .name(format!("omoikane-resource-{index}"))
                    .spawn(move || worker(shared))
                {
                    state.stopped = true;
                    self.pool.shared.ready.notify_all();
                    return Err(error);
                }
            }
        }
        let id = state.next_id;
        state.next_id = id
            .checked_add(1)
            .ok_or_else(|| io::Error::other("resource job identity exhausted"))?;
        let token = scope.token();
        let (sender, receiver) = mpsc::channel();
        state.jobs.push(Queued {
            id,
            priority,
            work: Box::new(move |run| {
                let outcome = if !run || token.is_cancelled() {
                    ResourceOutcome::Cancelled
                } else {
                    match catch_unwind(AssertUnwindSafe(|| work(token.clone()))) {
                        Ok(value) if !token.is_cancelled() => ResourceOutcome::Complete(value),
                        Ok(_) => ResourceOutcome::Cancelled,
                        Err(_) => ResourceOutcome::Panicked,
                    }
                };
                let _ = sender.send(outcome);
                if let Some(waker) = waker {
                    let _ = catch_unwind(AssertUnwindSafe(|| waker.wake()));
                }
            }),
        });
        drop(state);
        self.pool.shared.ready.notify_one();
        Ok(ResourceTask { id, kind, receiver })
    }
}

fn worker(shared: Arc<Shared>) {
    loop {
        let job = {
            let mut state = shared.state.lock().unwrap();
            while state.jobs.is_empty() && !state.stopped {
                state = shared.ready.wait(state).unwrap();
            }
            if state.stopped {
                return;
            }
            state.jobs.pop().unwrap()
        };
        (job.work)(true);
    }
}
