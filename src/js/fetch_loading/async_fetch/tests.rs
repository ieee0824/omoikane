use super::*;
use crate::http::{HttpParseError, HttpResponse, HttpTransport};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;

struct Held {
    entered: mpsc::Sender<String>,
    gates: Vec<Mutex<mpsc::Receiver<()>>>,
    requests: AtomicUsize,
    fail: AtomicBool,
}

impl HttpTransport for Held {
    fn send(
        &self,
        request: &HttpRequest,
        _: Option<Duration>,
    ) -> Result<HttpResponse, HttpParseError> {
        let url = request.url().to_string();
        let index = if url.ends_with("/2") {
            1
        } else if url.ends_with("/3") {
            2
        } else {
            0
        };
        self.requests.fetch_add(1, Ordering::SeqCst);
        self.entered.send(url).unwrap();
        self.gates[index]
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        if self.fail.load(Ordering::SeqCst) {
            return Err(HttpParseError::Io(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "controlled fetch failure",
            )));
        }
        Ok(HttpResponse::new(
            200,
            "OK",
            vec![
                ("Content-Type".into(), "text/plain".into()),
                ("Access-Control-Allow-Origin".into(), "*".into()),
            ],
            [b"one".to_vec(), b"two".to_vec(), b"three".to_vec()][index].clone(),
        ))
    }
}

fn runtime() -> JsRuntime {
    let document = crate::html::TreeBuilder::parse("<body>text</body>").document();
    let mut runtime = JsRuntime::with_document_and_url(document, "https://fetch.test/").unwrap();
    runtime.set_nonblocking_fetch_loading(true);
    runtime
        .eval("globalThis.events=[];globalThis.ticks=0")
        .unwrap();
    runtime
}

fn held(runtime: &mut JsRuntime) -> (Arc<Held>, mpsc::Receiver<String>, Vec<mpsc::Sender<()>>) {
    let (entered, started) = mpsc::channel();
    let (release, gates): (Vec<_>, Vec<_>) = (0..3).map(|_| mpsc::channel()).unzip();
    let transport = Arc::new(Held {
        entered,
        gates: gates.into_iter().map(Mutex::new).collect(),
        requests: AtomicUsize::new(0),
        fail: AtomicBool::new(false),
    });
    runtime.set_http_resource_transport(Some(transport.clone()));
    (transport, started, release)
}

fn pump_until(runtime: &mut JsRuntime, source: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        runtime.run_until_idle().unwrap();
        if runtime.eval(source).unwrap().as_boolean() == Some(true) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "fetch completion did not satisfy {source}"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn fetch_and_xhr_wait_independently_while_timers_and_gc_remain_safe() {
    let mut runtime = runtime();
    let (transport, started, release) = held(&mut runtime);
    runtime.eval("fetch('/1').then(r=>r.text()).then(t=>events.push('fetch1:'+t));fetch('/2').then(r=>r.text()).then(t=>events.push('fetch2:'+t));let x=new XMLHttpRequest();x.open('GET','/3');x.onload=()=>events.push('xhr3:'+x.responseText);x.send();setTimeout(()=>ticks++,0)").unwrap();
    runtime.run_until_idle().unwrap();
    let mut urls: Vec<_> = (0..3)
        .map(|_| started.recv_timeout(Duration::from_secs(5)).unwrap())
        .collect();
    urls.sort();
    assert_eq!(
        urls,
        [
            "https://fetch.test/1",
            "https://fetch.test/2",
            "https://fetch.test/3"
        ]
    );
    runtime.tick(1).unwrap();
    assert_eq!(runtime.eval("ticks").unwrap().as_number(), Some(1.0));
    assert_eq!(
        runtime.eval("events.length").unwrap().as_number(),
        Some(0.0)
    );
    runtime.context.clear_kept_objects();
    boa_gc::force_collect();
    release[1].send(()).unwrap();
    pump_until(&mut runtime, "events.join(',')==='fetch2:two'");
    release[2].send(()).unwrap();
    pump_until(&mut runtime, "events.join(',')==='fetch2:two,xhr3:three'");
    release[0].send(()).unwrap();
    pump_until(
        &mut runtime,
        "events.join(',')==='fetch2:two,xhr3:three,fetch1:one'",
    );
    assert_eq!(transport.requests.load(Ordering::SeqCst), 3);
    assert!(!runtime.host_state.borrow().fetch_loads.has_pending());
}

#[test]
fn abort_rejects_fetch_and_releases_its_scope_before_the_network_returns() {
    let mut runtime = runtime();
    let (_, started, release) = held(&mut runtime);
    runtime.eval("globalThis.controller=new AbortController();globalThis.reason={cancelled:true};fetch('/1',{signal:controller.signal}).then(()=>events.push('loaded'),e=>events.push(e===reason?'aborted':'wrong reason'))").unwrap();
    runtime.run_until_idle().unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    let tokens: Vec<_> = runtime
        .host_state
        .borrow()
        .fetch_loads
        .pending
        .values()
        .map(|pending| pending.scope.token())
        .collect();
    assert_eq!(tokens.len(), 1);
    runtime.eval("controller.abort(reason)").unwrap();
    pump_until(&mut runtime, "events.join(',')==='aborted'");
    assert!(tokens.iter().all(|token| token.is_cancelled()));
    assert!(!runtime.host_state.borrow().fetch_loads.has_pending());
    release[0].send(()).unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime.eval("events.length").unwrap().as_number(),
        Some(1.0)
    );
}

