use omoikane::cdp::CdpSession;
use omoikane::html::TreeBuilder;
use omoikane::js::JsRuntime;
use omoikane::platform_browser::PlatformBrowser;
use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpListener;

fn evaluate(session: &mut CdpSession, expression: &str) -> serde_json::Value {
    session
        .dispatch("Runtime.evaluate", json!({ "expression": expression }))
        .unwrap()["result"]["value"]
        .clone()
}

#[test]
fn document_visibility_events_and_animation_frames_follow_host_state() {
    let mut runtime = JsRuntime::new().unwrap();
    assert!(runtime
        .eval("document.visibilityState === 'visible' && !document.hidden && 'onvisibilitychange' in document")
        .unwrap()
        .to_boolean());
    runtime
        .eval(
            "globalThis.visibilityLog = []; \
             document.addEventListener('visibilitychange', () => visibilityLog.push(document.visibilityState)); \
             document.onvisibilitychange = () => visibilityLog.push('handler:' + document.visibilityState); \
             requestAnimationFrame(() => visibilityLog.push('frame'));",
        )
        .unwrap();

    runtime.set_page_visibility(true);
    runtime.set_page_visibility(true);
    assert_eq!(runtime.run_animation_frame(16).unwrap(), 0);
    assert!(
        runtime
            .eval("document.hidden && visibilityLog.join('|') === 'hidden|handler:hidden'")
            .unwrap()
            .to_boolean()
    );

    runtime.set_page_visibility(false);
    assert_eq!(runtime.run_animation_frame(16).unwrap(), 1);
    assert!(runtime
        .eval("!document.hidden && visibilityLog.join('|') === 'hidden|handler:hidden|visible|handler:visible|frame'")
        .unwrap()
        .to_boolean());
}

#[test]
fn iframe_document_inherits_visibility_and_receives_change_event() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            "globalThis.frame = document.createElement('iframe'); \
             frame.srcdoc = '<p>child</p>'; document.body.appendChild(frame); \
             globalThis.childDocument = frame.contentDocument; \
             globalThis.childStates = []; \
             childDocument.addEventListener('visibilitychange', () => childStates.push(childDocument.visibilityState));",
        )
        .unwrap();
    assert!(
        runtime
            .eval("childDocument.visibilityState === 'visible'")
            .unwrap()
            .to_boolean()
    );
    runtime.set_page_visibility(true);
    assert!(
        runtime
            .eval("childDocument.hidden && childStates.join(',') === 'hidden'")
            .unwrap()
            .to_boolean()
    );
    runtime.set_page_visibility(false);
    assert!(
        runtime
            .eval("!childDocument.hidden && childStates.join(',') === 'hidden,visible'")
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn removing_iframe_hides_departing_document_before_teardown() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            "globalThis.frame = document.createElement('iframe'); \
             frame.srcdoc = '<p>child</p>'; document.body.appendChild(frame); \
             globalThis.childDocument = frame.contentDocument; \
             globalThis.departure = []; \
             childDocument.addEventListener('visibilitychange', () => \
               departure.push(childDocument.visibilityState)); \
             frame.remove();",
        )
        .unwrap();
    assert!(
        runtime
            .eval("childDocument.visibilityState === 'hidden' && departure.join(',') === 'hidden'")
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn nested_iframe_loads_then_hides_all_departing_documents() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 2048];
        let _ = stream.read(&mut request).unwrap();
        let body = "<body onload='parent.startTest()'><iframe onload='parent.parent.startTest()'></iframe><iframe onload='parent.parent.startTest()'></iframe></body>";
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });
    let url = format!("http://{address}/index.html");
    let document = TreeBuilder::parse(&format!(
        "<html><body><iframe id='outer' src='http://{address}/child.html'></iframe></body></html>"
    ))
    .document();
    let mut runtime = JsRuntime::with_document_and_url(document, &url).unwrap();
    runtime
        .eval("globalThis.loadCount = 0; globalThis.startTest = () => loadCount++")
        .unwrap();
    let base = url.parse().unwrap();
    assert!(runtime.execute_document_scripts(Some(&base)).is_empty());
    runtime.run_timers(5_000, 10, 2_000);
    runtime.run_jobs().unwrap();
    let count = runtime.eval("loadCount").unwrap().as_number().unwrap();
    assert_eq!(count, 3.0, "nested load callbacks must all run");
    runtime
        .eval(
            "globalThis.departure = []; \
             const outer = document.getElementById('outer'); \
             const frameDocuments = [outer.contentDocument, \
               outer.contentWindow[0].document, outer.contentWindow[1].document]; \
             for (let index = 0; index < frameDocuments.length; index++) { \
               const childDocument = frameDocuments[index]; \
               childDocument.addEventListener('visibilitychange', () => \
                 departure.push(index + ':' + childDocument.visibilityState)); \
             } \
             outer.remove();",
        )
        .unwrap_or_else(|error| panic!("nested iframe departure: {error}"));
    assert_eq!(
        runtime
            .eval("departure.sort().join(',')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "0:hidden,1:hidden,2:hidden"
    );
    server.join().unwrap();
}

