//! Cookie Store reads share the network and document cookie jar.

use omoikane::{
    html::TreeBuilder,
    http::{HttpParseError, HttpRequest, HttpResponse, HttpTransport},
    js::JsRuntime,
};
use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

struct CookieTransport;

impl HttpTransport for CookieTransport {
    fn send(
        &self,
        request: &HttpRequest,
        _: Option<Duration>,
    ) -> Result<HttpResponse, HttpParseError> {
        assert_eq!(request.url().path(), "/set");
        HttpResponse::parse(&mut Cursor::new(
            b"HTTP/1.1 204 No Content\r\n\
              Set-Cookie: visible=server; Path=/\r\n\
              Set-Cookie: scoped=inner; Path=/inside\r\n\
              Set-Cookie: secret=hidden; HttpOnly; Path=/\r\n\
              Set-Cookie: expired=gone; Max-Age=0; Path=/\r\n\
              Set-Cookie: foreign=wrong; Domain=other.test; Path=/\r\n\
              Content-Length: 0\r\n\r\n",
        ))
    }
}

fn runtime_at(url: &str) -> JsRuntime {
    let document = TreeBuilder::parse("<body></body>").document();
    let mut runtime = JsRuntime::with_document_and_url(document, url).unwrap();
    runtime.set_http_resource_transport(Some(Arc::new(CookieTransport)));
    runtime
}

#[test]
fn get_and_get_all_read_only_script_visible_matching_cookies() {
    let mut runtime = runtime_at("https://example.test/inside/page");
    runtime.eval("fetch('/set')").unwrap();
    runtime.run_until_idle().unwrap();
    runtime
        .eval("document.cookie = 'script=2; Path=/'")
        .unwrap();
    runtime
        .eval(
            r#"
            globalThis.cookieResults = null;
            Promise.all([
              cookieStore.get('visible'),
              cookieStore.get('absent'),
              cookieStore.get().then(() => false, error => error instanceof TypeError),
              cookieStore.get({}).then(() => false, error => error instanceof TypeError),
              cookieStore.getAll(),
              cookieStore.getAll({name: 'scoped'}),
              cookieStore.getAll('visible'),
              cookieStore.getAll({url: 'https://example.test/inside/page#fragment'}),
              cookieStore.getAll({url: '/outside'}).then(() => false, error => error instanceof TypeError),
            ]).then(results => { cookieResults = results; });
            "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .eval(
                r#"cookieResults !== null &&
                   cookieResults[0].name === 'visible' && cookieResults[0].value === 'server' &&
                   cookieResults[1] === null &&
                   cookieResults[2] === true && cookieResults[3] === true &&
                   cookieResults[4].map(cookie => cookie.name).sort().join(',') === 'scoped,script,visible' &&
                   cookieResults[5].length === 1 && cookieResults[5][0].value === 'inner' &&
                   cookieResults[6].length === 1 && cookieResults[6][0].value === 'server' &&
                   cookieResults[7].length === 3 && cookieResults[8] === true &&
                   document.cookie.includes('visible=server') && document.cookie.includes('script=2')"#
            )
            .unwrap()
            .to_boolean(),
        "results: {:?}; task errors: {:?}",
        runtime.eval("JSON.stringify(cookieResults)").unwrap(),
        runtime.take_task_errors()
    );
    runtime
        .eval(
            "history.replaceState(null, '', '/outside/page'); globalThis.afterHistory = null; cookieStore.getAll().then(cookies => afterHistory = cookies.map(cookie => cookie.name));",
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .eval("afterHistory.includes('scoped') && !document.cookie.includes('scoped=inner')")
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn get_all_respects_cookie_path_and_secure_context_exposure() {
    let mut outside = runtime_at("https://example.test/outside/page");
    outside.eval("fetch('/set')").unwrap();
    outside.run_until_idle().unwrap();
    outside
        .eval("globalThis.outsideCookies = null; cookieStore.getAll().then(cookies => outsideCookies = cookies)")
        .unwrap();
    outside.run_until_idle().unwrap();
    assert!(
        outside
            .eval("outsideCookies.length === 1 && outsideCookies[0].name === 'visible'")
            .unwrap()
            .to_boolean()
    );

    let mut insecure = runtime_at("http://example.test/page");
    assert!(
        insecure
            .eval("typeof cookieStore === 'undefined' && typeof CookieStore === 'undefined'")
            .unwrap()
            .to_boolean()
    );
    let mut loopback = runtime_at("http://localhost/page");
    assert!(
        loopback
            .eval("typeof cookieStore === 'object' && cookieStore instanceof CookieStore")
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn opaque_child_rejects_cookie_query_with_security_error() {
    let mut runtime = runtime_at("https://example.test/page");
    runtime
        .eval("globalThis.cookieReply = null; addEventListener('message', event => cookieReply = [event.data, event.origin]);")
        .unwrap();
    let child_markup = r#"<script>
        Promise.all([
          cookieStore.get().then(() => 'unexpected', error => error.name),
          cookieStore.getAll({url: 'https://other.test/'}).then(() => 'unexpected', error => error.name),
        ]).then(errors => parent.postMessage(errors.join(','), '*'));
    </script>"#;
    runtime
        .eval(&format!(
            "const frame = document.createElement('iframe'); frame.setAttribute('sandbox', 'allow-scripts'); frame.srcdoc = {}; document.body.appendChild(frame);",
            serde_json::to_string(child_markup).unwrap()
        ))
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .eval("JSON.stringify(cookieReply) === '[\"SecurityError,SecurityError\",\"null\"]'")
            .unwrap()
            .to_boolean(),
        "reply: {:?}; task errors: {:?}",
        runtime.eval("JSON.stringify(cookieReply)").unwrap(),
        runtime.take_task_errors()
    );
}

#[test]
fn same_origin_srcdoc_child_has_no_http_cookies_for_its_creation_url() {
    let mut runtime = runtime_at("https://example.test/page");
    runtime
        .eval("globalThis.cookieReply = null; addEventListener('message', event => cookieReply = [event.data, event.origin]);")
        .unwrap();
    let child_markup = r#"<script>
        cookieStore.getAll().then(
          cookies => parent.postMessage(cookies.length, '*'),
          error => parent.postMessage(error.name, '*'));
    </script>"#;
    runtime
        .eval(&format!(
            "const frame = document.createElement('iframe'); frame.srcdoc = {}; document.body.appendChild(frame);",
            serde_json::to_string(child_markup).unwrap()
        ))
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .eval("JSON.stringify(cookieReply) === '[0,\"https://example.test\"]'")
            .unwrap()
            .to_boolean(),
        "reply: {:?}; task errors: {:?}",
        runtime.eval("JSON.stringify(cookieReply)").unwrap(),
        runtime.take_task_errors()
    );
}
