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
