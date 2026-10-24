use super::*;
use crate::http::{HttpParseError, HttpResponse, HttpTransport};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

struct HeldResources {
    entered: mpsc::Sender<String>,
    release: Mutex<mpsc::Receiver<()>>,
    requests: AtomicUsize,
}

impl HttpTransport for HeldResources {
    fn send(
        &self,
        request: &HttpRequest,
        _: Option<Duration>,
    ) -> Result<HttpResponse, HttpParseError> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        self.entered.send(request.url().to_string()).unwrap();
        self.release
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        let (content_type, bytes) = if request.url().to_string().ends_with(".png") {
            let mut canvas = crate::paint::Canvas::new(1, 1);
            canvas.set_pixel(0, 0, crate::paint::Color::rgb(255, 0, 0));
            ("image/png", canvas.encode_png())
        } else {
            (
                "font/ttf",
                include_bytes!(
                    "../../../../tests/fixtures/anonymized-arabic-fallback/DejaVuSans.ttf"
                )
                .to_vec(),
            )
        };
        Ok(HttpResponse::new(
            200,
            "OK",
            vec![
                ("Content-Type".into(), content_type.into()),
                ("Access-Control-Allow-Origin".into(), "*".into()),
            ],
            bytes,
        ))
    }
}

fn runtime() -> JsRuntime {
    let document = crate::html::TreeBuilder::parse("<body>text</body>").document();
    let mut runtime = JsRuntime::with_document_and_url(document, "https://font.test/").unwrap();
    runtime.set_nonblocking_font_loading(true);
    runtime.set_nonblocking_image_loading(true);
    runtime
}

fn settle(runtime: &mut JsRuntime) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        runtime.run_until_idle().unwrap();
        if !runtime.host_state.borrow().font_loading.has_pending() {
            return;
        }
        assert!(Instant::now() < deadline, "font resource failed to settle");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn held_font_keeps_tasks_responsive_and_completion_callbacks_alive_through_gc() {
    let mut runtime = runtime();
    let (entered, started) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let transport = Arc::new(HeldResources {
        entered,
        release: Mutex::new(gate),
        requests: AtomicUsize::new(0),
    });
    runtime.set_http_resource_transport(Some(transport.clone()));
    runtime.eval("globalThis.events=[];globalThis.ticks=0;(()=>{const f=new FontFace('Async','url(/held.ttf)');globalThis.faceRef=new WeakRef(f);const p=f.load();globalThis.same=p===f.load();p.then(()=>events.push('loaded'),()=>events.push('error'))})();setTimeout(()=>ticks++,0)").unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        started
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .ends_with("/held.ttf")
    );
    runtime.advance_clock_without_tasks(1);
    runtime.run_until_idle().unwrap();
    assert_eq!(runtime.eval("ticks").unwrap().as_number(), Some(1.0));
    assert_eq!(runtime.eval("same").unwrap().as_boolean(), Some(true));
    assert_eq!(
        runtime.eval("events.length").unwrap().as_number(),
        Some(0.0)
    );
    assert_eq!(
        runtime.next_rendering(),
        crate::cdp::NextRendering::EveryFrame
    );
    runtime.context.clear_kept_objects();
    boa_gc::force_collect();
    release.send(()).unwrap();
    settle(&mut runtime);
    assert_eq!(
        runtime
            .eval("events.join(',')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "loaded"
    );
    assert_eq!(transport.requests.load(Ordering::SeqCst), 1);
}

#[test]
fn images_and_fonts_share_one_execution_limit() {
    let mut runtime = runtime();
    runtime.set_resource_queue(crate::resource_worker::ResourceQueue::new(
        NonZeroUsize::new(2).unwrap(),
    ));
    let (entered, started) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let transport = Arc::new(HeldResources {
        entered,
        release: Mutex::new(gate),
        requests: AtomicUsize::new(0),
    });
    runtime.set_http_resource_transport(Some(transport.clone()));
    runtime
        .eval("globalThis.first=new FontFace('First','url(/first.ttf)');first.load()")
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        started
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .ends_with("/first.ttf")
    );
    runtime
        .eval("globalThis.image=new Image();image.src='/held.png';document.body.append(image)")
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        started
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .ends_with("/held.png")
    );
    runtime
        .eval("globalThis.second=new FontFace('Second','url(/second.ttf)');second.load()")
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(started.try_recv(), Err(mpsc::TryRecvError::Empty));
    assert_eq!(transport.requests.load(Ordering::SeqCst), 2);
    release.send(()).unwrap();
    assert!(
        started
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .ends_with("/second.ttf")
    );
    release.send(()).unwrap();
    release.send(()).unwrap();
    settle(&mut runtime);
    assert_eq!(
        runtime
            .eval("first.status+','+second.status")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "loaded,loaded"
    );
}

