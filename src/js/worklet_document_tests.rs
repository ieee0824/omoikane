//! Document ownership regressions for Worklet globals and module fetching.
use super::*;
use crate::http::{HttpParseError, HttpResponse, HttpTransport};
use crate::js::storage_access::StorageAccessPermission;
use std::time::Duration;

#[derive(Default)]
struct DocumentWorkletTransport {
    requests: Mutex<Vec<(String, Option<String>)>>,
}

impl HttpTransport for DocumentWorkletTransport {
    fn send(
        &self,
        request: &HttpRequest,
        _: Option<Duration>,
    ) -> Result<HttpResponse, HttpParseError> {
        let path = request.url().path();
        self.requests.lock().unwrap().push((
            request.url().to_string(),
            request.header("Cookie").map(str::to_owned),
        ));
        let (mime, source) = if path.ends_with(".js") {
            let stem = path.trim_start_matches('/').trim_end_matches(".js");
            let source = if stem.ends_with("-root") {
                format!("import './{stem}-dep.js'; registerPaint('{stem}',class {{}});")
            } else {
                format!(
                    "globalThis.runs=(globalThis.runs||0)+1; registerPaint('{stem}-'+runs,class {{}}); setTimeout(()=>registerPaint('{stem}-timer',class {{}}),10);"
                )
            };
            ("text/javascript", source)
        } else {
            ("text/html", "<!doctype html><body>child</body>".into())
        };
        let mut headers = vec![("Content-Type".into(), mime.into())];
        if let Some(origin) = request.header("Origin") {
            headers.push(("Access-Control-Allow-Origin".into(), origin.into()));
            headers.push(("Access-Control-Allow-Credentials".into(), "true".into()));
        }
        if path.starts_with("/credentials-") && path.ends_with("-root.js") {
            headers.push((
                "Set-Cookie".into(),
                "rootstored=1; Secure; SameSite=None; Path=/".into(),
            ));
        }
        Ok(HttpResponse::new(200, "OK", headers, source.into_bytes()))
    }
}

fn fixture() -> (JsRuntime, Arc<DocumentWorkletTransport>) {
    let mut runtime =
        JsRuntime::with_document_and_url(blank_html_document(), "https://top.test/").unwrap();
    let transport = Arc::new(DocumentWorkletTransport::default());
    runtime.set_http_resource_transport(Some(transport.clone()));
    (runtime, transport)
}

fn add_frame(runtime: &mut JsRuntime, name: &str, url: &str) -> NodeHandle {
    runtime.eval(&format!("{{ const f=document.createElement('iframe'); f.id={name:?}; f.src={url:?}; document.body.appendChild(f); }}")).unwrap();
    runtime.tick(0).unwrap();
    let state = runtime.host_state.borrow();
    state
        .iframe_documents
        .iter()
        .find_map(|(id, entry)| {
            (state.get_node(*id)?.get_attribute("id").as_deref() == Some(name))
                .then(|| entry.document.clone())
        })
        .unwrap()
}

fn load(runtime: &mut JsRuntime, document: &NodeHandle, url: &str) {
    runtime.eval_worklet_test_in_document(document.identity(),&format!("globalThis.result='pending'; CSS.paintWorklet.addModule({url:?}).then(()=>globalThis.result='ok',error=>globalThis.result=error.name)")).unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval_worklet_test_in_document(document.identity(), "result")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "ok"
    );
}

fn names(runtime: &mut JsRuntime, document: &NodeHandle) -> String {
    runtime
        .eval_worklet_test_in_document(
            document.identity(),
            "CSS.paintWorklet.registeredNames.join(',')",
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped()
}

#[test]
fn worklet_same_origin_documents_keep_module_maps_and_globals_separate() {
    let (mut runtime, transport) = fixture();
    let top = runtime.document();
    let child = add_frame(&mut runtime, "child", "https://top.test/frame");
    load(&mut runtime, &top, "https://top.test/shared.js");
    load(&mut runtime, &child, "https://top.test/shared.js");
    assert_eq!(names(&mut runtime, &top), "shared-1");
    assert_eq!(names(&mut runtime, &child), "shared-1");
    assert_eq!(
        transport
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(url, _)| url.ends_with("/shared.js"))
            .count(),
        2
    );
    load(&mut runtime, &child, "https://top.test/child.js");
    assert_eq!(names(&mut runtime, &child), "child-2,shared-1");
    assert_eq!(names(&mut runtime, &top), "shared-1");
}

