use super::*;
use crate::html::TreeBuilder;

struct WriteServer {
    origin: String,
    requests: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl WriteServer {
    fn new(routes: &[(&str, &str)]) -> Self {
        use std::io::{BufRead, BufReader, Write};
        use std::sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
        };
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let routes: HashMap<String, String> = routes
            .iter()
            .map(|(path, source)| (path.to_string(), source.to_string()))
            .collect();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_requests = requests.clone();
        let thread_stop = stop.clone();
        let thread = std::thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(stream) => stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                        continue;
                    }
                    Err(error) => panic!("{error}"),
                };
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(&mut stream);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let path = line.split_whitespace().nth(1).unwrap().to_string();
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                }
                thread_requests.lock().unwrap().push(path.clone());
                let body = routes
                    .get(&path)
                    .unwrap_or_else(|| panic!("unexpected request: {path}"));
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        Self {
            origin,
            requests,
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for WriteServer {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap();
    }
}

fn execute(mut runtime: JsRuntime, page_task: bool) -> JsRuntime {
    if page_task {
        use std::future::Future;
        use std::task::{Context, Poll, Waker};
        let mut task = Box::pin(runtime.into_document_page_task(1, None));
        let mut context = Context::from_waker(Waker::noop());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Poll::Ready(completed) = task.as_mut().poll(&mut context) {
                assert_eq!(completed.result, Ok(Vec::new()));
                return completed.runtime;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "page task did not settle"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    let errors = runtime.execute_document_scripts(None);
    assert!(errors.is_empty(), "{errors:?}");
    runtime
}

#[test]
fn document_write_nested_script_precedes_unparsed_tail() {
    let document = TreeBuilder::parse("<body><p id='existing'>old</p>").document();
    let mut runtime = JsRuntime::with_document(document.clone()).unwrap();
    runtime.eval(r#"
        globalThis.order = [];
        document.write('<div id="container"><script id="written">order.push(document.getElementById("tail") === null); document.write("<b id=inner>X</b>"); order.push(document.getElementById("tail") === null);<\/script><i id="tail">tail</i></div>');
        order.push(document.getElementById('inner').parentNode === document.getElementById('container'));
        order.push(document.getElementById('inner').nextSibling.id === 'tail');
    "#).unwrap();
    assert_eq!(
        runtime
            .eval("JSON.stringify(order)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[true,true,true,true]"
    );
}

#[test]
fn document_write_split_tags_and_live_open_elements() {
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse("<body>").document()).unwrap();
    runtime
        .eval(
            r#"
        document.open();
        document.write('<!DOC');
        document.write('TYPE html><html><head><title>title</title></head><body><b');
        document.write(' id="kept">');
        globalThis.kept = document.getElementById('kept');
        kept.marker = 42;
        document.write('x&amp');
        document.write(';y</b><!-- com');
        document.write('ment --><script>globalThis.splitRan = ');
        globalThis.didNotRun = typeof splitRan === 'undefined';
        document.write('true;<\/scr');
        document.write('ipt><p>end</p>');
        document.close();
    "#,
        )
        .unwrap();
    assert_eq!(runtime.eval("kept === document.getElementById('kept') && kept.marker === 42 && kept.textContent === 'x&y' && didNotRun && splitRan && document.doctype.name === 'html'").unwrap().as_boolean(), Some(true));
}

#[test]
fn document_write_child_script_uses_its_own_realm() {
    let mut runtime = JsRuntime::with_document_and_url(
        TreeBuilder::parse("<body><iframe></iframe>").document(),
        "http://localhost/",
    )
    .unwrap();
    runtime.eval(r#"
        const child = document.querySelector('iframe').contentDocument;
        child.open();
        child.write('<body><script>globalThis.childWritten = document; document.write("<b id=child>child</b>");<\/script><p id=tail>tail</p>');
        child.close();
        globalThis.childCorrect = child.defaultView.Function("return childWritten === document")() && child.defaultView.childWritten.__id === child.__id && typeof globalThis.childWritten === 'undefined' && child.getElementById('child').nextSibling.id === 'tail';
    "#).unwrap();
    assert_eq!(
        runtime.eval("childCorrect").unwrap().as_boolean(),
        Some(true)
    );
}

#[test]
fn document_write_external_script_blocks_its_tail_until_parent_returns() {
    for page_task in [false, true] {
        let server = WriteServer::new(&[(
            "/written.js",
            "order.push('external:'+!!document.getElementById('tail')); document.write('<b id=inner>inner</b>');",
        )]);
        let document = TreeBuilder::parse(r#"<body><script>
        globalThis.order = [];
        document.write('<script src="/written.js"><\/script><i id=tail>tail</i>');
        order.push('returned:'+!!document.getElementById('tail')); Promise.resolve().then(()=>order.push('microtask'));
        </script><script>order.push('next:'+document.getElementById('inner').nextSibling.id);</script>"#).document();
        let mut runtime = JsRuntime::with_document_and_url(document, &server.origin).unwrap();
        runtime = execute(runtime, page_task);
        runtime.run_until_idle().unwrap();
        assert_eq!(
            runtime
                .eval("JSON.stringify(order)")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            "[\"returned:false\",\"microtask\",\"external:false\",\"next:tail\"]"
        );
        assert_eq!(*server.requests.lock().unwrap(), vec!["/written.js"]);
    }
}

#[test]
fn document_write_inline_and_external_modules_run_after_classics_before_dcl() {
    for page_task in [false, true] {
        let server = WriteServer::new(&[
            (
                "/entry.js",
                "import {value} from './dependency.js'; order.push('external:'+value+':'+(document.currentScript===null));",
            ),
            ("/dependency.js", "export const value = 42;"),
        ]);
        let document = TreeBuilder::parse(r#"<body><script>
        globalThis.order = [];
        document.addEventListener('DOMContentLoaded',()=>order.push('dcl'));
        document.write('<script type=module>export const value=1;order.push("inline:"+!!document.getElementById("tail"));<\/script><script type=module src="/entry.js"><\/script><b id=tail>tail</b>');
        order.push('returned');
        </script><script>order.push('next');</script><script type=module>order.push('parsed-module');</script>"#).document();
        let mut runtime = JsRuntime::with_document_and_url(document, &server.origin).unwrap();
        runtime = execute(runtime, page_task);
        runtime.run_until_idle().unwrap();
        assert_eq!(
            runtime
                .eval("JSON.stringify(order)")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            "[\"returned\",\"next\",\"inline:true\",\"external:42:true\",\"parsed-module\",\"dcl\"]"
        );
        assert!(runtime.take_task_errors().is_empty());
        let mut paths = server.requests.lock().unwrap().clone();
        paths.sort();
        assert_eq!(paths, vec!["/dependency.js", "/entry.js"]);
    }
}

#[test]
fn document_write_recursion_is_catchable_and_parser_recovers() {
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse("<body>").document()).unwrap();
    runtime
        .eval(
            r#"
        globalThis.depth = 0;
        globalThis.limit = '';
        function recurse() {
            depth++;
            try { document.write('<script>recurse()<\/script>'); }
            catch(error) { limit = error.name; }
        }
        recurse();
        document.write('<b id=recovered>ok</b>');
    "#,
        )
        .unwrap();
    assert_eq!(runtime.eval("limit === 'RangeError' && depth > 1 && depth <= 33 && document.getElementById('recovered').textContent === 'ok'").unwrap().as_boolean(), Some(true));
}

#[test]
fn document_write_csp_and_load_events_do_not_stall_the_stream() {
    let server = WriteServer::new(&[(
        "/allowed.js",
        "globalThis.loaded = (globalThis.loaded || 0) + 1;",
    )]);
    let document = TreeBuilder::parse("<body>").document();
    let mut runtime = JsRuntime::with_document_and_url(document, &server.origin).unwrap();
    runtime.install_csp_policy(&[format!(
        "script-src 'unsafe-inline' {}/allowed.js",
        server.origin
    )]);
    runtime.eval(r#"
        globalThis.events = [];
        document.write('<script src="/blocked.js" onerror="events.push(\'blocked\')"><\/script><b id=afterBlocked>after</b><script src="/allowed.js" onload="events.push(\'loaded\')"><\/script><script type=module src="/blocked-module.js" onerror="events.push(\'module-blocked\')"><\/script><i id=end>end</i>');
    "#).unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("JSON.stringify(events)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[\"blocked\",\"loaded\",\"module-blocked\"]"
    );
    assert_eq!(runtime.eval("loaded === 1 && !!document.getElementById('end') && document.cspViolations.length === 2").unwrap().as_boolean(), Some(true));
    assert_eq!(*server.requests.lock().unwrap(), vec!["/allowed.js"]);
}

#[test]
fn document_write_open_and_close_are_scoped_to_the_executing_document() {
    let mut runtime = JsRuntime::with_document_and_url(
        TreeBuilder::parse("<body><iframe></iframe>").document(),
        "http://localhost/",
    )
    .unwrap();
    runtime.eval(r#"
        globalThis.childWindow = document.querySelector('iframe').contentWindow;
        globalThis.child = childWindow.document;
        document.write('<script>document.open(); child.open(); child.write("<body><b id=written>x</b>"); child.close(); document.write("<b id=kept>y</b>"); document.close();<\/script><i id=tail>tail</i>');
    "#).unwrap();
    assert_eq!(runtime.eval("childWindow === child.defaultView && !childWindow.closed && childWindow.document === child && !!document.querySelector('iframe') && document.getElementById('kept').nextSibling.id === 'tail' && child.getElementById('written').textContent === 'x'").unwrap().as_boolean(), Some(true));
}

#[test]
fn document_write_prepared_external_script_survives_removal_and_attribute_changes() {
    let server = WriteServer::new(&[(
        "/original.js",
        "globalThis.originalRan = (globalThis.originalRan || 0) + 1;",
    )]);
    let document = TreeBuilder::parse(
        r#"<body><script>
        document.write('<script id=written src="/original.js"><\/script><b id=tail>tail</b>');
        const written = document.getElementById('written');
        written.src = '/changed.js';
        written.type = 'application/json';
        written.remove();
        </script><script>globalThis.tailPresent = !!document.getElementById('tail');</script>"#,
    )
    .document();
    let runtime = JsRuntime::with_document_and_url(document, &server.origin).unwrap();
    let mut runtime = execute(runtime, false);
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("originalRan === 1 && tailPresent")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    assert_eq!(*server.requests.lock().unwrap(), vec!["/original.js"]);
}

/// Boa currently rejects modal suspension from its synchronous eval runner.
/// Preserve error reporting without leaving an already-cancelled dialog open.
#[test]
fn document_write_sync_dialog_limit_does_not_leave_pending_host_state() {
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse("<body>").document()).unwrap();
    runtime
        .eval(r#"document.write('<script>alert("written");<\/script><b id=tail>tail</b>');"#)
        .unwrap();
    assert!(runtime.javascript_dialog_controller().pending().is_none());
    assert!(runtime.take_task_errors().iter().any(|error| {
        error.contains("native call suspension requires asynchronous script evaluation")
    }));
    assert_eq!(
        runtime
            .eval("!!document.getElementById('tail')")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn document_write_registers_foster_parented_text_and_retains_text_identity() {
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse("<body>").document()).unwrap();
    runtime
        .eval(
            r#"
        document.write('<table>outside<tr><td>cell</td></tr></table><b id=text>x');
        globalThis.text = document.getElementById('text').firstChild;
        document.write('y</b>');
    "#,
        )
        .unwrap();
    assert_eq!(runtime.eval("document.body.firstChild.nodeType === 3 && document.body.firstChild.data === 'outside' && text === document.getElementById('text').firstChild && text.data === 'xy'").unwrap().as_boolean(), Some(true));
}

#[test]
fn document_write_async_script_does_not_block_parsing_or_dcl() {
    let server = WriteServer::new(&[(
        "/async.js",
        "order.push('async:'+!!document.getElementById('tail'));",
    )]);
    let document = TreeBuilder::parse(
        r#"<body><script>
        globalThis.order=[];
        document.addEventListener('DOMContentLoaded',()=>order.push('dcl'));
        document.write('<script async src="/async.js"><\/script><b id=tail>tail</b>');
        order.push('returned:'+!!document.getElementById('tail'));
        </script>"#,
    )
    .document();
    let runtime = JsRuntime::with_document_and_url(document, &server.origin).unwrap();
    let mut runtime = execute(runtime, false);
    assert_eq!(
        runtime
            .eval("JSON.stringify(order)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[\"returned:true\",\"dcl\"]"
    );
    assert!(server.requests.lock().unwrap().is_empty());
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("JSON.stringify(order)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[\"returned:true\",\"dcl\",\"async:true\"]"
    );
}
