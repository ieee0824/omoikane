use super::*;
use crate::http::{HttpParseError, HttpResponse, HttpTransport};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

struct HeldScripts {
    entered: mpsc::Sender<String>,
    gates: [Mutex<mpsc::Receiver<()>>; 2],
    requests: AtomicUsize,
}

impl HttpTransport for HeldScripts {
    fn send(
        &self,
        request: &HttpRequest,
        _: Option<Duration>,
    ) -> Result<HttpResponse, HttpParseError> {
        let url = request.url().to_string();
        let index = usize::from(url.ends_with("/2.js"));
        self.requests.fetch_add(1, Ordering::SeqCst);
        self.entered.send(url).unwrap();
        self.gates[index]
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        Ok(HttpResponse::new(
            200,
            "OK",
            vec![("Content-Type".into(), "text/javascript".into())],
            format!("executed.push({});", index + 1).into_bytes(),
        ))
    }
}

fn runtime() -> JsRuntime {
    let document = crate::html::TreeBuilder::parse("<body>text</body>").document();
    let mut runtime = JsRuntime::with_document_and_url(document, "https://scripts.test/").unwrap();
    runtime.set_nonblocking_script_loading(true);
    runtime
        .eval("globalThis.executed=[];globalThis.loaded=[];globalThis.errors=[];globalThis.ticks=0")
        .unwrap();
    runtime
}

fn held(
    runtime: &mut JsRuntime,
) -> (
    Arc<HeldScripts>,
    mpsc::Receiver<String>,
    [mpsc::Sender<()>; 2],
) {
    let (entered, started) = mpsc::channel();
    let (first, gate1) = mpsc::channel();
    let (second, gate2) = mpsc::channel();
    let transport = Arc::new(HeldScripts {
        entered,
        gates: [Mutex::new(gate1), Mutex::new(gate2)],
        requests: AtomicUsize::new(0),
    });
    runtime.set_http_resource_transport(Some(transport.clone()));
    (transport, started, [first, second])
}

fn insert(runtime: &mut JsRuntime, asynchronous: bool) {
    runtime
        .eval(&format!(
            r#"for(let n of [1,2]){{
        let s=document.createElement('script');s.async={asynchronous};s.src='/'+n+'.js';
        s.onload=()=>loaded.push(n);s.onerror=()=>errors.push(n);document.body.append(s);
    }}setTimeout(()=>ticks++,0)"#
        ))
        .unwrap();
    runtime.run_until_idle().unwrap();
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
            "script completion did not satisfy {source}"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn both_started(started: &mpsc::Receiver<String>) {
    let mut urls = [
        started.recv_timeout(Duration::from_secs(5)).unwrap(),
        started.recv_timeout(Duration::from_secs(5)).unwrap(),
    ];
    urls.sort();
    assert!(urls[0].ends_with("/1.js"));
    assert!(urls[1].ends_with("/2.js"));
}

#[test]
fn held_async_scripts_fetch_concurrently_keep_timers_responsive_and_survive_gc() {
    let mut runtime = runtime();
    let (transport, started, release) = held(&mut runtime);
    insert(&mut runtime, true);
    both_started(&started);
    runtime.tick(1).unwrap();
    assert_eq!(runtime.eval("ticks").unwrap().as_number(), Some(1.0));
    assert_eq!(
        runtime
            .eval("executed.length+loaded.length")
            .unwrap()
            .as_number(),
        Some(0.0)
    );
    runtime.context.clear_kept_objects();
    boa_gc::force_collect();
    release[1].send(()).unwrap();
    pump_until(
        &mut runtime,
        "loaded.join(',')==='2' && executed.join(',')==='2'",
    );
    release[0].send(()).unwrap();
    pump_until(
        &mut runtime,
        "loaded.join(',')==='2,1' && executed.join(',')==='2,1'",
    );
    assert_eq!(transport.requests.load(Ordering::SeqCst), 2);
    assert!(!runtime.host_state.borrow().script_loads.has_pending());
}

#[test]
fn explicitly_ordered_scripts_fetch_concurrently_but_execute_in_insertion_order() {
    let mut runtime = runtime();
    let (_, started, release) = held(&mut runtime);
    insert(&mut runtime, false);
    both_started(&started);
    release[1].send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        runtime.run_until_idle().unwrap();
        if runtime
            .host_state
            .borrow()
            .script_loads
            .pending
            .values()
            .any(|load| load.prepared.script.1.ends_with("2.js") && load.ready.is_some())
        {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        runtime
            .eval("executed.length+loaded.length")
            .unwrap()
            .as_number(),
        Some(0.0)
    );
    release[0].send(()).unwrap();
    pump_until(
        &mut runtime,
        "executed.join(',')==='1,2' && loaded.join(',')==='1,2'",
    );
}