#[test]
fn xhr_aborted_before_its_microtask_does_not_submit_a_background_request() {
    let mut runtime = runtime();
    let (transport, started, _release) = held(&mut runtime);
    runtime.eval("const xhr=new XMLHttpRequest();xhr.open('GET','/1');xhr.onload=()=>events.push('load');xhr.onloadend=()=>events.push('end');xhr.send();xhr.abort()").unwrap();
    pump_until(&mut runtime, "events.join(',')==='end'");
    assert!(!runtime.host_state.borrow().fetch_loads.has_pending());
    assert_eq!(transport.requests.load(Ordering::SeqCst), 0);
    assert!(matches!(started.try_recv(), Err(mpsc::TryRecvError::Empty)));
}

#[test]
fn aborting_xhr_drops_its_background_request_and_suppresses_load() {
    let mut runtime = runtime();
    let (_, started, release) = held(&mut runtime);
    runtime.eval("globalThis.xhr=new XMLHttpRequest();xhr.open('GET','/1');xhr.onload=()=>events.push('load');xhr.onabort=()=>events.push('abort');xhr.onloadend=()=>events.push('end');xhr.send()").unwrap();
    runtime.run_until_idle().unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    runtime.eval("xhr.abort()").unwrap();
    pump_until(&mut runtime, "events.join(',')==='abort,end'");
    assert!(!runtime.host_state.borrow().fetch_loads.has_pending());
    release[0].send(()).unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("events.join(',')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "abort,end"
    );
}

#[test]
fn connect_csp_rejection_happens_before_any_transport_or_worker_request() {
    let mut runtime = runtime();
    let (transport, _, _) = held(&mut runtime);
    runtime.install_csp_policy(&["connect-src 'none'".into()]);
    runtime.eval("fetch('/1').then(()=>events.push('loaded'),e=>events.push(e instanceof TypeError?'blocked':'wrong error'))").unwrap();
    pump_until(&mut runtime, "events.join(',')==='blocked'");
    assert_eq!(transport.requests.load(Ordering::SeqCst), 0);
    assert!(!runtime.host_state.borrow().fetch_loads.has_pending());
}

