//! Bounded HTTP workers for module graphs. Only HTTP data crosses threads;
//! Boa module records, parsing, and evaluation stay on the owning JS thread.

use crate::http::corp::EmbedderPolicyValue;
use crate::http::cors::{self, CredentialsMode, Origin, PreflightCache, RedirectMode, RequestMode};
use crate::http::{
    Client, CookieJar, CookieRequestContext, HttpRequest, HttpResponse, HttpTransport,
};
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
    cookie_context: Option<CookieRequestContext>,
    isolation: Option<ModuleIsolation>,
    fetch: ModuleFetch,
}

/// The requesting document's origin, enforced COEP and graph credentials.
#[derive(Clone)]
pub(super) struct ModuleIsolation {
    pub origin: Origin,
    pub embedder: EmbedderPolicyValue,
    pub credentials: CredentialsMode,
}

#[derive(Debug)]
pub(super) struct ModuleFetchPool {
    sender: mpsc::Sender<Request>,
    live: Arc<AtomicBool>,
}

impl ModuleFetchPool {
    pub fn new(cookies: Arc<Mutex<CookieJar>>) -> std::io::Result<Self> {
        Self::new_with_transport(cookies, None)
    }

    /// Uses a synthetic transport to exercise whole module graphs in tests.
    #[cfg(test)]
    pub(super) fn new_for_test(
        cookies: Arc<Mutex<CookieJar>>,
        transport: Arc<dyn HttpTransport>,
    ) -> std::io::Result<Self> {
        Self::new_with_transport(cookies, Some(transport))
    }

