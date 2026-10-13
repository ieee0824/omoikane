//! Bounded, ordered HTTP delivery for reports. No JS or DOM values cross the
//! worker boundary, and runtime teardown never waits for a slow endpoint.

use super::{CorsOrigin, CredentialsMode, PreflightCache, RedirectMode, RequestMode};
use crate::http::{Client, HttpRequest};
use std::collections::{HashSet, VecDeque};
use std::io;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const POST_DROP_DELIVERY_GRACE: Duration = Duration::from_secs(5);

/// Identifies the endpoint configuration which accepted a report. A response
/// from an old configuration must not disable its replacement.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct EndpointKey {
    pub document_id: usize,
    pub revision: u64,
    pub group: String,
    pub url: String,
}

/// A 410 key is needed only while reports from that endpoint generation may
/// still be outstanding. The delivery queue bounds those reports, so keeping
/// the most recent `capacity` keys also bounds this worker's lifetime storage.
struct GoneEndpoints {
    keys: HashSet<EndpointKey>,
    order: VecDeque<EndpointKey>,
    capacity: usize,
}

impl GoneEndpoints {
    fn new(capacity: usize) -> Self {
        Self {
            keys: HashSet::new(),
            order: VecDeque::new(),
            capacity,
        }
    }

    fn contains(&self, key: &EndpointKey) -> bool {
        self.keys.contains(key)
    }

    fn mark(&mut self, key: EndpointKey) {
        if self.keys.insert(key.clone()) {
            self.order.push_back(key);
        }
        if self.order.len() > self.capacity
            && let Some(oldest) = self.order.pop_front()
        {
            self.keys.remove(&oldest);
        }
    }
}

pub(super) enum ReportRequest {
    Legacy {
        client: Client,
        request: HttpRequest,
        credentials: bool,
    },
    Modern {
        client: Client,
        request: HttpRequest,
        origin: CorsOrigin,
        endpoint: EndpointKey,
    },
}

pub(super) enum ReportCompletion {
    Sent,
    Gone(EndpointKey),
}

/// One lazily started worker per runtime preserves report order without
/// allowing an endpoint's I/O to run inside a JS task. The queue has the same
/// cap as the runtime's CSP violation history; excess network reports are
/// dropped without suppressing their page-visible events or observers.
pub(super) struct ReportDelivery {
    sender: SyncSender<ReportRequest>,
    completions: Receiver<ReportCompletion>,
    capacity: usize,
    pending: usize,
    dropped: usize,
    post_drop_deadline: Arc<Mutex<Option<Instant>>>,
    post_drop_grace: Duration,
}

impl ReportDelivery {
    pub fn new() -> io::Result<Self> {
        Self::with_capacity(super::MAX_CSP_VIOLATIONS)
    }

    pub(super) fn with_capacity(capacity: usize) -> io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(capacity);
        let (completed_sender, completions) = mpsc::channel();
        let post_drop_deadline = Arc::new(Mutex::new(None));
        let worker_deadline = Arc::clone(&post_drop_deadline);
        std::thread::Builder::new()
            .name("omoikane-report-delivery".into())
            .spawn(move || {
                let mut cache = PreflightCache::default();
                let mut gone = GoneEndpoints::new(capacity);
                while let Ok(report) = receiver.recv() {
                    if worker_deadline
                        .lock()
                        .unwrap()
                        .as_ref()
                        .is_some_and(|deadline| Instant::now() >= *deadline)
                    {
                        break;
                    }
                    let result = send_report(report, &mut cache, &mut gone);
                    // The runtime may already be gone. Endpoint removal then
                    // has no owner to update, but queued HTTP work may finish.
                    let _ = completed_sender.send(result);
                }
            })?;
        Ok(Self {
            sender,
            completions,
            capacity,
            pending: 0,
            dropped: 0,
            post_drop_deadline,
            post_drop_grace: POST_DROP_DELIVERY_GRACE,
        })
    }

    /// Queues a report without blocking the owning JS task. A full or stopped
    /// queue rejects only network delivery, as Reporting API permits.
    pub fn enqueue(&mut self, report: ReportRequest) -> bool {
        if self.pending >= self.capacity {
            self.dropped += 1;
            return false;
        }
        if self.sender.try_send(report).is_ok() {
            self.pending += 1;
            true
        } else {
            self.dropped += 1;
            false
        }
    }

    /// Collects completed sends so 410 endpoint removal runs on the owning
    /// JS thread, after rechecking the Document and endpoint generation.
    pub fn drain(&mut self) -> Vec<ReportCompletion> {
        let mut results = Vec::new();
        while let Ok(result) = self.completions.try_recv() {
            self.pending -= 1;
            results.push(result);
        }
        results
    }

    #[cfg(test)]
    pub fn wait_for_pending(&mut self) -> Vec<ReportCompletion> {
        use std::time::Instant;

        let mut results = self.drain();
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.pending > 0 {
            let timeout = deadline.saturating_duration_since(Instant::now());
            let result = self
                .completions
                .recv_timeout(timeout)
                .expect("report HTTP worker did not complete");
            self.pending -= 1;
            results.push(result);
        }
        results
    }

    #[cfg(test)]
    pub fn dropped(&self) -> usize {
        self.dropped
    }

    #[cfg(test)]
    pub fn set_post_drop_grace(&mut self, grace: Duration) {
        self.post_drop_grace = grace;
    }
}