#[test]
fn iframe_fetch_failure_is_created_in_its_own_realm_after_gc() {
    let mut runtime = runtime();
    let (transport, started, release) = held(&mut runtime);
    transport.fail.store(true, Ordering::SeqCst);
    runtime.eval("globalThis.frame=document.createElement('iframe');frame.srcdoc='<body>child</body>';document.body.append(frame)").unwrap();
    runtime.run_until_idle().unwrap();
    runtime.eval("let child=frame.contentWindow;child.fetch('https://fetch.test/1').then(()=>events.push('loaded'),e=>events.push(Object.getPrototypeOf(e)===child.Error.prototype?'child error':'wrong realm'))").unwrap();
    runtime.run_until_idle().unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    runtime.context.clear_kept_objects();
    boa_gc::force_collect();
    release[0].send(()).unwrap();
    pump_until(&mut runtime, "events.join(',')==='child error'");
}

#[test]
fn iframe_retirement_cancels_fetch_without_late_promise_delivery() {
    let mut runtime = runtime();
    let (_, started, release) = held(&mut runtime);
    runtime.eval("globalThis.frame=document.createElement('iframe');frame.srcdoc='<body>child</body>';document.body.append(frame)").unwrap();
    runtime.run_until_idle().unwrap();
    runtime.eval("frame.contentWindow.fetch('https://fetch.test/1').then(()=>events.push('loaded'),()=>events.push('error'))").unwrap();
    runtime.run_until_idle().unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    let tokens: Vec<_> = runtime
        .host_state
        .borrow()
        .fetch_loads
        .pending
        .values()
        .map(|pending| pending.scope.token())
        .collect();
    assert_eq!(tokens.len(), 1);
    runtime.eval("frame.remove()").unwrap();
    assert!(tokens.iter().all(|token| token.is_cancelled()));
    assert!(!runtime.host_state.borrow().fetch_loads.has_pending());
    release[0].send(()).unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime.eval("events.length").unwrap().as_number(),
        Some(0.0)
    );
}

#[test]
fn dedicated_worker_fetch_shares_its_creators_resource_budget() {
    let mut runtime = runtime();
    runtime.set_resource_queue(crate::resource_worker::ResourceQueue::new(
        std::num::NonZeroUsize::new(2).unwrap(),
    ));
    let (_, started, release) = held(&mut runtime);
    runtime.eval("fetch('/1').then(r=>r.text()).then(t=>events.push(t));fetch('/2').then(r=>r.text()).then(t=>events.push(t))").unwrap();
    runtime.run_until_idle().unwrap();
    for _ in 0..2 {
        started.recv_timeout(Duration::from_secs(5)).unwrap();
    }
    let last_owner_job = runtime
        .host_state
        .borrow()
        .fetch_loads
        .pending
        .values()
        .map(|pending| pending.task.id())
        .max()
        .unwrap();
    runtime.eval("globalThis.worker=new Worker('data:text/javascript,'+encodeURIComponent(\"fetch('https://fetch.test/3').then(r=>r.text()).then(t=>postMessage(t))\"));worker.onmessage=e=>events.push('worker:'+e.data)").unwrap();
    runtime.run_until_idle().unwrap();
    let worker = runtime
        .host_state
        .borrow()
        .workers
        .values()
        .next()
        .unwrap()
        .clone();
    let child_job = worker
        .borrow()
        .runtime
        .host_state
        .borrow()
        .fetch_loads
        .pending
        .values()
        .next()
        .unwrap()
        .task
        .id();
    assert_eq!(
        child_job,
        last_owner_job + 1,
        "worker must submit to the same queue"
    );
    assert_eq!(started.try_recv(), Err(mpsc::TryRecvError::Empty));
    release[1].send(()).unwrap();
    assert!(
        started
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .ends_with("/3")
    );
    release[2].send(()).unwrap();
    release[0].send(()).unwrap();
    pump_until(
        &mut runtime,
        "events.includes('one') && events.includes('two') && events.includes('worker:three')",
    );
    runtime.eval("worker.terminate()").unwrap();
}

