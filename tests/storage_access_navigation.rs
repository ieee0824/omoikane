//! Storage Access activation and navigation cookies must describe the same grant.
use omoikane::html::TreeBuilder;
use omoikane::http::{HttpParseError, HttpRequest, HttpResponse, HttpTransport};
use omoikane::js::JsRuntime;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const NAVIGATION_DOCUMENT: &str = r#"<!doctype html><body><script>
          addEventListener('message', async event => {
            if (typeof event.data !== 'string') {
              if (event.data && event.data.kind === 'state') globalThis.nestedState=event.data.value;
              return;
            }
            if (event.data === 'grant') {
              __omoikane_automation_set_permission('storage-access', 'granted');
              await document.requestStorageAccess();
              document.cookie = 'allowed=1; Secure; SameSite=None; Path=/';
              event.source.postMessage('granted', '*');
            } else if (event.data === 'reload') {
              location.reload();
            } else if (event.data === 'history-reload') {
              history.go(0);
            } else if (event.data === 'history-back') {
              location.href='/frame?history=1';
            } else if (event.data === 'back') {
              history.back();
            } else if (event.data === 'assign') {
              location.href = '/frame?self=1';
            } else if (event.data === 'assign-same') {
              location.href = location.href;
            } else if (event.data === 'cross-origin') {
              location.href = 'https://other.test/frame';
            } else if (event.data === 'top-origin') {
              location.href = 'https://top.test/frame';
            } else if (event.data === 'same-site-origin') {
              location.href = 'https://sub.third.test/frame';
            } else if (event.data === 'redirect' || event.data === 'bounce') {
              location.href = '/' + event.data;
            } else if (event.data === 'revoke') {
              __omoikane_automation_set_permission('storage-access', 'denied');
              location.reload();
            } else if (event.data === 'policy') {
              location.href = '/frame?policy=deny';
            } else if (event.data === 'form' || event.data === 'form-post') {
              const form = document.createElement('form');
              form.action = '/frame?form=1';
              if (event.data === 'form-post') form.method='POST';
              document.body.appendChild(form);
              form.submit();
            } else if (event.data.startsWith('nested')) {
              const frame = document.createElement('iframe');
              frame.name='nested';
              frame.src='/frame';
              frame.onload=()=>frame.contentWindow.postMessage('state','*');
              if (event.data !== 'nested') frame.onload=() => {
                frame.onload=()=>frame.contentWindow.postMessage('state','*');
                const form=document.createElement('form');
                form.action='/frame';
                form.target='nested';
                form.method=event.data === 'nested-post' ? 'POST' : 'GET';
                document.body.appendChild(form);
                form.submit();
              };
              document.body.appendChild(frame);
            } else if (event.data === 'state') {
              let cookies;
              try { cookies=document.cookie; } catch (error) { cookies=error.name; }
              if (cookies === 'SecurityError') {
                try { document.cookie='unexpected=1'; cookies='write-accepted'; }
                catch (error) { cookies+='|'+error.name; }
              }
              event.source.postMessage({kind:'state', value:globalThis.nestedState || [await document.hasStorageAccess(), cookies]}, '*');
            }
          });
        </script></body>"#;

#[derive(Default)]
struct NavigationTransport {
    cookies: Mutex<Vec<Option<String>>>,
}

