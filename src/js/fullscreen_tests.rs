use super::{FullscreenTransition, JsRuntime};
use crate::cdp::CdpSession;
use crate::html::TreeBuilder;
use crate::paint::{Canvas, Color, Image};
use serde_json::json;
use std::fs;
use std::path::PathBuf;

const FULLSCREEN_RENDER_FIXTURE: &str =
    include_str!("../../tests/fixtures/anonymized-fullscreen-top-layer/page.html");

fn fullscreen_render_fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("anonymized-fullscreen-top-layer")
}

fn fullscreen_render_output_path(variant: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("output")
        .join(format!(
            "anonymized-fullscreen-top-layer.local-baseline.{variant}.png"
        ))
}

fn render_fullscreen_fixture() -> Canvas {
    let document = TreeBuilder::parse(FULLSCREEN_RENDER_FIXTURE).document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    runtime.set_viewport(160.0, 120.0);
    activate_with_key(
        &mut runtime,
        "document.getElementById('target').requestFullscreen()",
    );
    runtime.paint_current_document().unwrap()
}

fn bool_result(runtime: &mut JsRuntime, source: &str) -> bool {
    let result = runtime
        .eval(&format!(
            "try {{ globalThis.__omoikane_test_bool = !!({source}); '' }} \
             catch (error) {{ String(error.name) + ':' + String(error.message) + ':' + String(error.stack) }}"
        ))
        .unwrap();
    let diagnostic = result.as_string().unwrap().to_std_string_escaped();
    assert!(
        diagnostic.is_empty(),
        "JavaScript assertion failed: {diagnostic}"
    );
    runtime
        .eval("globalThis.__omoikane_test_bool")
        .unwrap()
        .as_boolean()
        .unwrap_or(false)
}

fn activate_with_key(runtime: &mut JsRuntime, handler: &str) {
    runtime
        .eval(&format!(
            "globalThis.activationError=''; \
             document.addEventListener('keydown', () => {{ try {{ {handler} }} catch (error) {{ \
               activationError=error.name+':'+error.message; }} }}, {{ once: true }}); \
             __omoikane_dispatch_keyboard_input('keydown', {{ key: 'Enter', code: 'Enter' }});"
        ))
        .unwrap();
    runtime.run_jobs().unwrap();
    let error = runtime
        .eval("activationError")
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert!(error.is_empty(), "activation handler failed: {error}");
}

#[test]
fn fullscreen_requires_activation_and_reports_host_success_and_rejection() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            "document.body.innerHTML='<main id=target></main>'; \
             globalThis.target=document.getElementById('target'); globalThis.log=[]; \
             target.onfullscreenchange=()=>log.push('change'); \
             target.onfullscreenerror=()=>log.push('error');",
        )
        .unwrap();

    runtime
        .eval("target.requestFullscreen().catch(error => log.push(error.name))")
        .unwrap();
    runtime.run_jobs().unwrap();
    assert!(bool_result(
        &mut runtime,
        "document.fullscreenElement === null"
    ));
    assert!(bool_result(
        &mut runtime,
        "log.join(',') === 'error,TypeError'"
    ));
    assert_eq!(runtime.take_fullscreen_transition(), None);

    runtime.eval("log=[]").unwrap();
    activate_with_key(
        &mut runtime,
        "target.requestFullscreen({navigationUI:'hide'}).then(() => log.push('resolved'))",
    );
    assert!(bool_result(
        &mut runtime,
        "document.fullscreenElement === target && target.matches(':fullscreen') && \
         document.fullscreenEnabled && log.join(',') === 'change,resolved'"
    ));
    assert_eq!(
        runtime.take_fullscreen_transition(),
        Some(FullscreenTransition::Enter)
    );

    runtime
        .eval("document.exitFullscreen().then(() => log.push('exited'))")
        .unwrap();
    runtime.run_jobs().unwrap();
    assert!(bool_result(
        &mut runtime,
        "document.fullscreenElement === null && log.slice(-2).join(',') === 'change,exited'"
    ));
    assert_eq!(
        runtime.take_fullscreen_transition(),
        Some(FullscreenTransition::Exit)
    );

    runtime.set_fullscreen_transition_allowed(false);
    runtime.eval("log=[]").unwrap();
    activate_with_key(
        &mut runtime,
        "target.requestFullscreen().catch(error => log.push(error.name))",
    );
    assert!(bool_result(
        &mut runtime,
        "document.fullscreenElement === null && log.join(',') === 'error,TypeError'"
    ));
    assert_eq!(runtime.take_fullscreen_transition(), None);
}