#[test]
fn worklet_document_teardown_keeps_other_documents_and_clocks_alive() {
    let (mut runtime, _) = fixture();
    let top = runtime.document();
    let child = add_frame(&mut runtime, "child", "https://third.test/frame");
    load(&mut runtime, &top, "https://top.test/top.js");
    load(&mut runtime, &child, "https://third.test/child.js");
    let old_child = runtime
        .host_state
        .borrow()
        .worklet
        .runtimes
        .get(&child.identity())
        .cloned()
        .unwrap();
    runtime
        .eval("document.getElementById('child').remove()")
        .unwrap();
    runtime.tick(10).unwrap();
    assert_eq!(names(&mut runtime, &top), "top-1,top-timer");
    assert!(old_child.borrow().host_state.borrow().worklet.terminated);
    assert!(
        old_child
            .borrow()
            .host_state
            .borrow()
            .worklet
            .owner
            .is_none()
    );
    assert!(
        !runtime
            .host_state
            .borrow()
            .worklet
            .runtimes
            .contains_key(&child.identity())
    );
    let replacement = add_frame(&mut runtime, "child", "https://third.test/frame");
    load(&mut runtime, &replacement, "https://third.test/child.js");
    assert_eq!(names(&mut runtime, &replacement), "child-1");
}

#[test]
fn worklet_module_graph_uses_its_own_documents_live_storage_access() {
    let (mut runtime, transport) = fixture();
    let first = add_frame(&mut runtime, "first", "https://third.test/frame");
    let second = add_frame(&mut runtime, "second", "https://third.test/frame");
    runtime
        .host_state
        .borrow()
        .cookie_store
        .lock()
        .unwrap()
        .add_from_header_for_url(
            "session=allowed; Secure; SameSite=None; Path=/",
            &"https://third.test/".parse().unwrap(),
        );
    load(&mut runtime, &first, "https://third.test/before-root.js");
    runtime
        .host_state
        .borrow_mut()
        .set_storage_access_permission(first.identity(), StorageAccessPermission::Granted)
        .unwrap();
    runtime
        .eval_worklet_test_in_document(first.identity(), "document.requestStorageAccess()")
        .unwrap();
    load(&mut runtime, &first, "https://third.test/granted-root.js");
    load(&mut runtime, &second, "https://third.test/granted-root.js");
    runtime
        .host_state
        .borrow_mut()
        .set_storage_access_permission(first.identity(), StorageAccessPermission::Denied)
        .unwrap();
    load(&mut runtime, &first, "https://third.test/revoked-root.js");
    let requests = transport.requests.lock().unwrap();
    for suffix in [
        "before-root.js",
        "before-root-dep.js",
        "revoked-root.js",
        "revoked-root-dep.js",
    ] {
        assert_eq!(
            requests
                .iter()
                .filter(|(url, _)| url.ends_with(suffix))
                .map(|(_, cookie)| cookie.as_deref())
                .collect::<Vec<_>>(),
            vec![None],
            "{suffix}"
        );
    }
    for suffix in ["granted-root.js", "granted-root-dep.js"] {
        assert_eq!(
            requests
                .iter()
                .filter(|(url, _)| url.ends_with(suffix))
                .map(|(_, cookie)| cookie.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("session=allowed"), None],
            "{suffix}"
        );
    }
}

