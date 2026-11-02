//! WPT automation through the page runtime's host input path.
use omoikane::js::JsRuntime;

pub(super) const VENDOR_SCRIPT: &str = include_str!("testdriver_vendor.js");

/// Executes one WebDriver tick without manufacturing page selection state.
/// The caller advances the virtual clock and rendering after the input tick.
pub(super) fn drive_action_tick(runtime: &mut JsRuntime, errors: &mut Vec<String>) -> Option<u64> {
    match runtime.advance_test_automation() {
        Ok(duration) => duration,
        Err(error) => {
            errors.push(format!("action testdriver: {error}"));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omoikane::html::TreeBuilder;

    fn scriptless_child_runtime() -> JsRuntime {
        let document = TreeBuilder::parse("<!doctype html><body></body>").document();
        let mut runtime = JsRuntime::with_document(document).unwrap();
        runtime.enable_test_automation().unwrap();
        runtime
            .eval("globalThis.test_driver_internal = {}")
            .unwrap();
        runtime.eval(VENDOR_SCRIPT).unwrap();
        runtime
            .eval(
                r#"
            var frame = document.createElement('iframe');
            document.body.appendChild(frame);
        "#,
            )
            .unwrap();
        runtime.tick(0).unwrap();
        runtime.eval(r#"
            var child = frame.contentDocument;
            child.body.innerHTML = '<button style="display:block;width:100px;height:40px">tap</button><input>';
            var completed = false;
            var failure = '';
            if (frame.contentWindow.test_driver_internal !== undefined) {
                throw Error('fixture must not preload a child test driver');
            }
        "#).unwrap();
        runtime
    }

    #[test]
    fn parent_click_routes_to_scriptless_child_input_queue() {
        let mut runtime = scriptless_child_runtime();
        runtime.eval(r#"
            var button = child.querySelector('button');
            button.addEventListener('click', event => {
                child.documentElement.setAttribute('data-click', String(event.isTrusted));
            });
            var rect = button.getBoundingClientRect();
            test_driver_internal.click(button, {x: rect.left + rect.width / 2, y: rect.top + rect.height / 2})
                .then(() => completed = true, error => failure = String(error));
        "#).unwrap();
        runtime.run_jobs().unwrap();
        assert_eq!(runtime.eval("completed").unwrap().as_boolean(), Some(false));
        for _ in 0..3 {
            assert_eq!(runtime.advance_test_automation().unwrap(), Some(0));
        }
        assert_eq!(runtime.eval("completed && failure === '' && child.documentElement.getAttribute('data-click') === 'true'").unwrap().as_boolean(), Some(true));
        assert_eq!(runtime.advance_test_automation().unwrap(), None);
    }

    #[test]
    fn parent_send_keys_routes_to_scriptless_child_input_queue() {
        let mut runtime = scriptless_child_runtime();
        runtime
            .eval(
                r#"
            var input = child.querySelector('input');
            test_driver_internal.send_keys(input, 'ab\uE003')
                .then(() => completed = true, error => failure = String(error));
        "#,
            )
            .unwrap();
        runtime.run_jobs().unwrap();
        assert_eq!(
            runtime
                .eval("input.value === '' && !completed")
                .unwrap()
                .as_boolean(),
            Some(true)
        );
        for _ in 0..6 {
            assert_eq!(runtime.advance_test_automation().unwrap(), Some(0));
        }
        assert_eq!(runtime.eval("completed && failure === '' && input.value === 'a' && child.activeElement === input").unwrap().as_boolean(), Some(true));
        assert_eq!(runtime.advance_test_automation().unwrap(), None);
    }

    #[test]
    fn parent_input_rejects_forged_disconnected_and_retired_child_elements() {
        for setup in [
            "var real = child.querySelector('button'); var target = {__id: real.__id, ownerDocument: child, isConnected: true};",
            "var target = child.querySelector('button'); target.remove();",
            "var target = child.querySelector('button'); frame.remove();",
        ] {
            let mut runtime = scriptless_child_runtime();
            runtime.eval(setup).unwrap();
            runtime
                .eval(
                    r#"
                var rejected = 0;
                test_driver_internal.click(target, {x: 1, y: 1}).then(
                    () => completed = true, () => rejected++);
                test_driver_internal.send_keys(target, 'x').then(
                    () => completed = true, () => rejected++);
            "#,
                )
                .unwrap();
            runtime.run_jobs().unwrap();
            assert_eq!(
                runtime
                    .eval("rejected === 2 && !completed")
                    .unwrap()
                    .as_boolean(),
                Some(true),
                "{setup}"
            );
            assert_eq!(runtime.advance_test_automation().unwrap(), None, "{setup}");
        }
    }

    #[test]
    fn child_input_rejects_retirement_or_adoption_during_coordinates_or_focus() {
        for (setup, command) in [
            (
                "",
                "test_driver_internal.click(child.querySelector('button'), {get x() { frame.remove(); return 1; }, y: 1})",
            ),
            (
                "child.querySelector('input').addEventListener('focus', () => frame.remove());",
                "test_driver_internal.send_keys(child.querySelector('input'), 'x')",
            ),
            (
                "",
                "test_driver_internal.click(child.querySelector('button'), {get x() { document.body.appendChild(child.querySelector('button')); return 1; }, y: 1})",
            ),
            (
                "child.querySelector('input').addEventListener('focus', event => document.body.appendChild(event.target));",
                "test_driver_internal.send_keys(child.querySelector('input'), 'x')",
            ),
            (
                "",
                "test_driver_internal.click(child.querySelector('button'), {get x() { const driver = frame.contentWindow.test_driver_internal; const enqueue = driver.action_sequence; Object.defineProperty(driver, 'action_sequence', {get() { frame.remove(); return enqueue; }}); return 1; }, y: 1})",
            ),
            (
                "child.querySelector('input').addEventListener('focus', () => { const driver = frame.contentWindow.test_driver_internal; const enqueue = driver.action_sequence; Object.defineProperty(driver, 'action_sequence', {get() { frame.remove(); return enqueue; }}); });",
                "test_driver_internal.send_keys(child.querySelector('input'), 'x')",
            ),
        ] {
            let mut runtime = scriptless_child_runtime();
            runtime.eval(setup).unwrap();
            runtime.eval("var rejected = false;").unwrap();
            runtime
                .eval(&format!(
                    "{command}.then(() => completed = true, () => rejected = true);"
                ))
                .unwrap();
            runtime.run_jobs().unwrap();
            assert_eq!(
                runtime.eval("rejected && !completed").unwrap().as_boolean(),
                Some(true),
                "{command}"
            );
            assert_eq!(runtime.advance_test_automation().unwrap(), None);
        }
    }

    #[test]
    fn native_routing_rechecks_after_installer_and_driver_getter() {
        for installer in [
            "function() { frameElement.remove(); globalThis.__wpt_enqueue_element_input = () => Promise.resolve(); }",
            "function() { Object.defineProperty(globalThis, '__wpt_enqueue_element_input', {get() { frameElement.remove(); return () => Promise.resolve(); }}); }",
        ] {
            let mut runtime = scriptless_child_runtime();
            let installer = serde_json::to_string(installer).unwrap();
            runtime
                .eval(&format!(
                    r#"
                var rejected = false;
                Promise.resolve().then(() => __omoikane_automation_route_input(
                    child.querySelector('button'), 'click', {{x: 1, y: 1}}, {installer}
                )).then(() => completed = true, () => rejected = true);
            "#
                ))
                .unwrap();
            runtime.run_jobs().unwrap();
            assert_eq!(
                runtime.eval("rejected && !completed").unwrap().as_boolean(),
                Some(true)
            );
            assert_eq!(runtime.advance_test_automation().unwrap(), None);
        }
    }

    #[test]
    fn mouse_actions_hit_test_and_dispatch_with_pressed_button_state() {
        let document = TreeBuilder::parse(
            "<!doctype html><style>#target {display:block;width:100px;height:40px}</style><button id=target>tap</button>",
        ).document();
        let mut runtime = JsRuntime::with_document(document).unwrap();
        runtime.enable_test_automation().unwrap();
        runtime
            .eval("globalThis.test_driver_internal = {}")
            .unwrap();
        runtime.eval(VENDOR_SCRIPT).unwrap();
        runtime.eval(r#"
            var target = document.getElementById('target'); var seen = []; var completed = false;
            for (const type of ['mousemove','mousedown','mouseup','click']) {
                target.addEventListener(type, event => seen.push([type,event.buttons,event.clientX,event.clientY]));
            }
            var rect=target.getClientRects()[0];
            var expectedX=Math.floor((rect.left+rect.right)/2);
            var expectedY=Math.floor((rect.top+rect.bottom)/2);
            test_driver_internal.action_sequence([{type:'pointer',id:'mouse',actions:[
                {type:'pointerMove',x:0,y:0,origin:target},
                {type:'pointerDown',button:0},
                {type:'pointerMove',x:1,y:0,origin:'pointer'},
                {type:'pointerUp',button:0}
            ]}]).then(()=>completed=true);
        "#).unwrap();
        let mut errors = Vec::new();
        for _ in 0..4 {
            assert_eq!(drive_action_tick(&mut runtime, &mut errors), Some(0));
            runtime.run_jobs().unwrap();
        }
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            runtime
                .eval(
                    r#"completed && JSON.stringify(seen) === JSON.stringify([
            ['mousemove',0,expectedX,expectedY], ['mousedown',1,expectedX,expectedY],
            ['mousemove',1,expectedX+1,expectedY], ['mouseup',0,expectedX+1,expectedY],
            ['click',0,expectedX+1,expectedY]
        ])"#
                )
                .unwrap()
                .as_boolean(),
            Some(true)
        );
    }

    #[test]
    fn send_keys_uses_host_defaults_and_resolves_after_input_ticks() {
        let document = TreeBuilder::parse("<input required><button>Next</button>").document();
        let mut runtime = JsRuntime::with_document(document).unwrap();
        runtime.enable_test_automation().unwrap();
        runtime
            .eval("globalThis.test_driver_internal = {}")
            .unwrap();
        runtime.eval(VENDOR_SCRIPT).unwrap();
        runtime
            .eval(
                r#"
            var input = document.querySelector('input'); var completed = false; var changed = false;
            input.addEventListener('change', () => { changed = input.matches(':user-valid'); });
            test_driver_internal.send_keys(input, 'ab\uE003c\uE004').then(() => completed = true);
            if (completed || input.value !== '') throw Error('send_keys must use the input pump');
        "#,
            )
            .unwrap();
        let mut errors = Vec::new();
        for _ in 0..10 {
            assert_eq!(drive_action_tick(&mut runtime, &mut errors), Some(0));
            runtime.run_jobs().unwrap();
        }
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(runtime.eval("completed && changed && input.value === 'ac' && document.activeElement === document.querySelector('button')").unwrap().as_boolean(), Some(true));
    }

    #[test]
    fn unsupported_sources_reject_without_synthesizing_events() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime.enable_test_automation().unwrap();
        runtime
            .eval("globalThis.test_driver_internal = {}")
            .unwrap();
        runtime.eval(VENDOR_SCRIPT).unwrap();
        runtime.eval("var rejected=false; var seen=0; document.addEventListener('mousedown',()=>seen++); test_driver_internal.action_sequence([{type:'wheel',id:'wheel',actions:[{type:'scroll',x:0,y:0,deltaX:0,deltaY:1}]}]).catch(()=>rejected=true);").unwrap();
        let mut errors = Vec::new();
        assert_eq!(drive_action_tick(&mut runtime, &mut errors), Some(0));
        runtime.run_jobs().unwrap();
        assert_eq!(
            runtime
                .eval("rejected && seen===0 && __wpt_action_commands.length===0")
                .unwrap()
                .as_boolean(),
            Some(true)
        );
    }
}
