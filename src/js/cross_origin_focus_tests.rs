use super::*;

struct FocusPages(HashMap<String, String>);

impl crate::http::HttpTransport for FocusPages {
    fn send(
        &self,
        request: &HttpRequest,
        _: Option<Duration>,
    ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
        let body = self.0.get(&request.url().to_string());
        Ok(crate::http::HttpResponse::new(
            if body.is_some() { 200 } else { 404 },
            "fixture",
            vec![("Content-Type".into(), "text/html".into())],
            body.cloned().unwrap_or_default().into_bytes(),
        ))
    }
}

fn runtime(pages: &[(&str, &str)]) -> JsRuntime {
    let document = crate::html::TreeBuilder::parse(
        "<html><body><button id=old>old</button><button id=redirect>redirect</button>\
         <iframe id=child allow='focus-without-user-activation' src='https://child.test/child'></iframe></body></html>",
    ).document();
    let mut runtime = JsRuntime::with_document_and_url(document, "https://top.test/page").unwrap();
    runtime.set_http_resource_transport(Some(Arc::new(FocusPages(
        pages
            .iter()
            .map(|(url, body)| (url.to_string(), body.to_string()))
            .collect(),
    ))));
    for _ in 0..8 {
        runtime.tick(0).unwrap();
    }
    assert_eq!(runtime.take_task_errors(), Vec::<String>::new());
    runtime
}

fn frame_document(runtime: &JsRuntime, parent: &NodeHandle, selector: &str) -> NodeHandle {
    let frame = parent.query_selector(selector).expect("fixture iframe");
    runtime.host_state.borrow().iframe_documents[&frame.identity()]
        .document
        .clone()
}

fn eval_document(runtime: &mut JsRuntime, document: &NodeHandle, source: &str) {
    runtime
        .eval_in_document_realm(document.identity(), source)
        .unwrap();
    assert_eq!(runtime.take_task_errors(), Vec::<String>::new());
}

fn attribute(document: &NodeHandle, name: &str) -> Option<String> {
    document.query_selector("html").unwrap().get_attribute(name)
}

const INPUT_PAGE: &str =
    "<html><body><script>globalThis.frameReady=true</script><input id=target></body></html>";

#[test]
fn cross_origin_focus_requires_delegation_or_user_activation() {
    let document = crate::html::TreeBuilder::parse(
        "<html><body><button id=old>old</button><iframe id=child src='https://child.test/child'></iframe></body></html>",
    ).document();
    let mut runtime = JsRuntime::with_document_and_url(document, "https://top.test/page").unwrap();
    runtime.set_http_resource_transport(Some(Arc::new(FocusPages(HashMap::from([(
        "https://child.test/child".to_owned(),
        INPUT_PAGE.to_owned(),
    )])))));
    for _ in 0..8 {
        runtime.tick(0).unwrap();
    }
    let child = frame_document(&runtime, &runtime.document(), "#child");
    runtime
        .eval("document.getElementById('old').focus()")
        .unwrap();
    eval_document(
        &mut runtime,
        &child,
        "const target=document.getElementById('target'); \
         document.documentElement.setAttribute('data-policy', \
         String(document.permissionsPolicy.allowsFeature('focus-without-user-activation'))); \
         target.focus(); \
         document.documentElement.setAttribute('data-denied', String(document.activeElement===target)); \
         __omoikane_dispatch_mouse_input(target.__id,'mousedown',{},true); \
         document.documentElement.setAttribute('data-activated', String(document.activeElement===target))",
    );
    assert_eq!(attribute(&child, "data-policy").as_deref(), Some("false"));
    assert_eq!(attribute(&child, "data-denied").as_deref(), Some("false"));
    assert_eq!(attribute(&child, "data-activated").as_deref(), Some("true"));
}