#[test]
fn iframe_navigation_dispatches_pagehide_before_visibilitychange() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            "globalThis.frame = document.createElement('iframe'); \
             frame.srcdoc = '<p>old</p>'; document.body.appendChild(frame); \
             globalThis.oldDocument = frame.contentDocument; \
             globalThis.departure = []; \
             frame.contentWindow.addEventListener('pagehide', event => \
               departure.push('pagehide:' + event.target.nodeName + ':' + \
                 oldDocument.visibilityState + ':' + event.cancelable)); \
             oldDocument.addEventListener('visibilitychange', event => \
               departure.push('visibilitychange:' + event.target.nodeName + ':' + \
                 oldDocument.visibilityState + ':' + event.cancelable)); \
             frame.contentWindow.location.href = 'data:text/html,<p>next</p>';",
        )
        .unwrap_or_else(|error| panic!("iframe navigation departure: {error}"));
    let events = runtime
        .eval("departure.join(',')")
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(
        events,
        "pagehide:#document:visible:true,visibilitychange:#document:hidden:false"
    );
}

#[test]
fn iframe_srcdoc_navigation_notifies_departing_document() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            "globalThis.frame = document.createElement('iframe'); \
             document.body.appendChild(frame); \
             globalThis.oldDocument = frame.contentDocument; \
             globalThis.departure = []; \
             frame.contentWindow.addEventListener('pagehide', event => \
               departure.push('pagehide:' + oldDocument.visibilityState)); \
             oldDocument.addEventListener('visibilitychange', () => \
               departure.push('visibilitychange:' + oldDocument.visibilityState)); \
             frame.srcdoc = '<p>next</p>';",
        )
        .unwrap();
    runtime.run_timers(5_000, 10, 2_000);
    assert_eq!(
        runtime
            .eval("departure.join(',')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "pagehide:visible,visibilitychange:hidden"
    );
}

#[test]
fn iframe_window_listener_survives_lazy_realm_creation_before_departure() {
    let mut runtime = JsRuntime::new().unwrap();
    assert!(
        runtime
            .eval(
                "const frame = document.createElement('iframe'); \
                 document.body.appendChild(frame); \
                 let pagehideCount = 0; \
                 frame.contentWindow.addEventListener('pagehide', () => pagehideCount++); \
                 void frame.contentWindow.Date; \
                 frame.srcdoc = '<p>next</p>'; \
                 pagehideCount === 1;",
            )
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn scripted_iframe_realm_receives_later_window_proxy_departure_listener() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            "globalThis.frame = document.createElement('iframe'); \
             frame.srcdoc = '<script>window.loaded = true<\\/script>'; \
             document.body.appendChild(frame);",
        )
        .unwrap();
    runtime.run_timers(5_000, 10, 2_000);
    runtime
        .eval(
            "globalThis.departure = []; \
             const oldDocument = frame.contentDocument; \
             oldDocument.addEventListener('visibilitychange', () => \
               departure.push('visibilitychange:' + oldDocument.visibilityState)); \
             frame.contentWindow.addEventListener('pagehide', () => \
               departure.push('pagehide:' + oldDocument.visibilityState)); \
             frame.srcdoc = '<p>next</p>';",
        )
        .unwrap();
    assert_eq!(
        runtime
            .eval("departure.join(',')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "pagehide:visible,visibilitychange:hidden"
    );
}

#[test]
fn iframe_listener_registered_before_child_script_survives_realm_creation() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            "globalThis.scriptRan = false; globalThis.departure = []; \
             globalThis.frame = document.createElement('iframe'); \
             frame.srcdoc = '<script>parent.scriptRan = true<\\/script>'; \
             document.body.appendChild(frame); \
             frame.contentWindow.addEventListener('pagehide', () => \
               departure.push('pagehide'));",
        )
        .unwrap();
    runtime.run_timers(5_000, 10, 2_000);
    assert!(runtime.eval("scriptRan").unwrap().to_boolean());
    runtime.eval("frame.srcdoc = '<p>next</p>'").unwrap();
    assert_eq!(
        runtime
            .eval("departure.join(',')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "pagehide"
    );
}

