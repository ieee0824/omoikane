//! Arrow chains retain the nearest non-arrow receiver without forcing all scopes to escape.
use super::*;

#[test]
fn lexical_this_nested_rest_task_callback_keeps_class_receiver() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime.eval("class TaskOwner{constructor(){this.items=[]}observe(tasks){Object.defineProperty(tasks,'push',{value:(...entries)=>(entries.forEach(entry=>this.processTask(entry)),0)})}processTask(entry){this.items.push(entry)}}globalThis.owner=new TaskOwner;globalThis.tasks=[];owner.observe(tasks)").unwrap();
    assert_eq!(
        runtime
            .eval("tasks.push(17);owner.items[0]")
            .unwrap()
            .as_number(),
        Some(17.0)
    );
    assert_eq!(
        runtime
            .eval("tasks.push(23);owner.items.join(',')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_lossy(),
        "17,23"
    );
}

#[test]
fn lexical_this_multiple_arrows_ignore_call_receivers() {
    let mut runtime = JsRuntime::new().unwrap();
    assert!(runtime.eval("(()=>{class Owner{make(){return ()=>()=>()=>this}}const owner=new Owner;return owner.make().call({}).call({}).call({})===owner})()").unwrap().to_boolean());
}

#[test]
fn lexical_this_default_parameter_arrow_keeps_receiver() {
    let mut runtime = JsRuntime::new().unwrap();
    assert!(runtime.eval("(()=>{class Owner{make(){return (callback=()=>this)=>callback}}const owner=new Owner;return owner.make()()()===owner})()").unwrap().to_boolean());
}

#[test]
fn lexical_this_stops_at_an_ordinary_function_boundary() {
    let mut runtime = JsRuntime::new().unwrap();
    assert!(runtime.eval("(()=>{class Owner{make(){return function normal(){return ()=>()=>this}}}const owner=new Owner;const receiver={};return owner.make().call(receiver)()()===receiver})()").unwrap().to_boolean());
}

#[test]
fn lexical_this_async_outer_arrow_keeps_receiver_after_completion() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime.eval("class Owner{make(){return async()=>()=>this}}globalThis.owner=new Owner;globalThis.retained=false;owner.make()().then(callback=>retained=callback()===owner)").unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.eval("retained").unwrap().to_boolean());
}

#[cfg(feature = "baseline-jit")]
#[test]
fn lexical_this_task_callbacks_match_interpreter_with_jit_entries() {
    const SOURCE: &str = r#"
        class TaskOwner {
            constructor() { this.total = 0; }
            observe(tasks) {
                Object.defineProperty(tasks, 'push', {
                    value: (...entries) => (entries.forEach(entry => this.processTask(entry)), 0)
                });
            }
            processTask(entry) { this.total += entry; }
        }
        const owner = new TaskOwner;
        const tasks = [];
        owner.observe(tasks);
        function dispatch(entry) { return tasks.push(entry); }
        for (let i = 0; i < 80; i++) dispatch(i);
        owner.total
    "#;
    for enabled in [false, true] {
        let mut runtime = JsRuntime::new().unwrap();
        runtime.set_baseline_jit_enabled(enabled);
        assert_eq!(runtime.eval(SOURCE).unwrap().as_number(), Some(3160.0));
        if enabled {
            assert!(runtime.baseline_jit_diagnostics().runtime_helper_entries > 0);
        }
    }
}
