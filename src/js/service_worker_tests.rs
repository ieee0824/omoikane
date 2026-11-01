//! ServiceWorker startup, response policy, and client-message regressions.
use super::*;
use crate::test_support::http_fixture::{
    ACCEPT_TIMEOUT, FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback,
    read_request_headers,
};
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};

const SERVICE_WORKER_SOURCE: &str = r#"
  const policy = trustedTypes.createPolicy('page-only', {createScript: value => value});
  addEventListener('message', event => {
    let blocked = false;
    try { eval('7'); } catch (error) { blocked = error instanceof EvalError; }
    event.source.postMessage({
      blocked,
      allowed: eval(policy.createScript('7')),
      webassembly: typeof WebAssembly === 'object' &&
        typeof WebAssembly.compileStreaming === 'function' &&
        WebAssembly.validate(Uint8Array.of(0,97,115,109,1,0,0,0)),
      isolated: typeof document === 'undefined' && typeof close === 'undefined' &&
        typeof globalThis.__omoikane_service_worker_lifecycle === 'undefined' &&
        typeof globalThis.__omoikane_service_worker_lifecycle_complete === 'undefined' &&
        typeof globalThis.__omoikane_dispatch_service_worker_lifecycle === 'undefined' &&
        typeof DedicatedWorkerGlobalScope === 'undefined' &&
        typeof ServiceWorkerGlobalScope === 'function',
    });
  });
"#;

const LIFECYCLE_WORKER_SOURCE: &str = r#"
  let activationCount = 0;
  addEventListener('activate', () => { activationCount++; });
  addEventListener('message', event => event.source.postMessage({
    activated: activationCount === 1,
    url: self.location.href,
  }));
"#;

const CLOSE_ON_ACTIVATE_SOURCE: &str = r#"
  addEventListener('activate', () => terminateDuringActivate());
"#;

const ORIGIN_WORKER_SOURCE: &str = r#"
  onmessage = event => event.source.postMessage({
    incomingOrigin: event.origin, workerOrigin: location.origin,
  });
"#;

const MESSAGE_PORT_WORKER_SOURCE: &str = r#"
  onmessage = event => {
    const port = event.ports[0];
    port.onmessage = reply => port.postMessage({
      value: reply.data.value,
      samePort: event.data.port === port,
      localPort: port instanceof MessagePort,
      localData: Object.getPrototypeOf(reply.data) === Object.prototype,
    });
    port.postMessage({ready: true});
  };
"#;

const COOKIE_STORE_WORKER_SOURCE: &str = r#"
  addEventListener('message', async event => {
    const available = typeof cookieStore === 'object' && cookieStore instanceof CookieStore;
    if (!available) {
      event.source.postMessage({available: false});
      return;
    }
    const seed = await cookieStore.get('seed');
    let byURL = null;
    let allByURL = [];
    let urlError = '';
    try {
      byURL = await cookieStore.get({name: 'seed', url: location.href});
      allByURL = await cookieStore.getAll({url: location.href});
    } catch (error) { urlError = error.name; }
    let mismatchedURLRejected = false;
    try { await cookieStore.get({name: 'seed', url: '/app/other'}); }
    catch (error) { mismatchedURLRejected = error instanceof TypeError; }
    await cookieStore.set({name: 'from-worker', value: 'yes', path: '/'});
    const written = await cookieStore.get(' from-worker \t');
    event.source.postMessage({
      available, seed: seed?.value, written: written?.value,
      byURL: byURL?.value, allByURL: allByURL.some(cookie => cookie.name === 'seed'),
      mismatchedURLRejected, urlError,
      noDocument: typeof document === 'undefined',
      noWindowChangeHandler: !('onchange' in cookieStore) &&
        typeof CookieChangeEvent === 'undefined',
    });
  });
"#;

const COOKIE_SUBSCRIPTIONS_WORKER_SOURCE: &str = r#"
  const lifecycle = [];
  addEventListener('install', () => lifecycle.push(registration.installing?.state));
  addEventListener('activate', () => lifecycle.push(registration.active?.state));
  addEventListener('message', async event => {
    if (event.data?.self) {
      selfClient.postMessage({lifecycle, active: registration.active?.state,
        installing: registration.installing, waiting: registration.waiting,
        selfSource: event.source === registration.active, value: event.data.value});
      return;
    }
    if (event.data === 'lifecycle') {
      globalThis.selfClient = event.source;
      const message = {self: true, value: 'cloned'};
      ServiceWorker.prototype.postMessage.call(registration.active, message);
      message.value = 'mutated';
      return;
    }
    try {
      const before = await registration.cookies.getSubscriptions();
      await registration.cookies.subscribe([{name: 'worker', url: '/app/page'}]);
      const after = await registration.cookies.getSubscriptions();
      await registration.cookies.unsubscribe([{name: 'page', url: '/app/page'}]);
      const remaining = await registration.cookies.getSubscriptions();
      event.source.postMessage({
        before, after, remaining,
        sameManager: registration.cookies === registration.cookies,
        managerType: registration.cookies instanceof CookieStoreManager,
      });
    } catch (error) {
      event.source.postMessage({error: error.name});
    }
  });
"#;

const COOKIE_CHANGE_WORKER_SOURCE: &str = r#"
  let client = null;
  addEventListener('message', async event => {
    client = event.source;
    await registration.cookies.subscribe([
      {name: 'watched', url: '/app/page'},
      {name: 'watched', url: '/app/'},
    ]);
    oncookiechange = change => {
      change.waitUntil(Promise.resolve());
      client.postMessage({
        changed: change.changed.map(cookie => [cookie.name, cookie.value]),
        deleted: change.deleted.map(cookie => cookie.name),
        extendable: change instanceof ExtendableCookieChangeEvent &&
          change instanceof ExtendableEvent && change.isTrusted,
      });
    };
    client.postMessage({armed: true});
  });
"#;

const ORIGIN_FRAME_SOURCE: &str = r#"<script>
  navigator.serviceWorker.onmessage = event => parent.postMessage({
    incomingOrigin: event.data.incomingOrigin,
    workerOrigin: event.data.workerOrigin,
    outgoingOrigin: event.origin,
    sameSource: event.source === globalThis.serviceOriginWorker,
  }, '*');
  navigator.serviceWorker.register('/app/origin.js').then(registration => {
    globalThis.serviceOriginWorker = registration.installing;
    serviceOriginWorker.postMessage('probe');
  }, error => parent.postMessage({error: error.name}, '*'));
</script>"#;

const LIFETIME_WORKER_SOURCE: &str = r#"
  const phase = location.pathname.includes('install-') ? 'install' : 'activate';
  let lifecycleEvent;
  let release;
  self['on' + phase] = event => {
    lifecycleEvent = event;
    event.waitUntil(new Promise(resolve => { release = resolve; }));
    const settled = Promise.resolve();
    self.settledLifetimePromise = new WeakRef(settled);
    event.waitUntil(settled);
    if (location.pathname.includes('reject')) event.waitUntil(Promise.reject(new Error('lifetime failure')));
  };
  onmessage = event => {
    let extension = 'allowed';
    try { lifecycleEvent.waitUntil(Promise.resolve()); } catch (error) { extension = error.name; }
    let synthetic = '';
    try { new ExtendableEvent('install').waitUntil(Promise.resolve()); } catch (error) { synthetic = error.name; }
    event.source.postMessage({ extension, synthetic, trusted: lifecycleEvent.isTrusted,
      retained: !!self.settledLifetimePromise.deref(),
      branded: lifecycleEvent instanceof ExtendableEvent });
    if (event.data === 'release') release();
  };
"#;

#[derive(Debug)]
pub(super) struct ServiceWorkerRequest {
    pub(super) path: String,
    pub(super) headers: String,
}

