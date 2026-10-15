//! Cookie Store reads, writes, and change events share the network and document cookie jar.

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
        if request.url().path() == "/child" {
            let body = "<script>cookieStore.addEventListener('change', event => parent.postMessage(event.changed.map(item => item.name).join(','), '*'))</script>";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            return HttpResponse::parse(&mut Cursor::new(response.into_bytes()));
        }
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
          cookieStore.set('x', '1').then(() => 'unexpected', error => error.name),
          cookieStore.delete('x').then(() => 'unexpected', error => error.name),
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
            .eval("JSON.stringify(cookieReply) === '[\"SecurityError,SecurityError,SecurityError,SecurityError\",\"null\"]'")
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

#[test]
fn set_and_delete_share_jar_with_document_and_preserve_httponly() {
    let mut runtime = runtime_at("https://example.test/inside/page");
    runtime.eval("fetch('/set')").unwrap();
    runtime.run_until_idle().unwrap();
    runtime
        .eval(
            r#"
            globalThis.writeResult = 'pending';
            (async () => {
              await cookieStore.set('first', 'one');
              await cookieStore.set({name: 'scopedByStore', value: 'two', path: '/inside', domain: 'example.test', sameSite: 'lax'});
              const before = await cookieStore.getAll();
              const visible = document.cookie;
              await cookieStore.set('secret', 'overwritten');
              await cookieStore.delete('secret');
              const hidden = await cookieStore.get('secret');
              await cookieStore.delete({name: 'scopedByStore', domain: 'example.test', path: '/inside'});
              await cookieStore.delete('first');
              writeResult = {
                before: before.map(cookie => cookie.name).sort().join(','),
                visible,
                hidden,
                after: (await cookieStore.getAll()).map(cookie => cookie.name).sort().join(','),
                document: document.cookie,
              };
            })().catch(error => writeResult = error.name + ':' + error.message);
            "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .eval(
                r#"typeof writeResult === 'object' &&
                   writeResult.before === 'first,scoped,scopedByStore,visible' &&
                   writeResult.visible.includes('first=one') &&
                   writeResult.visible.includes('scopedByStore=two') &&
                   writeResult.hidden === null &&
                   writeResult.after === 'scoped,visible' &&
                   !writeResult.document.includes('secret=')"#
            )
            .unwrap()
            .to_boolean(),
        "write result: {:?}; task errors: {:?}",
        runtime.eval("JSON.stringify(writeResult)").unwrap(),
        runtime.take_task_errors()
    );
}

#[test]
fn set_rejects_invalid_fields_and_expires_cookies() {
    let mut runtime = runtime_at("https://example.test/inside/page");
    runtime
        .eval(
            r#"
            globalThis.writeFailures = null;
            (async () => {
              const invalid = [
                () => cookieStore.set('bad=name', '1'),
                () => cookieStore.set('bad;name', '1'),
                () => cookieStore.set('name', 'bad;value'),
                () => cookieStore.set(' ', ' '),
                () => cookieStore.set({name:'x', value:'1', domain:'other.test'}),
                () => cookieStore.set({name:'x', value:'1', domain:'.example.test'}),
                () => cookieStore.set({name:'x', value:'1', path:'relative'}),
                () => cookieStore.set({name:'x', value:'1', expires:Date.now()+1000, maxAge:3}),
                () => cookieStore.set({name:'x', value:'1', partitioned:true}),
                () => cookieStore.set({name:'__Host-x', value:'1', path:'/inside'}),
                () => cookieStore.set({name:'__Http-x', value:'1'}),
              ];
              const failures = await Promise.all(invalid.map(call => call().then(() => false, error => error instanceof TypeError)));
              await cookieStore.set({name:'expiring', value:'soon', maxAge:30});
              const before = await cookieStore.get('expiring');
              await cookieStore.set({name:'expiring', value:'gone', maxAge:0});
              const after = await cookieStore.get('expiring');
              await cookieStore.set(' spaced ', ' value ');
              writeFailures = [failures.every(Boolean), before?.value, after, await cookieStore.get('x'), (await cookieStore.get('spaced'))?.value];
            })().catch(error => writeFailures = error.name + ':' + error.message);
            "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .eval("Array.isArray(writeFailures) && writeFailures[0] === true && writeFailures[1] === 'soon' && writeFailures[2] === null && writeFailures[3] === null && writeFailures[4] === 'value'")
            .unwrap()
            .to_boolean(),
        "failures: {:?}; task errors: {:?}",
        runtime.eval("JSON.stringify(writeFailures)").unwrap(),
        runtime.take_task_errors()
    );
}

