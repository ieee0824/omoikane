use super::*;

struct StorageAccessTransport;

#[derive(Default)]
struct CookieEchoTransport {
    requests: Mutex<Vec<(String, Option<String>)>>,
}

#[derive(Default)]
struct ResourceCookieTransport {
    requests: Mutex<Vec<(Method, String, Option<String>)>>,
}

impl crate::http::HttpTransport for ResourceCookieTransport {
    fn send(
        &self,
        request: &HttpRequest,
        _timeout: Option<Duration>,
    ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
        let path = request.url().path();
        self.requests.lock().unwrap().push((
            request.method(),
            path.to_owned(),
            request.header("Cookie").map(str::to_owned),
        ));
        if path == "/redirect-font.ttf" {
            return Ok(crate::http::HttpResponse::new(
                302,
                "Found",
                vec![("Location".into(), "https://other.test/hop-font.ttf".into())],
                vec![],
            ));
        }
        if path == "/hop-font.ttf" {
            return Ok(crate::http::HttpResponse::new(
                302,
                "Found",
                vec![
                    (
                        "Location".into(),
                        "https://third.test/post-return-font.ttf".into(),
                    ),
                    (
                        "Access-Control-Allow-Origin".into(),
                        "https://third.test".into(),
                    ),
                ],
                vec![],
            ));
        }
        let worker_source = if path.ends_with("-internal-worker.js")
            || path.ends_with("-internal-shared.js")
        {
            let name = path.trim_start_matches('/').trim_end_matches(".js");
            let origin = format!("{}://{}", request.url().scheme(), request.url().authority());
            Some(format!(
                "fetch('{origin}/{name}-fetch'); importScripts('{origin}/{name}-import.js'); \
                 onmessage = () => fetch('{origin}/{name}-later-fetch')"
            ))
        } else if path == "/nested-owner.js" {
            Some(
                "globalThis.nested = new Worker('https://third.test/nested-internal-worker.js')"
                    .to_owned(),
            )
        } else if path == "/nested-shared-owner.js" {
            Some("onconnect = () => { globalThis.nested = new Worker('https://third.test/nested-internal-worker.js') }".to_owned())
        } else if path == "/mixed-shared.js" {
            Some("let count = 0; onconnect = () => { count++; fetch('https://top.test/mixed-' + count + '-fetch'); if (count === 1) globalThis.child = new Worker('https://top.test/mixed-internal-worker.js'); else child.postMessage('check') }".to_owned())
        } else {
            None
        };
        let (mime, body) = if let Some(source) = worker_source {
            ("text/javascript", source.into_bytes())
        } else if path.ends_with(".js") {
            (
                "text/javascript",
                b"globalThis.resourceLoaded = true".to_vec(),
            )
        } else if path.ends_with(".ttf") {
            (
                "font/ttf",
                include_bytes!("../../tests/fixtures/acid3/font.ttf").to_vec(),
            )
        } else {
            (
                "text/html",
                b"<html><body><script>window.frameReady = true</script></body></html>".to_vec(),
            )
        };
        let mut headers = vec![("Content-Type".into(), mime.into())];
        if path.starts_with("/pre-")
            || path.starts_with("/post-")
            || path.starts_with("/first-")
            || path.starts_with("/mixed-")
            || path.starts_with("/nested-")
            || path == "/popup"
        {
            let name = path
                .trim_start_matches('/')
                .replace('-', "_")
                .replace('.', "_");
            headers.push((
                "Set-Cookie".into(),
                format!("{name}=1; Path=/; Secure; SameSite=None"),
            ));
        }
        Ok(crate::http::HttpResponse::new(200, "OK", headers, body))
    }
}

impl crate::http::HttpTransport for StorageAccessTransport {
    fn send(
        &self,
        _request: &HttpRequest,
        _timeout: Option<Duration>,
    ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
        Ok(crate::http::HttpResponse::new(
            200,
            "OK",
            vec![("Content-Type".into(), "text/html".into())],
            b"<html><body><script>window.frameReady = true</script></body></html>".to_vec(),
        ))
    }
}

impl crate::http::HttpTransport for CookieEchoTransport {
    fn send(
        &self,
        request: &HttpRequest,
        _timeout: Option<Duration>,
    ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
        self.requests.lock().unwrap().push((
            request.url().to_string(),
            request.header("Cookie").map(str::to_owned),
        ));
        if request.url().path() == "/redirect" {
            return Ok(crate::http::HttpResponse::new(
                302,
                "Found",
                vec![("Location".into(), "/probe".into())],
                vec![],
            ));
        }
        if request.url().path() != "/probe" {
            return Ok(crate::http::HttpResponse::new(
                200,
                "OK",
                vec![("Content-Type".into(), "text/html".into())],
                b"<html><body><script>window.frameReady = true</script></body></html>".to_vec(),
            ));
        }
        Ok(crate::http::HttpResponse::new(
            200,
            "OK",
            vec![
                ("Content-Type".into(), "text/plain".into()),
                (
                    "Set-Cookie".into(),
                    "response=stored; Path=/; Secure; SameSite=None".into(),
                ),
            ],
            request
                .header("Cookie")
                .unwrap_or("none")
                .as_bytes()
                .to_vec(),
        ))
    }
}

fn runtime_at(url: &str) -> JsRuntime {
    JsRuntime::with_document_and_url(
        crate::html::TreeBuilder::parse("<html><body></body></html>").document(),
        url,
    )
    .unwrap()
}

fn child_document(runtime: &JsRuntime) -> NodeHandle {
    runtime
        .host_state
        .borrow()
        .iframe_documents
        .values()
        .next()
        .expect("iframe navigation committed")
        .document
        .clone()
}

fn child_result(document: &NodeHandle) -> Option<String> {
    child_attribute(document, "data-result")
}

fn child_attribute(document: &NodeHandle, attribute: &str) -> Option<String> {
    document
        .child_nodes()
        .into_iter()
        .find(|node| node.node_type() == NodeType::Element)
        .and_then(|html| html.get_attribute(attribute))
}

