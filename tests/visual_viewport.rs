use omoikane::{html::TreeBuilder, js::JsRuntime};

fn runtime(html: &str) -> JsRuntime {
    JsRuntime::with_document_and_url(
        TreeBuilder::parse(html).document(),
        "https://example.test/visual-viewport.html",
    )
    .expect("create JavaScript runtime")
}

fn check(runtime: &mut JsRuntime, source: &str) {
    assert!(
        runtime
            .eval(source)
            .unwrap_or_else(|error| panic!("{source}: {error}"))
            .to_boolean(),
        "{source}"
    );
}

fn string(runtime: &mut JsRuntime, source: &str) -> String {
    runtime
        .eval(source)
        .unwrap_or_else(|error| panic!("{source}: {error}"))
        .as_string()
        .expect("expression must return a string")
        .to_std_string_escaped()
}

#[test]
fn exposes_same_visual_viewport_with_live_default_geometry() {
    let mut runtime = runtime("<body></body>");
    check(
        &mut runtime,
        r#"(() => {
          let illegal = false;
          try { new VisualViewport(); } catch (error) { illegal = error instanceof TypeError; }
          return typeof VisualViewport === "function" &&
            visualViewport === window.visualViewport &&
            visualViewport instanceof VisualViewport &&
            visualViewport instanceof EventTarget &&
            visualViewport.toString() === "[object VisualViewport]" && illegal &&
            visualViewport.width === 1280 && visualViewport.height === 720 &&
            visualViewport.offsetLeft === 0 && visualViewport.offsetTop === 0 &&
            visualViewport.pageLeft === 0 && visualViewport.pageTop === 0 &&
            visualViewport.scale === 1;
        })()"#,
    );

    runtime.set_viewport(640.0, 480.0);
    check(
        &mut runtime,
        "visualViewport.width === 640 && visualViewport.height === 480 && innerWidth === 640 && innerHeight === 480",
    );
}

#[test]
fn coalesces_resize_and_host_visual_viewport_events_in_spec_order() {
    let mut runtime =
        runtime("<style>html,body{margin:0;width:2000px;height:2000px}</style><body></body>");
    runtime
        .eval(
            r#"
            globalThis.viewportEvents = [];
            onresize = () => viewportEvents.push("window-attribute");
            addEventListener("resize", () => viewportEvents.push("window-listener"));
            onscroll = () => viewportEvents.push("window-scroll-attribute");
            addEventListener("scroll", () => viewportEvents.push("window-scroll-listener"));
            visualViewport.onresize = () => viewportEvents.push("visual-resize-attribute");
            visualViewport.addEventListener("resize", () => viewportEvents.push("visual-resize-listener"));
            visualViewport.onscroll = () => viewportEvents.push("visual-scroll-attribute");
            visualViewport.addEventListener("scroll", () => viewportEvents.push("visual-scroll-listener"));
            "#,
        )
        .unwrap();

    runtime.set_viewport(640.0, 480.0);
    runtime.set_viewport(600.0, 400.0);
    check(&mut runtime, "viewportEvents.length === 0");
    runtime.run_animation_frame(16).unwrap();
    assert_eq!(
        string(&mut runtime, "viewportEvents.join(',')"),
        "window-attribute,window-listener,visual-resize-attribute,visual-resize-listener"
    );

    runtime.eval("viewportEvents.length = 0").unwrap();
    runtime.set_visual_viewport(300.0, 200.0, 40.0, 30.0, 2.0);
    check(
        &mut runtime,
        "visualViewport.width === 300 && visualViewport.height === 200 && visualViewport.offsetLeft === 40 && visualViewport.offsetTop === 30 && visualViewport.pageLeft === 40 && visualViewport.pageTop === 30 && visualViewport.scale === 2 && viewportEvents.length === 0",
    );
    runtime.run_animation_frame(16).unwrap();
    assert_eq!(
        string(&mut runtime, "viewportEvents.join(',')"),
        "visual-resize-attribute,visual-resize-listener,visual-scroll-attribute,visual-scroll-listener"
    );

    runtime.eval("viewportEvents.length = 0").unwrap();
    runtime.set_visual_viewport(300.0, 200.0, 40.0, 30.0, 2.0);
    runtime.run_animation_frame(16).unwrap();
    check(&mut runtime, "viewportEvents.length === 0");

    runtime.set_visual_viewport(300.0, 200.0, 50.0, 30.0, 2.0);
    runtime.eval("scrollTo(10, 20)").unwrap();
    runtime.run_animation_frame(16).unwrap();
    assert_eq!(
        string(&mut runtime, "viewportEvents.join(',')"),
        "window-scroll-attribute,window-scroll-listener,visual-scroll-attribute,visual-scroll-listener"
    );
}

#[test]
fn page_position_combines_layout_scroll_with_visual_offset() {
    let mut runtime =
        runtime("<style>html,body{margin:0;width:2000px;height:2000px}</style><body></body>");
    runtime.set_viewport(640.0, 480.0);
    runtime.set_visual_viewport(320.0, 240.0, 40.0, 30.0, 2.0);
    runtime.eval("scrollTo(100, 120)").unwrap();
    check(
        &mut runtime,
        "scrollX === 100 && scrollY === 120 && visualViewport.offsetLeft === 40 && visualViewport.offsetTop === 30 && visualViewport.pageLeft === 140 && visualViewport.pageTop === 150",
    );
}

#[test]
fn iframe_has_independent_geometry_scroll_and_resize_events() {
    let mut runtime = runtime(
        "<style>html,body{margin:0}iframe{display:block;width:200px;height:300px;border:0}</style><body><iframe id=frame></iframe></body>",
    );
    runtime
        .eval(
            r#"
            globalThis.frame = document.getElementById("frame");
            globalThis.child = frame.contentWindow;
            child.eval(`
              globalThis.viewportEvents = [];
              onresize = () => viewportEvents.push("window-attribute");
              addEventListener("resize", () => viewportEvents.push("window-listener"));
              visualViewport.onresize = () => viewportEvents.push("visual-attribute");
              visualViewport.addEventListener("resize", () => viewportEvents.push("visual-listener"));
              document.body.style.width = "2000px";
              document.body.style.height = "2000px";
              scrollTo(1000, 1200);
            `);
            "#,
        )
        .unwrap();
    check(
        &mut runtime,
        r#"child.visualViewport.width === 200 && child.visualViewport.height === 300 &&
            child.visualViewport.offsetLeft === 0 && child.visualViewport.offsetTop === 0 &&
            child.visualViewport.pageLeft === 1000 && child.visualViewport.pageTop === 1200 &&
            child.visualViewport.scale === 1 && visualViewport.pageLeft === 0 && visualViewport.pageTop === 0"#,
    );

    // Drain the child's queued Window scroll before checking resize ordering.
    runtime.run_animation_frame(16).unwrap();
    runtime.eval("child.viewportEvents.length = 0").unwrap();
    runtime
        .eval("frame.style.width = '250px'; frame.style.height = '180px'; void frame.offsetWidth")
        .unwrap();
    runtime.run_animation_frame(16).unwrap();
    check(
        &mut runtime,
        "child.visualViewport.width === 250 && child.visualViewport.height === 180",
    );
    assert_eq!(
        string(&mut runtime, "child.viewportEvents.join(',')"),
        "window-attribute,window-listener,visual-attribute,visual-listener"
    );
}