#[test]
fn partitioned_delete_succeeds_without_touching_unpartitioned_cookie() {
    let mut runtime = runtime_at("https://example.test/page");
    runtime
        .eval(
            r#"
            globalThis.partitionedDeleteResult = null;
            (async () => {
              await cookieStore.set('session', 'unpartitioned');
              const deleted = await cookieStore.delete({name:'session', partitioned:true});
              const afterPartitionedDelete = await cookieStore.get('session');
              const rejectedSet = await cookieStore.set({name:'session', value:'other', partitioned:true})
                .then(() => false, error => error instanceof TypeError);
              const rejectedZeroAgeSet = await cookieStore.set({name:'session', value:'other', partitioned:true, maxAge:0})
                .then(() => false, error => error instanceof TypeError);
              const rejectedInvalidDelete = await cookieStore.delete({name:'session', partitioned:true, path:'relative'})
                .then(() => false, error => error instanceof TypeError);
              const stillPresent = await cookieStore.get('session');
              await cookieStore.delete('session');
              partitionedDeleteResult = {
                deleted,
                afterPartitionedDelete: afterPartitionedDelete?.value,
                rejectedSet,
                rejectedZeroAgeSet,
                rejectedInvalidDelete,
                stillPresent: stillPresent?.value,
                afterOrdinaryDelete: await cookieStore.get('session'),
              };
            })().catch(error => partitionedDeleteResult = error.name + ':' + error.message);
            "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .eval(
                "partitionedDeleteResult?.deleted === undefined && partitionedDeleteResult?.afterPartitionedDelete === 'unpartitioned' && partitionedDeleteResult?.rejectedSet === true && partitionedDeleteResult?.rejectedZeroAgeSet === true && partitionedDeleteResult?.rejectedInvalidDelete === true && partitionedDeleteResult?.stillPresent === 'unpartitioned' && partitionedDeleteResult?.afterOrdinaryDelete === null"
            )
            .unwrap()
            .to_boolean(),
        "result: {:?}; task errors: {:?}",
        runtime.eval("JSON.stringify(partitionedDeleteResult)").unwrap(),
        runtime.take_task_errors()
    );
}

#[test]
fn set_distinguishes_host_only_and_domain_cookies() {
    let mut runtime = runtime_at("https://example.test/page");
    runtime
        .eval(
            r#"
            globalThis.domainResults = null;
            (async () => {
              await cookieStore.set('same', 'host');
              await cookieStore.set({name:'same', value:'domain', domain:'example.test'});
              const before = (await cookieStore.getAll('same')).map(cookie => cookie.value).sort();
              await cookieStore.delete('same');
              const after = await cookieStore.getAll('same');
              domainResults = [before.join(','), after.length, after[0]?.value];
            })().catch(error => domainResults = error.name + ':' + error.message);
            "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .eval("Array.isArray(domainResults) && domainResults[0] === 'domain,host' && domainResults[1] === 1 && domainResults[2] === 'domain'")
            .unwrap()
            .to_boolean(),
        "results: {:?}; task errors: {:?}",
        runtime.eval("JSON.stringify(domainResults)").unwrap(),
        runtime.take_task_errors()
    );
}