#[test]
fn unchanged_iframe_srcdoc_does_not_depart() {
    let mut runtime = JsRuntime::new().unwrap();
    assert!(
        runtime
            .eval(
                "const frame = document.createElement('iframe'); \
             frame.srcdoc = '<p>old</p>'; document.body.appendChild(frame); \
             const oldDocument = frame.contentDocument; \
             let departures = 0; \
             oldDocument.addEventListener('visibilitychange', () => departures++); \
             frame.srcdoc = '<p>old</p>'; \
             departures === 0 && frame.contentDocument === oldDocument;",
            )
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn cross_origin_iframe_navigation_does_not_throw_departure_error() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            "globalThis.frame = document.createElement('iframe'); \
             frame.src = 'data:text/html,<p>old</p>'; document.body.appendChild(frame);",
        )
        .unwrap();
    runtime.run_timers(5_000, 10, 2_000);
    assert!(
        runtime
            .eval("frame.contentDocument === null")
            .unwrap()
            .to_boolean()
    );
    runtime
        .eval("frame.contentWindow.location.href = 'data:text/html,<p>next</p>'")
        .unwrap_or_else(|error| panic!("cross-origin iframe departure: {error}"));
}

#[test]
fn cross_origin_iframe_receives_departure_events_in_its_own_realm() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            r#"
            globalThis.departureMessages = [];
            window.addEventListener('message', event => departureMessages.push(event.data));
            globalThis.frame = document.createElement('iframe');
            const child = `<script>
              window.addEventListener('pagehide', () =>
                parent.postMessage('pagehide:' + document.visibilityState, '*'));
              document.addEventListener('visibilitychange', () =>
                parent.postMessage('visibilitychange:' + document.visibilityState, '*'));
            <\/script>`;
            frame.src = 'data:text/html,' + encodeURIComponent(child);
            document.body.appendChild(frame);
            "#,
        )
        .unwrap();
    runtime.run_timers(5_000, 10, 2_000);
    runtime
        .eval("frame.contentWindow.location.href = 'data:text/html,<p>next</p>'")
        .unwrap();
    runtime.run_timers(5_000, 10, 2_000);
    runtime.run_jobs().unwrap();
    let messages = runtime
        .eval("departureMessages.join(',')")
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(messages, "pagehide:visible,visibilitychange:hidden");
}

#[test]
fn iframe_window_load_handler_can_initiate_visibility_work() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 2048];
        let _ = stream.read(&mut request).unwrap();
        let body = "<body onload='parent.childBodyLoaded = true'><script>onload = () => { parent.childLoaded = true; }</script></body>";
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });
    let url = format!("http://{address}/index.html");
    let document = TreeBuilder::parse(&format!(
        "<html><body><iframe src='http://{address}/child.html'></iframe></body></html>"
    ))
    .document();
    let mut runtime = JsRuntime::with_document_and_url(document, &url).unwrap();
    runtime
        .eval("globalThis.childLoaded = false; globalThis.childBodyLoaded = false")
        .unwrap();
    let base = url.parse().unwrap();
    assert!(runtime.execute_document_scripts(Some(&base)).is_empty());
    runtime.tick(0).unwrap();
    let diagnostic = runtime
        .eval("JSON.stringify({childLoaded, childBodyLoaded, childOnload: typeof document.querySelector('iframe').contentWindow.onload})")
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    let errors = runtime.take_task_errors();
    assert!(
        runtime
            .eval("childLoaded === true && childBodyLoaded === true")
            .unwrap()
            .to_boolean(),
        "{diagnostic}, errors={errors:?}"
    );
    server.join().unwrap();
}

