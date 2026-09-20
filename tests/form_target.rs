//! Form submission into an existing child browsing context.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread;
use std::time::Duration;

use omoikane::{html::TreeBuilder, js::JsRuntime};

#[derive(Debug)]
struct Request {
    line: String,
    content_type: String,
    body: Vec<u8>,
}

struct Server {
    origin: String,
    requests: mpsc::Receiver<Request>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Server {
    fn start() -> Self {
        Self::with_html(b"<!doctype html><title>received</title><p id=result>sent</p>")
    }

    fn with_html(html: &'static [u8]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let (send, requests) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = thread::spawn(move || {
            while !worker_stop.load(Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    Err(error) => panic!("accept: {error}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut reader = BufReader::new(&mut stream);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let mut content_type = String::new();
                let mut length = 0;
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    if header == "\r\n" || header.is_empty() {
                        break;
                    }
                    if let Some((key, value)) = header.split_once(':') {
                        if key.eq_ignore_ascii_case("content-length") {
                            length = value.trim().parse::<usize>().unwrap();
                        } else if key.eq_ignore_ascii_case("content-type") {
                            content_type = value.trim().to_string();
                        }
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                drop(reader);
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", html.len()).unwrap();
                stream.write_all(html).unwrap();
                let _ = send.send(Request {
                    line,
                    content_type,
                    body,
                });
            }
        });
        Self {
            origin,
            requests,
            stop,
            worker: Some(worker),
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.worker.take().unwrap().join().unwrap();
    }
}

fn runtime(server: &Server, method: &str) -> JsRuntime {
    let html = format!(
        "<iframe name=receiver srcdoc='<p>initial</p>'></iframe>\
         <form action=/receive target=receiver method={method}><x-value name=answer></x-value></form>"
    );
    let mut runtime = JsRuntime::with_document_and_url(
        TreeBuilder::parse(&html).document(),
        &format!("{}/owner", server.origin),
    )
    .unwrap();
    runtime.eval(r#"
        customElements.define('x-value', class extends HTMLElement {
            static formAssociated = true;
            constructor() { super(); this.i = this.attachInternals(); this.i.setFormValue('a b&c'); }
        });
        globalThis.frame = document.querySelector('iframe');
        globalThis.form = document.querySelector('form');
    "#).unwrap();
    runtime.run_until_idle().unwrap();
    runtime
        .eval(
            r#"
        globalThis.oldDocument = frame.contentDocument;
        globalThis.oldWindow = frame.contentWindow;
        globalThis.loads = 0;
        frame.onload = () => loads++;
        form.submit();
    "#,
        )
        .unwrap();
    assert!(
        runtime
            .eval("loads === 0 && frame.contentDocument === oldDocument")
            .unwrap()
            .to_boolean()
    );
    runtime
}

fn verify_target(runtime: &mut JsRuntime, server: &Server) -> Request {
    runtime.run_until_idle().unwrap();
    assert!(
        runtime.take_navigation_requests().is_empty(),
        "named iframe submission must not navigate the top-level page"
    );
    let request = server
        .requests
        .recv_timeout(Duration::from_secs(3))
        .unwrap();
    let actual = runtime
        .eval(
            r#"JSON.stringify({
        loads, topPath: location.pathname, sameWindow: frame.contentWindow === oldWindow,
        newDocument: frame.contentDocument !== oldDocument,
        text: frame.contentDocument.querySelector('#result')?.textContent,
        childPath: frame.contentWindow.location.pathname,
        locationString: String(frame.contentWindow.location) === frame.contentDocument.URL,
        src: frame.getAttribute('src'), srcdoc: frame.getAttribute('srcdoc')
    })"#,
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&actual).unwrap(),
        serde_json::json!({
            "loads": 1, "topPath": "/owner", "sameWindow": true, "newDocument": true,
            "text": "sent", "childPath": "/receive", "locationString": true, "src": null, "srcdoc": "<p>initial</p>"
        })
    );
    runtime
        .eval("frame.setAttribute('srcdoc', '<p id=reset>reset</p>')")
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.eval("loads === 2 && frame.contentDocument.querySelector('#reset').textContent === 'reset'").unwrap().to_boolean());
    request
}

