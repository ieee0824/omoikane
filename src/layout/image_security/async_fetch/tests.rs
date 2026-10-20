use super::*;
use crate::http::{HttpParseError, HttpRequest, HttpResponse, HttpTransport};
use std::sync::atomic::AtomicUsize;
use std::time::{Duration, Instant};

fn red_png() -> Vec<u8> {
    let mut canvas = crate::paint::Canvas::new(1, 1);
    canvas.set_pixel(0, 0, crate::paint::Color::rgb(255, 0, 0));
    canvas.encode_png()
}

#[test]
fn image_liveness_scopes_restore_and_expired_tokens_deny_without_policy_reports() {
    let live = Arc::new(AtomicBool::new(true));
    let cancelled = Arc::new(AtomicBool::new(false));
    assert!(image_request_is_live());
    with_image_request_liveness(Arc::downgrade(&live), || {
        assert!(image_request_is_live());
        let result = std::panic::catch_unwind(|| {
            with_image_request_liveness(Arc::downgrade(&cancelled), || {
                assert!(!image_request_is_live());
                assert!(!allows_image_request(
                    "https://image.test/child.png",
                    0,
                    "initial"
                ));
                panic!("exercise liveness scope restoration");
            });
        });
        assert!(result.is_err());
        assert!(image_request_is_live());
    });
    let expired = Arc::downgrade(&live);
    drop(live);
    with_image_request_liveness(expired, || assert!(!image_request_is_live()));
    assert!(image_request_is_live());

    let mut fixture = Fixture::new(Arc::new(RedirectImage(Mutex::new(Vec::new()))));
    let document = crate::html::TreeBuilder::parse("<body></body>").document();
    fixture.policy = CspPolicy::from_response_headers_and_document(
        &[("Content-Security-Policy".into(), "img-src 'none'".into())],
        &document,
        "https://image.test/page",
    );
    let (allowed, reports) = fixture.run_reports(|| {
        with_image_request_liveness(Arc::downgrade(&cancelled), || {
            allows_image_request("https://image.test/child.png", 0, "initial")
        })
    });
    assert!(!allowed);
    assert!(reports.is_empty(), "cancellation is not a CSP violation");
}

struct CancelDuringSvgResponse {
    live: Arc<AtomicBool>,
    requests: Mutex<Vec<String>>,
}

impl HttpTransport for CancelDuringSvgResponse {
    fn send(
        &self,
        request: &HttpRequest,
        _: Option<Duration>,
    ) -> Result<HttpResponse, HttpParseError> {
        self.requests
            .lock()
            .unwrap()
            .push(request.url().to_string());
        // Deterministically model retirement during the root's in-flight I/O.
        self.live.store(false, Ordering::Release);
        Ok(HttpResponse::new(
            200,
            "OK",
            vec![("Content-Type".into(), "image/svg+xml".into())],
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1">
              <image href="https://image.test/child.png" width="1" height="1"/>
            </svg>"#
                .to_vec(),
        ))
    }
}

#[test]
fn cancelled_image_response_skips_svg_decode_and_nested_http_requests() {
    let live = Arc::new(AtomicBool::new(true));
    let transport = Arc::new(CancelDuringSvgResponse {
        live: Arc::clone(&live),
        requests: Mutex::new(Vec::new()),
    });
    let fixture = Fixture::new(transport.clone());
    fixture.state.borrow_mut().set_async_loading(false);
    let url = "https://image.test/cancelled.svg";
    let (image, reports) = fixture.run_reports(|| {
        with_image_request_liveness(Arc::downgrade(&live), || {
            crate::layout::decode_or_fetch_image_asset(url, None)
        })
    });
    assert!(image.is_none(), "cancelled SVG must not be decoded");
    assert_eq!(*transport.requests.lock().unwrap(), [url.to_string()]);
    assert!(reports.is_empty());
    assert!(image_request_is_live());
}

struct HeldImage {
    entered: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
    requests: AtomicUsize,
    cookies: Mutex<Vec<Option<String>>>,
}

impl HttpTransport for HeldImage {
    fn send(
        &self,
        request: &HttpRequest,
        _: Option<Duration>,
    ) -> Result<HttpResponse, HttpParseError> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        self.cookies
            .lock()
            .unwrap()
            .push(request.header("Cookie").map(str::to_owned));
        let _ = self.entered.send(());
        let _ = self
            .release
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5));
        Ok(HttpResponse::new(
            200,
            "OK",
            vec![("Content-Type".into(), "image/png".into())],
            red_png(),
        ))
    }
}