#[test]
fn iframe_body_onload_without_script_runs() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 2048];
        let _ = stream.read(&mut request).unwrap();
        let body = "<body onload='parent.bodyOnlyLoaded = true'></body>";
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });
    let url = format!("http://{address}/index.html");
    let document = TreeBuilder::parse(&format!(
        "<html><body><iframe src='http://{address}/child.html'></iframe></body></html>"
    ))
    .document();
    let mut runtime = JsRuntime::with_document_and_url(document, &url).unwrap();
    runtime.eval("globalThis.bodyOnlyLoaded = false").unwrap();
    let base = url.parse().unwrap();
    assert!(runtime.execute_document_scripts(Some(&base)).is_empty());
    runtime.tick(0).unwrap();
    assert!(runtime.eval("bodyOnlyLoaded").unwrap().to_boolean());
    server.join().unwrap();
}

#[test]
fn cdp_lifecycle_changes_visibility_and_new_navigation_inherits_it() {
    let mut session = CdpSession::new().unwrap();
    assert_eq!(
        evaluate(&mut session, "document.visibilityState"),
        "visible"
    );
    session.set_host_visibility(true);
    assert_eq!(evaluate(&mut session, "document.hidden"), true);
    session
        .dispatch("Page.setWebLifecycleState", json!({ "state": "frozen" }))
        .unwrap();
    assert_eq!(evaluate(&mut session, "document.hidden"), true);
    session
        .dispatch(
            "Page.navigate",
            json!({ "url": "data:text/html,<script>globalThis.wasHiddenAtStart=document.hidden</script>" }),
        )
        .unwrap();
    assert_eq!(evaluate(&mut session, "wasHiddenAtStart"), true);
    session
        .dispatch("Page.setWebLifecycleState", json!({ "state": "active" }))
        .unwrap();
    assert_eq!(evaluate(&mut session, "document.hidden"), true);
    session.set_host_visibility(false);
    assert_eq!(
        evaluate(&mut session, "document.visibilityState"),
        "visible"
    );
    assert!(
        session
            .dispatch("Page.setWebLifecycleState", json!({ "state": "discarded" }))
            .is_err()
    );
}

#[test]
fn navigation_hides_departing_document_between_beforeunload_and_pagehide() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let first = r#"<script>
            sessionStorage.setItem('departureOrder', '');
            for (const type of ['beforeunload', 'visibilitychange', 'pagehide', 'unload']) {
              const target = type === 'visibilitychange' ? document : window;
              target.addEventListener(type, () => {
                const old = sessionStorage.getItem('departureOrder');
                sessionStorage.setItem('departureOrder', old + type + ':' + document.visibilityState + ',');
              });
            }
        </script>"#;
        for (path, body) in [("/first", first), ("/second", "<p>next</p>")] {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 2048];
            let count = stream.read(&mut request).unwrap();
            assert!(
                String::from_utf8_lossy(&request[..count]).starts_with(&format!("GET {path} "))
            );
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        }
    });
    let mut session = CdpSession::new().unwrap();
    session
        .dispatch(
            "Page.navigate",
            json!({ "url": format!("http://{address}/first") }),
        )
        .unwrap();
    session
        .dispatch(
            "Page.navigate",
            json!({ "url": format!("http://{address}/second") }),
        )
        .unwrap();
    assert_eq!(
        evaluate(&mut session, "sessionStorage.getItem('departureOrder')"),
        "beforeunload:visible,visibilitychange:hidden,pagehide:hidden,unload:hidden,"
    );
    server.join().unwrap();
}

#[test]
fn switching_tabs_hides_previous_page_and_restores_it_on_activation() {
    let mut browser = PlatformBrowser::with_tab(Some("data:text/html,<p>first</p>")).unwrap();
    let first = browser.active_tab().unwrap();
    browser
        .active_session_mut()
        .unwrap()
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "globalThis.states=[]; document.onvisibilitychange=()=>states.push(document.visibilityState)" }),
        )
        .unwrap();
    let second = browser
        .open_tab(Some("data:text/html,<p>second</p>"))
        .unwrap();
    assert_eq!(
        evaluate(browser.active_session_mut().unwrap(), "document.hidden"),
        false
    );
    browser.activate_tab(first).unwrap();
    assert_eq!(
        evaluate(browser.active_session_mut().unwrap(), "states.join(',')"),
        "hidden,visible"
    );
    browser.activate_tab(second).unwrap();
    assert_eq!(
        evaluate(browser.active_session_mut().unwrap(), "document.hidden"),
        false
    );
}