impl HttpTransport for NavigationTransport {
    fn send(
        &self,
        request: &HttpRequest,
        _: Option<Duration>,
    ) -> Result<HttpResponse, HttpParseError> {
        self.cookies
            .lock()
            .unwrap()
            .push(request.header("Cookie").map(str::to_owned));
        let redirect = match request.url().path() {
            "/redirect" => Some("https://third.test/frame?redirected=1"),
            "/bounce" => Some("https://other.test/return"),
            "/return" => Some("https://third.test/frame?returned=1"),
            "/frame" => None,
            path => panic!("unexpected navigation path: {path}"),
        };
        if let Some(location) = redirect {
            let response =
                format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\n\r\n");
            return HttpResponse::parse(&mut response.as_bytes());
        }
        let body = NAVIGATION_DOCUMENT;
        let policy = if request.url().query() == Some("policy=deny") {
            "Permissions-Policy: storage-access=()\r\n"
        } else {
            ""
        };
        let response = format!(
            "HTTP/1.1 200 OK\r\n{policy}Content-Type: text/html\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        HttpResponse::parse(&mut response.as_bytes())
    }
}

fn pump(runtime: &mut JsRuntime) {
    for _ in 0..8 {
        runtime
            .tick(16)
            .unwrap_or_else(|error| panic!("navigation task failed: {error}"));
    }
    assert!(runtime.take_task_errors().is_empty());
}

fn navigation_result(command: &str) -> (String, Vec<Option<String>>) {
    let document = TreeBuilder::parse("<!doctype html><body></body>").document();
    let mut runtime = JsRuntime::with_document_and_url(document, "https://top.test/page").unwrap();
    runtime.enable_test_automation().unwrap();
    if command == "top-origin" {
        runtime
            .eval("document.cookie='top=1; Secure; SameSite=None; Path=/'")
            .unwrap();
    }
    let transport = Arc::new(NavigationTransport::default());
    runtime.set_http_resource_transport(Some(transport.clone()));
    runtime
        .eval(
            r#"
        globalThis.messages=[];
        globalThis.lastState=null;
        addEventListener('message', event => {
          if (event.data && event.data.kind === 'state') lastState=event.data.value;
          else messages.push(event.data);
        });
        globalThis.targetFrame=document.createElement('iframe');
        targetFrame.src='https://third.test/frame';
        document.body.appendChild(targetFrame);
    "#,
        )
        .unwrap();
    pump(&mut runtime);
    runtime
        .eval("targetFrame.contentWindow.postMessage('grant','*')")
        .unwrap();
    pump(&mut runtime);
    assert_eq!(
        runtime
            .eval("messages.includes('granted')")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    if command == "sandbox" || command == "opaque" {
        let sandbox = if command == "sandbox" {
            "allow-scripts allow-same-origin"
        } else {
            "allow-scripts"
        };
        runtime.eval(&format!(
            "targetFrame.setAttribute('sandbox','{sandbox}'); targetFrame.contentWindow.postMessage('reload','*')"
        )).unwrap();
    } else if command == "parent-same" {
        runtime.eval("targetFrame.src=targetFrame.src").unwrap();
    } else if command == "parent" {
        runtime
            .eval("targetFrame.src='https://third.test/frame?parent=1'")
            .unwrap();
    } else {
        runtime
            .eval(&format!(
                "targetFrame.contentWindow.postMessage('{command}','*')"
            ))
            .unwrap();
    }
    pump(&mut runtime);
    if command == "history-back" {
        runtime
            .eval("targetFrame.contentWindow.postMessage('back','*')")
            .unwrap();
        pump(&mut runtime);
    }
    runtime
        .eval("targetFrame.contentWindow.postMessage('state','*')")
        .unwrap();
    pump(&mut runtime);
    let state = runtime
        .eval("JSON.stringify(lastState)")
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    let cookies = transport.cookies.lock().unwrap().clone();
    (state, cookies)
}

#[test]
fn self_reload_keeps_active_storage_access_and_initial_request_cookies() {
    assert_eq!(
        navigation_result("reload"),
        (
            r#"[true,"allowed=1"]"#.into(),
            vec![None, Some("allowed=1".into())]
        )
    );
}

#[test]
fn self_same_origin_navigation_keeps_active_storage_access_and_cookies() {
    assert_eq!(
        navigation_result("assign"),
        (
            r#"[true,"allowed=1"]"#.into(),
            vec![None, Some("allowed=1".into())]
        )
    );
}

#[test]
fn parent_same_origin_navigation_requires_fresh_document_activation() {
    assert_eq!(
        navigation_result("parent"),
        (r#"[false,""]"#.into(), vec![None, None])
    );
}

#[test]
fn self_cross_site_navigation_does_not_inherit_another_sites_access() {
    assert_eq!(
        navigation_result("cross-origin"),
        (r#"[false,""]"#.into(), vec![None, None])
    );
}

#[test]
fn parent_setting_the_same_src_starts_a_new_document_without_activation() {
    assert_eq!(
        navigation_result("parent-same"),
        (r#"[false,""]"#.into(), vec![None, None])
    );
}

#[test]
fn same_origin_redirect_preserves_activation_and_request_cookies() {
    assert_eq!(
        navigation_result("redirect"),
        (
            r#"[true,"allowed=1"]"#.into(),
            vec![None, Some("allowed=1".into()), Some("allowed=1".into())]
        )
    );
}

#[test]
fn cross_origin_redirect_cannot_restore_access_when_returning() {
    assert_eq!(
        navigation_result("bounce"),
        (
            r#"[false,""]"#.into(),
            vec![None, Some("allowed=1".into()), None, None]
        )
    );
}

#[test]
fn same_site_cross_origin_navigation_does_not_inherit_activation() {
    assert_eq!(
        navigation_result("same-site-origin"),
        (r#"[false,""]"#.into(), vec![None, None])
    );
}

#[test]
fn revoked_permission_prevents_activation_and_navigation_cookies() {
    assert_eq!(
        navigation_result("revoke"),
        (r#"[false,""]"#.into(), vec![None, None])
    );
}

#[test]
fn replacement_response_policy_prevents_activation() {
    assert_eq!(
        navigation_result("policy"),
        (r#"[false,""]"#.into(), vec![None, Some("allowed=1".into())])
    );
}

#[test]
fn self_form_navigation_preserves_activation_and_request_cookies() {
    assert_eq!(
        navigation_result("form"),
        (
            r#"[true,"allowed=1"]"#.into(),
            vec![None, Some("allowed=1".into())]
        )
    );
}

#[test]
fn assigning_the_current_location_loads_a_new_document_with_access() {
    assert_eq!(
        navigation_result("assign-same"),
        (
            r#"[true,"allowed=1"]"#.into(),
            vec![None, Some("allowed=1".into())]
        )
    );
}

#[test]
fn replacement_sandbox_without_storage_access_does_not_inherit_activation() {
    assert_eq!(
        navigation_result("sandbox"),
        (r#"[false,""]"#.into(), vec![None, Some("allowed=1".into())])
    );
}

#[test]
fn replacement_opaque_document_does_not_inherit_activation() {
    assert_eq!(
        navigation_result("opaque"),
        (
            r#"[false,"SecurityError|SecurityError"]"#.into(),
            vec![None, Some("allowed=1".into())]
        )
    );
}

#[test]
fn activated_parent_can_fetch_a_child_without_activating_its_document() {
    assert_eq!(
        navigation_result("nested"),
        (r#"[false,""]"#.into(), vec![None, Some("allowed=1".into())])
    );
}

#[test]
fn self_post_form_navigation_preserves_activation_and_request_cookies() {
    assert_eq!(
        navigation_result("form-post"),
        (
            r#"[true,"allowed=1"]"#.into(),
            vec![None, Some("allowed=1".into())]
        )
    );
}

#[test]
fn activated_parent_get_form_uses_source_access_without_activating_a_child() {
    assert_eq!(
        navigation_result("nested-get"),
        (
            r#"[false,""]"#.into(),
            vec![None, Some("allowed=1".into()), Some("allowed=1".into())]
        )
    );
}

#[test]
fn activated_parent_post_form_uses_source_access_without_activating_a_child() {
    assert_eq!(
        navigation_result("nested-post"),
        (
            r#"[false,""]"#.into(),
            vec![None, Some("allowed=1".into()), Some("allowed=1".into())]
        )
    );
}

#[test]
fn navigation_back_to_the_top_level_origin_keeps_ordinary_first_party_cookies() {
    assert_eq!(
        navigation_result("top-origin"),
        (r#"[true,"top=1"]"#.into(), vec![None, Some("top=1".into())])
    );
}

#[test]
fn self_history_zero_reload_preserves_activation_and_request_cookies() {
    assert_eq!(
        navigation_result("history-reload"),
        (
            r#"[true,"allowed=1"]"#.into(),
            vec![None, Some("allowed=1".into())]
        )
    );
}

#[test]
fn self_history_traversal_preserves_activation_and_request_cookies() {
    assert_eq!(
        navigation_result("history-back"),
        (
            r#"[true,"allowed=1"]"#.into(),
            vec![None, Some("allowed=1".into()), Some("allowed=1".into())]
        )
    );
}

#[test]
fn srcdoc_reassignment_navigates_but_shadowed_src_and_absent_removal_do_not() {
    let document = TreeBuilder::parse("<!doctype html><body></body>").document();
    let mut runtime = JsRuntime::with_document_and_url(document, "https://top.test/page").unwrap();
    runtime
        .eval(
            r#"
      globalThis.frame = document.createElement('iframe');
      frame.srcdoc = '<body>child</body>';
      document.body.appendChild(frame);
    "#,
        )
        .unwrap();
    pump(&mut runtime);
    runtime
        .eval("globalThis.oldDocument=frame.contentDocument; frame.srcdoc=frame.srcdoc")
        .unwrap();
    pump(&mut runtime);
    assert_eq!(
        runtime
            .eval("frame.contentDocument !== oldDocument")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    runtime
        .eval(
            r#"
      oldDocument=frame.contentDocument;
      frame.src='https://third.test/frame';
      frame.removeAttribute('src');
      frame.removeAttributeNS(null, 'src');
    "#,
        )
        .unwrap();
    pump(&mut runtime);
    assert_eq!(
        runtime
            .eval("frame.contentDocument === oldDocument")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}