fn automated_third_party_frame() -> (JsRuntime, NodeHandle) {
    let mut runtime = runtime_at("https://top.test/");
    runtime.enable_test_automation().unwrap();
    runtime.set_http_resource_transport(Some(Arc::new(StorageAccessTransport)));
    runtime.eval("const f = document.createElement('iframe'); f.src = 'https://third.test/frame'; document.body.appendChild(f)").unwrap();
    runtime.tick(0).unwrap();
    let child = child_document(&runtime);
    runtime
        .eval_in_document_realm(child.identity(), "globalThis.test_driver_internal = {}")
        .unwrap();
    runtime
        .eval_in_document_realm(
            child.identity(),
            include_str!("../../tests/wpt_smoke/testdriver_vendor.js"),
        )
        .unwrap();
    runtime.eval_in_document_realm(child.identity(), r#"
        const button = document.createElement('button');
        button.style.cssText = 'display:block;width:100px;height:40px';
        button.textContent = 'Click';
        document.body.appendChild(button);
        const observedInput = [];
        for (const type of ['mousemove','mousedown','mouseup','click']) {
          document.addEventListener(type, event => {
            observedInput.push([type, event.target === button]);
            document.documentElement.setAttribute('data-input', JSON.stringify(observedInput));
          });
        }
        button.addEventListener('click', () => document.requestStorageAccess().then(
          () => document.documentElement.setAttribute('data-result', 'granted'),
          error => document.documentElement.setAttribute('data-result', error.name)));
        function queueClick() {
          const rect = button.getBoundingClientRect();
          document.documentElement.setAttribute('data-rect', JSON.stringify(rect));
          return test_driver_internal.click(button, {x: rect.left + rect.width/2, y: rect.top + rect.height/2})
            .catch(error => document.documentElement.setAttribute('data-error', String(error)));
        }
    "#).unwrap();
    (runtime, child)
}

#[test]
fn test_automation_bindings_are_absent_from_ordinary_pages() {
    let mut runtime = runtime_at("https://top.test/");
    assert_eq!(runtime.eval("['__omoikane_automation_register_driver', '__omoikane_automation_set_permission', '__omoikane_automation_delete_all_cookies'].every(name => !(name in globalThis))").unwrap().as_boolean(), Some(true));
    assert_eq!(runtime.advance_test_automation().unwrap(), None);
}

#[test]
fn test_automation_child_click_requires_the_host_trusted_input_scope() {
    let (mut runtime, child) = automated_third_party_frame();
    runtime.eval_in_document_realm(child.identity(), "queueClick(); __wpt_advance_action_tick(); __wpt_advance_action_tick(); __wpt_advance_action_tick();").unwrap();
    assert_eq!(
        child_result(&child).as_deref(),
        Some("NotAllowedError"),
        "input={:?}, rect={:?}, error={:?}",
        child_attribute(&child, "data-input"),
        child_attribute(&child, "data-rect"),
        child_attribute(&child, "data-error")
    );
    runtime
        .eval_in_document_realm(child.identity(), "queueClick()")
        .unwrap();
    for _ in 0..3 {
        assert_eq!(runtime.advance_test_automation().unwrap(), Some(0));
    }
    assert_eq!(child_result(&child).as_deref(), Some("granted"));
    assert_eq!(runtime.advance_test_automation().unwrap(), None);
    assert!(!runtime.host_state.borrow().trusted_storage_access_input);
}

#[test]
fn test_automation_permission_denial_and_revocation_control_real_storage_access() {
    let (mut runtime, child) = automated_third_party_frame();
    runtime
        .eval_in_document_realm(
            child.identity(),
            r#"
        let storagePermission;
        const permissionChanges = [];
        navigator.permissions.query({name:'storage-access'}).then(status => {
          storagePermission = status;
          permissionChanges.push(status.state);
          status.onchange = () => permissionChanges.push(status.state);
        });
    "#,
        )
        .unwrap();
    runtime.eval_in_document_realm(child.identity(), "test_driver_internal.set_permission({descriptor:{name:'storage-access'},state:'denied'}); queueClick()").unwrap();
    for _ in 0..3 {
        runtime.advance_test_automation().unwrap();
    }
    assert_eq!(child_result(&child).as_deref(), Some("NotAllowedError"));
    runtime.eval_in_document_realm(child.identity(), "__omoikane_permission_changed('storage-access', 'granted'); document.documentElement.setAttribute('data-permission', storagePermission.state)").unwrap();
    assert_eq!(
        child_attribute(&child, "data-permission").as_deref(),
        Some("prompt")
    );
    runtime.eval_in_document_realm(child.identity(), r#"
        test_driver_internal.set_permission({descriptor:{name:'storage-access'},state:'granted'}).then(() =>
          document.requestStorageAccess()).then(() => {
            document.cookie = 'allowed=1; Secure; SameSite=None; Path=/';
            document.documentElement.setAttribute('data-result', document.cookie);
          });
    "#).unwrap();
    assert_eq!(child_result(&child).as_deref(), Some("allowed=1"));
    runtime.eval_in_document_realm(child.identity(), r#"
        test_driver_internal.set_permission({descriptor:{name:'storage-access'},state:'prompt'}).then(() =>
          Promise.all([document.hasStorageAccess(), document.requestStorageAccess().then(() => 'granted', e => e.name)]))
          .then(([has, result]) => document.documentElement.setAttribute('data-result', [has, result, document.cookie].join('|')));
    "#).unwrap();
    assert_eq!(
        child_result(&child).as_deref(),
        Some("false|NotAllowedError|")
    );
    runtime.eval_in_document_realm(child.identity(), r#"
        test_driver_internal.set_permission({descriptor:{name:'storage-access'},state:'granted'}).then(() =>
          test_driver_internal.delete_all_cookies()).then(() =>
          document.documentElement.setAttribute('data-result', document.cookie));
    "#).unwrap();
    assert_eq!(child_result(&child).as_deref(), Some(""));
    runtime.eval_in_document_realm(child.identity(), "document.documentElement.setAttribute('data-permission', JSON.stringify([storagePermission.state, permissionChanges]))").unwrap();
    assert_eq!(
        child_attribute(&child, "data-permission").as_deref(),
        Some(r#"["granted",["prompt","granted","prompt","granted"]]"#)
    );
    assert_eq!(
        runtime
            .host_state
            .borrow()
            .cookie_store
            .lock()
            .unwrap()
            .cookie_header(&"https://third.test/".parse().unwrap()),
        None
    );
}

#[test]
fn test_automation_keeps_a_replaced_driver_alive_through_its_pending_tick() {
    let mut runtime = runtime_at("https://top.test/");
    runtime.enable_test_automation().unwrap();
    runtime.eval("const frame=document.createElement('iframe'); frame.id='automation-frame'; frame.srcdoc='<html><body></body></html>'; document.body.appendChild(frame)").unwrap();
    runtime.tick(0).unwrap();
    let child = iframe_document_by_id(&runtime, "automation-frame");
    let realm = runtime
        .realm_for_document(child.identity())
        .unwrap()
        .unwrap();
    let previous = runtime.context.enter_realm(realm);
    runtime
        .context
        .register_global_builtin_callable(
            js_string!("collectAutomationGarbage"),
            0,
            NativeFunction::from_copy_closure(|_, _, _| {
                boa_gc::force_collect();
                Ok(JsValue::undefined())
            }),
        )
        .unwrap();
    runtime.context.enter_realm(previous);
    runtime.eval_in_document_realm(child.identity(), r#"
        const replacementDriver = () => null;
        function collectAfterPoll() {
          collectAutomationGarbage();
          const survived = driverWeak.deref() !== undefined;
          parent.document.documentElement.setAttribute('data-driver-root', survived ? 'alive' : 'lost');
          // Retire the document if collection exposed the missing root, so
          // the host skips its obsolete callback instead of dereferencing it.
          if (!survived) parent.document.getElementById('automation-frame').remove();
        }
        function registerAndWatch(driver) {
          globalThis.driverWeak = new WeakRef(driver);
          __omoikane_automation_register_driver(driver);
        }
        registerAndWatch(function(execute) {
          if (!execute) {
            __omoikane_automation_register_driver(replacementDriver);
            queueMicrotask(collectAfterPoll);
            return false;
          }
          parent.document.documentElement.setAttribute('data-driver-executed', 'true');
          return 0;
        });
    "#).unwrap();
    runtime.context.clear_kept_objects();
    let duration = runtime.advance_test_automation().unwrap();
    let html = runtime.document().query_selector("html").unwrap();
    assert_eq!(
        html.get_attribute("data-driver-root").as_deref(),
        Some("alive")
    );
    assert_eq!(
        html.get_attribute("data-driver-executed").as_deref(),
        Some("true")
    );
    assert_eq!(duration, Some(0));
    assert_eq!(runtime.advance_test_automation().unwrap(), None);
}

#[test]
fn test_automation_does_not_drive_retired_child_documents() {
    let (mut runtime, child) = automated_third_party_frame();
    runtime
        .eval_in_document_realm(child.identity(), "queueClick()")
        .unwrap();
    runtime.eval("f.remove()").unwrap();
    assert_eq!(runtime.advance_test_automation().unwrap(), None);
    assert_eq!(child_result(&child), None);
}

fn iframe_document_by_id(runtime: &JsRuntime, id: &str) -> NodeHandle {
    let state = runtime.host_state.borrow();
    state
        .iframe_documents
        .iter()
        .find_map(|(iframe_id, entry)| {
            (state.get_node(*iframe_id)?.get_attribute("id").as_deref() == Some(id))
                .then(|| entry.document.clone())
        })
        .expect("named iframe navigation committed")
}

fn activate_storage_access(runtime: &mut JsRuntime, document: &NodeHandle) {
    runtime
        .with_trusted_storage_access_input(|runtime| {
            runtime.eval_in_document_realm(
                document.identity(),
                "document.addEventListener('click', () => \
                   document.requestStorageAccess().then(() => \
                     document.documentElement.setAttribute('data-granted', 'yes')), \
                   {once: true}); \
                 __omoikane_dispatch_mouse_input(document.body.__id, 'click', {button: 0}, false)",
            )
        })
        .unwrap();
    assert_eq!(
        child_attribute(document, "data-granted").as_deref(),
        Some("yes")
    );
}

#[test]
fn storage_access_top_level_secure_insecure_and_opaque_documents() {
    let mut secure = runtime_at("https://top.test/");
    secure
        .eval(
            "document.hasStorageAccess().then(v => globalThis.hasAccess = v); \
             document.requestStorageAccess().then(() => globalThis.requested = true)",
        )
        .unwrap();
    secure.run_jobs().unwrap();
    assert_eq!(
        secure.eval("hasAccess && requested").unwrap().as_boolean(),
        Some(true)
    );
    assert_eq!(
        secure
            .eval("typeof __omoikane_storage_access_request")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "undefined"
    );
    secure
        .eval(
            "Promise.all([ \
               Document.prototype.hasStorageAccess.call({__id: document.__id}) \
                 .then(() => 'accepted', error => error.name), \
               Document.prototype.requestStorageAccess.call({__id: document.__id}) \
                 .then(() => 'accepted', error => error.name) \
             ]).then(names => globalThis.invalidReceiverErrors = names.join('|'))",
        )
        .unwrap();
    secure.run_jobs().unwrap();
    assert_eq!(
        secure
            .eval("invalidReceiverErrors")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "TypeError|TypeError"
    );

    for url in ["http://insecure.test/", "data:text/html,opaque"] {
        let mut runtime = runtime_at(url);
        runtime
            .eval(
                "document.hasStorageAccess().then(v => globalThis.hasAccess = v); \
                 document.requestStorageAccess().catch(e => globalThis.errorName = e.name)",
            )
            .unwrap();
        runtime.run_jobs().unwrap();
        assert_eq!(
            runtime
                .eval("String(hasAccess) + '|' + errorName")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            "false|NotAllowedError"
        );
    }
}

#[test]
fn generated_document_storage_access_methods_reject_as_inactive() {
    let mut runtime = runtime_at("https://top.test/");
    runtime
        .eval(
            "const created = document.implementation.createDocument('', null); \
             Promise.all([ \
               created.hasStorageAccess().then(() => 'resolved', e => e.name), \
               created.requestStorageAccess().then(() => 'resolved', e => e.name) \
             ]).then(names => globalThis.storageAccessErrors = names.join('|'))",
        )
        .unwrap();
    runtime.run_jobs().unwrap();
    assert_eq!(
        runtime
            .eval("storageAccessErrors")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "InvalidStateError|InvalidStateError"
    );
}

#[test]
fn same_site_iframe_has_storage_access_without_activation() {
    let mut runtime = runtime_at("https://top.test/");
    runtime.set_http_resource_transport(Some(Arc::new(StorageAccessTransport)));
    runtime
        .eval(
            "const frame = document.createElement('iframe'); \
             frame.src = 'https://sub.top.test/frame'; document.body.appendChild(frame)",
        )
        .unwrap();
    runtime.tick(0).unwrap();
    let child = child_document(&runtime);
    runtime
        .eval_in_document_realm(
            child.identity(),
            "document.hasStorageAccess().then(before => \
               document.requestStorageAccess().then(() => \
                 document.hasStorageAccess().then(after => \
                   document.documentElement.setAttribute('data-result', \
                     [before, after].join('|')))))",
        )
        .unwrap();
    assert_eq!(child_result(&child).as_deref(), Some("true|true"));
}

#[test]
fn storage_access_policy_header_blocks_request_but_not_first_party_access() {
    let mut runtime = runtime_at("https://top.test/");
    runtime.install_response_security_headers(&[(
        "Permissions-Policy".into(),
        "storage-access=()".into(),
    )]);
    runtime
        .eval(
            "Promise.all([document.hasStorageAccess(), \
              document.requestStorageAccess().then(() => 'granted', e => e.name)]) \
              .then(([has, requested]) => globalThis.policyResult = JSON.stringify([ \
                document.permissionsPolicy.allowsFeature('storage-access'), \
                document.permissionsPolicy.getAllowlistForFeature('storage-access'), \
                has, requested]))",
        )
        .unwrap();
    runtime.run_jobs().unwrap();
    assert_eq!(
        runtime
            .eval("policyResult")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[false,[],true,\"NotAllowedError\"]"
    );
}

#[test]
fn storage_access_policy_inheritance_controls_third_party_cookie_requests() {
    for (name, header, allow, expected_policy, expected_result, expected_cookie_count) in [
        (
            "default wildcard",
            None,
            None,
            "[true,true,[\"*\"]]",
            "granted|session=preexisting|session=preexisting|false",
            2,
        ),
        (
            "container denial",
            None,
            Some("storage-access 'none'"),
            "[false,false,[]]",
            "NotAllowedError||none|true",
            1,
        ),
        (
            "parent denial cannot be overridden",
            Some("storage-access=(self)"),
            Some("storage-access *"),
            "[false,false,[]]",
            "NotAllowedError||none|true",
            1,
        ),
        (
            "explicit wildcard",
            Some("storage-access=*"),
            Some("storage-access *"),
            "[true,true,[\"*\"]]",
            "granted|session=preexisting|session=preexisting|false",
            2,
        ),
    ] {
        assert_storage_access_policy_case(
            name,
            header,
            allow,
            expected_policy,
            expected_result,
            expected_cookie_count,
        );
    }
}

fn assert_storage_access_policy_case(
    name: &str,
    header: Option<&str>,
    allow: Option<&str>,
    expected_policy: &str,
    expected_result: &str,
    expected_cookie_count: usize,
) {
    let mut runtime = runtime_at("https://top.test/");
    runtime.set_http_resource_transport(Some(Arc::new(CookieEchoTransport::default())));
    if let Some(header) = header {
        runtime.install_response_security_headers(&[("Permissions-Policy".into(), header.into())]);
    }
    runtime
        .host_state
        .borrow()
        .cookie_store
        .lock()
        .unwrap()
        .add_from_header_for_url(
            "session=preexisting; Path=/; Secure; SameSite=None",
            &"https://third.test/".parse().unwrap(),
        );
    let set_allow = allow.map_or(String::new(), |allow| {
        format!("f.setAttribute('allow', {allow:?});")
    });
    runtime
        .eval(&format!(
            "const f = document.createElement('iframe'); \
             {set_allow} f.src = 'https://third.test/frame'; \
             document.body.appendChild(f)"
        ))
        .unwrap();
    runtime.tick(0).unwrap();
    let child = child_document(&runtime);
    runtime
        .eval_in_document_realm(
            child.identity(),
            "document.documentElement.setAttribute('data-policy', JSON.stringify([ \
              document.permissionsPolicy.allowsFeature('storage-access'), \
              document.permissionsPolicy.allowsFeature('storage-access', 'https://other.test'), \
              document.permissionsPolicy.getAllowlistForFeature('storage-access')]))",
        )
        .unwrap();
    assert_eq!(
        child_attribute(&child, "data-policy").as_deref(),
        Some(expected_policy),
        "{name}"
    );
    runtime
        .with_trusted_storage_access_input(|runtime| {
            runtime.eval_in_document_realm(
                child.identity(),
                "document.addEventListener('click', () => { \
                   document.requestStorageAccess() \
                     .then(() => 'granted', error => error.name) \
                     .then(outcome => { \
                       const visible = document.cookie; \
                       return Promise.all([ \
                         fetch('https://third.test/probe').then(response => response.text()), \
                         cookieStore.get('session') \
                       ]).then(([sent, cookie]) => \
                         document.documentElement.setAttribute('data-result', \
                           [outcome, visible, sent, cookie === null].join('|'))); \
                     }); \
                 }, {once: true}); \
                 __omoikane_dispatch_mouse_input(document.body.__id, 'click', \
                   {button: 0}, false)",
            )
        })
        .unwrap();
    assert_eq!(
        child_result(&child).as_deref(),
        Some(expected_result),
        "{name}"
    );
    assert_eq!(
        runtime
            .host_state
            .borrow()
            .cookie_store
            .lock()
            .unwrap()
            .len(),
        expected_cookie_count,
        "{name}"
    );
}

#[test]
fn storage_access_site_grant_does_not_bypass_new_document_policy() {
    let mut runtime = runtime_at("https://top.test/");
    runtime.set_http_resource_transport(Some(Arc::new(CookieEchoTransport::default())));
    runtime
        .host_state
        .borrow()
        .cookie_store
        .lock()
        .unwrap()
        .add_from_header_for_url(
            "session=preexisting; Path=/; Secure; SameSite=None",
            &"https://third.test/".parse().unwrap(),
        );
    runtime
        .eval(
            "const first = document.createElement('iframe'); \
             first.id = 'first'; first.src = 'https://third.test/frame'; \
             document.body.appendChild(first)",
        )
        .unwrap();
    runtime.tick(0).unwrap();
    let first = iframe_document_by_id(&runtime, "first");
    activate_storage_access(&mut runtime, &first);
    assert_eq!(runtime.host_state.borrow().storage_access_grants.len(), 1);

    runtime
        .eval(
            "const blocked = document.createElement('iframe'); \
             blocked.id = 'blocked'; \
             blocked.setAttribute('allow', \"storage-access 'none'\"); \
             blocked.src = 'https://third.test/frame'; \
             document.body.appendChild(blocked)",
        )
        .unwrap();
    runtime.tick(0).unwrap();
    let blocked = iframe_document_by_id(&runtime, "blocked");
    runtime
        .eval_in_document_realm(
            blocked.identity(),
            "document.hasStorageAccess().then(value => \
             document.documentElement.setAttribute('data-has-access', value))",
        )
        .unwrap();
    assert_eq!(
        child_attribute(&blocked, "data-has-access").as_deref(),
        Some("false"),
        "a new Document does not inherit an active flag from the site grant"
    );
    runtime
        .with_trusted_storage_access_input(|runtime| {
            runtime.eval_in_document_realm(
                blocked.identity(),
                "document.addEventListener('click', () => \
                   document.requestStorageAccess().then( \
                     () => document.documentElement.setAttribute('data-result', 'granted'), \
                     error => document.documentElement.setAttribute('data-result', error.name)), \
                   {once: true}); \
                 __omoikane_dispatch_mouse_input(document.body.__id, 'click', \
                   {button: 0}, false)",
            )
        })
        .unwrap();
    assert_eq!(child_result(&blocked).as_deref(), Some("NotAllowedError"));

    // Model a restored active-document bit separately from the site grant.
    // Even if status is granted, this Document's policy must block both sinks.
    runtime
        .host_state
        .borrow_mut()
        .storage_access_active_documents
        .insert(blocked.identity());
    runtime
        .eval_in_document_realm(
            blocked.identity(),
            "document.hasStorageAccess().then(value => \
             document.documentElement.setAttribute('data-has-access', value))",
        )
        .unwrap();
    assert_eq!(
        child_attribute(&blocked, "data-has-access").as_deref(),
        Some("true")
    );
    runtime
        .eval_in_document_realm(
            blocked.identity(),
            "document.documentElement.setAttribute('data-cookie', document.cookie)",
        )
        .unwrap();
    assert_eq!(
        child_attribute(&blocked, "data-cookie").as_deref(),
        Some("")
    );
    runtime
        .eval_in_document_realm(
            blocked.identity(),
            "cookieStore.get('session').then(cookie => \
             document.documentElement.setAttribute('data-cookie-store', cookie === null))",
        )
        .unwrap();
    assert_eq!(
        child_attribute(&blocked, "data-cookie-store").as_deref(),
        Some("true")
    );
    runtime
        .eval_in_document_realm(
            blocked.identity(),
            "fetch('https://third.test/probe').then(response => response.text()).then(body => \
             document.documentElement.setAttribute('data-echo', body))",
        )
        .unwrap();
    assert_eq!(
        child_attribute(&blocked, "data-echo").as_deref(),
        Some("none")
    );
    assert_eq!(
        runtime
            .host_state
            .borrow()
            .cookie_store
            .lock()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn trusted_input_marker_is_cleared_after_script_error() {
    let mut runtime = runtime_at("https://top.test/");
    let result = runtime.with_trusted_storage_access_input(|runtime| {
        runtime.eval("throw new Error('input handler failed')")
    });
    assert!(result.is_err());
    assert!(!runtime.host_state.borrow().trusted_storage_access_input);
}

#[test]
fn third_party_storage_access_grant_changes_dom_cookie_visibility() {
    let mut runtime = runtime_at("https://top.test/");
    runtime.set_http_resource_transport(Some(Arc::new(StorageAccessTransport)));
    runtime
        .host_state
        .borrow()
        .cookie_store
        .lock()
        .unwrap()
        .add_from_header_for_url(
            "session=preexisting; Path=/; Secure; SameSite=None",
            &"https://third.test/".parse().unwrap(),
        );
    runtime
        .eval(
            "const f = document.createElement('iframe'); \
             f.src = 'https://third.test/frame'; document.body.appendChild(f)",
        )
        .unwrap();
    runtime.tick(0).unwrap();
    let child = child_document(&runtime);
    let id = child.identity();

    runtime
        .eval_in_document_realm(
            id,
            "Promise.all([document.hasStorageAccess(), \
              document.requestStorageAccess().then(() => 'granted', e => e.name)]) \
              .then(([has, requested]) => { \
                document.cookie = 'blocked=1; Secure; SameSite=None'; \
                document.documentElement.setAttribute('data-result', \
                  [has, requested, document.cookie].join('|')); \
              })",
        )
        .unwrap();
    assert_eq!(
        child_result(&child).as_deref(),
        Some("false|NotAllowedError|")
    );
    runtime
        .eval_in_document_realm(
            id,
            "Promise.all([cookieStore.get('session'), cookieStore.set('blocked-store', '1')]) \
              .then(([entry]) => cookieStore.getAll().then(all => \
                document.documentElement.setAttribute('data-store', \
                  [entry === null, all.length, document.cookie].join('|'))))",
        )
        .unwrap();
    assert_eq!(
        child_attribute(&child, "data-store").as_deref(),
        Some("true|0|")
    );

    runtime
        .eval_in_document_realm(
            id,
            "document.addEventListener('click', () => { \
                document.requestStorageAccess().then( \
                  () => document.documentElement.setAttribute('data-result', 'forged-grant'), \
                  e => document.documentElement.setAttribute('data-result', e.name)); \
              }, {once: true}); \
              __omoikane_dispatch_mouse_input(document.body.__id, 'click', {button: 0}, false)",
        )
        .unwrap();
    assert_eq!(child_result(&child).as_deref(), Some("NotAllowedError"));

    runtime
        .with_trusted_storage_access_input(|runtime| {
            runtime.eval_in_document_realm(
                id,
                "document.addEventListener('click', () => { \
                    Promise.resolve().then(() => document.requestStorageAccess()).then(() => { \
                      document.cookie = 'written=1; Secure; SameSite=None'; \
                      document.hasStorageAccess().then(has => \
                        document.documentElement.setAttribute('data-result', \
                          [has, document.cookie].join('|'))); \
                    }); \
                  }, {once: true}); \
                  __omoikane_dispatch_mouse_input(document.body.__id, 'click', {button: 0}, false)",
            )
        })
        .unwrap();
    assert_eq!(
        child_result(&child).as_deref(),
        Some("true|session=preexisting; written=1")
    );
    runtime
        .eval_in_document_realm(
            id,
            "cookieStore.set({name: 'via-store', value: '1', sameSite: 'none'}) \
              .then(() => cookieStore.get('session')) \
              .then(entry => document.documentElement.setAttribute('data-store', \
                [entry.value, document.cookie].join('|')))",
        )
        .unwrap();
    assert_eq!(
        child_attribute(&child, "data-store").as_deref(),
        Some("preexisting|session=preexisting; written=1; via-store=1")
    );
    assert!(!runtime.host_state.borrow().trusted_storage_access_input);
}

#[test]
fn storage_access_permission_is_site_scoped_but_activation_is_document_local() {
    let mut runtime = runtime_at("https://top.test/");
    runtime.set_http_resource_transport(Some(Arc::new(StorageAccessTransport)));
    runtime
        .eval(
            "for (const [id, origin] of [['first', 'https://third.test'], \
               ['second', 'https://sub.third.test']]) { \
               const frame = document.createElement('iframe'); frame.id = id; \
               frame.src = origin + '/frame'; document.body.appendChild(frame); \
             }",
        )
        .unwrap();
    runtime.tick(0).unwrap();
    let first = iframe_document_by_id(&runtime, "first");
    let second = iframe_document_by_id(&runtime, "second");

    runtime
        .with_trusted_storage_access_input(|runtime| {
            runtime.eval_in_document_realm(
                first.identity(),
                "document.addEventListener('click', () => \
                   document.requestStorageAccess().then(() => \
                     document.documentElement.setAttribute('data-result', 'granted')), \
                   {once: true}); \
                 __omoikane_dispatch_mouse_input(document.body.__id, 'click', {button: 0}, false)",
            )
        })
        .unwrap();
    assert_eq!(child_result(&first).as_deref(), Some("granted"));

    runtime
        .eval_in_document_realm(
            second.identity(),
            "document.hasStorageAccess().then(before => \
               document.requestStorageAccess().then(() => \
                 document.hasStorageAccess().then(after => \
                   document.documentElement.setAttribute('data-result', \
                     [before, after].join('|')))))",
        )
        .unwrap();
    assert_eq!(child_result(&second).as_deref(), Some("false|true"));

    runtime
        .eval("document.getElementById('second').src = 'https://sub.third.test/next'")
        .unwrap();
    runtime.tick(0).unwrap();
    let replacement = iframe_document_by_id(&runtime, "second");
    assert_ne!(replacement.identity(), second.identity());
    runtime
        .eval_in_document_realm(
            replacement.identity(),
            "document.hasStorageAccess().then(value => \
               document.documentElement.setAttribute('data-result', String(value)))",
        )
        .unwrap();
    assert_eq!(child_result(&replacement).as_deref(), Some("false"));
}

#[test]
fn third_party_fetch_uses_cookies_only_after_its_document_activates_access() {
    let mut runtime = runtime_at("https://top.test/");
    runtime.set_http_resource_transport(Some(Arc::new(CookieEchoTransport::default())));
    runtime
        .host_state
        .borrow()
        .cookie_store
        .lock()
        .unwrap()
        .add_from_header_for_url(
            "session=preexisting; Path=/; Secure; SameSite=None",
            &"https://third.test/".parse().unwrap(),
        );
    runtime
        .eval(
            "const f = document.createElement('iframe'); \
             f.src = 'https://third.test/frame'; document.body.appendChild(f)",
        )
        .unwrap();
    runtime.tick(0).unwrap();
    let child = child_document(&runtime);

    runtime
        .eval_in_document_realm(
            child.identity(),
            "fetch('https://third.test/probe').then(r => r.text()).then(body => \
               document.documentElement.setAttribute('data-result', body))",
        )
        .unwrap();
    assert_eq!(child_result(&child).as_deref(), Some("none"));
    assert_eq!(
        runtime
            .host_state
            .borrow()
            .cookie_store
            .lock()
            .unwrap()
            .len(),
        1,
        "a blocked third-party response must not store Set-Cookie",
    );

    runtime
        .with_trusted_storage_access_input(|runtime| {
            runtime.eval_in_document_realm(
                child.identity(),
                "document.addEventListener('click', () => \
                   document.requestStorageAccess().then(() => \
                     fetch('https://third.test/probe').then(r => r.text()).then(body => \
                       document.documentElement.setAttribute('data-result', body))), \
                   {once: true}); \
                 __omoikane_dispatch_mouse_input(document.body.__id, 'click', {button: 0}, false)",
            )
        })
        .unwrap();
    assert_eq!(child_result(&child).as_deref(), Some("session=preexisting"));
    assert_eq!(
        runtime
            .host_state
            .borrow()
            .cookie_store
            .lock()
            .unwrap()
            .document_cookie(
                &"https://third.test/".parse().unwrap(),
                &"https://third.test/".parse().unwrap(),
            ),
        "session=preexisting; response=stored"
    );
}

#[test]
fn cross_site_ancestor_blocks_aba_frame_until_it_requests_access() {
    let mut runtime = runtime_at("https://top.test/");
    let transport = Arc::new(CookieEchoTransport::default());
    runtime.set_http_resource_transport(Some(transport.clone()));
    runtime
        .host_state
        .borrow()
        .cookie_store
        .lock()
        .unwrap()
        .add_from_header_for_url(
            "session=preexisting; Path=/; Secure; SameSite=None",
            &"https://top.test/".parse().unwrap(),
        );
    runtime
        .host_state
        .borrow()
        .cookie_store
        .lock()
        .unwrap()
        .add_from_header_for_url(
            "strict=hidden; Path=/; Secure; SameSite=Strict",
            &"https://top.test/".parse().unwrap(),
        );
    runtime
        .eval(
            "const middle = document.createElement('iframe'); middle.id = 'middle'; \
             middle.src = 'https://middle.test/frame'; document.body.appendChild(middle)",
        )
        .unwrap();
    runtime.tick(0).unwrap();
    let middle = iframe_document_by_id(&runtime, "middle");
    runtime
        .eval_in_document_realm(
            middle.identity(),
            "const inner = document.createElement('iframe'); inner.id = 'inner'; \
             inner.src = 'https://top.test/frame'; document.body.appendChild(inner)",
        )
        .unwrap();
    runtime.tick(0).unwrap();
    let inner = iframe_document_by_id(&runtime, "inner");
    assert_eq!(
        transport
            .requests
            .lock()
            .unwrap()
            .iter()
            .find(|(url, _)| url == "https://top.test/frame")
            .map(|(_, cookie)| cookie.as_deref()),
        Some(None),
        "a same-site iframe behind a cross-site ancestor must omit cookies on navigation",
    );

    runtime
        .eval_in_document_realm(
            inner.identity(),
            "Promise.all([document.hasStorageAccess(), \
              fetch('https://top.test/probe').then(r => r.text())]) \
              .then(([has, cookie]) => document.documentElement.setAttribute('data-result', \
                [has, cookie, document.cookie].join('|')))",
        )
        .unwrap();
    assert_eq!(child_result(&inner).as_deref(), Some("false|none|"));

    runtime
        .eval_in_document_realm(
            inner.identity(),
            "document.requestStorageAccess().then(() => Promise.all([ \
              fetch('https://top.test/probe').then(r => r.text()), \
              cookieStore.get('strict')])).then(([cookie, strict]) => \
                document.documentElement.setAttribute('data-result', \
                  [cookie, document.cookie, strict === null].join('|')))",
        )
        .unwrap();
    assert_eq!(
        child_result(&inner).as_deref(),
        Some("session=preexisting|session=preexisting; response=stored|true")
    );

    runtime
        .eval_in_document_realm(
            inner.identity(),
            "fetch('https://top.test/redirect').then(r => r.text()).then(body => \
               document.documentElement.setAttribute('data-redirect', body))",
        )
        .unwrap();
    assert_eq!(
        child_attribute(&inner, "data-redirect").as_deref(),
        Some("session=preexisting; response=stored"),
        "a same-origin redirect retains the grant but still filters SameSite=Strict",
    );
    let requests = transport.requests.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .find(|(url, _)| url == "https://top.test/redirect")
            .map(|(_, cookie)| cookie.as_deref()),
        Some(Some("session=preexisting; response=stored")),
    );
}

#[test]
fn third_party_script_and_font_requests_follow_document_access() {
    let mut runtime = runtime_at("https://top.test/");
    let transport = Arc::new(ResourceCookieTransport::default());
    runtime.set_http_resource_transport(Some(transport.clone()));
    runtime
        .host_state
        .borrow()
        .cookie_store
        .lock()
        .unwrap()
        .add_from_header_for_url(
            "session=pre; Path=/; Secure; SameSite=None",
            &"https://third.test/".parse().unwrap(),
        );
    runtime
        .eval(
            "const frame = document.createElement('iframe'); \
             frame.src = 'https://third.test/frame'; document.body.appendChild(frame)",
        )
        .unwrap();
    runtime.tick(0).unwrap();
    let child = child_document(&runtime);

    for phase in ["pre", "post"] {
        if phase == "post" {
            activate_storage_access(&mut runtime, &child);
        }
        runtime
            .eval_in_document_realm(
                child.identity(),
                &format!(
                    "const script_{phase} = document.createElement('script'); \
                     script_{phase}.src = 'https://third.test/{phase}-script.js'; \
                     document.body.appendChild(script_{phase}); \
                     const font_{phase} = new FontFace('Probe{phase}', \
                       'url(https://third.test/{phase}-font.ttf)'); \
                     font_{phase}.load()"
                ),
            )
            .unwrap();
        runtime.run_until_idle().unwrap();
    }

    let requests = transport.requests.lock().unwrap();
    for resource in ["script.js", "font.ttf"] {
        assert_eq!(
            requests
                .iter()
                .find(|(_, path, _)| path == &format!("/pre-{resource}"))
                .map(|(_, _, cookie)| cookie.as_deref()),
            Some(None),
            "grant-free {resource} request must omit Cookie",
        );
        assert!(
            requests
                .iter()
                .find(|(_, path, _)| path == &format!("/post-{resource}"))
                .and_then(|(_, _, cookie)| cookie.as_deref())
                .is_some_and(|cookie| cookie.contains("session=pre")),
            "granted {resource} request must include the Document cookie",
        );
    }
    let cookies = runtime
        .host_state
        .borrow()
        .cookie_store
        .lock()
        .unwrap()
        .document_cookie(
            &"https://third.test/".parse().unwrap(),
            &"https://third.test/".parse().unwrap(),
        );
    assert!(!cookies.contains("pre_script_js"));
    assert!(!cookies.contains("pre_font_ttf"));
    assert!(cookies.contains("post_script_js=1"));
    assert!(cookies.contains("post_font_ttf=1"));
}

#[test]
fn third_party_worker_and_worklet_loads_follow_owner_document_access() {
    let mut runtime = runtime_at("https://top.test/");
    let transport = Arc::new(ResourceCookieTransport::default());
    runtime.set_http_resource_transport(Some(transport.clone()));
    runtime
        .host_state
        .borrow()
        .cookie_store
        .lock()
        .unwrap()
        .add_from_header_for_url(
            "session=pre; Path=/; Secure; SameSite=None",
            &"https://third.test/".parse().unwrap(),
        );
    runtime
        .eval(
            "const frame = document.createElement('iframe'); \
             frame.src = 'https://third.test/frame'; document.body.appendChild(frame)",
        )
        .unwrap();
    runtime.tick(0).unwrap();
    let child = child_document(&runtime);

    runtime
        .eval_in_document_realm(
            child.identity(),
            "try { new Worker('https://top.test/foreign-worker.js'); \
               document.documentElement.setAttribute('data-foreign-dedicated', 'accepted'); \
             } catch (_) { \
               document.documentElement.setAttribute('data-foreign-dedicated', 'rejected'); \
             } \
             try { new SharedWorker('https://top.test/foreign-shared.js', 'foreign'); \
               document.documentElement.setAttribute('data-foreign-worker', 'accepted'); \
             } catch (_) { \
               document.documentElement.setAttribute('data-foreign-worker', 'rejected'); \
             } \
             CSS.paintWorklet.addModule('https://top.test/foreign-worklet.js') \
               .catch(() => document.documentElement.setAttribute('data-foreign-worklet', 'rejected'))",
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        child_attribute(&child, "data-foreign-dedicated").as_deref(),
        Some("rejected")
    );
    assert_eq!(
        child_attribute(&child, "data-foreign-worker").as_deref(),
        Some("rejected")
    );
    assert_eq!(
        child_attribute(&child, "data-foreign-worklet").as_deref(),
        Some("rejected")
    );

    for phase in ["pre", "post"] {
        if phase == "post" {
            activate_storage_access(&mut runtime, &child);
        }
        runtime
            .eval_in_document_realm(
                child.identity(),
                &format!(
                    "new Worker('https://third.test/{phase}-worker.js'); \
                     new SharedWorker('https://third.test/{phase}-shared.js', '{phase}'); \
                     CSS.paintWorklet.addModule('https://third.test/{phase}-worklet.js')"
                ),
            )
            .unwrap();
        runtime.run_until_idle().unwrap();
    }

    let requests = transport.requests.lock().unwrap();
    assert!(
        requests.iter().all(|(_, path, _)| !matches!(
            path.as_str(),
            "/foreign-worker.js" | "/foreign-shared.js"
        )),
        "a child cannot fetch a worker script from the top Document's origin"
    );
    // Worklet modules use CORS rather than the classic Worker's same-origin
    // constructor restriction. A denied CORS request still goes to the network,
    // but must not carry the child Document's cookies or execute the module.
    let foreign_worklet: Vec<_> = requests
        .iter()
        .filter(|(_, path, _)| path == "/foreign-worklet.js")
        .collect();
    assert_eq!(foreign_worklet.len(), 1);
    assert_eq!(foreign_worklet[0].0, Method::Get);
    assert_eq!(foreign_worklet[0].2, None);
    for resource in ["worker.js", "shared.js", "worklet.js"] {
        assert_eq!(
            requests
                .iter()
                .find(|(_, path, _)| path == &format!("/pre-{resource}"))
                .map(|(_, _, cookie)| cookie.as_deref()),
            Some(None),
            "grant-free {resource} request must omit Cookie",
        );
        assert!(
            requests
                .iter()
                .find(|(_, path, _)| path == &format!("/post-{resource}"))
                .and_then(|(_, _, cookie)| cookie.as_deref())
                .is_some_and(|cookie| cookie.contains("session=pre")),
            "granted {resource} request must include the Document cookie",
        );
    }
    let cookies = runtime
        .host_state
        .borrow()
        .cookie_store
        .lock()
        .unwrap()
        .document_cookie(
            &"https://third.test/".parse().unwrap(),
            &"https://third.test/".parse().unwrap(),
        );
    assert!(!cookies.contains("pre_shared_js"));
    assert!(!cookies.contains("pre_worker_js"));
    assert!(!cookies.contains("pre_worklet_js"));
    assert!(cookies.contains("post_worker_js=1"));
    assert!(cookies.contains("post_shared_js=1"));
    assert!(cookies.contains("post_worklet_js=1"));
}

#[test]
fn worker_internal_fetch_and_import_scripts_use_shared_jar_without_owner_grant() {
    let mut runtime = runtime_at("https://top.test/");
    let transport = Arc::new(ResourceCookieTransport::default());
    runtime.set_http_resource_transport(Some(transport.clone()));
    runtime
        .host_state
        .borrow()
        .cookie_store
        .lock()
        .unwrap()
        .add_from_header_for_url(
            "session=pre; Path=/; Secure; SameSite=None",
            &"https://third.test/".parse().unwrap(),
        );
    runtime
        .eval(
            "const frame = document.createElement('iframe'); \
             frame.src = 'https://third.test/frame'; document.body.appendChild(frame)",
        )
        .unwrap();
    runtime.tick(0).unwrap();
    let child = child_document(&runtime);

    for phase in ["pre", "post"] {
        if phase == "post" {
            activate_storage_access(&mut runtime, &child);
            runtime
                .eval_in_document_realm(child.identity(), "preWorker.postMessage('after grant')")
                .unwrap();
            runtime.run_until_idle().unwrap();
        }
        runtime
            .eval_in_document_realm(
                child.identity(),
                &format!(
                    "globalThis.{phase}Worker = new Worker('https://third.test/{phase}-internal-worker.js'); \
                     globalThis.{phase}Shared = new SharedWorker('https://third.test/{phase}-internal-shared.js', '{phase}')"
                ),
            )
            .unwrap();
        runtime.run_until_idle().unwrap();
    }

    let requests = transport.requests.lock().unwrap();
    for kind in ["worker", "shared"] {
        for phase in ["pre", "post"] {
            let initial = format!("/{phase}-internal-{kind}.js");
            let initial_cookie = requests
                .iter()
                .find(|(_, path, _)| path == &initial)
                .map(|(_, _, cookie)| cookie.as_deref());
            assert_eq!(
                initial_cookie.is_some_and(|cookie| cookie.is_some()),
                phase == "post",
                "initial {kind} script follows its owner Document's grant: {requests:?}"
            );
            for suffix in ["fetch", "import.js"] {
                let internal = format!("/{phase}-internal-{kind}-{suffix}");
                assert_eq!(
                    requests
                        .iter()
                        .find(|(_, path, _)| path == &internal)
                        .map(|(_, _, cookie)| cookie.as_deref()),
                    Some(None),
                    "internal {kind} request must remain third-party after owner grant: {requests:?}"
                );
            }
        }
    }
    assert_eq!(
        requests
            .iter()
            .find(|(_, path, _)| path == "/pre-internal-worker-later-fetch")
            .map(|(_, _, cookie)| cookie.as_deref()),
        Some(None),
        "an already-created Worker must not observe a later owner grant: {requests:?}"
    );
    drop(requests);
    let cookies = runtime
        .host_state
        .borrow()
        .cookie_store
        .lock()
        .unwrap()
        .document_cookie(
            &"https://third.test/".parse().unwrap(),
            &"https://third.test/".parse().unwrap(),
        );
    for kind in ["worker", "shared"] {
        assert!(cookies.contains(&format!("post_internal_{kind}_js=1")));
        assert!(!cookies.contains(&format!("pre_internal_{kind}_js=")));
        for phase in ["pre", "post"] {
            assert!(!cookies.contains(&format!("{phase}_internal_{kind}_fetch=")));
            assert!(!cookies.contains(&format!("{phase}_internal_{kind}_import_js=")));
        }
    }
    assert!(!cookies.contains("pre_internal_worker_later_fetch="));
}

#[test]
fn first_party_worker_internal_requests_share_owner_cookie_jar() {
    let transport = Arc::new(ResourceCookieTransport::default());
    let mut first_party = runtime_at("https://third.test/");
    first_party.set_http_resource_transport(Some(transport.clone()));
    first_party
        .host_state
        .borrow()
        .cookie_store
        .lock()
        .unwrap()
        .add_from_header_for_url(
            "session=first; Path=/; Secure; SameSite=Strict",
            &"https://third.test/".parse().unwrap(),
        );
    first_party
        .eval(
            "globalThis.worker = new Worker('https://third.test/first-internal-worker.js'); \
             globalThis.shared = new SharedWorker('https://third.test/first-internal-shared.js', 'first')",
        )
        .unwrap();
    first_party.run_until_idle().unwrap();
    let requests = transport.requests.lock().unwrap();
    for kind in ["worker", "shared"] {
        for suffix in ["fetch", "import.js"] {
            let path = format!("/first-internal-{kind}-{suffix}");
            assert!(
                requests
                    .iter()
                    .find(|(_, observed, _)| observed == &path)
                    .and_then(|(_, _, cookie)| cookie.as_deref())
                    .is_some_and(|cookie| cookie.contains("session=first")),
                "first-party internal {kind} request must use owner jar: {requests:?}"
            );
        }
    }
    drop(requests);
    let cookies = first_party
        .host_state
        .borrow()
        .cookie_store
        .lock()
        .unwrap()
        .document_cookie(
            &"https://third.test/".parse().unwrap(),
            &"https://third.test/".parse().unwrap(),
        );
    for kind in ["worker", "shared"] {
        assert!(cookies.contains(&format!("first_internal_{kind}_fetch=1")));
        assert!(cookies.contains(&format!("first_internal_{kind}_import_js=1")));
    }
}

#[test]
fn cross_site_ancestor_blocks_internal_worker_cookies_after_grant() {
    let mut runtime = runtime_at("https://top.test/");
    let transport = Arc::new(ResourceCookieTransport::default());
    runtime.set_http_resource_transport(Some(transport.clone()));
    runtime
        .host_state
        .borrow()
        .cookie_store
        .lock()
        .unwrap()
        .add_from_header_for_url(
            "session=pre; Path=/; Secure; SameSite=None",
            &"https://top.test/".parse().unwrap(),
        );
    runtime
        .eval(
            "const middle = document.createElement('iframe'); middle.id = 'middle'; \
             middle.src = 'https://middle.test/frame'; document.body.appendChild(middle)",
        )
        .unwrap();
    runtime.tick(0).unwrap();
    let middle = iframe_document_by_id(&runtime, "middle");
    runtime
        .eval_in_document_realm(
            middle.identity(),
            "const inner = document.createElement('iframe'); inner.id = 'inner'; \
             inner.src = 'https://top.test/frame'; document.body.appendChild(inner)",
        )
        .unwrap();
    runtime.tick(0).unwrap();
    let inner = iframe_document_by_id(&runtime, "inner");
    activate_storage_access(&mut runtime, &inner);
    runtime
        .eval_in_document_realm(
            inner.identity(),
            "new Worker('https://top.test/post-internal-worker.js')",
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    let requests = transport.requests.lock().unwrap();
    assert!(
        requests
            .iter()
            .find(|(_, path, _)| path == "/post-internal-worker.js")
            .and_then(|(_, _, cookie)| cookie.as_deref())
            .is_some_and(|cookie| cookie.contains("session=pre"))
    );
    for suffix in ["fetch", "import.js"] {
        assert_eq!(
            requests
                .iter()
                .find(|(_, path, _)| path == &format!("/post-internal-worker-{suffix}"))
                .map(|(_, _, cookie)| cookie.as_deref()),
            Some(None),
            "an A→B→A worker remains third-party after its Document grant: {requests:?}"
        );
    }
}

#[test]
fn shared_worker_connection_with_cross_site_ancestor_downgrades_internal_requests() {
    // Both owners have top-level site top.test and origin top.test, so the
    // registry key matches even though only the nested owner has a cross-site
    // ancestor. Different top-level sites are covered by the partition test.
    let mut runtime = runtime_at("https://top.test/");
    let transport = Arc::new(ResourceCookieTransport::default());
    runtime.set_http_resource_transport(Some(transport.clone()));
    runtime
        .host_state
        .borrow()
        .cookie_store
        .lock()
        .unwrap()
        .add_from_header_for_url(
            "session=first; Path=/; Secure; SameSite=Strict",
            &"https://top.test/".parse().unwrap(),
        );
    runtime
        .eval("globalThis.shared = new SharedWorker('https://top.test/mixed-shared.js', 'mixed')")
        .unwrap();
    runtime.run_until_idle().unwrap();

    runtime
        .eval(
            "const middle = document.createElement('iframe'); middle.id = 'middle'; \
             middle.src = 'https://middle.test/frame'; document.body.appendChild(middle)",
        )
        .unwrap();
    runtime.tick(0).unwrap();
    let middle = iframe_document_by_id(&runtime, "middle");
    runtime
        .eval_in_document_realm(
            middle.identity(),
            "const inner = document.createElement('iframe'); inner.id = 'inner'; \
             inner.src = 'https://top.test/frame'; document.body.appendChild(inner)",
        )
        .unwrap();
    runtime.tick(0).unwrap();
    let inner = iframe_document_by_id(&runtime, "inner");
    activate_storage_access(&mut runtime, &inner);
    runtime
        .eval_in_document_realm(
            inner.identity(),
            "globalThis.shared = new SharedWorker('https://top.test/mixed-shared.js', 'mixed')",
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    runtime
        .eval("globalThis.third = new SharedWorker('https://top.test/mixed-shared.js', 'mixed')")
        .unwrap();
    runtime.run_until_idle().unwrap();
    runtime
        .eval("globalThis.independent = new SharedWorker('https://top.test/first-internal-shared.js', 'independent')")
        .unwrap();
    runtime.run_until_idle().unwrap();

    let requests = transport.requests.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|(_, path, _)| path == "/mixed-shared.js")
            .count(),
        1,
        "the second owner must reuse the existing SharedWorker"
    );
    assert!(
        requests
            .iter()
            .find(|(_, path, _)| path == "/mixed-1-fetch")
            .and_then(|(_, _, cookie)| cookie.as_deref())
            .is_some_and(|cookie| cookie.contains("session=first")),
        "first-party connection may use its cookies: {requests:?}"
    );
    assert!(
        requests
            .iter()
            .find(|(_, path, _)| path == "/mixed-internal-worker-fetch")
            .and_then(|(_, _, cookie)| cookie.as_deref())
            .is_some_and(|cookie| cookie.contains("session=first")),
        "a nested Worker initially inherits the first-party cookie session: {requests:?}"
    );
    assert_eq!(
        requests
            .iter()
            .find(|(_, path, _)| path == "/mixed-2-fetch")
            .map(|(_, _, cookie)| cookie.as_deref()),
        Some(None),
        "a granted third-party connection cannot borrow another owner's worker cookies: {requests:?}"
    );
    assert!(
        requests
            .iter()
            .filter(|(_, path, _)| path == "/mixed-internal-worker-later-fetch")
            .all(|(_, _, cookie)| cookie.is_none())
            && requests
                .iter()
                .any(|(_, path, _)| path == "/mixed-internal-worker-later-fetch"),
        "existing descendants also lose cookie access when a cross-site owner connects: {requests:?}"
    );
    assert_eq!(
        requests
            .iter()
            .find(|(_, path, _)| path == "/mixed-3-fetch")
            .map(|(_, _, cookie)| cookie.as_deref()),
        Some(None),
        "a SharedWorker stays restricted after a cross-site owner has connected: {requests:?}"
    );
    assert!(
        requests
            .iter()
            .find(|(_, path, _)| path == "/first-internal-shared-fetch")
            .and_then(|(_, _, cookie)| cookie.as_deref())
            .is_some_and(|cookie| cookie.contains("session=first")),
        "other first-party Workers in the same cookie session remain credentialed: {requests:?}"
    );
    drop(requests);
    let cookies = runtime
        .host_state
        .borrow()
        .cookie_store
        .lock()
        .unwrap()
        .document_cookie(
            &"https://top.test/".parse().unwrap(),
            &"https://top.test/".parse().unwrap(),
        );
    assert!(cookies.contains("mixed_1_fetch=1"));
    assert!(!cookies.contains("mixed_2_fetch="));
    assert!(!cookies.contains("mixed_3_fetch="));
    assert!(!cookies.contains("mixed_internal_worker_later_fetch="));
}

#[test]
fn shared_worker_registry_separates_independent_cookie_sessions() {
    let transport = Arc::new(ResourceCookieTransport::default());
    let mut first = runtime_at("https://top.test/");
    let mut second = runtime_at("https://top.test/");
    for (runtime, session) in [(&mut first, "first"), (&mut second, "second")] {
        runtime.set_http_resource_transport(Some(transport.clone()));
        runtime
            .host_state
            .borrow()
            .cookie_store
            .lock()
            .unwrap()
            .add_from_header_for_url(
                &format!("session={session}; Path=/; Secure; SameSite=Strict"),
                &"https://top.test/".parse().unwrap(),
            );
        runtime
            .eval("globalThis.shared = new SharedWorker('https://top.test/session-shared.js', 'session')")
            .unwrap();
    }

    let requests = transport.requests.lock().unwrap();
    let cookies: Vec<_> = requests
        .iter()
        .filter(|(_, path, _)| path == "/session-shared.js")
        .map(|(_, _, cookie)| cookie.as_deref())
        .collect();
    assert_eq!(
        cookies,
        [Some("session=first"), Some("session=second")],
        "independent browsing sessions cannot reuse the same SharedWorker"
    );
}

#[test]
fn nested_dedicated_worker_inherits_creator_cookie_session() {
    let transport = Arc::new(ResourceCookieTransport::default());
    let mut runtime = runtime_at("https://third.test/");
    runtime.set_http_resource_transport(Some(transport.clone()));
    runtime
        .host_state
        .borrow()
        .cookie_store
        .lock()
        .unwrap()
        .add_from_header_for_url(
            "session=nested; Path=/; Secure; SameSite=Strict",
            &"https://third.test/".parse().unwrap(),
        );
    runtime
        .eval("globalThis.owner = new Worker('https://third.test/nested-owner.js')")
        .unwrap();
    runtime.run_until_idle().unwrap();

    let requests = transport.requests.lock().unwrap();
    for path in [
        "/nested-internal-worker.js",
        "/nested-internal-worker-fetch",
        "/nested-internal-worker-import.js",
    ] {
        assert!(
            requests
                .iter()
                .find(|(_, requested, _)| requested == path)
                .and_then(|(_, _, cookie)| cookie.as_deref())
                .is_some_and(|cookie| cookie.contains("session=nested")),
            "nested Worker {path} must retain the first-party creator's jar and site: {requests:?}"
        );
    }
    drop(requests);
    let cookies = runtime
        .host_state
        .borrow()
        .cookie_store
        .lock()
        .unwrap()
        .document_cookie(
            &"https://third.test/".parse().unwrap(),
            &"https://third.test/".parse().unwrap(),
        );
    assert!(cookies.contains("nested_internal_worker_fetch=1"));
    assert!(cookies.contains("nested_internal_worker_import_js=1"));
}

#[test]
fn shared_worker_global_exposes_dedicated_worker_and_inherits_cookie_session() {
    let transport = Arc::new(ResourceCookieTransport::default());
    let mut runtime = runtime_at("https://third.test/");
    runtime.set_http_resource_transport(Some(transport.clone()));
    runtime
        .host_state
        .borrow()
        .cookie_store
        .lock()
        .unwrap()
        .add_from_header_for_url(
            "session=nested; Path=/; Secure; SameSite=Strict",
            &"https://third.test/".parse().unwrap(),
        );
    runtime
        .eval(
            "globalThis.shared = new SharedWorker('https://third.test/nested-shared-owner.js', 'nested')",
        )
        .unwrap();
    runtime.run_until_idle().unwrap();

    let requests = transport.requests.lock().unwrap();
    for path in [
        "/nested-internal-worker.js",
        "/nested-internal-worker-fetch",
        "/nested-internal-worker-import.js",
    ] {
        assert!(
            requests
                .iter()
                .find(|(_, requested, _)| requested == path)
                .and_then(|(_, _, cookie)| cookie.as_deref())
                .is_some_and(|cookie| cookie.contains("session=nested")),
            "a SharedWorker may construct a Dedicated Worker in the same cookie session: {requests:?}"
        );
    }
}

#[test]
fn nested_dedicated_worker_keeps_cross_site_ancestry() {
    let transport = Arc::new(ResourceCookieTransport::default());
    let mut runtime = runtime_at("https://top.test/");
    runtime.set_http_resource_transport(Some(transport.clone()));
    runtime
        .host_state
        .borrow()
        .cookie_store
        .lock()
        .unwrap()
        .add_from_header_for_url(
            "session=third; Path=/; Secure; SameSite=None",
            &"https://third.test/".parse().unwrap(),
        );
    runtime
        .eval(
            "const frame = document.createElement('iframe'); \
             frame.src = 'https://third.test/frame'; document.body.appendChild(frame)",
        )
        .unwrap();
    runtime.tick(0).unwrap();
    let child = child_document(&runtime);
    runtime
        .eval_in_document_realm(
            child.identity(),
            "globalThis.owner = new Worker('https://third.test/nested-owner.js')",
        )
        .unwrap();
    runtime.run_until_idle().unwrap();

    let requests = transport.requests.lock().unwrap();
    for path in [
        "/nested-internal-worker.js",
        "/nested-internal-worker-fetch",
        "/nested-internal-worker-import.js",
    ] {
        assert_eq!(
            requests
                .iter()
                .find(|(_, requested, _)| requested == path)
                .map(|(_, _, cookie)| cookie.as_deref()),
            Some(None),
            "nested Worker {path} must remain third-party: {requests:?}"
        );
    }
    drop(requests);
    let cookies = runtime
        .host_state
        .borrow()
        .cookie_store
        .lock()
        .unwrap()
        .document_cookie(
            &"https://third.test/".parse().unwrap(),
            &"https://third.test/".parse().unwrap(),
        );
    assert!(!cookies.contains("nested_internal_worker_fetch="));
    assert!(!cookies.contains("nested_internal_worker_import_js="));
}

#[test]
fn dedicated_worker_global_does_not_expose_shared_worker_constructor() {
    let mut runtime = runtime_at("https://top.test/");
    runtime.set_http_resource_transport(Some(Arc::new(ResourceCookieTransport::default())));
    runtime
        .eval("globalThis.worker = new Worker('https://top.test/worker.js')")
        .unwrap();
    let worker = runtime
        .host_state
        .borrow()
        .workers
        .values()
        .next()
        .cloned()
        .unwrap();
    assert_eq!(
        worker
            .borrow_mut()
            .runtime
            .eval("[typeof Worker, typeof SharedWorker].join(',')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "function,undefined"
    );
}

#[test]
fn shared_worker_registry_partitions_first_and_third_party_owners() {
    let transport = Arc::new(ResourceCookieTransport::default());
    // Keep the cookie session constant so this isolates the top-level-site key.
    let cookies = Arc::new(Mutex::new(crate::http::CookieJar::new()));
    let mut first_party = runtime_at("https://third.test/");
    first_party.set_shared_cookie_store(Arc::clone(&cookies));
    first_party.set_http_resource_transport(Some(transport.clone()));
    first_party
        .host_state
        .borrow()
        .cookie_store
        .lock()
        .unwrap()
        .add_from_header_for_url(
            "session=pre; Path=/; Secure; SameSite=None",
            &"https://third.test/".parse().unwrap(),
        );
    first_party
        .eval(
            "globalThis.partitionWorker = \
             new SharedWorker('https://third.test/partition-shared.js', 'partition')",
        )
        .unwrap();

    let mut third_party = runtime_at("https://top.test/");
    third_party.set_shared_cookie_store(Arc::clone(&cookies));
    third_party.set_http_resource_transport(Some(transport.clone()));
    third_party
        .eval(
            "const frame = document.createElement('iframe'); \
             frame.src = 'https://third.test/frame'; document.body.appendChild(frame)",
        )
        .unwrap();
    third_party.tick(0).unwrap();
    let child = child_document(&third_party);
    third_party
        .eval_in_document_realm(
            child.identity(),
            "globalThis.partitionWorker = \
             new SharedWorker('https://third.test/partition-shared.js', 'partition')",
        )
        .unwrap();

    let requests = transport.requests.lock().unwrap();
    let shared: Vec<_> = requests
        .iter()
        .filter(|(_, path, _)| path == "/partition-shared.js")
        .map(|(_, _, cookie)| cookie.as_deref())
        .collect();
    assert_eq!(shared, [Some("session=pre"), None]);
}

#[test]
fn worklet_does_not_reuse_a_different_origin_runtime() {
    let mut runtime = runtime_at("https://top.test/");
    let transport = Arc::new(ResourceCookieTransport::default());
    runtime.set_http_resource_transport(Some(transport.clone()));
    runtime
        .eval("CSS.paintWorklet.addModule('https://top.test/top-worklet.js')")
        .unwrap();
    runtime.run_until_idle().unwrap();
    runtime
        .eval(
            "const frame = document.createElement('iframe'); \
             frame.src = 'https://third.test/frame'; document.body.appendChild(frame)",
        )
        .unwrap();
    runtime.tick(0).unwrap();
    let child = child_document(&runtime);
    runtime
        .eval_in_document_realm(
            child.identity(),
            "CSS.paintWorklet.addModule('https://third.test/child-worklet.js') \
             .catch(error => document.documentElement.setAttribute('data-error', error.name))",
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        child_attribute(&child, "data-error").as_deref(),
        Some("SecurityError")
    );
    assert!(
        transport
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|(_, path, _)| path != "/child-worklet.js"),
        "different-origin module bytes cannot enter an existing WorkletGlobalScope"
    );
}

#[test]
fn font_redirect_back_to_granted_origin_does_not_restore_cookie_access() {
    let mut runtime = runtime_at("https://top.test/");
    let transport = Arc::new(ResourceCookieTransport::default());
    runtime.set_http_resource_transport(Some(transport.clone()));
    runtime
        .host_state
        .borrow()
        .cookie_store
        .lock()
        .unwrap()
        .add_from_header_for_url(
            "session=pre; Path=/; Secure; SameSite=None",
            &"https://third.test/".parse().unwrap(),
        );
    runtime
        .eval(
            "const frame = document.createElement('iframe'); \
             frame.src = 'https://third.test/frame'; document.body.appendChild(frame)",
        )
        .unwrap();
    runtime.tick(0).unwrap();
    let child = child_document(&runtime);
    activate_storage_access(&mut runtime, &child);
    runtime
        .eval_in_document_realm(
            child.identity(),
            "new FontFace('RedirectProbe', \
             'url(https://third.test/redirect-font.ttf)').load()",
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    let requests = transport.requests.lock().unwrap();
    for (path, cookie) in [
        ("/redirect-font.ttf", Some("session=pre")),
        ("/hop-font.ttf", None),
        ("/post-return-font.ttf", None),
    ] {
        assert_eq!(
            requests
                .iter()
                .find(|(_, request_path, _)| request_path == path)
                .map(|(_, _, cookie)| cookie.as_deref()),
            Some(cookie),
            "cookie policy on {path} after a cross-origin font redirect",
        );
    }
    assert!(
        !runtime
            .host_state
            .borrow()
            .cookie_store
            .lock()
            .unwrap()
            .document_cookie(
                &"https://third.test/".parse().unwrap(),
                &"https://third.test/".parse().unwrap(),
            )
            .contains("post_return_font_ttf"),
        "the final response must not store Set-Cookie after a cross-origin hop",
    );
}

#[test]
fn third_party_iframe_forms_use_their_source_documents_access() {
    let mut runtime = runtime_at("https://top.test/");
    let transport = Arc::new(ResourceCookieTransport::default());
    runtime.set_http_resource_transport(Some(transport.clone()));
    runtime
        .host_state
        .borrow()
        .cookie_store
        .lock()
        .unwrap()
        .add_from_header_for_url(
            "session=pre; Path=/; Secure; SameSite=None",
            &"https://third.test/".parse().unwrap(),
        );
    runtime
        .eval(
            "const frame = document.createElement('iframe'); \
             frame.src = 'https://third.test/frame'; document.body.appendChild(frame)",
        )
        .unwrap();
    runtime.tick(0).unwrap();
    let child = child_document(&runtime);
    runtime
        .eval_in_document_realm(
            child.identity(),
            "const sink = document.createElement('iframe'); sink.name = 'sink'; \
             document.body.appendChild(sink)",
        )
        .unwrap();

    for (phase, method) in [
        ("pre-get", "get"),
        ("pre-post", "post"),
        ("post-get", "get"),
        ("post-post", "post"),
    ] {
        if phase == "post-get" {
            activate_storage_access(&mut runtime, &child);
        }
        runtime
            .eval_in_document_realm(
                child.identity(),
                &format!(
                    "(() => {{ const form = document.createElement('form'); \
                     form.method = '{method}'; form.target = 'sink'; \
                     form.action = 'https://third.test/{phase}-form'; \
                     document.body.appendChild(form); form.submit() }})()"
                ),
            )
            .unwrap();
        runtime.tick(0).unwrap();
    }
    let requests = transport.requests.lock().unwrap();
    for (phase, method) in [
        ("pre-get", Method::Get),
        ("pre-post", Method::Post),
        ("post-get", Method::Get),
        ("post-post", Method::Post),
    ] {
        let request = requests
            .iter()
            .find(|(_, path, _)| path == &format!("/{phase}-form"))
            .unwrap_or_else(|| panic!("missing {phase} form request: {requests:?}"));
        assert_eq!(request.0, method);
        if phase.starts_with("pre-") {
            assert_eq!(request.2, None);
        } else {
            assert!(
                request
                    .2
                    .as_deref()
                    .is_some_and(|cookie| cookie.contains("session=pre")),
                "granted {phase} form must include the Document cookie",
            );
        }
    }
    let cookies = runtime
        .host_state
        .borrow()
        .cookie_store
        .lock()
        .unwrap()
        .document_cookie(
            &"https://third.test/".parse().unwrap(),
            &"https://third.test/".parse().unwrap(),
        );
    assert!(!cookies.contains("pre_get_form"));
    assert!(!cookies.contains("pre_post_form"));
    assert!(cookies.contains("post_get_form=1"));
    assert!(cookies.contains("post_post_form=1"));
}

#[test]
fn iframe_form_history_replay_reevaluates_the_original_document_access() {
    let mut runtime = runtime_at("https://top.test/");
    let transport = Arc::new(ResourceCookieTransport::default());
    runtime.set_http_resource_transport(Some(transport.clone()));
    runtime
        .host_state
        .borrow()
        .cookie_store
        .lock()
        .unwrap()
        .add_from_header_for_url(
            "session=pre; Path=/; Secure; SameSite=None",
            &"https://third.test/".parse().unwrap(),
        );
    runtime
        .eval(
            "const source = document.createElement('iframe'); source.id = 'source'; \
             source.src = 'https://third.test/frame'; document.body.appendChild(source); \
             const sink = document.createElement('iframe'); sink.id = 'sink'; \
             sink.name = 'sink'; document.body.appendChild(sink)",
        )
        .unwrap();
    runtime.tick(0).unwrap();
    let source = iframe_document_by_id(&runtime, "source");
    activate_storage_access(&mut runtime, &source);
    runtime
        .eval_in_document_realm(
            source.identity(),
            "const form = document.createElement('form'); form.method = 'post'; \
             form.target = 'sink'; form.action = 'https://third.test/post-replay-form'; \
             document.body.appendChild(form); form.submit()",
        )
        .unwrap();
    runtime.tick(0).unwrap();
    let first_sink = iframe_document_by_id(&runtime, "sink");
    runtime
        .eval_in_document_realm(first_sink.identity(), "location.reload()")
        .unwrap();
    runtime.run_until_idle().unwrap();

    runtime
        .eval("document.getElementById('source').remove()")
        .unwrap();
    runtime.tick(0).unwrap();
    let second_sink = iframe_document_by_id(&runtime, "sink");
    runtime
        .eval_in_document_realm(second_sink.identity(), "location.reload()")
        .unwrap();
    runtime.run_until_idle().unwrap();

    let requests = transport.requests.lock().unwrap();
    let replayed: Vec<_> = requests
        .iter()
        .filter(|(method, path, _)| *method == Method::Post && path == "/post-replay-form")
        .map(|(_, _, cookie)| cookie.as_deref())
        .collect();
    assert_eq!(
        replayed.len(),
        3,
        "initial, active-source replay and retired-source replay"
    );
    assert!(replayed[0].is_some_and(|cookie| cookie.contains("session=pre")));
    assert!(replayed[1].is_some_and(|cookie| cookie.contains("session=pre")));
    assert_eq!(
        replayed[2], None,
        "retired source Document cannot retain access"
    );
}

#[test]
fn iframe_form_history_replay_keeps_first_party_cookies_after_source_removal() {
    let mut runtime = runtime_at("https://top.test/");
    let transport = Arc::new(ResourceCookieTransport::default());
    runtime.set_http_resource_transport(Some(transport.clone()));
    runtime
        .host_state
        .borrow()
        .cookie_store
        .lock()
        .unwrap()
        .add_from_header_for_url(
            "session=pre; Path=/; Secure; SameSite=Strict",
            &"https://top.test/".parse().unwrap(),
        );
    runtime
        .eval(
            "const source = document.createElement('iframe'); source.id = 'source'; \
             source.src = 'https://top.test/frame'; document.body.appendChild(source); \
             const sink = document.createElement('iframe'); sink.id = 'sink'; \
             sink.name = 'sink'; document.body.appendChild(sink)",
        )
        .unwrap();
    runtime.tick(0).unwrap();
    let source = iframe_document_by_id(&runtime, "source");
    runtime
        .eval_in_document_realm(
            source.identity(),
            "const form = document.createElement('form'); form.method = 'post'; \
             form.target = 'sink'; form.action = 'https://top.test/post-first-party-form'; \
             document.body.appendChild(form); form.submit()",
        )
        .unwrap();
    runtime.tick(0).unwrap();
    runtime
        .eval("document.getElementById('source').remove()")
        .unwrap();
    runtime.tick(0).unwrap();
    let sink = iframe_document_by_id(&runtime, "sink");
    runtime
        .eval_in_document_realm(sink.identity(), "location.reload()")
        .unwrap();
    runtime.run_until_idle().unwrap();

    let requests = transport.requests.lock().unwrap();
    let replayed: Vec<_> = requests
        .iter()
        .filter(|(method, path, _)| *method == Method::Post && path == "/post-first-party-form")
        .map(|(_, _, cookie)| cookie.as_deref())
        .collect();
    assert_eq!(replayed.len(), 2, "initial and replayed POST");
    assert!(
        replayed
            .iter()
            .all(|cookie| cookie.is_some_and(|value| value.contains("session=pre")))
    );
}

#[test]
fn popup_navigation_keeps_top_level_cookie_rules() {
    let mut runtime = runtime_at("https://top.test/");
    let transport = Arc::new(ResourceCookieTransport::default());
    runtime.set_http_resource_transport(Some(transport.clone()));
    for cookie in [
        "strict=1; Secure; SameSite=Strict",
        "lax=1; Secure; SameSite=Lax",
        "none=1; Secure; SameSite=None",
    ] {
        runtime
            .host_state
            .borrow()
            .cookie_store
            .lock()
            .unwrap()
            .add_from_header_for_url(cookie, &"https://third.test/".parse().unwrap());
    }
    runtime
        .eval("window.open('https://third.test/popup', 'cookie-probe')")
        .unwrap();
    runtime.tick(0).unwrap();
    let requests = transport.requests.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .find(|(_, path, _)| path == "/popup")
            .map(|(_, _, cookie)| cookie.as_deref()),
        Some(Some("lax=1; none=1")),
    );
    assert_eq!(
        runtime
            .host_state
            .borrow()
            .cookie_store
            .lock()
            .unwrap()
            .len(),
        4,
        "a top-level popup response should store Set-Cookie",
    );
}

#[test]
fn sandbox_token_controls_storage_access_request() {
    for (tokens, expected) in [
        ("allow-scripts allow-same-origin", "NotAllowedError"),
        (
            "allow-scripts allow-same-origin allow-storage-access-by-user-activation",
            "granted",
        ),
    ] {
        let mut runtime = runtime_at("https://top.test/");
        runtime.set_http_resource_transport(Some(Arc::new(StorageAccessTransport)));
        runtime
            .eval(&format!(
                "const f = document.createElement('iframe'); \
                 f.setAttribute('sandbox', {tokens:?}); \
                 f.src = 'https://third.test/frame'; document.body.appendChild(f)"
            ))
            .unwrap();
        runtime.tick(0).unwrap();
        let child = child_document(&runtime);
        runtime
            .with_trusted_storage_access_input(|runtime| {
                runtime.eval_in_document_realm(
                    child.identity(),
                    "document.addEventListener('click', () => { \
                        document.requestStorageAccess().then( \
                          () => document.documentElement.setAttribute('data-result', 'granted'), \
                          e => document.documentElement.setAttribute('data-result', e.name)); \
                      }, {once: true}); \
                      __omoikane_dispatch_mouse_input(document.body.__id, 'click', {button: 0}, false)",
                )
            })
            .unwrap();
        assert_eq!(child_result(&child).as_deref(), Some(expected));
    }
}