#[test]
fn get_submission_replaces_only_the_named_iframe_document() {
    let server = Server::start();
    let mut runtime = runtime(&server, "get");
    let request = verify_target(&mut runtime, &server);
    assert_eq!(request.line.trim(), "GET /receive?answer=a+b%26c HTTP/1.1");
    assert!(request.body.is_empty());
}

#[test]
fn post_submission_delivers_the_body_to_the_named_iframe() {
    let server = Server::start();
    let mut runtime = runtime(&server, "post");
    let request = verify_target(&mut runtime, &server);
    assert_eq!(request.line.trim(), "POST /receive HTTP/1.1");
    assert_eq!(request.content_type, "application/x-www-form-urlencoded");
    assert_eq!(request.body, b"answer=a+b%26c");
}

fn make_runtime(server: &Server, html: &str) -> JsRuntime {
    let mut runtime = JsRuntime::with_document_and_url(
        TreeBuilder::parse(html).document(),
        &format!("{}/owner", server.origin),
    )
    .unwrap();
    runtime.run_until_idle().unwrap();
    runtime
}

fn check(runtime: &mut JsRuntime, source: &str) {
    assert!(
        runtime
            .eval(source)
            .unwrap_or_else(|error| panic!("{error}: {source}"))
            .to_boolean(),
        "{source}"
    );
}

#[test]
fn submitter_target_overrides_form_and_base_target_supplies_the_default() {
    let server = Server::start();
    let mut runtime = make_runtime(
        &server,
        "<base target=base><iframe name=base srcdoc='<p>initial</p>'></iframe><iframe name=chosen srcdoc='<p>initial</p>'></iframe><form action=/receive target=absent><button formtarget=chosen name=button value=yes>Send</button></form>",
    );
    runtime
        .eval("document.querySelector('form').requestSubmit(document.querySelector('button'))")
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        server
            .requests
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .line
            .trim(),
        "GET /receive?button=yes HTTP/1.1"
    );
    check(
        &mut runtime,
        "document.querySelector('iframe[name=chosen]').contentDocument.querySelector('#result') !== null && document.querySelector('iframe[name=base]').contentDocument.body.textContent === 'initial'",
    );
    runtime.eval("document.querySelector('form').removeAttribute('target'); document.querySelector('form').submit()").unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        server
            .requests
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .line
            .starts_with("GET /receive ")
    );
    check(
        &mut runtime,
        "document.querySelector('iframe[name=base]').contentDocument.querySelector('#result') !== null",
    );
    assert!(runtime.take_navigation_requests().is_empty());
}

#[test]
fn later_submission_or_attribute_navigation_replaces_a_queued_submission() {
    let server = Server::start();
    let mut runtime = make_runtime(
        &server,
        "<iframe name=receiver srcdoc='<p>initial</p>'></iframe><form action=/first target=receiver></form>",
    );
    runtime.eval("globalThis.f=document.querySelector('form'); f.submit(); f.action='/second'; f.submit()").unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        server
            .requests
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .line
            .trim(),
        "GET /second HTTP/1.1"
    );
    assert!(server.requests.try_recv().is_err());
    runtime.eval("f.action='/third'; f.submit(); document.querySelector('iframe').srcdoc='<p id=replacement>replacement</p>'").unwrap();
    runtime.run_until_idle().unwrap();
    check(
        &mut runtime,
        "document.querySelector('iframe').contentDocument.querySelector('#replacement') !== null",
    );
    assert!(server.requests.try_recv().is_err());
    assert!(runtime.take_navigation_requests().is_empty());
}

