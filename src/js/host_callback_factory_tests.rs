//! Actual factory regressions for the private stylesheet callback.
//! Fixture lifecycle cases derive from the reviewed Issue #1697 stage2a probes.
use crate::html::TreeBuilder;
use crate::http::{HttpParseError, HttpRequest, HttpResponse, HttpTransport};
use crate::js::JsRuntime;
use serde_json::{Value, json};
use std::{collections::BTreeMap, io::Cursor, sync::Arc, time::Duration};

const OBSERVER: &str = r#"(factory, phase) => {
    const name = '__omoikane_flush_stylesheets';
    let descriptorAbsent = true;
    let current = globalThis;
    const seen = new Set();
    while (current !== null) {
        if (seen.has(current)) throw new Error('cyclic global prototype');
        seen.add(current);
        if (Object.getOwnPropertyDescriptor(current, name) !== undefined) descriptorAbsent = false;
        current = Object.getPrototypeOf(current);
    }
    return {factory, phase, descriptorAbsent,
        propertyAbsent: globalThis[name] === undefined,
        identifierAbsent: typeof __omoikane_flush_stylesheets === 'undefined',
        inAbsent: !(name in globalThis)};
}"#;

fn observation(factory: &str, phase: &str) -> String {
    format!("({OBSERVER})({},{})", json!(factory), json!(phase))
}

struct Fixture(BTreeMap<String, (String, String)>);
impl HttpTransport for Fixture {
    fn send(
        &self,
        request: &HttpRequest,
        _: Option<Duration>,
    ) -> Result<HttpResponse, HttpParseError> {
        let response = self.0.get(request.url().path());
        let (status, mime, body) = response
            .map_or(("404 Not Found", "text/plain", ""), |(mime, body)| {
                ("200 OK", mime.as_str(), body.as_str())
            });
        HttpResponse::parse(&mut Cursor::new(
            format!(
                "HTTP/1.1 {status}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .into_bytes(),
        ))
    }
}

fn page(script: &str, routes: BTreeMap<String, (String, String)>) -> JsRuntime {
    let script = script.replace("</script", "<\\/script");
    let html = format!("<html><body><script>{script}</script></body></html>");
    let document = TreeBuilder::parse(&html).document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "http://localhost/private-flush-factory/page")
            .unwrap();
    runtime.set_http_resource_transport(Some(Arc::new(Fixture(routes))));
    let errors = runtime.execute_document_scripts(None);
    assert!(errors.is_empty(), "author startup: {errors:?}");
    runtime.run_until_idle().unwrap();
    runtime
}

fn reports(runtime: &mut JsRuntime) -> Value {
    let result = runtime.eval("JSON.stringify(sakuraReports)").unwrap();
    serde_json::from_str(&result.as_string().unwrap().to_std_string_escaped()).unwrap()
}

fn expected_phases(case: &str) -> &'static [&'static str] {
    match case {
        "main-window" => &["author-script", "after-document-lifecycle"],
        "main-resource" => &[
            "before-resource",
            "external-script-author",
            "resource-load-event",
        ],
        "dedicated-worker" => &[
            "initial-author",
            "message-event",
            "message-microtask",
            "next-timer-task",
        ],
        "shared-worker" => &[
            "connect-event",
            "message-event",
            "message-microtask",
            "next-timer-task",
        ],
        "paint-worklet" | "audio-worklet" => &["module-author", "module-microtask"],
        "service-worker" => &["initial-author", "message-event"],
        "auxiliary-recreated" | "iframe-srcdoc-recreated" => &[
            "generation-1-author",
            "generation-1-message",
            "generation-2-author",
            "generation-2-message",
        ],
        "iframe-srcdoc" | "iframe-blank" | "iframe-src" | "iframe-src-cross-origin" => {
            &["initial-child-author", "message-after-load"]
        }
        _ => panic!("unknown factory case: {case}"),
    }
}