#[test]
fn empty_path_uses_creation_url_directory() {
    let mut runtime = runtime_at("https://example.test/inside/page");
    runtime
        .eval(
            r#"
            globalThis.pathResults = null;
            (async () => {
              await cookieStore.set({name:'directory', value:'scoped', path:''});
              const before = await cookieStore.get('directory');
              const rejected = await cookieStore.set({name:'__Host-directory', value:'x', path:''})
                .then(() => false, error => error instanceof TypeError);
              history.replaceState(null, '', '/outside/page');
              const after = await cookieStore.get('directory');
              pathResults = [before?.value, rejected, after?.value, document.cookie.includes('directory=')];
            })().catch(error => pathResults = error.name + ':' + error.message);
            "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .eval("JSON.stringify(pathResults) === '[\"scoped\",true,\"scoped\",false]'")
            .unwrap()
            .to_boolean(),
        "results: {:?}; task errors: {:?}",
        runtime.eval("JSON.stringify(pathResults)").unwrap(),
        runtime.take_task_errors()
    );
}

#[test]
fn query_url_resolves_against_document_base_url() {
    let mut runtime = runtime_at("https://example.test/inside/page");
    runtime
        .eval(
            "const base = document.createElement('base'); base.setAttribute('href', 'https://example.test/other/'); document.head.appendChild(base); globalThis.baseUrlRejected = null; cookieStore.get({url:'page'}).then(() => baseUrlRejected = false, error => baseUrlRejected = error instanceof TypeError);",
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .eval("baseUrlRejected === true")
            .unwrap()
            .to_boolean(),
        "result: {:?}; task errors: {:?}",
        runtime.eval("baseUrlRejected").unwrap(),
        runtime.take_task_errors()
    );
}

#[test]
fn change_events_follow_http_document_and_cookie_store_writes() {
    let mut runtime = runtime_at("https://example.test/inside/page");
    runtime
        .eval(
            r#"
            globalThis.cookieEvents = [];
            cookieStore.addEventListener('change', event => cookieEvents.push({
              type: event.type,
              changed: event.changed.map(item => item.name + '=' + item.value).sort(),
              deleted: event.deleted.map(item => item.name).sort(),
              frozen: Object.isFrozen(event.changed) && Object.isFrozen(event.deleted),
              bubbles: event.bubbles,
              cancelable: event.cancelable,
            }));
            globalThis.propertyEvents = 0;
            cookieStore.onchange = () => propertyEvents++;
            "#,
        )
        .unwrap();
    runtime.eval("fetch('/set')").unwrap();
    runtime.run_until_idle().unwrap();
    runtime
        .eval("document.cookie = 'fromDocument=one; Path=/'")
        .unwrap();
    runtime.run_until_idle().unwrap();
    runtime
        .eval("cookieStore.set('fromStore', 'two').then(() => cookieStore.delete('fromStore'))")
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .eval(
                r#"cookieEvents.length >= 3 && propertyEvents === cookieEvents.length &&
                   cookieEvents.every(event => event.type === 'change' && event.frozen && !event.bubbles && !event.cancelable) &&
                   cookieEvents.some(event => event.changed.includes('visible=server') && event.changed.includes('scoped=inner') && !event.changed.some(item => item.startsWith('secret='))) &&
                   cookieEvents.some(event => event.changed.includes('fromDocument=one')) &&
                   cookieEvents.some(event => event.changed.includes('fromStore=two')) &&
                   cookieEvents.some(event => event.deleted.includes('fromStore')) &&
                   cookieEvents.filter(event => event.changed.includes('fromStore=two') || event.deleted.includes('fromStore')).length === 2 &&
                   new CookieChangeEvent('custom', {changed:[{name:'a', value:'1'}]}).changed[0].value === '1'"#
            )
            .unwrap()
            .to_boolean(),
        "events: {:?}; task errors: {:?}",
        runtime.eval("JSON.stringify(cookieEvents)").unwrap(),
        runtime.take_task_errors()
    );
}