#[test]
fn worklet_explicit_teardown_only_ends_the_calling_documents_global() {
    let (mut runtime, _) = fixture();
    let top = runtime.document();
    let child = add_frame(&mut runtime, "child", "https://top.test/frame");
    load(&mut runtime, &top, "https://top.test/top.js");
    load(&mut runtime, &child, "https://top.test/child.js");
    runtime
        .eval_worklet_test_in_document(child.identity(), "CSS.paintWorklet.teardown()")
        .unwrap();
    runtime.tick(10).unwrap();
    assert_eq!(names(&mut runtime, &top), "top-1,top-timer");
    assert_eq!(names(&mut runtime, &child), "");
    runtime
        .eval_worklet_test_in_document(
            child.identity(),
            "CSS.paintWorklet.addModule('/again.js').catch(error=>globalThis.ended=error.name)",
        )
        .unwrap();
    assert_eq!(
        runtime
            .eval_worklet_test_in_document(child.identity(), "ended")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "InvalidStateError"
    );
    load(&mut runtime, &top, "https://top.test/after.js");
    assert!(names(&mut runtime, &top).contains("after-2"));
}

fn credentials_case(mode: &str, cross_origin: bool, third_party: bool, expected_cookie: bool) {
    let (mut runtime, transport) = fixture();
    let document = if third_party {
        let child = add_frame(&mut runtime, "granted", "https://third.test/frame");
        runtime
            .host_state
            .borrow_mut()
            .set_storage_access_permission(child.identity(), StorageAccessPermission::Granted)
            .unwrap();
        runtime
            .eval_worklet_test_in_document(child.identity(), "document.requestStorageAccess()")
            .unwrap();
        child
    } else {
        runtime.document()
    };
    let origin = if third_party {
        "https://third.test"
    } else if cross_origin {
        "https://sub.top.test"
    } else {
        "https://top.test"
    };
    runtime
        .host_state
        .borrow()
        .cookie_store
        .lock()
        .unwrap()
        .add_from_header_for_url(
            "session=allowed; Secure; SameSite=None; Path=/",
            &format!("{origin}/").parse().unwrap(),
        );
    let url = format!("{origin}/credentials-{mode}-root.js");
    runtime.eval_worklet_test_in_document(document.identity(), &format!("globalThis.result='pending'; CSS.paintWorklet.addModule({url:?},{{credentials:{mode:?}}}).then(()=>globalThis.result='ok',e=>globalThis.result=e.name)")).unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval_worklet_test_in_document(document.identity(), "result")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "ok"
    );
    let requests = transport.requests.lock().unwrap();
    let scripts: Vec<_> = requests
        .iter()
        .filter(|(url, _)| url.contains("/credentials-"))
        .collect();
    assert_eq!(scripts.len(), 2, "root and static dependency both fetch");
    for (url, cookie) in scripts {
        assert_eq!(
            cookie
                .as_deref()
                .is_some_and(|value| value.contains("session=allowed")),
            expected_cookie,
            "{mode} {url}"
        );
    }
    let store = runtime.host_state.borrow().cookie_store.clone();
    assert_eq!(
        store.lock().unwrap().len(),
        if expected_cookie { 2 } else { 1 },
        "Set-Cookie follows the same credentials mode"
    );
}

#[test]
fn worklet_credentials_omit_suppresses_root_and_import_cookies_after_storage_access_grant() {
    credentials_case("omit", false, true, false);
    credentials_case("same-origin", false, true, true);
    credentials_case("include", false, true, true);
}

#[test]
fn worklet_credentials_distinguish_include_from_same_origin_for_cors_modules() {
    credentials_case("omit", true, false, false);
    credentials_case("same-origin", true, false, false);
    credentials_case("include", true, false, true);
}

impl JsRuntime {
    fn eval_worklet_test_in_document(
        &mut self,
        document: usize,
        source: &str,
    ) -> JsResult<JsValue> {
        let realm = self.realm_for_document(document)?;
        let old_realm = realm.map(|realm| self.context.enter_realm(realm));
        let result = (|| {
            let value = boa_gc::Rooted::new(self.eval(source)?);
            self.run_jobs()?;
            Ok((*value).clone())
        })();
        if let Some(old_realm) = old_realm {
            self.context.enter_realm(old_realm);
        }
        result
    }
}