    fn new_with_transport(
        cookies: Arc<Mutex<CookieJar>>,
        transport: Option<Arc<dyn HttpTransport>>,
    ) -> std::io::Result<Self> {
        let (sender, receiver) = mpsc::channel::<Request>();
        let receiver = Arc::new(Mutex::new(receiver));
        let live = Arc::new(AtomicBool::new(true));
        for index in 0..WORKERS {
            let receiver = receiver.clone();
            let cookies = cookies.clone();
            let live = live.clone();
            let transport = transport.clone();
            std::thread::Builder::new()
                .name(format!("omoikane-module-{index}"))
                .spawn(move || {
                    let mut client = Client::new();
                    if let Some(transport) = transport {
                        client.set_transport(Some(transport));
                    }
                    client.set_shared_cookie_store(cookies.clone());
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
                            if let Some(context) = request.cookie_context.as_ref() {
                                context.apply(&mut http);
                            }
                            let response = if let Some(isolation) = request.isolation.as_ref() {
                                cors::fetch_with_embedder_policy(
                                    &mut client,
                                    http,
                                    &isolation.origin,
                                    RequestMode::Cors,
                                    isolation.credentials,
                                    RedirectMode::Follow,
                                    &mut PreflightCache::default(),
                                    isolation.embedder,
                                    None,
                                )
                                .map_err(|error| Arc::<str>::from(error.to_string()))?
                                .response
                            } else {
                                client
                                    .send_with_shared_cookies(http, &cookies)
                                    .map_err(|error| Arc::<str>::from(error.to_string()))?
                            };
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

    pub fn fetch(
        &self,
        url: String,
        public_only: bool,
        cookie_context: Option<CookieRequestContext>,
    ) -> ModuleFetch {
        self.fetch_with_isolation(url, public_only, cookie_context, None)
    }

    /// Downloads a module under a COEP owner's CORS policy.
    pub fn fetch_with_isolation(
        &self,
        url: String,
        public_only: bool,
        cookie_context: Option<CookieRequestContext>,
        isolation: Option<ModuleIsolation>,
    ) -> ModuleFetch {
        let fetch = ModuleFetch::default();
        if self
            .sender
            .send(Request {
                url,
                public_only,
                cookie_context,
                isolation,
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
    use crate::http::Url;
    use crate::test_support::http_fixture::{
        ACCEPT_TIMEOUT, FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback,
        read_request_headers,
    };
    use std::io::Write;
    use std::sync::atomic::AtomicUsize;
    use std::task::Wake;

    struct Counter(AtomicUsize);
    impl Wake for Counter {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn module_workers_use_top_level_site_for_samesite_cookies() {
        let listener = bind_loopback().unwrap();
        let url = format!("http://{}/module.js", listener.local_addr().unwrap());
        let target: Url = url.parse().unwrap();
        let server = FixtureWorker::spawn(move || {
            let mut cookies = Vec::new();
            for _ in 0..2 {
                let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
                let request = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
                let cookie = request
                    .lines()
                    .find_map(|line| line.strip_prefix("Cookie: "))
                    .map(str::trim)
                    .map(str::to_string);
                cookies.push(cookie);
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            }
            cookies
        });
        let store = Arc::new(Mutex::new(CookieJar::new()));
        for header in [
            "strict=1; SameSite=Strict; Path=/",
            "lax=2; SameSite=Lax; Path=/",
        ] {
            store
                .lock()
                .unwrap()
                .add_from_header_for_url(header, &target);
        }
        let pool = ModuleFetchPool::new(store).unwrap();
        let other: Url = "http://localhost/".parse().unwrap();
        for site in [other, target] {
            let mut fetch = pool.fetch(
                url.clone(),
                false,
                Some(CookieRequestContext::new(Some(site), None, false)),
            );
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let mut context = Context::from_waker(Waker::noop());
                if let Poll::Ready(result) = Pin::new(&mut fetch).poll(&mut context) {
                    assert!(result.is_ok(), "{result:?}");
                    break;
                }
                assert!(Instant::now() < deadline, "module fetch timed out");
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        assert_eq!(server.join(), [None, Some("strict=1; lax=2".to_string())]);
    }

    #[test]
    fn module_downloads_keep_document_grants_out_of_worker_internal_requests() {
        let listener = bind_loopback().unwrap();
        let address = listener.local_addr().unwrap();
        let target: Url = format!("http://{address}/post.js").parse().unwrap();
        let server = FixtureWorker::spawn(move || {
            let mut cookies = Vec::new();
            for phase in ["pre", "post", "worker", "nested"] {
                let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
                let request = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
                assert!(request.starts_with(&format!("GET /{phase}.js HTTP/1.1")));
                cookies.push(
                    request
                        .lines()
                        .find_map(|line| line.strip_prefix("Cookie: "))
                        .map(str::trim)
                        .map(str::to_owned),
                );
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\nContent-Length: 0\r\nSet-Cookie: {phase}=1; Path=/; Secure; SameSite=None\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
            }
            cookies
        });
        let store = Arc::new(Mutex::new(CookieJar::new()));
        store
            .lock()
            .unwrap()
            .add_from_header_for_url("session=pre; Path=/; Secure; SameSite=None", &target);
        let pool = ModuleFetchPool::new(Arc::clone(&store)).unwrap();
        let top: Url = format!("http://localhost:{}/", address.port())
            .parse()
            .unwrap();
        for phase in ["pre", "post", "worker", "nested"] {
            let context = match phase {
                "pre" => CookieRequestContext::new(Some(top.clone()), None, false),
                "post" => CookieRequestContext::new(Some(top.clone()), Some(target.clone()), false),
                "worker" => {
                    CookieRequestContext::new(Some(top.clone()), Some(target.clone()), false)
                        .for_worker_internal()
                }
                "nested" => {
                    CookieRequestContext::new(Some(target.clone()), Some(target.clone()), true)
                        .for_worker_internal()
                }
                _ => unreachable!(),
            };
            let mut fetch =
                pool.fetch(format!("http://{address}/{phase}.js"), false, Some(context));
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let mut context = Context::from_waker(Waker::noop());
                if let Poll::Ready(result) = Pin::new(&mut fetch).poll(&mut context) {
                    assert!(result.is_ok(), "{result:?}");
                    break;
                }
                assert!(Instant::now() < deadline, "module fetch timed out");
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        assert_eq!(
            server.join(),
            [None, Some("session=pre".to_string()), None, None]
        );
        let cookies = store.lock().unwrap().document_cookie(&target, &target);
        assert!(!cookies.contains("pre=1"));
        assert!(cookies.contains("post=1"));
        assert!(!cookies.contains("worker=1"));
        assert!(!cookies.contains("nested=1"));
    }

    #[test]
    fn module_fetch_requires_cors_response_permission_with_or_without_coep() {
        let listener = bind_loopback().unwrap();
        let port = listener.local_addr().unwrap().port();
        let owner: Url = format!("http://localhost:{port}/page").parse().unwrap();
        let origin = Origin::from_url(&owner);
        let access_control = origin.serialize();
        let server = FixtureWorker::spawn(move || {
            for _ in 0..4 {
                let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
                let request = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
                let permitted = request.starts_with("GET /permitted.js ");
                let header = if permitted {
                    format!("Access-Control-Allow-Origin: {access_control}\r\n")
                } else {
                    String::new()
                };
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\n{header}Content-Type: text/javascript\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
            }
        });
        let pool = ModuleFetchPool::new(Arc::new(Mutex::new(CookieJar::new()))).unwrap();
        for embedder in [
            EmbedderPolicyValue::RequireCorp,
            EmbedderPolicyValue::UnsafeNone,
        ] {
            let isolation = Some(ModuleIsolation {
                origin: origin.clone(),
                embedder,
                credentials: CredentialsMode::SameOrigin,
            });
            for (path, allowed) in [("permitted.js", true), ("denied.js", false)] {
                let url = format!("http://127.0.0.1:{port}/{path}");
                let mut fetch = pool.fetch_with_isolation(
                    url,
                    false,
                    Some(CookieRequestContext::new(Some(owner.clone()), None, false)),
                    isolation.clone(),
                );
                let deadline = Instant::now() + Duration::from_secs(5);
                loop {
                    let mut context = Context::from_waker(Waker::noop());
                    if let Poll::Ready(result) = Pin::new(&mut fetch).poll(&mut context) {
                        assert_eq!(result.is_ok(), allowed, "{embedder:?} {path}: {result:?}");
                        break;
                    }
                    assert!(Instant::now() < deadline, "module fetch timed out");
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        }
        server.join();
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
        let listener = bind_loopback().unwrap();
        let url = format!("http://{}/module.js", listener.local_addr().unwrap());
        let pool = ModuleFetchPool::new(Arc::new(Mutex::new(CookieJar::new()))).unwrap();
        let active: Vec<_> = (0..WORKERS)
            .map(|_| pool.fetch(url.clone(), false, None))
            .collect();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut connections = Vec::new();
        while connections.len() < WORKERS {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "workers did not start");
            connections
                .push(accept_with_timeout(&listener, remaining).expect("workers did not start"));
        }
        let mut queued = pool.fetch(url, false, None);
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
