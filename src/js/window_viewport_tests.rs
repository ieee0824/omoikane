use super::*;

struct ViewportPages(HashMap<String, String>);

impl crate::http::HttpTransport for ViewportPages {
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

const EMPTY_PAGE: &str = "<html><body></body></html>";

fn runtime(attributes: &str, pages: &[(&str, &str)]) -> JsRuntime {
    let document = crate::html::TreeBuilder::parse(&format!(
        "<html><body><iframe id=child {attributes} src='{}'></iframe></body></html>",
        pages[0].0
    ))
    .document();
    let mut runtime = JsRuntime::with_document_and_url(document, "https://top.test/page")
        .unwrap_or_else(|error| panic!("create fixture: {error}"));
    runtime.set_viewport(800.0, 600.0);
    runtime.set_http_resource_transport(Some(Arc::new(ViewportPages(
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
    let frame = parent.query_selector(selector).unwrap();
    runtime.host_state.borrow().iframe_documents[&frame.identity()]
        .document
        .clone()
}

fn measure(runtime: &mut JsRuntime, document: &NodeHandle) -> String {
    runtime.eval_in_document_realm(document.identity(),
        "document.documentElement.setAttribute('data-size',[innerWidth,innerHeight,visualViewport.width,visualViewport.height].join('|'))"
    ).unwrap_or_else(|error| panic!("measure child: {error}"));
    document
        .query_selector("html")
        .unwrap()
        .get_attribute("data-size")
        .unwrap()
}

#[test]
fn iframe_inner_size_uses_the_current_document_viewport() {
    for child_url in ["https://top.test/child", "https://other.test/child"] {
        let mut runtime = runtime(
            "style='width:220px;height:90px;border:0'",
            &[(child_url, EMPTY_PAGE)],
        );
        let child = frame_document(&runtime, &runtime.document(), "#child");
        assert_eq!(
            measure(&mut runtime, &child),
            "220|90|220|90",
            "{child_url}"
        );
        let parent_read = runtime.eval(
            "(() => { try { const view=document.getElementById('child').contentWindow; return [view.innerWidth,view.innerHeight].join('|'); } catch(error) { return error.name; } })()"
        ).unwrap().as_string().unwrap().to_std_string_escaped();
        assert_eq!(
            parent_read,
            if child_url.contains("other.test") {
                "SecurityError"
            } else {
                "220|90"
            }
        );
        runtime.eval_in_document_realm(child.identity(),
            "let parentAccess; try { void parent.document; parentAccess='allowed'; } catch(error) { parentAccess=error.name; } document.documentElement.setAttribute('data-parent',parentAccess)"
        ).unwrap();
        assert_eq!(
            child
                .query_selector("html")
                .unwrap()
                .get_attribute("data-parent")
                .as_deref(),
            Some(if child_url.contains("other.test") {
                "SecurityError"
            } else {
                "allowed"
            })
        );
    }
}

#[test]
fn iframe_inner_size_tracks_defaults_attributes_and_css_changes() {
    for (attributes, initial) in [
        ("", "300|150|300|150"),
        ("width=180 height=70", "180|70|180|70"),
    ] {
        let mut runtime = runtime(attributes, &[("https://other.test/child", EMPTY_PAGE)]);
        let child = frame_document(&runtime, &runtime.document(), "#child");
        assert_eq!(measure(&mut runtime, &child), initial);
        runtime.eval("document.getElementById('child').style.cssText='width:340px;height:140px;border:0'").unwrap();
        assert_eq!(measure(&mut runtime, &child), "340|140|340|140");
    }
}

#[test]
fn nested_inner_sizes_follow_their_own_frame_and_parent_resize() {
    let mut runtime = runtime(
        "style='width:50vw;height:25vh;border:0'",
        &[
            (
                "https://other.test/child",
                "<html><body><iframe id=inner style='width:50vw;height:50vh;border:0' src='https://inner.test/inner'></iframe></body></html>",
            ),
            ("https://inner.test/inner", EMPTY_PAGE),
        ],
    );
    let child = frame_document(&runtime, &runtime.document(), "#child");
    let inner = frame_document(&runtime, &child, "#inner");
    assert_eq!(measure(&mut runtime, &child), "400|150|400|150");
    assert_eq!(measure(&mut runtime, &inner), "200|75|200|75");
    runtime.set_viewport(1200.0, 800.0);
    assert_eq!(measure(&mut runtime, &child), "600|200|600|200");
    assert_eq!(measure(&mut runtime, &inner), "300|100|300|100");
}

#[test]
fn top_level_inner_size_uses_layout_viewport_and_remains_replaceable() {
    let mut runtime = runtime("", &[("https://other.test/child", EMPTY_PAGE)]);
    runtime.set_viewport(640.0, 480.0);
    runtime.set_visual_viewport(320.0, 240.0, 0.0, 0.0, 2.0);
    let document = runtime.document();
    assert_eq!(measure(&mut runtime, &document), "640|480|320|240");
    runtime
        .eval("innerWidth='page value'; innerHeight=17")
        .unwrap();
    runtime.set_viewport(800.0, 600.0);
    assert_eq!(
        runtime
            .eval("innerWidth === 'page value' && innerHeight === 17")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn clipped_button_center_stays_inside_its_cross_origin_frame() {
    let mut runtime = runtime(
        "style='width:220px;height:90px;border:0'",
        &[("https://other.test/child", EMPTY_PAGE)],
    );
    let child = frame_document(&runtime, &runtime.document(), "#child");
    runtime
        .eval_in_document_realm(
            child.identity(),
            r#"
        const target = document.createElement('button');
        target.style.cssText = 'display:block;width:600px;height:40px';
        document.body.appendChild(target);
        const rect = target.getBoundingClientRect();
        const x = Math.floor((Math.max(0, rect.left) + Math.min(innerWidth, rect.right)) / 2);
        const y = Math.floor((Math.max(0, rect.top) + Math.min(innerHeight, rect.bottom)) / 2);
        const hit = document.elementsFromPoint(x,y).some(node => target.contains(node));
        document.documentElement.setAttribute('data-hit', String(hit));
    "#,
        )
        .unwrap();
    assert_eq!(
        child
            .query_selector("html")
            .unwrap()
            .get_attribute("data-hit")
            .as_deref(),
        Some("true")
    );
}
