//! WPT automation through the page runtime's host input path.
use omoikane::js::JsRuntime;

pub(super) const VENDOR_SCRIPT: &str = include_str!("testdriver_vendor.js");

/// Executes one WebDriver tick without manufacturing page selection state.
/// The caller advances the virtual clock and rendering after the input tick.
pub(super) fn drive_action_tick(runtime: &mut JsRuntime, errors: &mut Vec<String>) -> Option<u64> {
    let pending = runtime
        .eval("Array.isArray(globalThis.__wpt_action_commands) && __wpt_action_commands.length > 0")
        .ok()
        .and_then(|value| value.as_boolean())
        .unwrap_or(false);
    if !pending {
        return None;
    }
    match runtime.eval("__wpt_advance_action_tick()") {
        Ok(value) => value.as_number().map(|duration| duration as u64),
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

    #[test]
    fn mouse_actions_hit_test_and_dispatch_with_pressed_button_state() {
        let document = TreeBuilder::parse(
            "<!doctype html><style>#target {display:block;width:100px;height:40px}</style><button id=target>tap</button>",
        ).document();
        let mut runtime = JsRuntime::with_document(document).unwrap();
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
    fn unsupported_sources_reject_without_synthesizing_events() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval("globalThis.test_driver_internal = {}")
            .unwrap();
        runtime.eval(VENDOR_SCRIPT).unwrap();
        runtime.eval("var rejected=false; var seen=0; document.addEventListener('mousedown',()=>seen++); test_driver_internal.action_sequence([{type:'key',id:'keyboard',actions:[{type:'keyDown',value:'a'}]}]).catch(()=>rejected=true);").unwrap();
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