#[test]
fn fullscreen_activation_survives_microtasks_in_the_input_task() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            "document.body.innerHTML='<main id=target></main>'; \
             globalThis.target=document.getElementById('target'); globalThis.result='';",
        )
        .unwrap();
    activate_with_key(
        &mut runtime,
        "Promise.resolve().then(()=>target.requestFullscreen()).then(\
          ()=>result='resolved', error=>result=error.name)",
    );
    assert!(bool_result(
        &mut runtime,
        "result === 'resolved' && document.fullscreenElement === target"
    ));

    runtime.eval("document.exitFullscreen()").unwrap();
    runtime.run_jobs().unwrap();
    activate_with_key(&mut runtime, "void 0");
    runtime.tick(0).unwrap();
    runtime
        .eval("result=''; target.requestFullscreen().catch(error=>result=error.name)")
        .unwrap();
    runtime.run_jobs().unwrap();
    assert!(bool_result(
        &mut runtime,
        "result === 'TypeError' && document.fullscreenElement === null"
    ));
}

#[test]
fn fullscreen_escape_removal_and_host_exit_clear_state() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            "document.body.innerHTML='<section id=outer><div id=target></div></section>'; \
             globalThis.outer=document.getElementById('outer'); globalThis.target=document.getElementById('target'); \
             globalThis.changes=0; globalThis.lastChangeTarget=null; \
             document.onfullscreenchange=event=>{changes++;lastChangeTarget=event.target};",
        )
        .unwrap();
    activate_with_key(&mut runtime, "target.requestFullscreen()");
    assert_eq!(
        runtime.take_fullscreen_transition(),
        Some(FullscreenTransition::Enter)
    );

    runtime
        .eval("__omoikane_dispatch_keyboard_input('keydown',{key:'Escape',code:'Escape'})")
        .unwrap();
    runtime.run_jobs().unwrap();
    assert!(bool_result(
        &mut runtime,
        "document.fullscreenElement === null && changes === 2"
    ));
    assert_eq!(
        runtime.take_fullscreen_transition(),
        Some(FullscreenTransition::Exit)
    );

    activate_with_key(&mut runtime, "target.requestFullscreen()");
    assert_eq!(
        runtime.take_fullscreen_transition(),
        Some(FullscreenTransition::Enter)
    );
    runtime.eval("outer.remove()").unwrap();
    runtime.run_jobs().unwrap();
    assert!(bool_result(
        &mut runtime,
        "document.fullscreenElement === null && changes === 4"
    ));
    assert_eq!(
        runtime.take_fullscreen_transition(),
        Some(FullscreenTransition::Exit)
    );

    runtime
        .eval("document.body.appendChild(outer); document.body.appendChild(target)")
        .unwrap();
    activate_with_key(&mut runtime, "target.requestFullscreen()");
    assert_eq!(
        runtime.take_fullscreen_transition(),
        Some(FullscreenTransition::Enter)
    );
    assert!(runtime.exit_fullscreen_from_host().unwrap());
    assert!(bool_result(
        &mut runtime,
        "document.fullscreenElement === null && changes === 6 && lastChangeTarget === target"
    ));
    assert_eq!(
        runtime.take_fullscreen_transition(),
        Some(FullscreenTransition::Exit)
    );
}