#[test]
fn identical_cookie_writes_do_not_dispatch_duplicate_change_events() {
    let mut runtime = runtime_at("https://example.test/page");
    runtime
        .eval(
            r#"
            globalThis.cookieEvents = [];
            globalThis.writeError = null;
            cookieStore.onchange = event => cookieEvents.push(
              event.changed.map(item => item.name + '=' + item.value).join(','));
            (async () => {
              await cookieStore.set('same', 'one');
              await cookieStore.set('same', 'one');
              await cookieStore.set('same', 'two');
              document.cookie = 'document=one; Path=/';
              document.cookie = 'document=one; Path=/';
              document.cookie = 'document=two; Path=/';
            })().catch(error => writeError = error.name + ':' + error.message);
            "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .eval("writeError === null && JSON.stringify(cookieEvents) === '[\"same=one\",\"same=two\",\"document=one\",\"document=two\"]'")
            .unwrap()
            .to_boolean(),
        "events: {:?}; error: {:?}; task errors: {:?}",
        runtime.eval("JSON.stringify(cookieEvents)").unwrap(),
        runtime.eval("writeError").unwrap(),
        runtime.take_task_errors()
    );
}

#[test]
fn natural_expiration_dispatches_one_deletion_without_another_cookie_operation() {
    let mut runtime = runtime_at("https://example.test/page");
    runtime
        .eval(
            r#"
            globalThis.expiryEvents = [];
            globalThis.expiryError = null;
            cookieStore.onchange = event => expiryEvents.push({
              changed: event.changed.map(item => item.name),
              deleted: event.deleted.map(item => item.name),
            });
            cookieStore.set({name:'short', value:'one', expires:Date.now()+1500})
              .catch(error => expiryError = error.name);
            "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();

    // Drive the host event loop, without another document.cookie or
    // cookieStore operation, until the real expiry deadline has passed.
    for _ in 0..120 {
        if runtime
            .eval("expiryEvents.some(event => event.deleted.includes('short'))")
            .unwrap()
            .to_boolean()
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
        runtime.tick(0).unwrap();
    }
    assert!(
        runtime
            .eval(
                "expiryError === null && expiryEvents.some(event => event.changed.includes('short')) && expiryEvents.filter(event => event.deleted.includes('short')).length === 1"
            )
            .unwrap()
            .to_boolean(),
        "events: {:?}; error: {:?}; task errors: {:?}",
        runtime.eval("JSON.stringify(expiryEvents)").unwrap(),
        runtime.eval("expiryError").unwrap(),
        runtime.take_task_errors()
    );
    runtime.tick(0).unwrap();
    assert!(
        runtime
            .eval("expiryEvents.filter(event => event.deleted.includes('short')).length === 1")
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn immediately_expired_cookies_report_deletions_even_without_a_previous_cookie() {
    let mut runtime = runtime_at("https://example.test/page");
    runtime
        .eval(
            r#"
            globalThis.immediateEvents = [];
            globalThis.immediateError = null;
            cookieStore.onchange = event => immediateEvents.push({
              changed: event.changed.map(item => item.name),
              deleted: event.deleted.map(item => item.name),
            });
            Promise.all([
              cookieStore.set({name:'zeroAge', value:'value', maxAge:0}),
              cookieStore.set({name:'pastExpiry', value:'value', expires:Date.now()-1000}),
              cookieStore.delete('neverPresent'),
            ]).catch(error => immediateError = error.name);
            "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .eval(
                "immediateError === null && JSON.stringify(immediateEvents.map(event => event.deleted)) === '[[\"zeroAge\"],[\"pastExpiry\"],[\"neverPresent\"]]' && immediateEvents.every(event => event.changed.length === 0) && document.cookie === ''"
            )
            .unwrap()
            .to_boolean(),
        "events: {:?}; error: {:?}; task errors: {:?}",
        runtime.eval("JSON.stringify(immediateEvents)").unwrap(),
        runtime.eval("immediateError").unwrap(),
        runtime.take_task_errors()
    );
    runtime.tick(0).unwrap();
    assert_eq!(
        runtime.eval("immediateEvents.length").unwrap().as_number(),
        Some(3.0)
    );
}

#[test]
fn page_global_shadowing_cannot_disable_cookie_change_delivery() {
    let mut runtime = runtime_at("https://example.test/page");
    runtime
        .eval(
            r#"
            globalThis.cookieChangeHooksWerePrivate =
              typeof __omoikane_cookie_change_dispatch === 'undefined' &&
              typeof __omoikane_cookie_change_reset === 'undefined' &&
              typeof __omoikane_dispatch_cookie_changes === 'undefined' &&
              typeof __omoikane_reset_cookie_changes === 'undefined';
            globalThis.__omoikane_cookie_change_dispatch = () => { throw new Error('shadowed dispatch'); };
            globalThis.__omoikane_cookie_change_reset = () => { throw new Error('shadowed reset'); };
            globalThis.__omoikane_dispatch_cookie_changes = () => { throw new Error('shadowed legacy dispatch'); };
            globalThis.__omoikane_reset_cookie_changes = () => { throw new Error('shadowed legacy reset'); };
            globalThis.cookieChangeNames = [];
            cookieStore.onchange = event => cookieChangeNames.push(...event.changed.map(item => item.name));
            cookieStore.set('privateDispatch', 'works');
            "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .eval("cookieChangeHooksWerePrivate && cookieChangeNames.includes('privateDispatch')")
            .unwrap()
            .to_boolean(),
        "names: {:?}; task errors: {:?}",
        runtime.eval("JSON.stringify(cookieChangeNames)").unwrap(),
        runtime.take_task_errors()
    );
}

#[test]
fn change_events_reach_same_origin_child_window() {
    let mut runtime = runtime_at("https://example.test/page");
    runtime
        .eval(
            "globalThis.childCookieEvents = []; addEventListener('message', event => childCookieEvents.push(event.data)); const frame = document.createElement('iframe'); frame.src = '/child'; document.body.appendChild(frame);",
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    runtime
        .eval("cookieStore.set('childVisible', 'yes')")
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .eval("childCookieEvents.includes('childVisible')")
            .unwrap()
            .to_boolean(),
        "child events: {:?}; task errors: {:?}",
        runtime.eval("JSON.stringify(childCookieEvents)").unwrap(),
        runtime.take_task_errors()
    );
}

#[test]
fn cookie_store_is_window_only_for_available_worker_globals() {
    let mut runtime = runtime_at("https://example.test/page");
    runtime
        .eval(
            r#"
            globalThis.workerCookieSurface = null;
            globalThis.sharedCookieSurface = null;
            const dedicated = new Worker('data:text/javascript,' + encodeURIComponent(
              'postMessage([typeof cookieStore, typeof CookieStore, typeof CookieChangeEvent].join(","))'));
            dedicated.onmessage = event => workerCookieSurface = event.data;
            const shared = new SharedWorker('data:text/javascript,' + encodeURIComponent(
              'onconnect = event => event.ports[0].postMessage([typeof cookieStore, typeof CookieStore, typeof CookieChangeEvent].join(","))'),
              'cookie-store-surface');
            shared.port.onmessage = event => sharedCookieSurface = event.data;
            "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .eval("workerCookieSurface === 'undefined,undefined,undefined' && sharedCookieSurface === 'undefined,undefined,undefined'")
            .unwrap()
            .to_boolean(),
        "worker: {:?}; shared: {:?}; task errors: {:?}",
        runtime.eval("workerCookieSurface").unwrap(),
        runtime.eval("sharedCookieSurface").unwrap(),
        runtime.take_task_errors()
    );
}