fn save(case: &str, value: &Value, expected: usize) {
    let rows = value.as_array().expect("author reports array");
    assert_eq!(
        rows.len(),
        expected,
        "missing author result for {case}: {value}"
    );
    let factory = match case {
        "main-resource" => "main-window",
        "auxiliary-recreated" => "auxiliary-window",
        _ => case,
    };
    let expected_phases = expected_phases(case);
    assert_eq!(expected_phases.len(), expected);
    let expected_phases: std::collections::BTreeSet<_> = expected_phases.iter().copied().collect();
    let mut observed_phases = std::collections::BTreeSet::new();
    for row in rows {
        assert_eq!(row["factory"], factory, "wrong factory for {case}: {row}");
        let phase = row["phase"].as_str().expect("author phase string");
        assert!(
            observed_phases.insert(phase),
            "duplicate phase for {case}: {phase}"
        );
        // Auxiliary Window's old `in` bug (#1699) cannot make this pass alone:
        // descriptors, property access and the bare name are all required too.
        for route in [
            "descriptorAbsent",
            "propertyAbsent",
            "identifierAbsent",
            "inAbsent",
        ] {
            assert_eq!(row[route], true, "{case} {route}: {row}");
        }
    }
    assert_eq!(observed_phases, expected_phases, "missing phase for {case}");
}

#[test]
fn observe_main_window() {
    let first = observation("main-window", "author-script");
    let mut runtime = page(
        &format!("globalThis.sakuraReports=[{first}];"),
        BTreeMap::new(),
    );
    runtime
        .eval(&format!(
            "sakuraReports.push({});",
            observation("main-window", "after-document-lifecycle")
        ))
        .unwrap();
    save("main-window", &reports(&mut runtime), 2);
}

#[test]
fn observe_dedicated_worker() {
    let source = format!(
        "postMessage({});onmessage=()=>{{postMessage({});queueMicrotask(()=>postMessage({}));setTimeout(()=>postMessage({}),0);}};",
        observation("dedicated-worker", "initial-author"),
        observation("dedicated-worker", "message-event"),
        observation("dedicated-worker", "message-microtask"),
        observation("dedicated-worker", "next-timer-task")
    );
    let script = format!(
        "globalThis.sakuraReports=[];const worker=new Worker('data:text/javascript,'+encodeURIComponent({}));worker.onmessage=e=>sakuraReports.push(e.data);worker.postMessage('observe');",
        json!(source)
    );
    let mut runtime = page(&script, BTreeMap::new());
    for _ in 0..8 {
        runtime.tick(16).unwrap();
    }
    save("dedicated-worker", &reports(&mut runtime), 4);
}

#[test]
fn observe_shared_worker() {
    let source = format!(
        "onconnect=e=>{{const p=e.ports[0];p.postMessage({});p.onmessage=()=>{{p.postMessage({});queueMicrotask(()=>p.postMessage({}));setTimeout(()=>p.postMessage({}),0);}};}};",
        observation("shared-worker", "connect-event"),
        observation("shared-worker", "message-event"),
        observation("shared-worker", "message-microtask"),
        observation("shared-worker", "next-timer-task")
    );
    let script = format!(
        "globalThis.sakuraReports=[];const worker=new SharedWorker('data:text/javascript,'+encodeURIComponent({}),'private-flush-factory');worker.port.onmessage=e=>sakuraReports.push(e.data);worker.port.start();worker.port.postMessage('observe');",
        json!(source)
    );
    let mut runtime = page(&script, BTreeMap::new());
    for _ in 0..8 {
        runtime.tick(16).unwrap();
    }
    save("shared-worker", &reports(&mut runtime), 4);
}

// Observe actual addModule execution and its microtask, not paint/process callbacks.
fn worklet(factory: &str, audio: bool) {
    let source = format!(
        "registerWorklet(JSON.stringify({}));queueMicrotask(()=>registerWorklet(JSON.stringify({})));",
        observation(factory, "module-author"),
        observation(factory, "module-microtask")
    );
    let object = if audio {
        "(new OfflineAudioContext(1,128,48000)).audioWorklet"
    } else {
        "CSS.paintWorklet"
    };
    let script = format!(
        "globalThis.sakuraReports=[];const worklet={object};worklet.addModule('data:text/javascript,'+encodeURIComponent({})).then(()=>{{sakuraReports=worklet.registeredNames.map(name=>JSON.parse(name));}});",
        json!(source)
    );
    let mut runtime = page(&script, BTreeMap::new());
    save(factory, &reports(&mut runtime), 2);
}