struct Fixture {
    state: Rc<RefCell<ImageDocumentState>>,
    client: Client,
    cookies: Arc<Mutex<CookieJar>>,
    url: Url,
    policy: CspPolicy,
    embedder: EmbedderPolicyValue,
}

impl Fixture {
    fn new(transport: Arc<dyn HttpTransport>) -> Self {
        let mut client = Client::new();
        client.set_transport(Some(transport));
        let state = Rc::new(RefCell::new(ImageDocumentState::default()));
        state.borrow_mut().set_async_loading(true);
        Self {
            state,
            client,
            cookies: Arc::new(Mutex::new(CookieJar::new())),
            url: "https://image.test/page".parse().unwrap(),
            policy: CspPolicy::default(),
            embedder: EmbedderPolicyValue::UnsafeNone,
        }
    }

    fn run<T>(&self, operation: impl FnOnce() -> T) -> T {
        self.run_reports(operation).0
    }

    fn run_reports<T>(&self, operation: impl FnOnce() -> T) -> (T, Vec<ImagePolicyNotice>) {
        let ((result, _), reports) = self.run_all_reports(operation);
        (result, reports)
    }

    fn run_all_reports<T>(
        &self,
        operation: impl FnOnce() -> T,
    ) -> ((T, Vec<CorpViolation>), Vec<ImagePolicyNotice>) {
        with_image_security_policy(
            Rc::clone(&self.state),
            self.policy.clone(),
            self.client.fork_for_worker(),
            false,
            || {
                with_image_cookie_store_reporting(
                    Arc::clone(&self.cookies),
                    crate::http::CookieRequestContext::new(Some(self.url.clone()), None, false),
                    123,
                    CorsOrigin::from_url(&self.url),
                    self.embedder,
                    EmbedderPolicyValue::UnsafeNone,
                    operation,
                )
            },
        )
    }
}

struct RedirectImage(Mutex<Vec<String>>);

#[test]
fn failed_or_unwound_cached_layout_cannot_publish_partial_image_coverage() {
    let fixture = Fixture::new(Arc::new(RedirectImage(Mutex::new(Vec::new()))));
    fixture.run(|| {
        let unrelated = HashSet::from(["https://image.test/background.png".to_owned()]);
        with_image_layout_dependency_capture(|| ((), true));
        let owner = IMAGE_CACHE_OWNER.with(|slot| slot.borrow().clone().unwrap());
        assert!(
            !fixture
                .state
                .borrow()
                .layout_dependencies
                .completion_requires_layout(Some(&owner), &unrelated,)
        );

        with_image_layout_dependency_capture(|| {
            // A nested successful layout must not survive an outer failure.
            with_image_layout_dependency_capture(|| ((), true));
            ((), false)
        });
        assert!(
            fixture
                .state
                .borrow()
                .layout_dependencies
                .completion_requires_layout(Some(&owner), &unrelated,)
        );

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            with_image_layout_dependency_capture::<()>(|| {
                with_image_layout_dependency_capture(|| ((), true));
                panic!("exercise failed outer layout cleanup");
            });
        }));
        assert!(result.is_err());
        assert!(ACTIVE_LAYOUT_DEPENDENCY_CAPTURES.with(|stack| stack.borrow().is_empty()));
        assert!(
            fixture
                .state
                .borrow()
                .layout_dependencies
                .completion_requires_layout(Some(&owner), &unrelated,)
        );
        with_image_layout_dependency_capture(|| ((), true));
        assert!(
            !fixture
                .state
                .borrow()
                .layout_dependencies
                .completion_requires_layout(Some(&owner), &unrelated,)
        );
    });
}