pub(super) struct ServiceWorkerFixture {
    pub(super) origin: String,
    stopped: Arc<AtomicBool>,
    worker: Option<FixtureWorker<Vec<ServiceWorkerRequest>>>,
}

impl ServiceWorkerFixture {
    pub(super) fn new() -> Self {
        Self::with_request_timeout(ACCEPT_TIMEOUT)
    }

    fn with_request_timeout(request_timeout: Duration) -> Self {
        let listener = bind_loopback().unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stopped = Arc::clone(&stopped);
        let worker = FixtureWorker::spawn(move || {
            let mut deadline = Instant::now() + request_timeout;
            let mut requests = Vec::new();
            while !worker_stopped.load(Ordering::Acquire) {
                assert!(Instant::now() < deadline, "ServiceWorker fixture timed out");
                let mut stream = match accept_with_timeout(&listener, Duration::from_millis(20)) {
                    Ok(stream) => stream,
                    Err(error) if error.kind() == io::ErrorKind::TimedOut => continue,
                    Err(error) => panic!("ServiceWorker accept failed: {error}"),
                };
                let headers = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
                let path = headers
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .to_owned();
                requests.push(ServiceWorkerRequest {
                    path: path.clone(),
                    headers,
                });
                let version = requests
                    .iter()
                    .filter(|request| request.path == path)
                    .count();
                respond_service_worker_fixture(&mut stream, &path, version);
                // Bound waiting for the next request, not the total duration
                // of all worker registrations performed by the owning test.
                deadline = Instant::now() + request_timeout;
            }
            requests
        });
        Self {
            origin,
            stopped,
            worker: Some(worker),
        }
    }

    pub(super) fn finish(self) -> Vec<String> {
        self.finish_requests()
            .into_iter()
            .map(|request| request.path)
            .collect()
    }

    pub(super) fn finish_requests(mut self) -> Vec<ServiceWorkerRequest> {
        self.stopped.store(true, Ordering::Release);
        self.worker.take().unwrap().join()
    }
}

#[test]
fn service_worker_fixture_still_times_out_without_request_progress() {
    let mut fixture = ServiceWorkerFixture::with_request_timeout(Duration::from_millis(30));
    let worker = fixture.worker.take().unwrap();
    // Do not send the owner's stop notification before observing the worker's
    // own deadline; scheduling delays must not turn this into a stop test.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| worker.join()));
    assert!(result.is_err(), "an idle fixture must retain its timeout");
}

#[test]
fn service_worker_fixture_allows_progress_beyond_one_request_timeout() {
    use std::io::Read;
    use std::net::TcpStream;

    let fixture = ServiceWorkerFixture::with_request_timeout(Duration::from_millis(300));
    let address = fixture.origin.strip_prefix("http://").unwrap();
    for _ in 0..3 {
        std::thread::sleep(Duration::from_millis(120));
        let mut stream = TcpStream::connect(address).unwrap();
        stream.set_read_timeout(Some(READ_TIMEOUT)).unwrap();
        stream
            .write_all(b"GET /plain.js HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK"));
    }
    assert_eq!(fixture.finish(), vec!["/plain.js"; 3]);
}

fn respond_service_worker_fixture(stream: &mut std::net::TcpStream, path: &str, version: usize) {
    let updating_source =
        format!("onmessage = event => event.source.postMessage({{version: {version}}});");
    let registration_source = format!(
        r#"
        const version = {version};
        onmessage = async event => {{
            try {{
                const result = await registration[event.data]();
                event.source.postMessage({{result: event.data === 'update' ? result === registration : result, version}});
            }} catch (error) {{ event.source.postMessage({{error: error.name}}); }}
        }};
    "#
    );
    let (body, policy, allowed) = match path {
        "/app/stale-registration.js" => (
            r#"
            onmessage = async event => {
                try {
                    if (event.data === 'unregister') await registration.unregister();
                    else await registration.cookies.subscribe([{name: 'ghost'}]);
                    event.source.postMessage('resolved');
                } catch (error) { event.source.postMessage(error.name); }
            };
        "#,
            "",
            "",
        ),
        "/app/registration.js" => (registration_source.as_str(), "", ""),
        "/app/disappearing.js" if version > 1 => {
            stream
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
            return;
        }
        "/app/disappearing.js" => ("", "", ""),
        "/app/importing-missing.js" => ("importScripts('disappearing.js');", "", ""),
        "/app/importing-changed-and-missing.js" => (
            "importScripts('changing-import.js', 'disappearing.js');",
            "",
            "",
        ),
        "/app/update-network.js" if version > 1 => {
            stream
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
            return;
        }
        "/app/update-mime.js" if version > 1 => {
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            return;
        }
        "/app/update-scope.js" if version > 1 => ("", "", "/app/sub/"),
        "/app/update-network.js" | "/app/update-mime.js" | "/app/update-scope.js" => ("", "", ""),
        "/app/update-syntax.js" if version > 1 => ("function (", "", ""),
        "/app/update-reject.js" if version > 1 => (
            "oninstall = event => event.waitUntil(Promise.reject(new Error('install failed')));",
            "",
            "",
        ),
        "/app/update-wait.js" if version > 1 => (
            "let release; oninstall = event => event.waitUntil(new Promise(resolve => release = resolve)); onmessage = () => release();",
            "",
            "",
        ),
        "/app/update-syntax.js" | "/app/update-reject.js" | "/app/update-wait.js" => ("", "", ""),
        "/app/updating.js" | "/app/changing-import.js" => (updating_source.as_str(), "", ""),
        "/app/importing-change.js" => ("importScripts('changing-import.js');", "", ""),
        "/app/importing-stable.js" => ("importScripts('worker.js');", "", ""),
        "/sw.js" | "/app/sw.js" => (
            SERVICE_WORKER_SOURCE,
            "require-trusted-types-for 'script'",
            "",
        ),
        "/app/worker.js" | "/app/worker-v2.js" => (LIFECYCLE_WORKER_SOURCE, "", ""),
        "/app/close-on-activate.js" => (CLOSE_ON_ACTIVATE_SOURCE, "", ""),
        "/app/origin.js" => (ORIGIN_WORKER_SOURCE, "", ""),
        "/app/message-port.js" => (MESSAGE_PORT_WORKER_SOURCE, "", ""),
        "/app/cookie-store.js" => (COOKIE_STORE_WORKER_SOURCE, "", ""),
        "/app/cookie-subscriptions.js" => (COOKIE_SUBSCRIPTIONS_WORKER_SOURCE, "", ""),
        "/app/cookie-change.js" => (COOKIE_CHANGE_WORKER_SOURCE, "", ""),
        "/app/frame.html" => (ORIGIN_FRAME_SOURCE, "", ""),
        "/app/empty-frame.html" => ("<!doctype html><body>child</body>", "", ""),
        "/app/install-wait.js"
        | "/app/install-reject.js"
        | "/app/activate-wait.js"
        | "/app/activate-reject.js" => (LIFETIME_WORKER_SOURCE, "", ""),
        "/allowed/sw.js" => ("", "", "/"),
        "/narrow/sw.js" => ("", "", "/narrow/sub/"),
        "/bad-allowed/sw.js" => ("", "", "http://["),
        "/cross-allowed/sw.js" => ("", "", "https://other.example.test/"),
        "/app/restricted.js" | "/target.js" => ("", "", ""),
        "/syntax.js" | "/app/syntax.js" => ("function (", "", ""),
        "/redirect.js" => {
            stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: /target.js\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            return;
        }
        "/plain.js" => {
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            return;
        }
        _ => {
            stream
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
            return;
        }
    };
    let policy_header = if policy.is_empty() {
        String::new()
    } else {
        format!("Content-Security-Policy: {policy}\r\n")
    };
    let allowed_header = if allowed.is_empty() {
        String::new()
    } else {
        format!("Service-Worker-Allowed: {allowed}\r\n")
    };
    let content_type = if path.ends_with(".html") {
        "text/html"
    } else {
        "text/javascript"
    };
    write!(stream,
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\n{policy_header}{allowed_header}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    ).unwrap();
}