#[test]
fn asynchronous_font_csp_check_prevents_io_and_preserves_call_source() {
    let mut runtime = runtime();
    runtime.install_csp_policy(&["font-src 'none'".into()]);
    let (entered, _) = mpsc::channel();
    let (_, gate) = mpsc::channel();
    let transport = Arc::new(HeldResources {
        entered,
        release: Mutex::new(gate),
        requests: AtomicUsize::new(0),
    });
    runtime.set_http_resource_transport(Some(transport.clone()));
    runtime.eval("globalThis.fontEvents=[];globalThis.fontResult='pending';document.addEventListener('securitypolicyviolation',e=>fontEvents.push([e.sourceFile,e.lineNumber,e.effectiveDirective]));").unwrap();
    let (result, ..) = runtime.eval_safe_timed_source(
        "new FontFace('Blocked','url(/blocked.ttf)').load().then(()=>fontResult='loaded',()=>fontResult='error');",
        Some("https://effective.test/bundle.js"), Some("https://font.test/caller.js#fragment"), (23, 4), false);
    result.unwrap();
    runtime.run_until_idle().unwrap();
    settle(&mut runtime);
    assert_eq!(transport.requests.load(Ordering::SeqCst), 0);
    assert_eq!(
        runtime
            .eval("fontResult")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "error"
    );
    assert_eq!(
        runtime
            .eval("JSON.stringify(fontEvents)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[[\"https://font.test/caller.js\",23,\"font-src\"]]"
    );
}

struct NoCorsFont;
impl HttpTransport for NoCorsFont {
    fn send(&self, _: &HttpRequest, _: Option<Duration>) -> Result<HttpResponse, HttpParseError> {
        Ok(HttpResponse::new(
            200,
            "OK",
            vec![("Content-Type".into(), "font/ttf".into())],
            include_bytes!("../../../../tests/fixtures/anonymized-arabic-fallback/DejaVuSans.ttf")
                .to_vec(),
        ))
    }
}

#[test]
fn asynchronous_cross_origin_font_without_cors_rejects_its_promise() {
    let mut runtime = runtime();
    runtime.set_http_resource_transport(Some(Arc::new(NoCorsFont)));
    runtime.eval("globalThis.corsResult='pending';new FontFace('Foreign','url(https://other.test/font.ttf)').load().then(()=>corsResult='loaded',e=>corsResult=e.name)").unwrap();
    runtime.run_until_idle().unwrap();
    settle(&mut runtime);
    assert_eq!(
        runtime
            .eval("corsResult")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "NetworkError"
    );
}

#[test]
fn retired_runtime_cancels_queued_fonts_without_starving_the_next_page() {
    let (entered, started) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let transport = Arc::new(HeldResources {
        entered,
        release: Mutex::new(gate),
        requests: AtomicUsize::new(0),
    });
    let mut old = runtime();
    old.set_http_resource_transport(Some(transport.clone()));
    old.eval("for(let i=0;i<5;i++)new FontFace('Old'+i,'url(/old'+i+'.ttf)').load().catch(()=>{})")
        .unwrap();
    old.run_until_idle().unwrap();
    for _ in 0..4 {
        assert!(
            started
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .contains("/old")
        );
    }
    assert_eq!(started.try_recv(), Err(mpsc::TryRecvError::Empty));
    drop(old);
    let mut next = runtime();
    next.set_http_resource_transport(Some(transport.clone()));
    next.eval("globalThis.newResult='pending';new FontFace('New','url(/new.ttf)').load().then(()=>newResult='loaded',()=>newResult='error')").unwrap();
    next.run_until_idle().unwrap();
    assert!(
        started
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .ends_with("/new.ttf")
    );
    for _ in 0..5 {
        release.send(()).unwrap();
    }
    settle(&mut next);
    assert_eq!(
        next.eval("newResult")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "loaded"
    );
    assert_eq!(
        transport.requests.load(Ordering::SeqCst),
        5,
        "the retired fifth request must never start"
    );
}