#[test]
fn observe_paint_worklet() {
    worklet("paint-worklet", false);
}

#[test]
fn observe_audio_worklet() {
    worklet("audio-worklet", true);
}

#[test]
fn observe_service_worker() {
    let source = format!(
        "const initial={};onmessage=e=>{{e.source.postMessage(initial);e.source.postMessage({});}};",
        observation("service-worker", "initial-author"),
        observation("service-worker", "message-event")
    );
    let routes = BTreeMap::from([(
        "/private-flush-factory/sw.js".into(),
        ("text/javascript".into(), source),
    )]);
    let script = "globalThis.sakuraReports=[];globalThis.sakuraStartupError='';navigator.serviceWorker.onmessage=e=>sakuraReports.push(e.data);navigator.serviceWorker.register('/private-flush-factory/sw.js').then(r=>globalThis.sakuraRegistration=r).catch(e=>sakuraStartupError=String(e));";
    let mut runtime = page(script, routes);
    let diagnostics = runtime.eval("JSON.stringify({error:sakuraStartupError,registration:typeof sakuraRegistration,active:typeof sakuraRegistration!=='undefined'&&sakuraRegistration.active?.state,installing:typeof sakuraRegistration!=='undefined'&&sakuraRegistration.installing?.state})").unwrap();
    eprintln!(
        "service-worker startup: {}",
        diagnostics.as_string().unwrap().to_std_string_escaped()
    );
    runtime.eval("if(sakuraStartupError)throw new Error(sakuraStartupError);(sakuraRegistration.active||sakuraRegistration.installing||sakuraRegistration.waiting).postMessage('observe');").unwrap();
    runtime.run_until_idle().unwrap();
    save("service-worker", &reports(&mut runtime), 2);
}

fn iframe_case(kind: &str, cross_origin: bool) {
    let factory = format!(
        "iframe-{kind}{}",
        if cross_origin { "-cross-origin" } else { "" }
    );
    let child_source = format!(
        "parent.postMessage({},'*');addEventListener('message',()=>parent.postMessage({},'*'));",
        observation(&factory, "initial-child-author"),
        observation(&factory, "message-after-load")
    );
    let child_html = format!("<html><body><script>{child_source}</script></body></html>");
    let receiver = "globalThis.sakuraReports=[];addEventListener('message',e=>{let result;try{void frame.contentWindow.document;result='allowed';}catch(error){result=error.name;}e.data.parentDocumentAccess=result;sakuraReports.push(e.data);});const frame=document.createElement('iframe');";
    let (setup, routes) = match kind {
        "srcdoc" => (
            format!(
                "frame.onload=()=>frame.contentWindow.postMessage('observe','*');frame.srcdoc={};document.body.appendChild(frame);",
                json!(child_html)
            ),
            BTreeMap::new(),
        ),
        "blank" => (
            format!(
                "frame.onload=()=>{{frame.onload=null;frame.contentWindow.eval({});frame.contentWindow.postMessage('observe','*');}};frame.src='about:blank';document.body.appendChild(frame);",
                json!(child_source)
            ),
            BTreeMap::new(),
        ),
        "src" => {
            let host = if cross_origin {
                "other.test"
            } else {
                "localhost"
            };
            (
                format!(
                    "frame.onload=()=>frame.contentWindow.postMessage('observe','*');frame.src='http://{host}/child';document.body.appendChild(frame);"
                ),
                BTreeMap::from([("/child".into(), ("text/html".into(), child_html))]),
            )
        }
        _ => unreachable!(),
    };
    let mut runtime = page(&format!("{receiver}{setup}"), routes);
    for _ in 0..8 {
        runtime.tick(16).unwrap();
    }
    let values = reports(&mut runtime);
    for value in values.as_array().unwrap() {
        assert_eq!(
            value["parentDocumentAccess"],
            if cross_origin {
                "SecurityError"
            } else {
                "allowed"
            }
        );
    }
    save(&factory, &values, 2);
}