#[test]
fn request_csp_blocks_before_worker_transport_starts() {
    let mut runtime = runtime();
    let (transport, started, _) = held(&mut runtime);
    runtime.install_csp_policy(&["script-src 'none'".into()]);
    insert(&mut runtime, true);
    assert_eq!(transport.requests.load(Ordering::SeqCst), 0);
    assert_eq!(started.try_recv(), Err(mpsc::TryRecvError::Empty));
    assert!(!runtime.host_state.borrow().script_loads.has_pending());
}

#[test]
fn runtime_drop_cancels_scripts_even_when_host_state_is_retained() {
    let mut runtime = runtime();
    let (_, started, release) = held(&mut runtime);
    insert(&mut runtime, true);
    both_started(&started);
    let state = Rc::clone(&runtime.host_state);
    let tokens: Vec<_> = state
        .borrow()
        .script_loads
        .pending
        .values()
        .map(|load| load._scope.token())
        .collect();
    drop(runtime);
    assert!(!state.borrow().script_loads.has_pending());
    assert!(tokens.iter().all(|token| token.is_cancelled()));
    for gate in release {
        gate.send(()).unwrap();
    }
}

#[test]
fn cross_origin_anonymous_script_without_cors_fires_error_without_execution() {
    let mut runtime = runtime();
    let (_, started, release) = held(&mut runtime);
    runtime.eval("let s=document.createElement('script');s.async=true;s.crossOrigin='anonymous';s.src='https://other.test/1.js';s.onload=()=>loaded.push(1);s.onerror=()=>errors.push(1);document.body.append(s)").unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        started.recv_timeout(Duration::from_secs(5)).unwrap(),
        "https://other.test/1.js"
    );
    release[0].send(()).unwrap();
    pump_until(&mut runtime, "errors.join(',')==='1'");
    assert_eq!(
        runtime
            .eval("executed.length+loaded.length")
            .unwrap()
            .as_number(),
        Some(0.0)
    );
}

#[test]
fn iframe_retirement_cancels_pending_script_and_suppresses_completion() {
    let mut runtime = runtime();
    let (_, started, release) = held(&mut runtime);
    runtime.eval("globalThis.frame=document.createElement('iframe');frame.srcdoc='<body>child</body>';document.body.append(frame)").unwrap();
    runtime.run_until_idle().unwrap();
    runtime.eval("frame.contentWindow.executed=executed;let s=frame.contentDocument.createElement('script');s.async=true;s.src='/1.js';s.onload=()=>loaded.push(1);s.onerror=()=>errors.push(1);frame.contentDocument.body.append(s)").unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        started
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .ends_with("/1.js")
    );
    let tokens: Vec<_> = runtime
        .host_state
        .borrow()
        .script_loads
        .pending
        .values()
        .map(|load| load._scope.token())
        .collect();
    assert_eq!(tokens.len(), 1);
    runtime.eval("frame.remove()").unwrap();
    assert!(tokens.iter().all(|token| token.is_cancelled()));
    assert!(!runtime.host_state.borrow().script_loads.has_pending());
    release[0].send(()).unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("executed.length+loaded.length+errors.length")
            .unwrap()
            .as_number(),
        Some(0.0)
    );
}

#[test]
fn script_force_async_distinguishes_creation_parser_idl_and_attribute_changes() {
    let document = crate::html::TreeBuilder::parse(
        "<body><script id='parsed' type='application/json'>{}</script></body>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
        const s=document.createElement('script');
        if(!s.async || s.hasAttribute('async')) return false;
        s.async=false;
        if(s.async || s.hasAttribute('async')) return false;
        if(!s.cloneNode().async) return false;
        s.async=true;
        if(!s.async || !s.hasAttribute('async')) return false;
        s.removeAttribute('async');
        if(s.async) return false;
        const a=document.createElement('script');
        a.setAttribute('async','');a.removeAttribute('async');
        if(a.async) return false;
        const n=document.createElement('script');
        n.setAttributeNS(null,'async','');n.removeAttributeNS(null,'async');
        if(n.async) return false;
        const parsed=document.getElementById('parsed');
        return parsed.async===false && parsed.cloneNode().async===true;
    })()"#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn scripts_created_without_an_async_assignment_execute_as_soon_as_ready() {
    let mut runtime = runtime();
    let (_, started, release) = held(&mut runtime);
    runtime.eval("for(let n of [1,2]){let s=document.createElement('script');s.src='/'+n+'.js';s.onload=()=>loaded.push(n);document.body.append(s)}").unwrap();
    runtime.run_until_idle().unwrap();
    both_started(&started);
    release[1].send(()).unwrap();
    pump_until(
        &mut runtime,
        "executed.join(',')==='2' && loaded.join(',')==='2'",
    );
    release[0].send(()).unwrap();
    pump_until(
        &mut runtime,
        "executed.join(',')==='2,1' && loaded.join(',')==='2,1'",
    );
}

