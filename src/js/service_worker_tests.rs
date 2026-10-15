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
      isolated: typeof document === 'undefined' && typeof close === 'undefined' &&
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
        let listener = bind_loopback().unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stopped = Arc::clone(&stopped);
        let worker = FixtureWorker::spawn(move || {
            let deadline = Instant::now() + ACCEPT_TIMEOUT;
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
                let (body, policy, allowed) = match path.as_str() {
                    "/sw.js" | "/app/sw.js" => (
                        SERVICE_WORKER_SOURCE,
                        "require-trusted-types-for 'script'",
                        "",
                    ),
                    "/app/worker.js" | "/app/worker-v2.js" => (LIFECYCLE_WORKER_SOURCE, "", ""),
                    "/app/close-on-activate.js" => (CLOSE_ON_ACTIVATE_SOURCE, "", ""),
                    "/allowed/sw.js" => ("", "", "/"),
                    "/narrow/sw.js" => ("", "", "/narrow/sub/"),
                    "/bad-allowed/sw.js" => ("", "", "http://["),
                    "/cross-allowed/sw.js" => ("", "", "https://other.example.test/"),
                    "/app/restricted.js" | "/target.js" => ("", "", ""),
                    "/syntax.js" | "/app/syntax.js" => ("function (", "", ""),
                    "/redirect.js" => {
                        stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: /target.js\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                        continue;
                    }
                    "/plain.js" => {
                        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                        continue;
                    }
                    _ => {
                        stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                        continue;
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
                write!(stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\n{policy_header}{allowed_header}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                ).unwrap();
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
          observed.messages[0].isolated"#
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
    let mut runtime = JsRuntime::with_document_and_url(
        blank_html_document(),
        &format!("{}/app/page", fixture.origin),
    )
    .unwrap();
    runtime.eval(r#"
      globalThis.results = {};
      const check = (name, promise) => promise.then(
        () => results[name] = 'resolved', error => results[name] = error.name);
      check('allowed', navigator.serviceWorker.register('/allowed/sw.js', {scope: '/'}));
      check('narrowAllowed', navigator.serviceWorker.register('/narrow/sw.js', {scope: '/narrow/sub/page/'}));
      check('defaultDenied', navigator.serviceWorker.register('/app/restricted.js', {scope: '/'}));
      check('headerDenied', navigator.serviceWorker.register('/narrow/sw.js', {scope: '/narrow/'}));
      check('invalidHeader', navigator.serviceWorker.register('/bad-allowed/sw.js', {scope: '/'}));
      check('crossOriginHeader', navigator.serviceWorker.register('/cross-allowed/sw.js', {scope: '/'}));
      check('mimeDenied', navigator.serviceWorker.register('/plain.js'));
    "#).unwrap();
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