#[test]
fn cached_layout_capture_cannot_publish_across_an_image_policy_reset() {
    let fixture = Fixture::new(Arc::new(RedirectImage(Mutex::new(Vec::new()))));
    fixture.run(|| {
        with_image_layout_dependency_capture(|| {
            fixture.state.borrow_mut().layout_dependencies.reset();
            // Restoring the same owner does not make the earlier capture valid.
            let owner = IMAGE_CACHE_OWNER.with(|slot| slot.borrow().clone().unwrap());
            fixture
                .state
                .borrow_mut()
                .layout_dependencies
                .select_owner(Some(&owner));
            ((), true)
        });
        let owner = IMAGE_CACHE_OWNER.with(|slot| slot.borrow().clone().unwrap());
        assert!(
            fixture
                .state
                .borrow()
                .layout_dependencies
                .completion_requires_layout(
                    Some(&owner),
                    &HashSet::from(["https://image.test/background.png".to_owned()]),
                )
        );
    });
}

struct RetiredImages {
    entered: mpsc::Sender<()>,
    release: (Mutex<bool>, std::sync::Condvar),
}

impl HttpTransport for RetiredImages {
    fn send(
        &self,
        request: &HttpRequest,
        _: Option<Duration>,
    ) -> Result<HttpResponse, HttpParseError> {
        if request.url().to_string().contains("/old-") {
            self.entered.send(()).unwrap();
            let (lock, ready) = &self.release;
            let _held = ready
                .wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(10), |released| {
                    !*released
                })
                .unwrap();
        }
        Ok(HttpResponse::new(
            200,
            "OK",
            vec![("Content-Type".into(), "image/png".into())],
            red_png(),
        ))
    }
}