#[test]
fn source_iframe_defaults_to_self_and_sandbox_blocks_forms_or_top_navigation() {
    for (sandbox, target, allowed) in [
        ("", "", true),
        ("allow-same-origin", "", false),
        ("allow-same-origin allow-forms", "_self", true),
        ("allow-same-origin allow-forms", "_top", false),
    ] {
        let server = Server::start();
        let attribute = if sandbox.is_empty() {
            String::new()
        } else {
            format!("sandbox='{sandbox}'")
        };
        let mut runtime = make_runtime(
            &server,
            &format!(
                "<iframe {attribute} srcdoc=\"<form action='/receive' target='{target}'><input name=answer value=child></form>\"></iframe>"
            ),
        );
        runtime
            .eval("document.querySelector('iframe').contentDocument.querySelector('form').submit()")
            .unwrap();
        runtime.run_until_idle().unwrap();
        assert!(
            runtime.take_navigation_requests().is_empty(),
            "sandbox={sandbox} target={target}"
        );
        if allowed {
            assert_eq!(
                server
                    .requests
                    .recv_timeout(Duration::from_secs(3))
                    .unwrap()
                    .line
                    .trim(),
                "GET /receive?answer=child HTTP/1.1"
            );
            check(
                &mut runtime,
                "document.querySelector('iframe').contentDocument.querySelector('#result') !== null",
            );
        } else {
            assert!(server.requests.try_recv().is_err());
            check(
                &mut runtime,
                "document.querySelector('iframe').contentDocument.querySelector('form') !== null",
            );
        }
    }
}

#[test]
fn cross_origin_submission_preserves_proxy_but_blocks_document_access() {
    let source = Server::start();
    let destination = Server::start();
    let mut runtime = make_runtime(
        &source,
        &format!(
            "<iframe name=receiver srcdoc='<p>initial</p>'></iframe><form action='{}/receive' target=receiver></form>",
            destination.origin
        ),
    );
    runtime.eval("globalThis.frame=document.querySelector('iframe'); globalThis.proxy=frame.contentWindow; document.querySelector('form').submit()").unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        destination
            .requests
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .line
            .starts_with("GET /receive ")
    );
    check(
        &mut runtime,
        "frame.contentWindow === proxy && frame.contentDocument === null",
    );
    check(
        &mut runtime,
        "(()=>{try {return proxy.location.pathname, false} catch(e) {return e.name === 'SecurityError'}})()",
    );
    assert!(runtime.take_navigation_requests().is_empty());
}

#[test]
fn iframe_history_replays_the_submitted_request_without_using_stale_srcdoc() {
    for method in ["get", "post"] {
        let server = Server::start();
        let mut runtime = runtime(&server, method);
        runtime.run_until_idle().unwrap();
        let initial = server
            .requests
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        check(
            &mut runtime,
            "oldWindow.history.length === 2 && oldWindow.document.querySelector('#result') !== null",
        );
        runtime.eval("oldWindow.history.back()").unwrap();
        runtime.run_until_idle().unwrap();
        check(
            &mut runtime,
            "oldWindow.document.body.textContent === 'initial'",
        );
        runtime.eval("oldWindow.history.forward()").unwrap();
        runtime.run_until_idle().unwrap();
        check(
            &mut runtime,
            "oldWindow.document.querySelector('#result') !== null",
        );
        let replay = server
            .requests
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        assert_eq!(replay.line, initial.line);
        assert_eq!(replay.body, initial.body);
        assert_eq!(replay.content_type, initial.content_type);
        runtime.eval("oldWindow.location.reload()").unwrap();
        runtime.run_until_idle().unwrap();
        let reload = server
            .requests
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        assert_eq!(reload.line, initial.line);
        assert_eq!(reload.body, initial.body);
        assert_eq!(reload.content_type, initial.content_type);
        check(
            &mut runtime,
            "oldWindow.history.length === 2 && oldWindow.document.querySelector('#result') !== null",
        );
    }
}

#[test]
fn window_name_tracks_the_browsing_context_for_form_targets() {
    let server = Server::start();
    let mut runtime = make_runtime(
        &server,
        "<iframe name=original srcdoc='<p>initial</p>'></iframe><form action=/named target=renamed></form>",
    );
    runtime.eval("globalThis.frame=document.querySelector('iframe'); globalThis.proxy=frame.contentWindow; globalThis.form=document.querySelector('form')").unwrap();
    check(&mut runtime, "proxy.name === 'original'");
    runtime.eval("proxy.name='renamed'; form.submit()").unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        server
            .requests
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .line
            .trim(),
        "GET /named HTTP/1.1"
    );
    check(
        &mut runtime,
        "proxy.name === 'renamed' && frame.name === 'original' && proxy.document.querySelector('#result') !== null",
    );
    runtime
        .eval("frame.name='attribute'; form.target='attribute'; form.submit()")
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(server.requests.recv_timeout(Duration::from_secs(3)).is_ok());
    check(&mut runtime, "proxy.name === 'attribute'");
    runtime
        .eval("proxy.name='discarded'; frame.remove(); document.body.appendChild(frame)")
        .unwrap();
    runtime.run_until_idle().unwrap();
    check(
        &mut runtime,
        "proxy.closed && frame.contentWindow.name === 'attribute'",
    );
    assert!(runtime.take_navigation_requests().is_empty());
}