#[test]
fn fullscreen_iframe_permission_boundary_and_reflection() {
    let document = TreeBuilder::parse(
        "<body><iframe id=frame srcdoc='<button id=target>go</button>'></iframe></body>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    runtime
        .eval(
            "globalThis.frame=document.getElementById('frame'); \
             globalThis.child=frame.contentWindow; \
             child.eval(\"globalThis.result=''; document.addEventListener('keydown',()=> \
               document.getElementById('target').requestFullscreen().then(()=>result='ok',e=>result=e.name),{once:true}); \
               __omoikane_dispatch_keyboard_input('keydown',{key:'Enter',code:'Enter'});\");",
        )
        .unwrap();
    runtime.run_jobs().unwrap();
    assert!(bool_result(
        &mut runtime,
        "child.result === 'ok' && frame.contentDocument.fullscreenElement.id === 'target' && \
         document.fullscreenElement === frame"
    ));
    runtime
        .eval("frame.contentDocument.exitFullscreen()")
        .unwrap();
    runtime.run_jobs().unwrap();
    runtime.take_fullscreen_transition();

    runtime
        .eval(
            "frame.setAttribute('allow', \"fullscreen 'none'\"); frame.srcdoc='<button id=blocked>no</button>'; \
             globalThis.child=frame.contentWindow; child.eval(\"globalThis.result=''; \
               document.addEventListener('keydown',()=>document.getElementById('blocked').requestFullscreen().then(()=>result='ok',e=>result=e.name),{once:true}); \
               __omoikane_dispatch_keyboard_input('keydown',{key:'Enter',code:'Enter'});\");",
        )
        .unwrap();
    runtime.run_jobs().unwrap();
    assert!(bool_result(
        &mut runtime,
        "child.result === 'TypeError' && frame.contentDocument.fullscreenEnabled === false && \
         document.fullscreenElement === null"
    ));
    assert!(bool_result(
        &mut runtime,
        "frame.allowFullscreen === false && (frame.allowFullscreen=true) && \
         frame.hasAttribute('allowfullscreen')"
    ));
}

#[test]
fn fullscreen_element_is_scoped_to_its_shadow_root() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            "globalThis.host=document.createElement('section'); document.body.appendChild(host); \
             globalThis.root=host.attachShadow({mode:'open'}); \
             globalThis.target=document.createElement('div'); root.appendChild(target);",
        )
        .unwrap();
    activate_with_key(&mut runtime, "target.requestFullscreen()");
    assert!(bool_result(
        &mut runtime,
        "document.fullscreenElement === host && root.fullscreenElement === target"
    ));
    runtime.eval("document.exitFullscreen()").unwrap();
    runtime.run_jobs().unwrap();
    assert!(bool_result(
        &mut runtime,
        "document.fullscreenElement === null && root.fullscreenElement === null"
    ));
}

#[test]
fn fullscreen_accepts_supported_element_namespaces() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            "globalThis.math=document.createElementNS('http://www.w3.org/1998/Math/MathML','math'); \
             document.body.appendChild(math);",
        )
        .unwrap();
    activate_with_key(&mut runtime, "math.requestFullscreen()");
    assert!(bool_result(
        &mut runtime,
        "document.fullscreenElement === math && math.matches(':fullscreen')"
    ));
}