#[test]
fn observe_iframe_srcdoc() {
    iframe_case("srcdoc", false);
}
#[test]
fn observe_iframe_blank() {
    iframe_case("blank", false);
}
#[test]
fn observe_iframe_src_same_origin() {
    iframe_case("src", false);
}
#[test]
fn observe_iframe_src_cross_origin() {
    iframe_case("src", true);
}

#[test]
fn observe_auxiliary_recreated() {
    let child_source = |generation| {
        format!(
            "opener.postMessage({},'*');onmessage=()=>opener.postMessage({},'*');",
            observation(
                "auxiliary-window",
                &format!("generation-{generation}-author")
            ),
            observation(
                "auxiliary-window",
                &format!("generation-{generation}-message")
            )
        )
    };
    let setup = format!(
        "globalThis.sakuraReports=[];addEventListener('message',e=>sakuraReports.push(e.data));globalThis.popup=window.open('','sakura-popup');popup.eval({});popup.postMessage('observe','*');",
        json!(child_source(1))
    );
    let mut runtime = page(&setup, BTreeMap::new());
    assert_eq!(reports(&mut runtime).as_array().unwrap().len(), 2);
    runtime.eval(&format!("const old=popup;popup.close();popup=window.open('','sakura-popup');globalThis.sakuraRecreated=popup!==old;popup.eval({});popup.postMessage('observe','*');",json!(child_source(2)))).unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime.eval("sakuraRecreated").unwrap().as_boolean(),
        Some(true)
    );
    save("auxiliary-recreated", &reports(&mut runtime), 4);
}

#[test]
fn observe_main_resource_event() {
    let external = format!(
        "globalThis.sakuraResourceRan=true;sakuraReports.push({});",
        observation("main-window", "external-script-author")
    );
    let routes = BTreeMap::from([(
        "/private-flush-factory/resource.js".into(),
        ("text/javascript".into(), external),
    )]);
    let setup = format!(
        "globalThis.sakuraReports=[{}];globalThis.sakuraResourceRan=false;const script=document.createElement('script');script.onload=()=>sakuraReports.push({});script.src='/private-flush-factory/resource.js';document.head.appendChild(script);",
        observation("main-window", "before-resource"),
        observation("main-window", "resource-load-event")
    );
    let mut runtime = page(&setup, routes);
    for _ in 0..8 {
        runtime.tick(16).unwrap();
    }
    assert_eq!(
        runtime.eval("sakuraResourceRan").unwrap().as_boolean(),
        Some(true)
    );
    save("main-resource", &reports(&mut runtime), 3);
}

#[test]
fn observe_iframe_srcdoc_recreated() {
    let child_html = |generation| {
        format!(
            "<html><body><script>parent.postMessage({},'*');onmessage=()=>parent.postMessage({},'*');</script></body></html>",
            observation(
                "iframe-srcdoc-recreated",
                &format!("generation-{generation}-author")
            ),
            observation(
                "iframe-srcdoc-recreated",
                &format!("generation-{generation}-message")
            )
        )
    };
    let setup = format!(
        "globalThis.sakuraReports=[];addEventListener('message',e=>sakuraReports.push(e.data));globalThis.frame=document.createElement('iframe');frame.onload=()=>frame.contentWindow.postMessage('observe','*');frame.srcdoc={};document.body.appendChild(frame);",
        json!(child_html(1))
    );
    let mut runtime = page(&setup, BTreeMap::new());
    for _ in 0..8 {
        runtime.tick(16).unwrap();
    }
    assert_eq!(reports(&mut runtime).as_array().unwrap().len(), 2);
    runtime
        .eval(&format!(
            "globalThis.sakuraOldDocument=frame.contentDocument;frame.srcdoc={};",
            json!(child_html(2))
        ))
        .unwrap();
    for _ in 0..8 {
        runtime.tick(16).unwrap();
    }
    assert_eq!(
        runtime
            .eval("sakuraOldDocument!==frame.contentDocument")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    save("iframe-srcdoc-recreated", &reports(&mut runtime), 4);
}