#[test]
fn form_target_can_address_the_named_top_level_context() {
    let server = Server::start();
    let mut runtime = make_runtime(
        &server,
        "<form action=/top target=owner><input name=answer value=ok></form>",
    );
    runtime
        .eval("window.name='owner'; document.querySelector('form').submit()")
        .unwrap();
    runtime.run_until_idle().unwrap();
    let requests = runtime.take_navigation_requests();
    assert_eq!(requests.len(), 1);
    let omoikane::js::NavigationRequest::FormSubmit { url, .. } = &requests[0] else {
        panic!("{requests:?}")
    };
    assert_eq!(url, &format!("{}/top?answer=ok", server.origin));
    assert!(server.requests.try_recv().is_err());
}

fn check_initial_inline_scripts(via_http: bool) {
    const HTML: &[u8] = br#"<!doctype html><body><script>
        globalThis.order=['first']; globalThis.ownScript=document.currentScript.ownerDocument===document;
        let inert=document.createElement('div');inert.innerHTML='<script>globalThis.inertRan=true<\/script>';document.body.appendChild(inert);
        </script><script>throw Error('expected-child-error')</script><script>
        order.push('last');setTimeout(()=>order.push('timer'),0);
        </script>"#;
    let server = Server::with_html(HTML);
    let mut runtime = make_runtime(&server, "<iframe></iframe>");
    let script = if via_http {
        "document.querySelector('iframe').src='/child'".to_string()
    } else {
        format!(
            "document.querySelector('iframe').srcdoc={}",
            serde_json::to_string(std::str::from_utf8(HTML).unwrap()).unwrap()
        )
    };
    runtime.eval(&script).unwrap();
    runtime.run_until_idle().unwrap();
    runtime.tick(0).unwrap();
    runtime
        .eval("globalThis.child=document.querySelector('iframe').contentWindow")
        .unwrap();
    let state = runtime.eval("JSON.stringify({order:child.order, ownScript:child.ownScript, inertType:typeof child.inertRan, parentType:typeof globalThis.order})").unwrap().as_string().unwrap().to_std_string_escaped();
    let errors = runtime.take_task_errors();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&state).unwrap(),
        serde_json::json!({"order":["first","last","timer"],"ownScript":true,"inertType":"undefined","parentType":"undefined"}),
        "task errors: {errors:?}"
    );
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("expected-child-error"), "{errors:?}");
    runtime.run_until_idle().unwrap();
    check(&mut runtime, "child.order.length === 3");
}

#[test]
fn html_iframe_initial_srcdoc_scripts_run_in_order_once_in_the_child_realm() {
    check_initial_inline_scripts(false);
}

#[test]
fn html_iframe_initial_http_scripts_run_in_order_once_in_the_child_realm() {
    check_initial_inline_scripts(true);
}