#[test]
fn retiring_document_does_not_block_new_document_on_its_four_held_images() {
    let (entered, started) = mpsc::channel();
    let transport = Arc::new(RetiredImages {
        entered,
        release: (Mutex::new(false), std::sync::Condvar::new()),
    });
    let old = Fixture::new(transport.clone());
    for index in 0..4 {
        let url = format!("https://image.test/old-{index}.png");
        assert!(
            old.run(|| crate::layout::decode_or_fetch_image_asset(&url, None))
                .is_none()
        );
    }
    for _ in 0..4 {
        started.recv_timeout(Duration::from_secs(2)).unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    drop(old);

    let fresh = Fixture::new(transport.clone());
    let url = "https://image.test/fresh.png";
    assert!(
        fresh
            .run(|| crate::layout::decode_or_fetch_image_asset(url, None))
            .is_none()
    );
    // Finish before any old response can expire its ten-second hold.
    while fresh.state.borrow().has_pending_images() {
        fresh.run(poll);
        assert!(
            Instant::now() < deadline,
            "retired document occupied new image workers"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let image = fresh
        .run(|| crate::layout::decode_or_fetch_image_asset(url, None))
        .unwrap();
    assert!(
        Instant::now() < deadline,
        "document teardown waited for image I/O"
    );
    assert_eq!(image.pixels(), &[255, 0, 0, 255]);
    assert!(!*transport.release.0.lock().unwrap());
    *transport.release.0.lock().unwrap() = true;
    transport.release.1.notify_all();
}

struct SvgWithBlobImage;

impl HttpTransport for SvgWithBlobImage {
    fn send(&self, _: &HttpRequest, _: Option<Duration>) -> Result<HttpResponse, HttpParseError> {
        Ok(HttpResponse::new(
            200,
            "OK",
            vec![("Content-Type".into(), "image/svg+xml".into())],
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1">
              <rect width="1" height="1" fill="blue"/>
              <image href="blob:https://image.test/http-svg-child" width="1" height="1"/>
            </svg>"#
                .to_vec(),
        ))
    }
}

#[test]
fn http_svg_worker_resolves_blob_image_registered_on_the_document_thread() {
    let blob = "blob:https://image.test/http-svg-child";
    crate::data::register_blob_url(blob.into(), red_png(), "image/png".into());
    let fixture = Fixture::new(Arc::new(SvgWithBlobImage));
    let url = "https://image.test/with-blob.svg";
    assert!(
        fixture
            .run(|| crate::layout::decode_or_fetch_image_asset(url, None))
            .is_none()
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while fixture.state.borrow().has_pending_images() {
        fixture.run(poll);
        assert!(Instant::now() < deadline, "HTTP SVG did not settle");
        std::thread::sleep(Duration::from_millis(1));
    }
    let image = fixture
        .run(|| crate::layout::decode_or_fetch_image_asset(url, None))
        .unwrap();
    assert_eq!(image.pixels(), &[255, 0, 0, 255]);
    crate::data::revoke_blob_url(blob);
}

#[test]
fn blob_svg_recovers_after_its_pending_image_dependency_completes() {
    let (entered, started) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let transport = Arc::new(HeldImage {
        entered,
        release: Mutex::new(gate),
        requests: AtomicUsize::new(0),
        cookies: Mutex::new(Vec::new()),
    });
    let fixture = Fixture::new(transport.clone());
    let blob = "blob:https://image.test/pending-svg";
    crate::data::register_blob_url(
        blob.into(),
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1">
          <rect width="1" height="1" fill="blue"/>
          <image href="https://image.test/held.png" width="1" height="1"/>
        </svg>"#
            .to_vec(),
        "image/svg+xml".into(),
    );
    let partial = fixture
        .run(|| crate::layout::decode_or_fetch_image_asset(blob, None))
        .unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(partial.pixels(), &[0, 0, 255, 255]);

    // Another blob without pending dependencies must still be cached, even
    // while the first blob's HTTP image is held.
    let independent = "blob:https://image.test/independent-svg";
    crate::data::register_blob_url(
        independent.into(),
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1">
          <rect width="1" height="1" fill="blue"/>
        </svg>"#
            .to_vec(),
        "image/svg+xml".into(),
    );
    assert!(
        fixture
            .run(|| crate::layout::decode_or_fetch_image_asset(independent, None))
            .is_some()
    );
    crate::data::revoke_blob_url(independent);
    assert!(
        fixture
            .run(|| crate::layout::decode_or_fetch_image_asset(independent, None))
            .is_some()
    );

    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while fixture.state.borrow().has_pending_images() {
        fixture.run(poll);
        assert!(Instant::now() < deadline, "SVG dependency did not settle");
        std::thread::sleep(Duration::from_millis(1));
    }
    let complete = fixture
        .run(|| crate::layout::decode_or_fetch_image_asset(blob, None))
        .unwrap();
    assert_eq!(complete.pixels(), &[255, 0, 0, 255]);
    crate::data::revoke_blob_url(blob);
    let cached = fixture
        .run(|| crate::layout::decode_or_fetch_image_asset(blob, None))
        .unwrap();
    assert_eq!(cached.pixels(), complete.pixels());
    assert_eq!(transport.requests.load(Ordering::SeqCst), 1);
}

#[test]
fn synchronous_blob_svg_keeps_its_complete_cached_image_after_revocation() {
    let (entered, _) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    release.send(()).unwrap();
    let fixture = Fixture::new(Arc::new(HeldImage {
        entered,
        release: Mutex::new(gate),
        requests: AtomicUsize::new(0),
        cookies: Mutex::new(Vec::new()),
    }));
    fixture.state.borrow_mut().set_async_loading(false);
    let blob = "blob:https://image.test/synchronous-svg";
    crate::data::register_blob_url(
        blob.into(),
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1">
          <image href="https://image.test/held.png" width="1" height="1"/>
        </svg>"#
            .to_vec(),
        "image/svg+xml".into(),
    );
    let image = fixture
        .run(|| crate::layout::decode_or_fetch_image_asset(blob, None))
        .unwrap();
    assert_eq!(image.pixels(), &[255, 0, 0, 255]);
    assert!(!fixture.state.borrow().has_pending_images());
    crate::data::revoke_blob_url(blob);
    let cached = fixture
        .run(|| crate::layout::decode_or_fetch_image_asset(blob, None))
        .unwrap();
    assert_eq!(cached.pixels(), image.pixels());
}

#[test]
fn asynchronous_cross_origin_image_preserves_coep_block_and_report() {
    let transport = Arc::new(RedirectImage(Mutex::new(Vec::new())));
    let mut fixture = Fixture::new(transport.clone());
    fixture.embedder = EmbedderPolicyValue::RequireCorp;
    let url = "https://other.test/no-corp.png";
    assert!(
        fixture
            .run(|| crate::layout::decode_or_fetch_image_asset(url, None))
            .is_none()
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut reports = Vec::new();
    while fixture.state.borrow().has_pending_images() {
        reports.extend(fixture.run_all_reports(poll).0.1);
        assert!(Instant::now() < deadline, "COEP image did not settle");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(*transport.0.lock().unwrap(), [url.to_string()]);
    assert_eq!(reports.len(), 1);
    assert!(
        fixture
            .run(|| crate::layout::decode_or_fetch_image_asset(url, None))
            .is_none()
    );
}

impl HttpTransport for RedirectImage {
    fn send(
        &self,
        request: &HttpRequest,
        _: Option<Duration>,
    ) -> Result<HttpResponse, HttpParseError> {
        let url = request.url().to_string();
        self.0.lock().unwrap().push(url.clone());
        if url == "https://image.test/redirect.png" {
            Ok(HttpResponse::new(
                302,
                "Found",
                vec![("Location".into(), "https://blocked.test/image.png".into())],
                Vec::new(),
            ))
        } else {
            Ok(HttpResponse::new(
                200,
                "OK",
                Vec::new(),
                crate::paint::Canvas::new(1, 1).encode_png(),
            ))
        }
    }
}

#[test]
fn asynchronous_redirect_checks_csp_before_contacting_blocked_destination() {
    let transport = Arc::new(RedirectImage(Mutex::new(Vec::new())));
    let mut fixture = Fixture::new(transport.clone());
    let document = crate::html::TreeBuilder::parse("<body></body>").document();
    fixture.policy = CspPolicy::from_response_headers_and_document(
        &[("Content-Security-Policy".into(), "img-src 'self'".into())],
        &document,
        "https://image.test/page",
    );
    let url = "https://image.test/redirect.png";
    assert!(
        fixture
            .run(|| crate::layout::decode_or_fetch_image_asset(url, None))
            .is_none()
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut reports = Vec::new();
    while fixture.state.borrow().has_pending_images() {
        reports.extend(fixture.run_reports(poll).1);
        assert!(Instant::now() < deadline, "redirect did not settle");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(*transport.0.lock().unwrap(), [url.to_string()]);
    assert_eq!(reports.len(), 1);
    assert!(!reports[0].enforced.is_empty());
    assert!(
        fixture
            .run(|| crate::layout::decode_or_fetch_image_asset(url, None))
            .is_none()
    );
}

#[test]
fn held_image_returns_without_blocking_and_deduplicates_until_completion() {
    let (entered, started) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let transport = Arc::new(HeldImage {
        entered,
        release: Mutex::new(gate),
        requests: AtomicUsize::new(0),
        cookies: Mutex::new(Vec::new()),
    });
    let fixture = Fixture::new(transport.clone());
    fixture.cookies.lock().unwrap().add_from_header_for_url(
        "session=test-session; Path=/; Secure; SameSite=Strict",
        &fixture.url,
    );
    let url = "https://image.test/held.png";
    assert!(
        fixture
            .run(|| crate::layout::decode_or_fetch_image_asset(url, None))
            .is_none()
    );
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(fixture.state.borrow().has_pending_images());
    for _ in 0..3 {
        assert!(!fixture.run(poll));
        assert!(
            fixture
                .run(|| crate::layout::decode_or_fetch_image_asset(url, None))
                .is_none()
        );
    }
    assert_eq!(transport.requests.load(Ordering::SeqCst), 1);
    assert_eq!(
        *transport.cookies.lock().unwrap(),
        [Some("session=test-session".into())]
    );
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while fixture.state.borrow().has_pending_images() {
        fixture.run(poll);
        assert!(
            Instant::now() < deadline,
            "image completion was not delivered"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let image = fixture
        .run(|| crate::layout::decode_or_fetch_image_asset(url, None))
        .unwrap();
    assert_eq!((image.width(), image.height()), (1, 1));
    assert_eq!(transport.requests.load(Ordering::SeqCst), 1);
}

#[test]
fn changing_request_context_discards_inflight_image_and_refetches() {
    let (entered, started) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let transport = Arc::new(HeldImage {
        entered,
        release: Mutex::new(gate),
        requests: AtomicUsize::new(0),
        cookies: Mutex::new(Vec::new()),
    });
    let mut fixture = Fixture::new(transport.clone());
    let url = "https://image.test/context.png";
    assert!(
        fixture
            .run(|| crate::layout::decode_or_fetch_image_asset(url, None))
            .is_none()
    );
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    fixture.url = "https://other.test/page".parse().unwrap();
    // Discarding the old context must request a repaint, including CSS-only images.
    assert!(fixture.run(poll));
    assert!(!fixture.state.borrow().has_pending_images());
    release.send(()).unwrap();
    release.send(()).unwrap();
    assert!(
        fixture
            .run(|| crate::layout::decode_or_fetch_image_asset(url, None))
            .is_none()
    );
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while fixture.state.borrow().has_pending_images() {
        fixture.run(poll);
        assert!(
            Instant::now() < deadline,
            "new context image did not settle"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        fixture
            .run(|| crate::layout::decode_or_fetch_image_asset(url, None))
            .is_some()
    );
    assert_eq!(transport.requests.load(Ordering::SeqCst), 2);
}

#[test]
fn completed_response_from_different_owner_requires_layout_without_merging() {
    let fixture = Fixture::new(Arc::new(RedirectImage(Mutex::new(Vec::new()))));
    let url = "https://image.test/mismatched-owner.png";
    let report_identity = (
        url.to_owned(),
        "https://other.test/redirect.png".to_owned(),
        1,
    );
    let (settled, ready) = mpsc::channel();

    fixture.run(|| {
        ensure_image_cache_context();
        let owner = IMAGE_CACHE_OWNER.with(|slot| slot.borrow().clone().unwrap());
        let mut mismatched_owner = owner.clone();
        mismatched_owner.2 = mismatched_owner.2.saturating_add(1);
        let mut cache = ImageCache::default();
        cache.owner = Some(mismatched_owner);
        cache.images.insert(url.to_owned(), None);
        let response = Response {
            cache,
            reported: HashSet::from([report_identity.clone()]),
            csp: vec![ImagePolicyNotice {
                reference: url.to_owned(),
                enforced: Vec::new(),
                report_only: Vec::new(),
            }],
            coep: vec![CorpViolation {
                blocked_url: url.to_owned(),
                report_only: true,
                enforced: false,
            }],
        };
        let task = {
            let state = fixture.state.borrow();
            state
                .asynchronous
                .pool
                .submit(
                    &state.asynchronous.scope,
                    ResourcePriority::Normal,
                    ResourceKind::Image,
                    move |_| {
                        settled.send(()).unwrap();
                        response
                    },
                )
                .unwrap()
        };
        let mut state = fixture.state.borrow_mut();
        state.asynchronous.owner = Some(owner);
        state.asynchronous.pending.insert(url.to_owned(), task);
    });
    ready.recv_timeout(Duration::from_secs(5)).unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut observed = None;
    while fixture.state.borrow().has_pending_images() {
        let ((effects, coep), csp) = fixture.run_all_reports(poll_effects);
        assert!(coep.is_empty(), "mismatched response merged COEP reports");
        assert!(csp.is_empty(), "mismatched response merged CSP reports");
        if effects.changed {
            assert!(observed.replace(effects).is_none());
        }
        assert!(
            Instant::now() < deadline,
            "mismatched response did not settle"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        observed,
        Some(ImageCompletionEffects {
            changed: true,
            requires_layout: true,
        })
    );

    let state = fixture.state.borrow();
    assert!(!state.cache.images.contains_key(url));
    assert!(!state.reported.borrow().contains(&report_identity));
}

#[test]
fn disabling_async_loading_reuses_an_existing_download_without_sync_refetch() {
    let (entered, started) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let transport = Arc::new(HeldImage {
        entered,
        release: Mutex::new(gate),
        requests: AtomicUsize::new(0),
        cookies: Mutex::new(Vec::new()),
    });
    let fixture = Fixture::new(transport.clone());
    let url = "https://image.test/held.png";
    assert!(
        fixture
            .run(|| crate::layout::decode_or_fetch_image_asset(url, None))
            .is_none()
    );
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    fixture.state.borrow_mut().set_async_loading(false);
    let before = Instant::now();
    assert!(
        fixture
            .run(|| crate::layout::decode_or_fetch_image_asset(url, None))
            .is_none()
    );
    assert!(
        before.elapsed() < Duration::from_secs(1),
        "existing download blocked caller"
    );
    assert_eq!(transport.requests.load(Ordering::SeqCst), 1);
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while fixture.state.borrow().has_pending_images() {
        fixture.run(poll);
        assert!(
            Instant::now() < deadline,
            "existing download did not finish"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        fixture
            .run(|| crate::layout::decode_or_fetch_image_asset(url, None))
            .is_some()
    );
    assert_eq!(transport.requests.load(Ordering::SeqCst), 1);
}