impl Drop for ReportDelivery {
    fn drop(&mut self) {
        // A short grace period delivers already queued reports after page
        // teardown. It cannot retain DOM/JS state, and the worker stops once
        // the bounded queue is empty or the grace period expires.
        *self.post_drop_deadline.lock().unwrap() = Some(Instant::now() + self.post_drop_grace);
    }
}

fn send_report(
    report: ReportRequest,
    cache: &mut PreflightCache,
    gone: &mut GoneEndpoints,
) -> ReportCompletion {
    match report {
        ReportRequest::Legacy {
            mut client,
            request,
            credentials,
        } => {
            let _ =
                client.send_once_with_timeout(request, credentials, Some(Duration::from_secs(2)));
            ReportCompletion::Sent
        }
        ReportRequest::Modern {
            mut client,
            request,
            origin,
            endpoint,
        } => {
            if gone.contains(&endpoint) {
                return ReportCompletion::Sent;
            }
            let result = crate::http::cors::fetch_with_timeout_restricting_redirects(
                &mut client,
                request,
                &origin,
                RequestMode::Cors,
                CredentialsMode::SameOrigin,
                RedirectMode::Follow,
                cache,
                Some(Duration::from_secs(2)),
            );
            if result.is_ok_and(|fetched| fetched.response.status_code() == 410) {
                gone.mark(endpoint.clone());
                ReportCompletion::Gone(endpoint)
            } else {
                ReportCompletion::Sent
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{HttpParseError, HttpResponse, HttpTransport, Method, Url};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn gone_endpoint_history_is_bounded_by_queue_capacity() {
        let mut gone = GoneEndpoints::new(2);
        let endpoint = |revision| EndpointKey {
            document_id: 1,
            revision,
            group: "reports".into(),
            url: "https://example.test/reports".into(),
        };
        gone.mark(endpoint(1));
        gone.mark(endpoint(2));
        gone.mark(endpoint(2));
        gone.mark(endpoint(3));
        assert_eq!(gone.keys.len(), 2);
        assert_eq!(gone.order.len(), 2);
        assert!(!gone.contains(&endpoint(1)));
        assert!(gone.contains(&endpoint(2)));
        assert!(gone.contains(&endpoint(3)));
    }

    struct SlowFirst {
        calls: AtomicUsize,
        started: mpsc::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
        finished: mpsc::Sender<()>,
    }

    impl HttpTransport for SlowFirst {
        fn send(
            &self,
            _: &HttpRequest,
            _: Option<Duration>,
        ) -> Result<HttpResponse, HttpParseError> {
            if self.calls.fetch_add(1, Ordering::AcqRel) == 0 {
                self.started.send(()).unwrap();
                let _ = self
                    .release
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(2));
            }
            self.finished.send(()).unwrap();
            HttpResponse::parse(&mut &b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n"[..])
        }
    }

    #[test]
    fn completed_but_uncollected_reports_still_count_toward_capacity() {
        struct Fast(mpsc::Sender<()>);
        impl HttpTransport for Fast {
            fn send(
                &self,
                _: &HttpRequest,
                _: Option<Duration>,
            ) -> Result<HttpResponse, HttpParseError> {
                self.0.send(()).unwrap();
                HttpResponse::parse(
                    &mut &b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n"[..],
                )
            }
        }

        let (sent_tx, sent_rx) = mpsc::channel();
        let mut client = Client::new();
        client.set_transport(Some(Arc::new(Fast(sent_tx))));
        let mut delivery = ReportDelivery::with_capacity(2).unwrap();
        let request = HttpRequest::new(
            Method::Post,
            "https://example.test/reports".parse::<Url>().unwrap(),
        );
        for _ in 0..2 {
            assert!(delivery.enqueue(ReportRequest::Legacy {
                client: client.fork_for_reporting(),
                request: request.clone(),
                credentials: false,
            }));
        }
        sent_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        sent_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(!delivery.enqueue(ReportRequest::Legacy {
            client: client.fork_for_reporting(),
            request: request.clone(),
            credentials: false,
        }));
        assert_eq!(delivery.dropped(), 1);
        assert_eq!(delivery.wait_for_pending().len(), 2);
        assert!(delivery.enqueue(ReportRequest::Legacy {
            client: client.fork_for_reporting(),
            request,
            credentials: false,
        }));
        assert_eq!(delivery.wait_for_pending().len(), 1);
    }

    #[test]
    fn teardown_grace_expires_before_starting_more_queued_reports() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let mut client = Client::new();
        client.set_transport(Some(Arc::new(SlowFirst {
            calls: AtomicUsize::new(0),
            started: started_tx,
            release: Mutex::new(release_rx),
            finished: finished_tx,
        })));
        let mut delivery = ReportDelivery::with_capacity(2).unwrap();
        delivery.set_post_drop_grace(Duration::from_millis(25));
        let request = HttpRequest::new(
            Method::Post,
            "https://example.test/reports".parse::<Url>().unwrap(),
        );
        assert!(delivery.enqueue(ReportRequest::Legacy {
            client: client.fork_for_reporting(),
            request: request.clone(),
            credentials: false,
        }));
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(delivery.enqueue(ReportRequest::Legacy {
            client: client.fork_for_reporting(),
            request,
            credentials: false,
        }));
        drop(client);
        drop(delivery);
        std::thread::sleep(Duration::from_millis(50));
        release_tx.send(()).unwrap();
        finished_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(
            finished_rx.recv_timeout(Duration::from_secs(2)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ));
    }
}