#[test]
fn html_iframe_initial_scripts_obey_sandbox_csp_and_detach() {
    let server = Server::start();
    for (sandbox, markup, expected) in [
        (
            "allow-same-origin",
            "<script>document.body.setAttribute('data-ran','yes')</script>",
            false,
        ),
        (
            "allow-same-origin allow-scripts",
            "<script>document.body.setAttribute('data-ran','yes')</script>",
            true,
        ),
        (
            "allow-same-origin allow-scripts",
            "<meta http-equiv=Content-Security-Policy content=\"script-src 'none'\"><body><script>document.body.setAttribute('data-ran','yes')</script>",
            false,
        ),
    ] {
        let mut runtime = make_runtime(&server, "<iframe></iframe>");
        runtime.eval(&format!("globalThis.frame=document.querySelector('iframe');frame.setAttribute('sandbox',{});frame.srcdoc={}",serde_json::to_string(sandbox).unwrap(),serde_json::to_string(markup).unwrap())).unwrap();
        runtime.run_until_idle().unwrap();
        assert_eq!(
            runtime
                .eval("frame.contentDocument.body.getAttribute('data-ran') === 'yes'")
                .unwrap()
                .to_boolean(),
            expected,
            "{sandbox}: {markup}"
        );
    }
    let mut runtime = make_runtime(&server, "<iframe></iframe>");
    runtime.eval("globalThis.detachedLoads=0;globalThis.detachedFrame=document.querySelector('iframe');detachedFrame.onload=()=>detachedLoads++").unwrap();
    let markup = "<script>frameElement.remove();setTimeout(()=>parent.lateTimer=true,0)</script><script>parent.lateScript=true</script>";
    runtime
        .eval(&format!(
            "document.querySelector('iframe').srcdoc={}",
            serde_json::to_string(markup).unwrap()
        ))
        .unwrap();
    runtime.run_until_idle().unwrap();
    check(
        &mut runtime,
        "document.querySelector('iframe') === null && globalThis.lateScript === undefined && globalThis.lateTimer === undefined",
    );
    runtime.tick(0).unwrap();
    check(
        &mut runtime,
        "globalThis.lateTimer === undefined && detachedLoads === 0",
    );
    assert!(runtime.take_task_errors().is_empty());
}

#[test]
fn replacing_an_initial_iframe_document_discards_its_remaining_scripts_and_load() {
    let server = Server::start();
    let mut runtime = make_runtime(&server, "<iframe></iframe>");
    runtime.eval("globalThis.frame=document.querySelector('iframe'); globalThis.loads=0;frame.onload=()=>loads++").unwrap();
    let replacement = "<body><script>parent.newRan=true</script>";
    let initial = format!(
        "<body><script>parent.frame.srcdoc={};void parent.frame.contentDocument;</script><script>parent.oldRan=true</script>",
        serde_json::to_string(replacement)
            .unwrap()
            .replace("</script>", "<\\/script>")
    );
    runtime
        .eval(&format!(
            "frame.srcdoc={}",
            serde_json::to_string(&initial).unwrap()
        ))
        .unwrap();
    runtime.run_until_idle().unwrap();
    runtime.tick(0).unwrap();
    check(
        &mut runtime,
        "globalThis.newRan === true && globalThis.oldRan === undefined && loads === 1",
    );
    assert!(runtime.take_task_errors().is_empty());
}

#[test]
fn form_navigation_captures_the_departing_iframes_custom_control_state() {
    let server = Server::start();
    for submit_from_child in [false, true] {
        let mut runtime = JsRuntime::with_document_and_url(
            TreeBuilder::parse("<body><form target=receiver action=/received></form><iframe name=receiver></iframe></body>").document(),
            &format!("{}/owner", server.origin),
        ).unwrap();
        let html = r#"<form action=/received><x-restored name=answer></x-restored></form><script>
            globalThis.restorations=[];
            customElements.define('x-restored', class extends HTMLElement {
                static formAssociated=true;
                constructor() { super(); this.i=this.attachInternals(); this.i.setFormValue('initial'); }
                formStateRestoreCallback(value,mode) { restorations.push([value,mode]); this.i.setFormValue(value); }
            });
            globalThis.control=document.querySelector('x-restored');
            globalThis.send=()=>document.querySelector('form').submit();
        </script>"#;
        runtime.eval(&format!("globalThis.frame=document.querySelector('iframe'); frame.srcdoc={}; globalThis.proxy=frame.contentWindow", serde_json::to_string(html).unwrap())).unwrap();
        runtime.run_until_idle().unwrap();
        runtime
            .eval("proxy.control.i.setFormValue('saved'); proxy.name='receiver'")
            .unwrap();
        runtime
            .eval(if submit_from_child {
                "proxy.send()"
            } else {
                "document.querySelector('form').submit()"
            })
            .unwrap();
        runtime.run_until_idle().unwrap();
        assert!(runtime.take_navigation_requests().is_empty());
        server
            .requests
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        check(
            &mut runtime,
            "proxy.document.querySelector('#result') !== null",
        );
        runtime.eval("proxy.history.back()").unwrap();
        runtime.run_until_idle().unwrap();
        check(
            &mut runtime,
            "JSON.stringify(proxy.restorations) === '[[\"saved\",\"restore\"]]' && new proxy.FormData(proxy.control.i.form).get('answer') === 'saved'",
        );
    }
}