#[test]
fn response_policy_blocks_script_focus_but_allows_user_input() {
    let mut runtime = runtime(&[]);
    runtime.install_response_security_headers(&[(
        "Permissions-Policy".into(),
        "focus-without-user-activation=()".into(),
    )]);
    assert_eq!(
        runtime.eval("!document.permissionsPolicy.allowsFeature('focus-without-user-activation') && \
            document.permissionsPolicy.getAllowlistForFeature('focus-without-user-activation').length === 0")
            .unwrap().as_boolean(),
        Some(true)
    );
    assert_eq!(
        runtime
            .eval(
                "const old=document.getElementById('old'); old.focus(); \
            document.activeElement===old"
            )
            .unwrap()
            .as_boolean(),
        Some(false)
    );
    assert_eq!(
        runtime
            .eval(
                "__omoikane_dispatch_mouse_input(old.__id,'mousedown',{},true); \
            document.activeElement===old"
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn cross_origin_child_focus_updates_its_own_and_parent_active_elements() {
    let mut runtime = runtime(&[("https://child.test/child", INPUT_PAGE)]);
    let child = frame_document(&runtime, &runtime.document(), "#child");
    eval_document(
        &mut runtime,
        &child,
        r#"
        const target = document.getElementById('target');
        let denied = false;
        try { void parent.document; } catch (error) { denied = error.name === 'SecurityError'; }
        let branded = false;
        try { Document.prototype.hasFocus.call({__id:document.__id}); }
        catch (error) { branded = error.name === 'TypeError'; }
        target.focus();
        document.documentElement.setAttribute('data-focus', [
          document.activeElement === target, document.hasFocus(), denied, branded,
          typeof globalThis.__omoikane_focus_chain === 'undefined',
        ].join('|'));
    "#,
    );
    assert_eq!(
        attribute(&child, "data-focus").as_deref(),
        Some("true|true|true|true|true")
    );
    assert_eq!(
        runtime
            .eval(
                "document.activeElement === document.getElementById('child') && document.hasFocus()"
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn cross_origin_mouse_click_and_parent_keyboard_route_to_the_focused_child() {
    let mut runtime = runtime(&[("https://child.test/child", INPUT_PAGE)]);
    let child = frame_document(&runtime, &runtime.document(), "#child");
    eval_document(
        &mut runtime,
        &child,
        r#"
        const target = document.getElementById('target');
        const types = [];
        for (const type of ['mousedown','mouseup','click']) target.addEventListener(type, () => types.push(type));
        target.addEventListener('keydown', event => document.documentElement.setAttribute('data-key', event.key));
        const rect = target.getBoundingClientRect();
        const init = {clientX:rect.left+rect.width/2, clientY:rect.top+rect.height/2, button:0};
        for (const type of ['mousedown','mouseup','click']) {
          __omoikane_dispatch_mouse_input(target.__id, type, init, type === 'mousedown');
        }
        document.documentElement.setAttribute('data-input', types.join('|'));
    "#,
    );
    assert_eq!(
        attribute(&child, "data-input").as_deref(),
        Some("mousedown|mouseup|click")
    );
    runtime.eval("__omoikane_dispatch_composition_input('commit','abc'); __omoikane_dispatch_keyboard_input('keydown',{key:'ArrowRight',code:'ArrowRight'})").unwrap();
    eval_document(
        &mut runtime,
        &child,
        "document.documentElement.setAttribute('data-value', document.getElementById('target').value)",
    );
    assert_eq!(attribute(&child, "data-value").as_deref(), Some("abc"));
    assert_eq!(attribute(&child, "data-key").as_deref(), Some("ArrowRight"));
    assert_eq!(
        runtime
            .eval("document.activeElement.id")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "child"
    );
}

#[test]
fn nested_cross_origin_focus_sets_every_ancestor_and_clears_the_old_chain() {
    let mut runtime = runtime(&[
        (
            "https://child.test/child",
            "<html><body><script>globalThis.frameReady=true</script><iframe id=inner allow='focus-without-user-activation' src='https://inner.test/inner'></iframe></body></html>",
        ),
        ("https://inner.test/inner", INPUT_PAGE),
    ]);
    let middle = frame_document(&runtime, &runtime.document(), "#child");
    let inner = frame_document(&runtime, &middle, "#inner");
    eval_document(
        &mut runtime,
        &inner,
        "document.getElementById('target').focus()",
    );
    eval_document(
        &mut runtime,
        &middle,
        "document.documentElement.setAttribute('data-focus', [document.activeElement.id,document.hasFocus()].join('|'))",
    );
    eval_document(
        &mut runtime,
        &inner,
        "document.documentElement.setAttribute('data-focus', [document.activeElement.id,document.hasFocus()].join('|'))",
    );
    assert_eq!(
        attribute(&middle, "data-focus").as_deref(),
        Some("inner|true")
    );
    assert_eq!(
        attribute(&inner, "data-focus").as_deref(),
        Some("target|true")
    );
    assert_eq!(
        runtime
            .eval("document.activeElement.id === 'child' && document.hasFocus()")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    runtime
        .eval("document.getElementById('old').focus()")
        .unwrap();
    for child in [&middle, &inner] {
        eval_document(
            &mut runtime,
            child,
            "document.documentElement.setAttribute('data-focus', [document.activeElement === document.body,document.hasFocus()].join('|'))",
        );
        assert_eq!(
            attribute(child, "data-focus").as_deref(),
            Some("true|false")
        );
    }
}

#[test]
fn parser_focus_reaches_through_an_ancestor_without_author_scripts() {
    for inner_url in ["https://inner.test/inner", "https://child.test/inner"] {
        let middle_page = format!(
            "<html><body><iframe id=inner name=innerWindow allow='focus-without-user-activation' src='{inner_url}'></iframe></body></html>"
        );
        let mut runtime = runtime(&[
            ("https://child.test/child", middle_page.as_str()),
            (
                inner_url,
                "<html><body><input id=target><script>document.getElementById('target').focus(); document.documentElement.setAttribute('data-focus',String(document.activeElement.id === 'target' && document.hasFocus()))</script></body></html>",
            ),
        ]);
        let middle = frame_document(&runtime, &runtime.document(), "#child");
        let inner = frame_document(&runtime, &middle, "#inner");
        assert_eq!(attribute(&inner, "data-focus").as_deref(), Some("true"));
        eval_document(
            &mut runtime,
            &middle,
            "document.documentElement.setAttribute('data-focus', [document.activeElement.id,document.hasFocus()].join('|'))",
        );
        assert_eq!(
            attribute(&middle, "data-focus").as_deref(),
            Some("inner|true")
        );
        assert_eq!(
            runtime
                .eval("document.activeElement.id === 'child' && document.hasFocus()")
                .unwrap()
                .as_boolean(),
            Some(true)
        );
    }
}

#[test]
fn cross_origin_focus_reentry_from_element_or_document_blur_wins() {
    for context_blur in [false, true] {
        let mut runtime = runtime(&[("https://child.test/child", INPUT_PAGE)]);
        let child = frame_document(&runtime, &runtime.document(), "#child");
        let receiver = if context_blur { "document" } else { "old" };
        runtime.eval(&format!(
            "const old=document.getElementById('old'); const redirect=document.getElementById('redirect'); old.focus(); {receiver}.addEventListener('blur', () => redirect.focus(), {{once:true}})"
        )).unwrap();
        eval_document(
            &mut runtime,
            &child,
            "document.getElementById('target').focus(); document.documentElement.setAttribute('data-focus', String(document.hasFocus()))",
        );
        assert_eq!(attribute(&child, "data-focus").as_deref(), Some("false"));
        assert_eq!(
            runtime
                .eval("document.activeElement === redirect && document.hasFocus()")
                .unwrap()
                .as_boolean(),
            Some(true)
        );
    }
}

#[test]
fn removing_a_cross_origin_frame_during_blur_does_not_resume_its_focus() {
    let mut runtime = runtime(&[("https://child.test/child", INPUT_PAGE)]);
    let child = frame_document(&runtime, &runtime.document(), "#child");
    runtime.eval("const old=document.getElementById('old'); old.focus(); old.addEventListener('blur',()=>document.getElementById('child').remove(),{once:true})").unwrap();
    eval_document(
        &mut runtime,
        &child,
        "document.getElementById('target').focus()",
    );
    assert_eq!(runtime.eval("document.querySelector('iframe') === null && document.activeElement === document.body && document.hasFocus()").unwrap().as_boolean(), Some(true));
}

#[test]
fn browser_focus_events_are_trusted_and_script_dispatched_events_are_not() {
    let mut runtime = runtime(&[("https://child.test/child", INPUT_PAGE)]);
    let child = frame_document(&runtime, &runtime.document(), "#child");
    runtime.eval("const old=document.getElementById('old'); old.focus(); let rootBlur; old.addEventListener('blur',event=>rootBlur=[event.isTrusted,event.relatedTarget===null,document.activeElement===document.body])").unwrap();
    eval_document(
        &mut runtime,
        &child,
        r#"
        const target = document.getElementById('target');
        const events = [];
        for (const type of ['focus', 'focusin', 'blur', 'focusout']) {
          target.addEventListener(type, event => events.push([event.type, event.isTrusted]));
        }
        target.focus();
        target.blur();
        target.dispatchEvent(new FocusEvent('focus'));
        document.documentElement.setAttribute('data-events', JSON.stringify(events));
        "#,
    );
    assert_eq!(
        attribute(&child, "data-events").as_deref(),
        Some(
            r#"[["focus",true],["focusin",true],["blur",true],["focusout",true],["focus",false]]"#
        )
    );
    assert_eq!(
        runtime
            .eval("JSON.stringify(rootBlur)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[true,true,true]"
    );
}