#[test]
fn shared_worker_fetch_keeps_only_its_connected_owner_runnable_until_delivery() {
    let mut runtime = runtime();
    let (_, started, release) = held(&mut runtime);
    runtime.eval("globalThis.shared=new SharedWorker('data:text/javascript,'+encodeURIComponent(\"onconnect=e=>{const port=e.ports[0];fetch('https://fetch.test/1').then(r=>r.text()).then(t=>{port.postMessage(t);setTimeout(()=>port.postMessage('timer'),100)})}\"),'pending-fetch');shared.port.onmessage=e=>events.push(e.data)").unwrap();
    runtime.run_until_idle().unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(!runtime.host_state.borrow().fetch_loads.has_pending());
    assert_eq!(
        runtime.next_rendering(),
        crate::cdp::NextRendering::EveryFrame
    );

    let mut unrelated = self::runtime();
    unrelated.run_until_idle().unwrap();
    assert_eq!(unrelated.next_rendering(), crate::cdp::NextRendering::Idle);

    release[0].send(()).unwrap();
    pump_until(&mut runtime, "events.join(',')==='one'");
    assert_eq!(
        runtime.next_rendering(),
        crate::cdp::NextRendering::After(Duration::from_millis(100))
    );
    runtime.tick(100).unwrap();
    pump_until(&mut runtime, "events.join(',')==='one,timer'");
    assert_eq!(runtime.next_rendering(), crate::cdp::NextRendering::Idle);
    runtime.eval("shared.port.close()").unwrap();
}

#[test]
fn runtime_drop_cancels_fetch_even_when_native_host_state_is_retained() {
    let mut runtime = runtime();
    let (_, started, release) = held(&mut runtime);
    runtime
        .eval("fetch('/1').then(()=>events.push('loaded'))")
        .unwrap();
    runtime.run_until_idle().unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    let state = Rc::clone(&runtime.host_state);
    let tokens: Vec<_> = state
        .borrow()
        .fetch_loads
        .pending
        .values()
        .map(|pending| pending.scope.token())
        .collect();
    drop(runtime);
    assert!(!state.borrow().fetch_loads.has_pending());
    assert!(tokens.iter().all(|token| token.is_cancelled()));
    release[0].send(()).unwrap();
}

struct CookieProbe {
    seen: Mutex<Vec<String>>,
}

impl HttpTransport for CookieProbe {
    fn send(
        &self,
        request: &HttpRequest,
        _: Option<Duration>,
    ) -> Result<HttpResponse, HttpParseError> {
        self.seen
            .lock()
            .unwrap()
            .push(request.header("Cookie").unwrap_or("").to_owned());
        let cookie = if request.url().to_string().ends_with("/2") {
            "omitted=3; Path=/"
        } else {
            "reply=2; Path=/"
        };
        Ok(HttpResponse::new(
            200,
            "OK",
            vec![("Set-Cookie".into(), cookie.into())],
            b"body".to_vec(),
        ))
    }
}

#[test]
fn background_fetch_preserves_cookie_session_credentials_and_header_filtering() {
    let mut runtime = runtime();
    let transport = Arc::new(CookieProbe {
        seen: Mutex::new(Vec::new()),
    });
    runtime.set_http_resource_transport(Some(transport.clone()));
    runtime.eval("document.cookie='session=1; Path=/';fetch('/1').then(r=>{globalThis.hiddenCookie=!r.headers.has('set-cookie');return r.text()}).then(t=>events.push(t))").unwrap();
    pump_until(&mut runtime, "events.length===1");
    assert!(transport.seen.lock().unwrap()[0].contains("session=1"));
    assert_eq!(
        runtime
            .eval("hiddenCookie && document.cookie.includes('reply=2')")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    runtime
        .eval("fetch('/2',{credentials:'omit'}).then(()=>events.push('omitted'))")
        .unwrap();
    pump_until(&mut runtime, "events.length===2");
    assert_eq!(transport.seen.lock().unwrap()[1], "");
    assert_eq!(
        runtime
            .eval("!document.cookie.includes('omitted=3')")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}