#[test]
fn cross_origin_child_reload_restores_state_without_exposing_its_document() {
    let child = Server::with_html(br#"<!doctype html><form><x-field name=answer></x-field></form><script>
        customElements.define('x-field',class extends HTMLElement {
            static formAssociated=true;
            constructor(){super();this.i=this.attachInternals();this.i.setFormValue('initial');}
            formStateRestoreCallback(value,mode){this.i.setFormValue(value);console.log(JSON.stringify([value,mode,new FormData(this.i.form).get('answer')]));}
        });
        if(sessionStorage.getItem('reloaded') !== 'yes') {
            sessionStorage.setItem('reloaded','yes');
            document.querySelector('x-field').i.setFormValue('cross origin saved');
            setTimeout(()=>location.reload(),0);
        }
    </script>"#);
    let owner = Server::start();
    let mut runtime = make_runtime(&owner, "<iframe></iframe>");
    runtime
        .eval(&format!(
            "globalThis.frame=document.querySelector('iframe'); frame.src={}",
            serde_json::to_string(&format!("{}/child", child.origin)).unwrap()
        ))
        .unwrap();
    runtime.run_until_idle().unwrap();
    runtime.tick(0).unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.take_navigation_requests().is_empty());
    let errors = runtime.take_task_errors();
    let mut requests = Vec::new();
    for _ in 0..2 {
        if let Ok(request) = child.requests.recv_timeout(Duration::from_secs(3)) {
            requests.push(request.line);
        } else {
            break;
        }
    }
    assert!(
        errors.is_empty(),
        "child task errors: {errors:?}; requests: {requests:?}"
    );
    assert_eq!(
        runtime.console_logs(),
        vec!["[\"cross origin saved\",\"restore\",\"cross origin saved\"]"]
    );
    assert_eq!(
        requests
            .iter()
            .filter(|line| line.starts_with("GET /child "))
            .count(),
        2,
        "{requests:?}"
    );
    check(
        &mut runtime,
        "frame.contentDocument === null && (()=>{try {void frame.contentWindow.document;return false}catch(e){return e.name==='SecurityError'}})()",
    );
    assert!(runtime.take_task_errors().is_empty());
}

