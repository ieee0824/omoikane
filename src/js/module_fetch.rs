//! Bounded HTTP workers for module graphs. Only HTTP data crosses threads;
//! Boa module records, parsing, and evaluation stay on the owning JS thread.

use crate::http::{Client, CookieJar, HttpRequest, HttpResponse};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

const WORKERS: usize = 4;

#[derive(Debug)]
pub(super) struct FetchedModule {
    pub response: HttpResponse,
    pub elapsed: Duration,
}

type FetchResult = Result<Arc<FetchedModule>, Arc<str>>;

#[derive(Debug, Default)]
struct FetchState {
    result: Option<FetchResult>,
    waiters: Vec<Waker>,
}

/// One download shared by all imports of a Document/URL pair. Every waiter
/// gets the same HTTP result; the loader then shares the parsed module record.
#[derive(Debug, Clone, Default)]
pub(super) struct ModuleFetch(Arc<Mutex<FetchState>>);

impl ModuleFetch {
    fn complete(&self, result: FetchResult) {
        let waiters = {
            let mut state = self.0.lock().unwrap();
            if state.result.is_some() {
                return;
            }
            state.result = Some(result);
            std::mem::take(&mut state.waiters)
        };
        for waiter in waiters {
            waiter.wake();
        }
    }

    pub fn same_request(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    pub fn cancel(&self) {
        self.complete(Err("module document is no longer live".into()));
    }

    fn is_complete(&self) -> bool {
        self.0.lock().unwrap().result.is_some()
    }
}

impl Future for ModuleFetch {
    type Output = FetchResult;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self.0.lock().unwrap();
        if let Some(result) = &state.result {
            return Poll::Ready(result.clone());
        }
        if !state
            .waiters
            .iter()
            .any(|waiter| waiter.will_wake(context.waker()))
        {
            state.waiters.push(context.waker().clone());
        }
        Poll::Pending
    }
}

struct Request {
    url: String,
    public_only: bool,
    fetch: ModuleFetch,
}

#[derive(Debug)]
pub(super) struct ModuleFetchPool {
    sender: mpsc::Sender<Request>,
    live: Arc<AtomicBool>,
}

impl ModuleFetchPool {
    pub fn new() -> std::io::Result<Self> {
        let (sender, receiver) = mpsc::channel::<Request>();
        let receiver = Arc::new(Mutex::new(receiver));
        let cookies = Arc::new(Mutex::new(CookieJar::new()));
        let live = Arc::new(AtomicBool::new(true));
        for index in 0..WORKERS {
            let receiver = receiver.clone();
            let cookies = cookies.clone();
            let live = live.clone();
            std::thread::Builder::new()
                .name(format!("omoikane-module-{index}"))
                .spawn(move || {
                    let mut client = Client::new();
                    loop {
                        // Release the receiver lock before doing any I/O so
                        // all workers can service independent requests.
                        let request = receiver.lock().unwrap().recv();
                        let Ok(request) = request else {
                            break;
                        };
                        if !live.load(Ordering::Acquire) {
                            request.fetch.cancel();
                            continue;
                        }
                        if request.fetch.is_complete() {
                            continue;
                        }
                        let started = Instant::now();
                        let result = (|| {
                            let mut http = HttpRequest::get(&request.url)
                                .map_err(|error| Arc::<str>::from(error.to_string()))?;
                            if request.public_only {
                                http.require_public_ip();
                            }
                            let response = client
                                .send_with_shared_cookies(http, &cookies)
                                .map_err(|error| Arc::<str>::from(error.to_string()))?;
                            Ok(Arc::new(FetchedModule {
                                response,
                                elapsed: started.elapsed(),
                            }))
                        })();
                        request.fetch.complete(result);
                    }
                })?;
        }
        Ok(Self { sender, live })
    }

    pub fn fetch(&self, url: String, public_only: bool) -> ModuleFetch {
        let fetch = ModuleFetch::default();
        if self
            .sender
            .send(Request {
                url,
                public_only,
                fetch: fetch.clone(),
            })
            .is_err()
        {
            fetch.complete(Err("module HTTP workers are unavailable".into()));
        }
        fetch
    }
}

impl Drop for ModuleFetchPool {
    fn drop(&mut self) {
        // Runtime teardown must not wait for a slow transport. Queued work is
        // discarded; at most WORKERS active requests finish under the HTTP
        // timeout, without retaining any JS state or running page code.
        self.live.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::task::Wake;

    struct Counter(AtomicUsize);
    impl Wake for Counter {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn cancelling_shared_download_wakes_every_waiter_and_stays_cancelled() {
        let mut first = ModuleFetch::default();
        let mut second = first.clone();
        let counters = [
            Arc::new(Counter(AtomicUsize::new(0))),
            Arc::new(Counter(AtomicUsize::new(0))),
        ];
        for (fetch, counter) in [(&mut first, &counters[0]), (&mut second, &counters[1])] {
            let waker = Waker::from(counter.clone());
            let mut context = Context::from_waker(&waker);
            assert!(Pin::new(&mut *fetch).poll(&mut context).is_pending());
            assert!(Pin::new(fetch).poll(&mut context).is_pending());
        }
        first.cancel();
        first.complete(Err("late transport error".into()));
        for counter in counters {
            assert_eq!(counter.0.load(Ordering::SeqCst), 1);
        }
        let mut context = Context::from_waker(Waker::noop());
        for fetch in [&mut first, &mut second] {
            let Poll::Ready(Err(error)) = Pin::new(fetch).poll(&mut context) else {
                panic!("cancelled fetch did not settle");
            };
            assert_eq!(&*error, "module document is no longer live");
        }
    }

    #[test]
    fn dropping_pool_discards_queued_requests_without_waiting_for_active_io() {
        use std::io::Write;
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/module.js", listener.local_addr().unwrap());
        let pool = ModuleFetchPool::new().unwrap();
        let active: Vec<_> = (0..WORKERS)
            .map(|_| pool.fetch(url.clone(), false))
            .collect();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut connections = Vec::new();
        while connections.len() < WORKERS {
            match listener.accept() {
                Ok((stream, _)) => connections.push(stream),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "workers did not start");
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(error) => panic!("accept: {error}"),
            }
        }
        let mut queued = pool.fetch(url, false);
        drop(pool);
        // Teardown returned even though all transports are still waiting.
        for mut stream in connections {
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .unwrap();
        }
        let mut context = Context::from_waker(Waker::noop());
        loop {
            if let Poll::Ready(result) = Pin::new(&mut queued).poll(&mut context) {
                assert!(result.is_err());
                break;
            }
            assert!(Instant::now() < deadline, "queued fetch was not cancelled");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
        drop(active);
    }
}