#[test]
fn script_csp_completion_uses_the_nonce_captured_before_fetch() {
    let mut runtime = runtime();
    let (_, started, release) = held(&mut runtime);
    runtime.install_csp_policy(&["script-src 'nonce-allowed'".into()]);
    runtime.eval("globalThis.script=document.createElement('script');script.async=true;script.setAttribute('nonce','allowed');script.src='/1.js';script.onload=()=>loaded.push(1);script.onerror=()=>errors.push(1);document.body.append(script)").unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        started
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .ends_with("/1.js")
    );
    runtime
        .eval("script.setAttribute('nonce','changed')")
        .unwrap();
    release[0].send(()).unwrap();
    pump_until(
        &mut runtime,
        "executed.join(',')==='1' && loaded.join(',')==='1'",
    );
    assert_eq!(
        runtime.eval("errors.length").unwrap().as_number(),
        Some(0.0)
    );
}

#[test]
fn removing_a_prepared_script_does_not_cancel_its_already_queued_completion() {
    let mut runtime = runtime();
    let (_, started, release) = held(&mut runtime);
    runtime.install_csp_policy(&["script-src 'self'".into()]);
    runtime.eval("globalThis.script=document.createElement('script');script.src='/1.js';script.onload=()=>loaded.push(1);document.body.append(script)").unwrap();
    runtime.run_until_idle().unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    release[0].send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        runtime.host_state.borrow_mut().poll_script_loads();
        if runtime
            .host_state
            .borrow()
            .script_loads
            .pending
            .values()
            .any(|load| load.queued)
        {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    runtime.eval("script.remove()").unwrap();
    pump_until(
        &mut runtime,
        "executed.join(',')==='1' && loaded.join(',')==='1'",
    );
    assert!(!runtime.host_state.borrow().script_loads.has_pending());
}

struct RedirectAfterGate {
    entered: mpsc::Sender<()>,
    gate: Mutex<mpsc::Receiver<()>>,
    requests: AtomicUsize,
}

impl HttpTransport for RedirectAfterGate {
    fn send(
        &self,
        request: &HttpRequest,
        _: Option<Duration>,
    ) -> Result<HttpResponse, HttpParseError> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        if request
            .url()
            .to_string()
            .starts_with("https://scripts.test/")
        {
            self.entered.send(()).unwrap();
            self.gate
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            Ok(HttpResponse::new(
                302,
                "Found",
                vec![("Location".into(), "https://other.test/final.js".into())],
                Vec::new(),
            ))
        } else {
            Ok(HttpResponse::new(
                200,
                "OK",
                vec![("Content-Type".into(), "text/javascript".into())],
                b"executed.push(1)".to_vec(),
            ))
        }
    }
}

#[test]
fn detaching_a_fetching_script_cannot_bypass_its_documents_redirect_csp_check() {
    let mut runtime = runtime();
    let (entered, started) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let transport = Arc::new(RedirectAfterGate {
        entered,
        gate: Mutex::new(gate),
        requests: AtomicUsize::new(0),
    });
    runtime.set_http_resource_transport(Some(transport.clone()));
    runtime.install_csp_policy(&["script-src 'self'".into()]);
    runtime.eval("globalThis.script=document.createElement('script');script.src='/redirect.js';script.onload=()=>loaded.push(1);script.onerror=()=>errors.push(1);document.body.append(script)").unwrap();
    runtime.run_until_idle().unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    runtime.eval("script.remove()").unwrap();
    release.send(()).unwrap();
    pump_until(&mut runtime, "errors.join(',')==='1'");
    assert_eq!(
        runtime
            .eval("executed.length+loaded.length")
            .unwrap()
            .as_number(),
        Some(0.0)
    );
    assert_eq!(transport.requests.load(Ordering::SeqCst), 2);
}
