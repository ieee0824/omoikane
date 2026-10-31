use omoikane::{html::TreeBuilder, js::JsRuntime};

fn runtime() -> JsRuntime {
    JsRuntime::with_document(TreeBuilder::parse("<main></main>").document()).expect("runtime")
}

fn json(runtime: &mut JsRuntime, source: &str) -> String {
    runtime
        .eval(source)
        .expect("evaluate modern body/url probe")
        .as_string()
        .expect("JSON string")
        .to_std_string_escaped()
}

#[test]
fn body_bytes_returns_an_owned_uint8array_and_consumes_once() {
    let mut runtime = runtime();
    runtime
        .eval(
            r#"globalThis.result = [];
               const response = new Response(new Uint8Array([1, 2, 3]));
               response.bytes().then(bytes => {
                 bytes[0] = 9;
                 result.push(bytes instanceof Uint8Array, [...bytes], response.bodyUsed);
                 return response.bytes().then(() => 'resolved', error => error instanceof TypeError);
               }).then(value => result.push(value));
               const request = new Request('/submit', {method:'POST', body:'ok'});
               request.bytes().then(bytes => result.push(new TextDecoder().decode(bytes)));"#,
        )
        .expect("consume bodies");
    runtime.run_jobs().expect("body jobs");
    assert_eq!(
        json(&mut runtime, "JSON.stringify(result)"),
        "[true,[9,2,3],true,\"ok\",true]"
    );
}

#[test]
fn url_parse_and_search_params_size_follow_current_state() {
    let mut runtime = runtime();
    assert_eq!(
        json(
            &mut runtime,
            r#"(() => {
              const parsed = URL.parse('/path?a=1&a=2', 'https://example.test/base');
              const params = parsed.searchParams;
              const initialHref = parsed.href;
              const before = params.size;
              params.append('b', '3');
              const afterAppend = params.size;
              params.delete('a');
              return JSON.stringify([
                parsed instanceof URL, initialHref,
                before, afterAppend, params.size,
                URL.parse('http://[invalid') === null,
                URL.parse.call(null, '/ok', 'https://example.test').href
              ]);
            })()"#,
        ),
        "[true,\"https://example.test/path?a=1&a=2\",2,3,1,true,\"https://example.test/ok\"]"
    );
}

#[test]
fn beacon_and_fetch_later_expose_keepalive_contracts() {
    let mut api_runtime = runtime();
    assert_eq!(
        json(
            &mut api_runtime,
            r#"(() => {
              const endpoint = 'http://127.0.0.1:9/keepalive';
              const result = fetchLater(endpoint, {
                method: 'POST', body: 'later', activateAfter: 0
              });
              let invalidDelay = false;
              try { fetchLater(endpoint, { activateAfter: -1 }); }
              catch (error) { invalidDelay = error instanceof RangeError; }
              let insecureTarget = false;
              try { fetchLater('http://example.test/'); }
              catch (error) { insecureTarget = error instanceof TypeError; }
              const controller = new AbortController();
              const aborted = fetchLater(endpoint, { signal: controller.signal });
              controller.abort();
              let quotaError = false;
              try { fetchLater(endpoint, {method:'POST', body:new Uint8Array(65537)}); }
              catch (error) { quotaError = error instanceof DOMException && error.name === 'QuotaExceededError'; }
              return JSON.stringify([
                typeof navigator.sendBeacon,
                typeof fetchLater,
                result instanceof FetchLaterResult,
                result.activated,
                invalidDelay,
                insecureTarget,
                aborted.activated,
                quotaError,
                Object.prototype.toString.call(result)
              ]);
            })()"#,
        ),
        "[\"function\",\"function\",true,true,true,true,false,true,\"[object FetchLaterResult]\"]"
    );

    let mut quota_runtime = runtime();
    assert_eq!(
        json(
            &mut quota_runtime,
            r#"JSON.stringify([
              navigator.sendBeacon('http://127.0.0.1:9/full', new Uint8Array(65536)),
              navigator.sendBeacon('http://127.0.0.1:9/overflow', new Uint8Array([1]))
            ])"#,
        ),
        "[true,false]"
    );

    let mut insecure_runtime = JsRuntime::with_document_and_url(
        TreeBuilder::parse("<main></main>").document(),
        "http://example.test/",
    )
    .expect("insecure runtime");
    assert_eq!(
        json(
            &mut insecure_runtime,
            "JSON.stringify([typeof fetchLater, typeof FetchLaterResult, typeof navigator.sendBeacon])",
        ),
        "[\"undefined\",\"undefined\",\"function\"]"
    );
}