#[test]
fn fullscreen_uses_top_layer_viewport_backdrop_and_hit_testing() {
    let document = TreeBuilder::parse(
        r#"<html><head><style>
          * { margin: 0; padding: 0; border: 0 }
          #cover { position: fixed; inset: 0; z-index: 2147483647; background: blue }
          #target { width: 5px; height: 5px; background: transparent; transform: translate(30px,30px) }
          #marker { width: 20px; height: 20px; background: red }
        </style></head><body><div id="cover"></div><div id="target"><div id="marker"></div></div></body></html>"#,
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    runtime.set_viewport(80.0, 60.0);
    activate_with_key(
        &mut runtime,
        "document.getElementById('target').requestFullscreen()",
    );

    let canvas = runtime.paint_current_document().unwrap();
    assert_eq!(canvas.pixel(0, 0), Some(Color::rgb(255, 0, 0)));
    assert_eq!(canvas.pixel(70, 50), Some(Color::rgb(0, 0, 0)));
    assert_eq!(
        runtime
            .hit_test(70.0, 50.0)
            .unwrap()
            .get_attribute("id")
            .as_deref(),
        Some("target")
    );
}

#[test]
fn fullscreen_document_element_fills_the_viewport() {
    let document = TreeBuilder::parse(
        r#"<html style="width:5px;height:5px;transform:translate(20px,20px);background:#00ff00">
             <body></body></html>"#,
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    runtime.set_viewport(40.0, 30.0);
    activate_with_key(&mut runtime, "document.documentElement.requestFullscreen()");

    let canvas = runtime.paint_current_document().unwrap();
    assert_eq!(canvas.pixel(0, 0), Some(Color::rgb(0, 255, 0)));
    assert_eq!(canvas.pixel(39, 29), Some(Color::rgb(0, 255, 0)));
    assert!(bool_result(
        &mut runtime,
        "document.fullscreenElement === document.documentElement && \
         getComputedStyle(document.documentElement).boxSizing === 'border-box'"
    ));
}

#[test]
fn fullscreen_top_layer_fixture_matches_local_baseline_png() {
    let actual = render_fullscreen_fixture();
    assert_eq!((actual.width(), actual.height()), (160, 120));
    let baseline_path = fullscreen_render_fixture_dir().join("page.baseline.png");
    let expected_png = fs::read(&baseline_path).unwrap_or_else(|error| {
        panic!(
            "missing fullscreen rendering baseline at {}: {error}",
            baseline_path.display()
        )
    });
    let expected = Image::decode_png(&expected_png).expect("decode fullscreen rendering baseline");
    assert_eq!((expected.width(), expected.height()), (160, 120));

    let changed = actual
        .pixels()
        .chunks_exact(4)
        .zip(expected.pixels().chunks_exact(4))
        .filter(|(actual, expected)| actual != expected)
        .count();
    if changed != 0 {
        let mut diff = Canvas::new(actual.width(), actual.height());
        for (index, (actual_pixel, expected_pixel)) in actual
            .pixels()
            .chunks_exact(4)
            .zip(expected.pixels().chunks_exact(4))
            .enumerate()
        {
            if actual_pixel != expected_pixel {
                diff.set_pixel(
                    index as u32 % actual.width(),
                    index as u32 / actual.width(),
                    Color::rgb(255, 0, 255),
                );
            }
        }
        fs::create_dir_all(fullscreen_render_output_path("actual").parent().unwrap()).unwrap();
        fs::write(fullscreen_render_output_path("actual"), actual.encode_png()).unwrap();
        fs::write(fullscreen_render_output_path("expected"), expected_png).unwrap();
        fs::write(fullscreen_render_output_path("diff"), diff.encode_png()).unwrap();
        panic!(
            "fullscreen rendering diverged from the checked-in baseline ({changed} pixels differ); wrote actual, expected, and diff PNGs to tests/output"
        );
    }
}

#[test]
#[ignore = "maintenance utility: set OMOIKANE_REFRESH_BASELINE=1 to rewrite the checked-in fullscreen baseline image"]
fn refresh_fullscreen_top_layer_baseline_png() {
    if std::env::var("OMOIKANE_REFRESH_BASELINE").as_deref() != Ok("1") {
        eprintln!(
            "skipping refresh_fullscreen_top_layer_baseline_png: set OMOIKANE_REFRESH_BASELINE=1 to rewrite {}",
            fullscreen_render_fixture_dir()
                .join("page.baseline.png")
                .display()
        );
        return;
    }
    fs::write(
        fullscreen_render_fixture_dir().join("page.baseline.png"),
        render_fullscreen_fixture().encode_png(),
    )
    .unwrap();
}

#[test]
fn navigation_exits_native_fullscreen() {
    let mut session = CdpSession::new().unwrap();
    session
        .dispatch(
            "Page.navigate",
            json!({"url":"data:text/html,<button id='target'>go</button>"}),
        )
        .unwrap();
    session
        .dispatch(
            "Runtime.evaluate",
            json!({"expression":"document.addEventListener('keydown',()=>document.getElementById('target').requestFullscreen(),{once:true}); __omoikane_dispatch_keyboard_input('keydown',{key:'Enter',code:'Enter'})"}),
        )
        .unwrap();
    assert_eq!(
        session.take_fullscreen_transition(),
        Some(FullscreenTransition::Enter)
    );
    session
        .dispatch("Page.navigate", json!({"url":"data:text/html,<p>next</p>"}))
        .unwrap();
    assert_eq!(
        session.take_fullscreen_transition(),
        Some(FullscreenTransition::Exit)
    );
}