#[test]
fn child_history_state_updates_its_document_and_location_urls() {
    let child = Server::with_html(br#"<!doctype html><script>
        const base=location.href+'?old=1#old';
        if(new URL('#new',base).href !== location.href+'?old=1#new' ||
           new URL('',base).href !== location.href+'?old=1') throw new Error('relative URL lost path or query');
        const state={step:2};
        history.pushState(state,'','?step=2#two');
        state.step=99;
        console.log(JSON.stringify([history.state.step,location.href,location.search,location.hash,document.URL,String(location)]));
    </script>"#);
    let other = Server::start();
    for owner in [&child, &other] {
        let mut runtime = make_runtime(owner, "<iframe></iframe>");
        runtime
            .eval(&format!(
                "globalThis.frame=document.querySelector('iframe'); frame.src={}",
                serde_json::to_string(&format!("{}/child", child.origin)).unwrap()
            ))
            .unwrap();
        runtime.run_until_idle().unwrap();
        assert!(runtime.take_navigation_requests().is_empty());
        let url = format!("{}/child?step=2#two", child.origin);
        let expected = serde_json::json!([2, url, "?step=2", "#two", url, url]).to_string();
        assert_eq!(runtime.console_logs(), vec![expected]);
        assert!(runtime.take_task_errors().is_empty());
    }
}

#[test]
fn iframe_location_search_setter_navigates_from_parent_and_child() {
    let server = Server::with_html(
        br#"<!doctype html><script>globalThis.changeQuery=()=>location.search='next=1';</script>"#,
    );
    for from_child in [false, true] {
        let mut runtime = make_runtime(&server, "<iframe></iframe>");
        runtime.eval(&format!("globalThis.frame=document.querySelector('iframe');frame.src={};globalThis.proxy=frame.contentWindow",serde_json::to_string(&format!("{}/child",server.origin)).unwrap())).unwrap();
        runtime.run_until_idle().unwrap();
        let first = server
            .requests
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        assert_eq!(first.line, "GET /child HTTP/1.1\r\n");
        runtime
            .eval(if from_child {
                "proxy.changeQuery()"
            } else {
                "proxy.location.search='next=1'"
            })
            .unwrap();
        runtime.run_until_idle().unwrap();
        let second = server
            .requests
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        assert_eq!(second.line, "GET /child?next=1 HTTP/1.1\r\n");
        assert!(runtime.take_navigation_requests().is_empty());
        check(
            &mut runtime,
            "proxy.location.search === '?next=1' && proxy.location.pathname === '/child'",
        );
    }
}

#[test]
fn departing_child_history_cannot_modify_the_replacement_document() {
    let server = Server::start();
    let mut runtime = make_runtime(&server, "<iframe></iframe>");
    runtime.eval("globalThis.frame=document.querySelector('iframe');frame.srcdoc='<script>globalThis.pushEntry=()=>history.pushState({stale:true},\"\")<\\/script>';globalThis.proxy=frame.contentWindow").unwrap();
    runtime.run_until_idle().unwrap();
    runtime.eval("globalThis.previousLength=proxy.history.length;globalThis.oldPush=proxy.pushEntry;frame.srcdoc='<p>replacement</p>';try {oldPush()} catch (_) {}").unwrap();
    runtime.run_until_idle().unwrap();
    let actual = runtime.eval("JSON.stringify([proxy.document.body.textContent, proxy.history.state, proxy.history.length-previousLength])").unwrap().as_string().unwrap().to_std_string_escaped();
    assert_eq!(actual, r#"["replacement",null,1]"#);
    assert!(runtime.take_navigation_requests().is_empty());
    assert!(runtime.take_task_errors().is_empty());
}

#[test]
fn iframe_form_action_uses_the_child_document_base_url() {
    let child=Server::with_html(br#"<!doctype html><base href="/forms/deep/"><form action="../submit" method=post><x-field name=answer></x-field></form><script>
        customElements.define('x-field',class extends HTMLElement {
            static formAssociated=true;
            constructor(){super();this.i=this.attachInternals();this.i.setFormValue('child value');}
        });
        if(location.pathname === '/forms/page') setTimeout(()=>document.querySelector('form').submit(),0);
    </script>"#);
    let other = Server::start();
    for owner in [&child, &other] {
        let mut runtime = make_runtime(owner, "<iframe></iframe>");
        runtime
            .eval(&format!(
                "document.querySelector('iframe').src={}",
                serde_json::to_string(&format!("{}/forms/page", child.origin)).unwrap()
            ))
            .unwrap();
        runtime.run_until_idle().unwrap();
        runtime.tick(0).unwrap();
        runtime.run_until_idle().unwrap();
        let first = child.requests.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(first.line, "GET /forms/page HTTP/1.1\r\n");
        let second = child.requests.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(second.line, "POST /forms/submit HTTP/1.1\r\n");
        assert_eq!(second.body, b"answer=child+value");
        assert_eq!(second.content_type, "application/x-www-form-urlencoded");
        assert!(runtime.take_navigation_requests().is_empty());
        assert!(runtime.take_task_errors().is_empty());
    }
}

#[test]
fn parent_submits_a_child_form_without_action_to_the_child_url() {
    let server = Server::with_html(
        b"<!doctype html><form method=post><input name=answer value=child></form>",
    );
    let mut runtime = make_runtime(&server, "<iframe></iframe>");
    runtime
        .eval(&format!(
            "globalThis.frame=document.querySelector('iframe');frame.src={}",
            serde_json::to_string(&format!("{}/forms/page", server.origin)).unwrap()
        ))
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        server
            .requests
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .line,
        "GET /forms/page HTTP/1.1\r\n"
    );
    runtime
        .eval("frame.contentDocument.querySelector('form').submit()")
        .unwrap();
    runtime.run_until_idle().unwrap();
    let sent = server
        .requests
        .recv_timeout(Duration::from_secs(3))
        .unwrap();
    assert_eq!(sent.line, "POST /forms/page HTTP/1.1\r\n");
    assert_eq!(sent.body, b"answer=child");
    assert!(runtime.take_navigation_requests().is_empty());
    assert!(runtime.take_task_errors().is_empty());
}