impl Drop for ServiceWorkerFixture {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
    }
}

#[test]
fn service_worker_executes_with_response_csp_and_replies_from_client() {
    let fixture = ServiceWorkerFixture::new();
    let mut runtime = JsRuntime::with_document_and_url(
        blank_html_document(),
        &format!("{}/app/page", fixture.origin),
    )
    .unwrap();
    runtime
        .eval(
            r#"
      globalThis.observed = {messages: [], error: ''};
      trustedTypes.createPolicy('page-only', {createScript: value => value});
      navigator.serviceWorker.addEventListener('message', event => {
        observed.sourceMatches = event.source === observed.worker;
        observed.messages.push(event.data);
      });
      navigator.serviceWorker.register('/app/sw.js', {scope: '/app/'}).then(registration => {
        observed.installing = registration.installing?.state === 'installing';
        observed.registration = registration;
        observed.worker = registration.installing;
        registration.installing.postMessage('probe');
      }, error => observed.error = error.name);
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"observed.error === '' && observed.installing &&
          observed.registration.active?.state === 'activated' &&
          observed.messages.length === 1 && observed.sourceMatches &&
          observed.messages[0].blocked && observed.messages[0].allowed === 7 &&
          observed.messages[0].isolated && observed.messages[0].webassembly"#
            )
            .unwrap()
            .as_boolean(),
        Some(true),
        "state: {:?}; task errors: {:?}",
        runtime.eval("JSON.stringify({error:observed.error,installing:observed.installing,active:observed.registration?.active?.state,waiting:observed.registration?.waiting?.state,messages:observed.messages})"),
        runtime.take_task_errors(),
    );
    drop(runtime);
    assert_eq!(fixture.finish(), vec!["/app/sw.js".to_owned()]);
}

#[test]
fn service_worker_cookie_store_reads_and_writes_shared_jar_without_document() {
    let fixture = ServiceWorkerFixture::new();
    let mut runtime = JsRuntime::with_document_and_url(
        blank_html_document(),
        &format!("{}/app/page", fixture.origin),
    )
    .unwrap();
    runtime
        .eval(
            r#"
      globalThis.observed = {error: '', message: null, pageCookie: null};
      navigator.serviceWorker.addEventListener('message', event => {
        observed.message = event.data;
        cookieStore.get('from-worker').then(cookie => observed.pageCookie = cookie?.value);
      });
      cookieStore.set({name: 'seed', value: 'page', path: '/'}).then(() =>
        navigator.serviceWorker.register('/app/cookie-store.js')
      ).then(registration => registration.installing.postMessage('probe'),
        error => observed.error = error.name);
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"observed.error === '' && observed.message?.available &&
                  observed.message.seed === 'page' &&
                  observed.message.byURL === 'page' &&
                  observed.message.allByURL &&
                  observed.message.mismatchedURLRejected &&
                  observed.message.urlError === '' &&
                  observed.message.written === 'yes' &&
                  observed.message.noDocument &&
                  observed.message.noWindowChangeHandler && observed.pageCookie === 'yes'"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true),
        "state: {:?}; task errors: {:?}",
        runtime.eval("JSON.stringify(observed)"),
        runtime.take_task_errors(),
    );
    drop(runtime);
    assert_eq!(fixture.finish(), vec!["/app/cookie-store.js".to_owned()]);
}

#[test]
fn service_worker_cookie_subscriptions_share_registration_between_window_and_worker() {
    let fixture = ServiceWorkerFixture::new();
    let mut runtime = JsRuntime::with_document_and_url(
        blank_html_document(),
        &format!("{}/app/page", fixture.origin),
    )
    .unwrap();
    runtime
        .eval(
            r#"
      globalThis.observed = {error: '', message: null, before: null, after: null};
      navigator.serviceWorker.addEventListener('message', event => observed.message = event.data);
      navigator.serviceWorker.register('/app/cookie-subscriptions.js').then(async registration => {
        observed.managerType = registration.cookies instanceof CookieStoreManager;
        observed.sameManager = registration.cookies === registration.cookies;
        observed.readOnlyManager = !Object.getOwnPropertyDescriptor(
          ServiceWorkerRegistration.prototype, 'cookies').set;
        await registration.cookies.subscribe([{}]);
        const defaults = await registration.cookies.getSubscriptions();
        observed.omittedName = defaults.length === 1 && !('name' in defaults[0]);
        await registration.cookies.unsubscribe(defaults);
        try { await registration.cookies.subscribe({name: 'invalid'}); }
        catch (error) { observed.nonIterableRejected = error instanceof TypeError; }
        await registration.cookies.subscribe(new Set([{name: 'page', url: '/app/page'}]));
        observed.before = await registration.cookies.getSubscriptions();
        try { await registration.cookies.subscribe([{url: '/outside'}]); }
        catch (error) { observed.outsideScopeRejected = error instanceof TypeError; }
        registration.installing.postMessage('probe');
      }).catch(error => observed.error = error.name);
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"observed.error === '' && observed.managerType && observed.sameManager &&
                   observed.readOnlyManager && observed.outsideScopeRejected &&
                   observed.omittedName && observed.nonIterableRejected &&
                   observed.before?.length === 1 && observed.before[0].name === 'page' &&
                   observed.message?.managerType && observed.message?.sameManager &&
                   observed.message.before?.length === 1 &&
                   observed.message.before[0].name === 'page' &&
                   observed.message.after?.length === 2 &&
                   observed.message.after[1].name === 'worker' &&
                   observed.message.remaining?.length === 1 &&
                   observed.message.remaining[0].name === 'worker'"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true),
        "state: {:?}; task errors: {:?}",
        runtime.eval("JSON.stringify(observed)"),
        runtime.take_task_errors(),
    );
    drop(runtime);
    assert_eq!(
        fixture.finish(),
        vec!["/app/cookie-subscriptions.js".to_owned()]
    );
}

