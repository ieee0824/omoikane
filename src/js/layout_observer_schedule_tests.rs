use super::*;
use crate::html::TreeBuilder;

fn runtime() -> JsRuntime {
    JsRuntime::with_document(
        TreeBuilder::parse("<div id=box style='width:10px;height:10px'></div>").document(),
    )
    .unwrap()
}

fn eval_string(runtime: &mut JsRuntime, source: &str) -> String {
    runtime
        .eval(source)
        .unwrap()
        .to_string(&mut runtime.context)
        .unwrap()
        .to_std_string_escaped()
}

#[test]
fn mutation_defers_layout_observer_work_until_rendering_opportunity() {
    let mut runtime = runtime();
    runtime
        .eval(
            r#"
            globalThis.sizes = [];
            globalThis.resizeObserver = new ResizeObserver(entries => {
                sizes.push(entries[0].contentRect.width);
            });
            resizeObserver.observe(document.getElementById("box"));
            "#,
        )
        .unwrap();
    runtime.run_jobs().unwrap();
    assert_eq!(eval_string(&mut runtime, "sizes.join(',')"), "10");

    runtime
        .eval(
            r#"
            const box = document.getElementById("box");
            box.style.width = "20px";
            box.style.width = "30px";
            "#,
        )
        .unwrap();
    runtime.run_jobs().unwrap();
    assert_eq!(eval_string(&mut runtime, "sizes.join(',')"), "10");
    assert_eq!(
        runtime
            .host_state
            .borrow()
            .pending_layout_observer_documents
            .len(),
        1,
        "mutations in one turn must queue one document"
    );

    runtime.run_animation_frame(16).unwrap();
    assert_eq!(eval_string(&mut runtime, "sizes.join(',')"), "10,30");
    assert!(
        runtime
            .host_state
            .borrow()
            .pending_layout_observer_documents
            .is_empty()
    );
}

#[test]
fn raf_mutation_delivers_observer_after_callback_in_same_opportunity() {
    let mut runtime = runtime();
    runtime
        .eval(
            r#"
            globalThis.order = [];
            globalThis.resizeObserver = new ResizeObserver(entries => {
                order.push(`resize:${entries[0].contentRect.width}`);
            });
            resizeObserver.observe(document.getElementById("box"));
            "#,
        )
        .unwrap();
    runtime.run_jobs().unwrap();
    runtime.eval("order = []").unwrap();
    runtime
        .eval(
            r#"
            requestAnimationFrame(() => {
                document.getElementById("box").style.width = "40px";
                order.push("raf");
            });
            "#,
        )
        .unwrap();

    runtime.run_animation_frame(16).unwrap();
    assert_eq!(
        eval_string(&mut runtime, "order.join(',')"),
        "raf,resize:40"
    );
}

#[test]
fn mutation_defers_intersection_geometry_until_rendering_opportunity() {
    let mut runtime = runtime();
    runtime.set_viewport(200.0, 100.0);
    runtime
        .eval(
            r#"
            globalThis.intersections = [];
            globalThis.intersectionObserver = new IntersectionObserver(entries => {
                intersections.push(entries[0].isIntersecting);
            });
            intersectionObserver.observe(document.getElementById("box"));
            "#,
        )
        .unwrap();
    runtime.run_jobs().unwrap();
    assert_eq!(eval_string(&mut runtime, "intersections.join(',')"), "true");

    runtime
        .eval("document.getElementById('box').style.transform = 'translateY(500px)'")
        .unwrap();
    runtime.run_jobs().unwrap();
    assert_eq!(eval_string(&mut runtime, "intersections.join(',')"), "true");

    runtime.run_animation_frame(16).unwrap();
    assert_eq!(
        eval_string(&mut runtime, "intersections.join(',')"),
        "true,false"
    );
}

#[test]
fn pending_observer_work_requests_a_frame_but_not_while_hidden() {
    let mut runtime = runtime();
    runtime
        .eval(
            r#"
            globalThis.resizeObserver = new ResizeObserver(() => {});
            resizeObserver.observe(document.getElementById("box"));
            "#,
        )
        .unwrap();
    runtime.run_jobs().unwrap();
    assert_eq!(runtime.next_rendering(), crate::cdp::NextRendering::Idle);

    runtime
        .eval("document.getElementById('box').style.width = '20px'")
        .unwrap();
    runtime.run_jobs().unwrap();
    assert_eq!(
        runtime.next_rendering(),
        crate::cdp::NextRendering::EveryFrame
    );

    runtime.set_page_visibility(true);
    assert_eq!(runtime.next_rendering(), crate::cdp::NextRendering::Idle);
    runtime.set_page_visibility(false);
    assert_eq!(
        runtime.next_rendering(),
        crate::cdp::NextRendering::EveryFrame
    );
    runtime.run_animation_frame(16).unwrap();
    assert!(
        runtime
            .host_state
            .borrow()
            .pending_layout_observer_documents
            .is_empty()
    );
}

#[test]
fn observer_callback_mutation_requests_the_followup_frame() {
    let mut runtime = runtime();
    runtime
        .eval(
            r#"
            globalThis.widths = [];
            globalThis.mutateFromObserver = false;
            globalThis.resizeObserver = new ResizeObserver(entries => {
                const width = entries[0].contentRect.width;
                widths.push(width);
                if (mutateFromObserver && width === 20) {
                    document.getElementById("box").style.width = "30px";
                }
            });
            resizeObserver.observe(document.getElementById("box"));
            "#,
        )
        .unwrap();
    runtime.run_jobs().unwrap();
    runtime
        .eval(
            r#"
            mutateFromObserver = true;
            document.getElementById("box").style.width = "20px";
            "#,
        )
        .unwrap();

    runtime.run_animation_frame(16).unwrap();
    assert_eq!(eval_string(&mut runtime, "widths.join(',')"), "10,20");
    assert_eq!(
        runtime.next_rendering(),
        crate::cdp::NextRendering::EveryFrame,
        "a callback mutation must not strand the next observer pass"
    );
    runtime.run_animation_frame(16).unwrap();
    assert_eq!(eval_string(&mut runtime, "widths.join(',')"), "10,20,30");
}