#[test]
fn service_worker_registration_tracks_local_lifecycle_and_self_messages() {
    let fixture = ServiceWorkerFixture::new();
    let mut runtime = JsRuntime::with_document_and_url(
        blank_html_document(),
        &format!("{}/app/page", fixture.origin),
    )
    .unwrap();
    runtime.eval(r#"
      globalThis.observed = null;
      navigator.serviceWorker.onmessage = event => globalThis.observed = event.data;
      navigator.serviceWorker.register('/app/cookie-subscriptions.js');
      navigator.serviceWorker.ready.then(registration => registration.active.postMessage('lifecycle'));
    "#).unwrap();
    runtime.run_until_idle().unwrap();
    let errors = runtime.take_task_errors();
    assert!(errors.is_empty(), "task errors: {errors:?}");
    assert_eq!(
        runtime
            .eval(r#"JSON.stringify(observed)"#)
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        r#"{"lifecycle":["installing","activating"],"active":"activated","installing":null,"waiting":null,"selfSource":true,"value":"cloned"}"#
    );
    assert!(runtime.take_task_errors().is_empty());
    drop(runtime);
    assert_eq!(
        fixture.finish(),
        vec!["/app/cookie-subscriptions.js".to_owned()]
    );
}

#[test]
fn service_worker_update_refetches_changed_script_and_retains_cookie_subscriptions() {
    assert_service_worker_update("/app/updating.js", true, None);
}

#[test]
fn service_worker_update_refetches_unchanged_script_without_reinstalling() {
    assert_service_worker_update("/app/worker.js", false, None);
}

#[test]
fn service_worker_update_detects_changed_import_with_unchanged_main() {
    assert_service_worker_update(
        "/app/importing-change.js",
        true,
        Some("/app/changing-import.js"),
    );
}

#[test]
fn service_worker_update_preserves_worker_when_main_and_import_are_unchanged() {
    assert_service_worker_update("/app/importing-stable.js", false, Some("/app/worker.js"));
}

#[test]
fn service_worker_update_ignores_bad_old_import_when_other_resources_are_unchanged() {
    assert_service_worker_update(
        "/app/importing-missing.js",
        false,
        Some("/app/disappearing.js"),
    );
}

#[test]
fn service_worker_update_reuses_bad_import_response_when_another_import_changed() {
    let fixture = ServiceWorkerFixture::new();
    let mut runtime = JsRuntime::with_document_and_url(
        blank_html_document(),
        &format!("{}/app/page", fixture.origin),
    )
    .unwrap();
    runtime
        .eval(
            r#"
        globalThis.outcome = 'pending';
        navigator.serviceWorker.register('/app/importing-changed-and-missing.js');
        navigator.serviceWorker.ready.then(async registration => {
            globalThis.reg = registration;
            globalThis.old = registration.active;
            try { await registration.update(); outcome = 'resolved'; }
            catch (error) { outcome = error.name; }
        });
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(eval_str(&mut runtime, "outcome"), "TypeError");
    assert_eq!(
        runtime
            .eval("reg.active === old && old.state === 'activated'")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    assert!(runtime.take_task_errors().is_empty());
    drop(runtime);
    assert_eq!(
        fixture.finish(),
        vec![
            "/app/importing-changed-and-missing.js",
            "/app/changing-import.js",
            "/app/disappearing.js",
            "/app/importing-changed-and-missing.js",
            "/app/changing-import.js",
            "/app/disappearing.js"
        ]
    );
}

#[test]
fn service_worker_global_update_settles_in_calling_realm() {
    assert_service_worker_global_registration("update");
}

#[test]
fn service_worker_global_unregister_settles_before_worker_retirement() {
    assert_service_worker_global_registration("unregister");
}

fn assert_service_worker_global_registration(operation: &str) {
    let fixture = ServiceWorkerFixture::new();
    let mut runtime = JsRuntime::with_document_and_url(
        blank_html_document(),
        &format!("{}/app/page", fixture.origin),
    )
    .unwrap();
    runtime
        .eval(&format!(
            r#"
        globalThis.reply = null;
        navigator.serviceWorker.onmessage = event => reply = event.data;
        navigator.serviceWorker.register('/app/registration.js');
        navigator.serviceWorker.ready.then(registration => {{
            globalThis.reg = registration;
            globalThis.old = registration.active;
            registration.active.postMessage({operation:?});
        }});
    "#
        ))
        .unwrap();
    boa_gc::force_collect();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        eval_str(&mut runtime, "JSON.stringify(reply)"),
        r#"{"result":true,"version":1}"#
    );
    if operation == "update" {
        assert_eq!(
            runtime
                .eval("reg.active !== old && reg.active.state === 'activated'")
                .unwrap()
                .as_boolean(),
            Some(true)
        );
    } else {
        runtime.eval("navigator.serviceWorker.getRegistrations().then(items => globalThis.count = items.length);").unwrap();
        runtime.run_until_idle().unwrap();
        assert_eq!(runtime.eval("count").unwrap().as_number(), Some(0.0));
    }
    assert!(runtime.take_task_errors().is_empty());
    drop(runtime);
    assert_eq!(
        fixture.finish(),
        vec!["/app/registration.js".to_owned(); if operation == "update" { 2 } else { 1 }]
    );
}

#[test]
fn service_worker_stale_document_update_rejects_before_fetch_after_reregistration() {
    let fixture = ServiceWorkerFixture::new();
    let mut runtime = JsRuntime::with_document_and_url(
        blank_html_document(),
        &format!("{}/app/page", fixture.origin),
    )
    .unwrap();
    runtime.eval(r#"
        navigator.serviceWorker.register('/app/updating.js').then(registration => globalThis.first = registration);
        const frame = document.createElement('iframe');
        frame.src = '/app/empty-frame.html';
        document.body.appendChild(frame);
    "#).unwrap();
    runtime.run_until_idle().unwrap();
    let child = runtime
        .host_state
        .borrow()
        .iframe_documents
        .values()
        .next()
        .unwrap()
        .document
        .clone();
    runtime.eval_in_document_realm(child.identity(), "navigator.serviceWorker.register('/app/updating.js').then(registration => globalThis.oldRegistration = registration);").unwrap();
    runtime.run_until_idle().unwrap();
    runtime.eval(r#"
        first.unregister().then(() => navigator.serviceWorker.register('/app/worker.js')).then(async registration => {
            globalThis.replacement = registration;
            await registration.cookies.subscribe([{name: 'kept'}]);
        });
    "#).unwrap();
    runtime.run_until_idle().unwrap();
    runtime.eval_in_document_realm(child.identity(), "globalThis.outcome = 'pending'; oldRegistration.update().then(() => outcome = 'resolved', error => outcome = error.name);").unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        eval_str(&mut runtime, "frame.contentWindow.outcome"),
        "InvalidStateError"
    );
    runtime.eval("replacement.cookies.getSubscriptions().then(items => globalThis.names = items.map(item => item.name).join(','));").unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(eval_str(&mut runtime, "names"), "kept");
    assert_eq!(
        eval_str(&mut runtime, "replacement.active.state"),
        "activated"
    );
    assert!(runtime.take_task_errors().is_empty());
    drop(runtime);
    assert_eq!(
        fixture.finish(),
        vec![
            "/app/updating.js",
            "/app/empty-frame.html",
            "/app/updating.js",
            "/app/worker.js"
        ]
    );
}

#[test]
fn service_worker_failed_initial_start_rolls_back_reserved_registration() {
    let fixture = ServiceWorkerFixture::new();
    let mut runtime = JsRuntime::with_document_and_url(
        blank_html_document(),
        &format!("{}/app/page", fixture.origin),
    )
    .unwrap();
    runtime.eval("navigator.serviceWorker.register('/app/syntax.js').catch(error => globalThis.failure = error.name);").unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(eval_str(&mut runtime, "failure"), "TypeError");
    let scope = format!("{}/app/", fixture.origin);
    let top = crate::http::SchemefulSite::from_url(&scope.parse().unwrap());
    assert!(
        runtime
            .host_state
            .borrow()
            .cookie_store
            .lock()
            .unwrap()
            .service_worker_registration_id(&scope, &top)
            .is_none()
    );
    assert!(runtime.take_task_errors().is_empty());
    drop(runtime);
    assert_eq!(fixture.finish(), vec!["/app/syntax.js"]);
}

#[test]
fn service_worker_old_registration_cannot_change_replacement_subscriptions() {
    let fixture = ServiceWorkerFixture::new();
    let mut runtime = JsRuntime::with_document_and_url(
        blank_html_document(),
        &format!("{}/app/page", fixture.origin),
    )
    .unwrap();
    runtime
        .eval(
            r#"
        globalThis.reply = '';
        navigator.serviceWorker.onmessage = event => reply = event.data;
        navigator.serviceWorker.register('/app/stale-registration.js');
        navigator.serviceWorker.ready.then(registration => {
            globalThis.oldRegistration = registration;
            globalThis.oldWorker = registration.active;
            oldWorker.postMessage('unregister');
        });
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(eval_str(&mut runtime, "reply"), "resolved");
    runtime.eval("navigator.serviceWorker.register('/app/worker.js').then(registration => globalThis.replacement = registration);").unwrap();
    runtime.run_until_idle().unwrap();
    runtime
        .eval(
            r#"
        oldWorker.postMessage('subscribe');
        globalThis.oldManagerOutcome = 'pending';
        oldRegistration.cookies.subscribe([{name: 'window-ghost'}]).then(
            () => oldManagerOutcome = 'resolved', error => oldManagerOutcome = error.name);
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(eval_str(&mut runtime, "reply"), "TypeError");
    assert_eq!(eval_str(&mut runtime, "oldManagerOutcome"), "TypeError");
    runtime.eval("replacement.cookies.getSubscriptions().then(items => globalThis.names = items.map(item => item.name).join(','));").unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(eval_str(&mut runtime, "names"), "");
    assert!(runtime.take_task_errors().is_empty());
    drop(runtime);
    assert_eq!(
        fixture.finish(),
        vec!["/app/stale-registration.js", "/app/worker.js"]
    );
}

#[test]
fn service_worker_update_network_failure_retains_active_and_subscriptions() {
    assert_service_worker_update_failure("/app/update-network.js", "TypeError");
}

#[test]
fn service_worker_update_mime_failure_retains_active_and_subscriptions() {
    assert_service_worker_update_failure("/app/update-mime.js", "SecurityError");
}

#[test]
fn service_worker_update_scope_failure_retains_active_and_subscriptions() {
    assert_service_worker_update_failure("/app/update-scope.js", "SecurityError");
}

#[test]
fn service_worker_update_parse_failure_retains_active_and_subscriptions() {
    assert_service_worker_update_failure("/app/update-syntax.js", "TypeError");
}

#[test]
fn service_worker_update_install_rejection_retains_active_and_subscriptions() {
    assert_service_worker_update_failure("/app/update-reject.js", "resolved");
}

fn assert_service_worker_update_failure(path: &str, outcome: &str) {
    let fixture = ServiceWorkerFixture::new();
    let mut runtime = JsRuntime::with_document_and_url(
        blank_html_document(),
        &format!("{}/app/page", fixture.origin),
    )
    .unwrap();
    runtime
        .eval(&format!(
            r#"
        globalThis.outcome = 'pending';
        navigator.serviceWorker.register({path:?});
        navigator.serviceWorker.ready.then(async registration => {{
            globalThis.reg = registration;
            globalThis.old = registration.active;
            await registration.cookies.subscribe([{{name: 'kept'}}]);
            try {{ await registration.update(); outcome = 'resolved'; }}
            catch (error) {{ outcome = error.name; }}
        }});
    "#
        ))
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(eval_str(&mut runtime, "outcome"), outcome);
    assert_eq!(runtime.eval("reg.active === old && old.state === 'activated' && reg.installing === null && reg.waiting === null").unwrap().as_boolean(), Some(true));
    runtime.eval("reg.cookies.getSubscriptions().then(items => globalThis.saved = items.map(item => item.name).join(','));").unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(eval_str(&mut runtime, "saved"), "kept");
    assert!(runtime.take_task_errors().is_empty());
    drop(runtime);
    assert_eq!(fixture.finish(), vec![path.to_owned(), path.to_owned()]);
}

#[test]
fn service_worker_update_waits_for_install_and_coalesces_concurrent_calls() {
    let fixture = ServiceWorkerFixture::new();
    let mut runtime = JsRuntime::with_document_and_url(
        blank_html_document(),
        &format!("{}/app/page", fixture.origin),
    )
    .unwrap();
    runtime
        .eval(
            r#"
        globalThis.updates = 0;
        navigator.serviceWorker.register('/app/update-wait.js');
        navigator.serviceWorker.ready.then(async registration => {
            globalThis.reg = registration;
            globalThis.old = registration.active;
            await Promise.all([registration.update(), registration.update()]);
            registration.onupdatefound = () => updates++;
        });
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("reg.active === old && reg.installing.state === 'installing' && updates === 1")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    runtime
        .eval("reg.installing.postMessage('release');")
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(runtime.eval("reg.active !== old && reg.active.state === 'activated' && old.state === 'redundant'").unwrap().as_boolean(), Some(true));
    assert!(runtime.take_task_errors().is_empty());
    drop(runtime);
    assert_eq!(fixture.finish(), vec!["/app/update-wait.js".to_owned(); 2]);
}

#[test]
fn service_worker_update_rejects_after_unregister() {
    let fixture = ServiceWorkerFixture::new();
    let mut runtime = JsRuntime::with_document_and_url(
        blank_html_document(),
        &format!("{}/app/page", fixture.origin),
    )
    .unwrap();
    runtime
        .eval(
            r#"
      globalThis.updateOutcome = 'pending';
      navigator.serviceWorker.register('/app/worker.js');
      navigator.serviceWorker.ready.then(async registration => {
        await registration.unregister();
        try {
          await registration.update();
          updateOutcome = 'resolved';
        } catch (error) {
          updateOutcome = error.name;
        }
      });
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(eval_str(&mut runtime, "updateOutcome"), "InvalidStateError");
    assert!(runtime.take_task_errors().is_empty());
    drop(runtime);
    assert_eq!(fixture.finish(), vec!["/app/worker.js".to_owned()]);
}

fn assert_service_worker_update(path: &str, changed: bool, import: Option<&str>) {
    let fixture = ServiceWorkerFixture::new();
    let mut runtime = JsRuntime::with_document_and_url(
        blank_html_document(),
        &format!("{}/app/page", fixture.origin),
    )
    .unwrap();
    runtime
        .eval(&format!(
            r#"
      globalThis.updates = 0;
      globalThis.updateError = '';
      navigator.serviceWorker.register({path:?});
      navigator.serviceWorker.ready.then(async registration => {{
        globalThis.registrationUnderTest = registration;
        globalThis.previousWorker = registration.active;
        await registration.cookies.subscribe([{{name: 'kept'}}]);
        registration.onupdatefound = () => updates++;
        await registration.update();
      }}).catch(error => updateError = error.name + ': ' + error.message);
    "#
        ))
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(eval_str(&mut runtime, "updateError"), "");
    assert_eq!(
        runtime
            .eval("registrationUnderTest.active !== previousWorker")
            .unwrap()
            .as_boolean(),
        Some(changed)
    );
    assert_eq!(
        runtime.eval("updates").unwrap().as_number(),
        Some(if changed { 1.0 } else { 0.0 })
    );
    assert_eq!(
        eval_str(&mut runtime, "registrationUnderTest.active.state"),
        "activated"
    );
    runtime.eval("registrationUnderTest.cookies.getSubscriptions().then(items => globalThis.savedNames = items.map(item => item.name).join(','));").unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(eval_str(&mut runtime, "savedNames"), "kept");
    if changed {
        runtime.eval("navigator.serviceWorker.onmessage = event => globalThis.newVersion = event.data.version; registrationUnderTest.active.postMessage('version');").unwrap();
        runtime.run_until_idle().unwrap();
        assert_eq!(runtime.eval("newVersion").unwrap().as_number(), Some(2.0));
    }
    assert!(runtime.take_task_errors().is_empty());
    drop(runtime);
    let mut expected = vec![path.to_owned()];
    if let Some(import) = import {
        expected.push(import.to_owned());
    }
    expected.extend(expected.clone());
    assert_eq!(fixture.finish(), expected);
}

#[test]
fn service_worker_transferred_message_port_delivers_bidirectional_cloned_messages() {
    let fixture = ServiceWorkerFixture::new();
    let mut runtime = JsRuntime::with_document_and_url(
        blank_html_document(),
        &format!("{}/app/page", fixture.origin),
    )
    .unwrap();
    runtime
        .eval(
            r#"
      globalThis.observed = [];
      globalThis.transferError = '';
      navigator.serviceWorker.register('/app/message-port.js');
      navigator.serviceWorker.ready.then(registration => {
        const channel = new MessageChannel();
        globalThis.keptPort = channel.port1;
        channel.port1.onmessage = event => {
          observed.push(event.data);
          if (event.data.ready) {
            const data = {value: 'cloned'};
            channel.port1.postMessage(data);
            data.value = 'mutated';
          }
        };
        registration.active.postMessage({port: channel.port2}, [channel.port2]);
        channel.port2.postMessage({value: 'detached'});
      }).catch(error => { transferError = error.name + ': ' + error.message; });
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    let errors = runtime.take_task_errors();
    assert!(errors.is_empty(), "task errors: {errors:?}");
    assert_eq!(
        runtime
            .eval("transferError")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        ""
    );
    assert_eq!(
        runtime
            .eval("JSON.stringify(observed)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        r#"[{"ready":true},{"value":"cloned","samePort":true,"localPort":true,"localData":true}]"#
    );
    drop(runtime);
    assert_eq!(fixture.finish(), vec!["/app/message-port.js".to_owned()]);
}

#[test]
fn service_worker_message_port_preserves_queued_messages_in_receiver_realm() {
    let fixture = ServiceWorkerFixture::new();
    let mut runtime = JsRuntime::with_document_and_url(
        blank_html_document(),
        &format!("{}/app/page", fixture.origin),
    )
    .unwrap();
    runtime
        .eval(
            r#"
      globalThis.observed = [];
      navigator.serviceWorker.register('/app/message-port.js');
      navigator.serviceWorker.ready.then(registration => {
        const channel = new MessageChannel();
        globalThis.keptPort = channel.port1;
        channel.port1.onmessage = event => observed.push(event.data);
        const data = {value: 'queued'};
        channel.port1.postMessage(data);
        data.value = 'mutated';
        registration.active.postMessage({port: channel.port2}, [channel.port2]);
      });
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.eval(r#"observed.length === 2 && observed.some(item => item.ready) &&
      observed.some(item => item.value === 'queued' && item.samePort && item.localPort && item.localData)"#)
      .unwrap().as_boolean().unwrap());
    assert!(runtime.take_task_errors().is_empty());
    drop(runtime);
    assert_eq!(fixture.finish(), vec!["/app/message-port.js".to_owned()]);
}

#[test]
fn dedicated_worker_startup_message_port_survives_gc_and_receives_replies() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            r#"
      globalThis.observed = [];
      const source = encodeURIComponent(`
        const channel = new MessageChannel();
        channel.port1.onmessage = event => channel.port1.postMessage({value: event.data.value});
        postMessage({port: channel.port2}, [channel.port2]);
        channel.port2.postMessage({value: 'detached'});
      `);
      globalThis.worker = new Worker('data:text/javascript,' + source);
      worker.onmessage = event => {
        globalThis.receivedPort = event.ports[0];
        observed.push({localPort: receivedPort instanceof MessagePort,
          samePort: event.data.port === receivedPort});
        receivedPort.onmessage = reply => observed.push(reply.data);
      };
    "#,
        )
        .unwrap();
    boa_gc::force_collect();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        eval_str(&mut runtime, "JSON.stringify(observed)"),
        r#"[{"localPort":true,"samePort":true}]"#
    );
    runtime.eval("const payload = {value: 'cloned'}; receivedPort.postMessage(payload); payload.value = 'mutated';").unwrap();
    boa_gc::force_collect();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        eval_str(&mut runtime, "JSON.stringify(observed)"),
        r#"[{"localPort":true,"samePort":true},{"value":"cloned"}]"#
    );
    assert!(runtime.take_task_errors().is_empty());
}

#[test]
fn service_worker_cookiechange_only_fires_for_subscribed_cookies() {
    let fixture = ServiceWorkerFixture::new();
    let mut runtime = JsRuntime::with_document_and_url(
        blank_html_document(),
        &format!("{}/app/page", fixture.origin),
    )
    .unwrap();
    runtime
        .eval(
            r#"
      globalThis.observed = {error: '', messages: []};
      navigator.serviceWorker.addEventListener('message', event => {
        observed.messages.push(event.data);
        if (event.data.armed) {
          cookieStore.set('untracked', 'skip').then(() => cookieStore.set('watched', 'yes'));
        }
      });
      navigator.serviceWorker.register('/app/cookie-change.js').then(
        registration => registration.installing.postMessage('arm'),
        error => observed.error = error.name);
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"observed.error === '' && observed.messages.length === 2 &&
                   observed.messages[0].armed && observed.messages[1].extendable &&
                   JSON.stringify(observed.messages[1].changed) ===
                     JSON.stringify([['watched', 'yes']]) &&
                   observed.messages[1].deleted.length === 0"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true),
        "state: {:?}; task errors: {:?}",
        runtime.eval("JSON.stringify(observed)"),
        runtime.take_task_errors(),
    );
    drop(runtime);
    assert_eq!(fixture.finish(), vec!["/app/cookie-change.js".to_owned()]);
}

#[test]
fn service_worker_cookie_store_does_not_inherit_cross_site_document_access() {
    let fixture = ServiceWorkerFixture::new();
    let mut runtime = JsRuntime::with_document_and_url(
        blank_html_document(),
        &format!("{}/app/page", fixture.origin),
    )
    .unwrap();
    runtime
        .eval("cookieStore.set({name: 'seed', value: 'page', path: '/'})")
        .unwrap();
    runtime.run_until_idle().unwrap();
    // Model the creator's cross-site ancestry on the request context that a
    // Worker inherits. The synthetic Worker document itself has no iframe.
    let inherited_context =
        crate::http::CookieRequestContext::new(Some(fixture.origin.parse().unwrap()), None, true);
    runtime
        .host_state
        .borrow_mut()
        .worker_cookie_request_context = Some(inherited_context);
    runtime
        .eval(
            r#"
      globalThis.observed = {error: '', message: null, pageCookie: null};
      navigator.serviceWorker.addEventListener('message', event => observed.message = event.data);
      navigator.serviceWorker.register('/app/cookie-store.js').then(
        registration => registration.installing.postMessage('probe'),
        error => observed.error = error.name);
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    runtime
        .host_state
        .borrow_mut()
        .worker_cookie_request_context = None;
    runtime
        .eval("cookieStore.get('from-worker').then(cookie => observed.pageCookie = cookie?.value)")
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"observed.error === '' && observed.message?.available &&
                  observed.message.seed === undefined &&
                  observed.message.written === undefined &&
                  observed.pageCookie === undefined"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true),
        "state: {:?}; task errors: {:?}",
        runtime.eval("JSON.stringify(observed)"),
        runtime.take_task_errors(),
    );
    drop(runtime);
    assert_eq!(fixture.finish(), vec!["/app/cookie-store.js".to_owned()]);
}

#[test]
fn service_worker_trusted_type_failure_precedes_fetch_and_script_failure() {
    let fixture = ServiceWorkerFixture::new();
    let mut runtime = JsRuntime::with_document_and_url(
        blank_html_document(),
        &format!("{}/app/page", fixture.origin),
    )
    .unwrap();
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    runtime
        .eval(
            r#"
      globalThis.results = {};
      const policy = trustedTypes.createPolicy('urls', {createScriptURL: value => value});
      navigator.serviceWorker.register('/sw.js').then(
        () => results.raw = 'resolved', error => results.raw = [error.name, error.message]);
      navigator.serviceWorker.register(policy.createScriptURL('/missing.js')).then(
        () => results.missing = 'resolved', error => results.missing = [error.name, error.message]);
      navigator.serviceWorker.register(policy.createScriptURL('/syntax.js')).then(
        () => results.syntax = 'resolved', error => results.syntax = [error.name, error.message]);
      navigator.serviceWorker.register(policy.createScriptURL('/plain.js')).then(
        () => results.plain = 'resolved', error => results.plain = [error.name, error.message]);
      navigator.serviceWorker.register(policy.createScriptURL('/sw.js')).then(
        () => results.valid = 'resolved', error => results.valid = error.name);
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"
          results.raw[0] === 'TypeError' &&
          results.raw[1] === 'Trusted Type is required for ServiceWorkerContainer register' &&
          results.missing[0] === 'TypeError' &&
          results.missing[1] === 'Service worker script could not be fetched or evaluated.' &&
          results.syntax[0] === 'TypeError' &&
          results.syntax[1] === results.missing[1] &&
          results.plain[0] === 'SecurityError' &&
          results.valid === 'resolved'
        "#
            )
            .unwrap()
            .as_boolean(),
        Some(true),
    );
    drop(runtime);
    assert_eq!(
        fixture.finish(),
        vec![
            "/missing.js".to_owned(),
            "/syntax.js".to_owned(),
            "/plain.js".to_owned(),
            "/sw.js".to_owned()
        ]
    );
}

#[test]
fn service_worker_rejects_encoded_separators_and_module_type_before_fetch() {
    let fixture = ServiceWorkerFixture::new();
    let mut runtime = JsRuntime::with_document_and_url(
        blank_html_document(),
        &format!("{}/app/page", fixture.origin),
    )
    .unwrap();
    runtime
        .eval(
            r#"
      const attempts = [
        navigator.serviceWorker.register('/app/sw%2Fbad.js'),
        navigator.serviceWorker.register('/app/sw%5cbad.js'),
        navigator.serviceWorker.register('/app/sw.js', {scope: '/app%2fwide/'}),
        navigator.serviceWorker.register('/app/sw.js', {scope: '/app%5Cwide/'}),
        navigator.serviceWorker.register('/app/sw.js', {type: 'module'}),
      ];
      globalThis.rejections = Promise.all(attempts.map(attempt =>
        attempt.then(() => 'resolved', error => error.name)));
      rejections.then(names => globalThis.rejectionNames = names);
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval(
                "rejectionNames.length === 5 && rejectionNames.every(name => name === 'TypeError')"
            )
            .unwrap()
            .as_boolean(),
        Some(true),
    );
    drop(runtime);
    assert!(fixture.finish().is_empty());
}

#[test]
fn service_worker_checks_response_allowed_scope_and_mime_after_fetch() {
    let fixture = ServiceWorkerFixture::new();
    // This test starts several workers through the fixture's HTTP server. Its
    // assertions cover response policy, not the page's five-second wall-clock limit.
    // Match the fixture's own request deadline so CI scheduling cannot abort
    // a registration before the server's timeout does.
    let mut runtime = JsRuntime::with_document_sandbox_and_url(
        blank_html_document(),
        SandboxConfig {
            timeout: ACCEPT_TIMEOUT,
            ..SandboxConfig::default()
        },
        &format!("{}/app/page", fixture.origin),
    )
    .unwrap();
    runtime
        .eval(
            r#"
      globalThis.results = {};
      globalThis.check = (name, promise) => promise.then(
        () => results[name] = 'resolved', error => results[name] = error.name);
    "#,
        )
        .unwrap();
    for registration in [
        "check('allowed', navigator.serviceWorker.register('/allowed/sw.js', {scope: '/'}))",
        "check('narrowAllowed', navigator.serviceWorker.register('/narrow/sw.js', {scope: '/narrow/sub/page/'}))",
        "check('defaultDenied', navigator.serviceWorker.register('/app/restricted.js', {scope: '/'}))",
        "check('headerDenied', navigator.serviceWorker.register('/narrow/sw.js', {scope: '/narrow/'}))",
        "check('invalidHeader', navigator.serviceWorker.register('/bad-allowed/sw.js', {scope: '/'}))",
        "check('crossOriginHeader', navigator.serviceWorker.register('/cross-allowed/sw.js', {scope: '/'}))",
        "check('mimeDenied', navigator.serviceWorker.register('/plain.js'))",
    ] {
        runtime.eval(registration).unwrap();
    }
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"
          results.allowed === 'resolved' && results.narrowAllowed === 'resolved' &&
          results.defaultDenied === 'SecurityError' &&
          results.headerDenied === 'SecurityError' &&
          results.invalidHeader === 'TypeError' &&
          results.crossOriginHeader === 'SecurityError' &&
          results.mimeDenied === 'SecurityError'
        "#
            )
            .unwrap()
            .as_boolean(),
        Some(true),
    );
    drop(runtime);
    assert_eq!(
        fixture.finish(),
        vec![
            "/allowed/sw.js",
            "/narrow/sw.js",
            "/app/restricted.js",
            "/narrow/sw.js",
            "/bad-allowed/sw.js",
            "/cross-allowed/sw.js",
            "/plain.js"
        ]
    );
}

#[test]
fn service_worker_script_request_has_header_and_never_follows_redirect() {
    let fixture = ServiceWorkerFixture::new();
    let mut runtime = JsRuntime::with_document_and_url(
        blank_html_document(),
        &format!("{}/app/page", fixture.origin),
    )
    .unwrap();
    runtime
        .eval(
            r#"
      navigator.serviceWorker.register('/redirect.js').then(
        () => globalThis.redirectResult = 'resolved',
        error => globalThis.redirectResult = error.name);
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("redirectResult === 'TypeError'")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    drop(runtime);
    let requests = fixture.finish_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/redirect.js");
    assert!(
        requests[0]
            .headers
            .to_ascii_lowercase()
            .contains("\r\nservice-worker: script\r\n")
    );
}

#[test]
fn stale_service_worker_registration_cannot_unregister_replacement() {
    let fixture = ServiceWorkerFixture::new();
    let mut runtime = JsRuntime::with_document_and_url(
        blank_html_document(),
        &format!("{}/app/page", fixture.origin),
    )
    .unwrap();
    runtime
        .eval(
            r#"
      globalThis.observed = {};
      navigator.serviceWorker.register('/app/worker.js').then(registration => {
        observed.old = registration;
      });
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    runtime
        .eval(
            r#"
      observed.old.unregister().then(result => {
        observed.firstUnregister = result;
        return navigator.serviceWorker.register('/app/worker-v2.js');
      }).then(registration => {
        observed.replacement = registration;
        return observed.old.unregister();
      }).then(result => {
        observed.staleUnregister = result;
        return navigator.serviceWorker.getRegistration('/app/page');
      }).then(registration => observed.stillRegistered = registration === observed.replacement);
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"
      observed.firstUnregister === true && observed.staleUnregister === false &&
      observed.stillRegistered && observed.replacement.active?.state === 'activated'
    "#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    drop(runtime);
    assert_eq!(
        fixture.finish(),
        vec!["/app/worker.js", "/app/worker-v2.js"]
    );
}

#[test]
fn replacement_preserves_active_until_activation_event_runs() {
    let fixture = ServiceWorkerFixture::new();
    let mut runtime = JsRuntime::with_document_and_url(
        blank_html_document(),
        &format!("{}/app/page", fixture.origin),
    )
    .unwrap();
    runtime.eval(r#"
      globalThis.observed = {states: [], messages: []};
      navigator.serviceWorker.addEventListener('message', event => observed.messages.push(event.data));
      navigator.serviceWorker.register('/app/worker.js').then(registration => observed.registration = registration);
    "#).unwrap();
    runtime.run_until_idle().unwrap();
    runtime
        .eval(
            r#"
      navigator.serviceWorker.register('/app/syntax.js', {scope: '/app/'}).then(
        () => observed.failedUpdate = 'resolved',
        error => observed.failedUpdate = error.name);
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"
      observed.failedUpdate === 'TypeError' &&
      observed.registration.active?.state === 'activated'
    "#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    runtime
        .eval(
            r#"
      const registration = observed.registration;
      const oldWorker = registration.active;
      observed.oldWorker = oldWorker;
      registration.addEventListener('updatefound', () => {
        observed.activeDuringUpdate = registration.active === oldWorker &&
          oldWorker.state === 'activated' && !oldWorker.__terminated;
        observed.newWorker = registration.installing;
        observed.newWorker.addEventListener('statechange', () =>
          observed.states.push(observed.newWorker.state));
      });
      navigator.serviceWorker.register('/app/worker-v2.js').then(updated => {
        observed.sameRegistration = updated === registration;
        observed.oldActiveWhileInstalling = updated.active === oldWorker &&
          updated.installing === observed.newWorker;
      });
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    runtime
        .eval("observed.registration.active.postMessage('probe')")
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"
      observed.activeDuringUpdate && observed.sameRegistration &&
      observed.oldActiveWhileInstalling && observed.oldWorker.state === 'redundant' &&
      observed.registration.active === observed.newWorker &&
      observed.newWorker.state === 'activated' &&
      observed.states.join(',') === 'installed,activating,activated' &&
      observed.messages.length === 1 && observed.messages[0].activated &&
      observed.messages[0].url.endsWith('/app/worker-v2.js')
    "#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    drop(runtime);
    assert_eq!(
        fixture.finish(),
        vec!["/app/worker.js", "/app/syntax.js", "/app/worker-v2.js"]
    );
}

#[test]
fn service_worker_host_termination_during_activate_does_not_reborrow_or_mark_activated() {
    let fixture = ServiceWorkerFixture::new();
    let mut runtime = JsRuntime::with_document_and_url(
        blank_html_document(),
        &format!("{}/app/page", fixture.origin),
    )
    .unwrap();
    runtime
        .eval(
            r#"
      globalThis.observed = {ready: false, error: ''};
      navigator.serviceWorker.ready.then(() => observed.ready = true);
      navigator.serviceWorker.register('/app/close-on-activate.js').then(
        registration => {
          observed.registration = registration;
          observed.worker = registration.installing;
        },
        error => observed.error = error.name);
    "#,
        )
        .unwrap();
    runtime.run_jobs().unwrap();
    for entry in runtime.host_state.borrow().workers.values() {
        entry
            .borrow_mut()
            .runtime
            .context
            .register_global_builtin_callable(
                js_string!("terminateDuringActivate"),
                0,
                NativeFunction::from_fn_ptr(worker_close_native),
            )
            .unwrap();
    }
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"
      observed.error === '' && observed.registration && observed.worker &&
      observed.worker.state === 'redundant' && observed.worker.__terminated &&
      observed.registration.installing === null &&
      observed.registration.waiting === null &&
      observed.registration.active === null &&
      navigator.serviceWorker.controller === null && !observed.ready
    "#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    assert!(runtime.host_state.borrow().workers.is_empty());
    drop(runtime);
    assert_eq!(fixture.finish(), vec!["/app/close-on-activate.js"]);
}

fn eval_str(runtime: &mut JsRuntime, source: &str) -> String {
    runtime
        .eval(source)
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped()
}

fn assert_lifetime_transition(path: &str, pending: &str, final_state: &str) {
    let fixture = ServiceWorkerFixture::new();
    let mut runtime = JsRuntime::with_document_and_url(
        blank_html_document(),
        &format!("{}/app/page", fixture.origin),
    )
    .unwrap();
    runtime
        .eval(&format!(
            r#"
      globalThis.lifetime = {{messages: [], ready: false}};
      navigator.serviceWorker.ready.then(() => lifetime.ready = true);
      navigator.serviceWorker.onmessage = event => lifetime.messages.push(event.data);
      navigator.serviceWorker.register({path:?}).then(registration => {{
        lifetime.registration = registration;
        lifetime.worker = registration.installing;
      }});
    "#
        ))
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(eval_str(&mut runtime, "lifetime.worker.state"), pending);
    assert_eq!(
        runtime.eval("lifetime.ready").unwrap().as_boolean(),
        Some(false)
    );
    boa_gc::force_collect();
    runtime
        .eval("lifetime.worker.postMessage('probe')")
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(eval_str(&mut runtime, "lifetime.worker.state"), pending);
    assert_eq!(runtime.eval(
        "lifetime.messages.length === 1 && lifetime.messages[0].extension === 'allowed' && lifetime.messages[0].synthetic === 'InvalidStateError' && lifetime.messages[0].trusted && lifetime.messages[0].branded && lifetime.messages[0].retained"
    ).unwrap().as_boolean(), Some(true), "messages: {}; task errors: {:?}",
        eval_str(&mut runtime, "JSON.stringify(lifetime.messages)"), runtime.take_task_errors());
    runtime
        .eval("lifetime.worker.postMessage('release')")
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(eval_str(&mut runtime, "lifetime.worker.state"), final_state);
    if final_state == "activated" {
        assert_eq!(
            runtime.eval("lifetime.ready").unwrap().as_boolean(),
            Some(true)
        );
        runtime.eval("lifetime.worker.postMessage('late')").unwrap();
        runtime.run_until_idle().unwrap();
        assert_eq!(
            eval_str(&mut runtime, "lifetime.messages.at(-1).extension"),
            "InvalidStateError"
        );
    } else {
        assert_eq!(runtime.eval(
            "!lifetime.ready && lifetime.registration.active === null && navigator.serviceWorker.controller === null"
        ).unwrap().as_boolean(), Some(true));
    }
    drop(runtime);
    assert_eq!(fixture.finish(), vec![path.to_owned()]);
}

#[test]
fn service_worker_install_waits_for_lifetime_promises() {
    assert_lifetime_transition("/app/install-wait.js", "installing", "activated");
}

#[test]
fn service_worker_install_rejection_waits_for_all_lifetime_promises() {
    assert_lifetime_transition("/app/install-reject.js", "installing", "redundant");
}

#[test]
fn service_worker_activation_waits_for_lifetime_promises() {
    assert_lifetime_transition("/app/activate-wait.js", "activating", "activated");
}

#[test]
fn service_worker_activation_rejection_still_finishes_after_all_promises_settle() {
    assert_lifetime_transition("/app/activate-reject.js", "activating", "activated");
}

#[test]
fn service_worker_iframe_messages_use_the_client_origin_in_both_directions() {
    let top = ServiceWorkerFixture::new();
    let child = ServiceWorkerFixture::new();
    let mut runtime =
        JsRuntime::with_document_and_url(blank_html_document(), &format!("{}/page", top.origin))
            .unwrap();
    runtime.eval(&format!(r#"
      globalThis.originReports = [];
      addEventListener('message', event => originReports.push({{frameOrigin: event.origin, ...event.data}}));
      const frame = document.createElement('iframe');
      frame.src = {url:?}; document.body.appendChild(frame);
    "#, url=format!("{}/app/frame.html", child.origin))).unwrap();
    runtime.run_until_idle().unwrap();
    let observed: serde_json::Value =
        serde_json::from_str(&eval_str(&mut runtime, "JSON.stringify(originReports)")).unwrap();
    assert_eq!(
        observed,
        serde_json::json!([{
            "frameOrigin": child.origin, "incomingOrigin": child.origin,
            "workerOrigin": child.origin, "outgoingOrigin": child.origin, "sameSource": true,
        }]),
        "task errors: {:?}",
        runtime.take_task_errors()
    );
    drop(runtime);
    assert_eq!(child.finish(), vec!["/app/frame.html", "/app/origin.js"]);
    assert!(top.finish().is_empty());
}
