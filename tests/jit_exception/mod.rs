//! Gate 4-5: generated exception handling through Omoikane's DOM embedding.
#![cfg(feature = "baseline-jit")]

use omoikane::{html::TreeBuilder, js::JsRuntime};

fn runtime(enabled: bool) -> JsRuntime {
    let document =
        TreeBuilder::parse("<!doctype html><html><head></head><body></body></html>").document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    runtime.set_baseline_jit_enabled(enabled);
    runtime
}

#[test]
fn nested_dom_exception_finally_rethrow_and_opaque_identity_match_interpreter() {
    const SOURCE: &str = r#"
        let payload = document.createElement('span');
        let log = [];
        function inner(fail) {
            try {
                if (fail) throw payload;
                return document.createElement('div').nodeName;
            } finally { if (fail) log.push('inner'); }
        }
        function outer(fail) {
            try { return inner(fail); }
            catch(e) { if(e!==payload) throw 'identity'; log.push(e.nodeName); throw e; }
            finally { if(fail) log.push('outer'); }
        }
        for(let i=0;i<40;i++)outer(false);
        try { outer(true); } catch(e) { log.push(e===payload); }
        JSON.stringify(log)
    "#;
    let mut off = runtime(false);
    let expected = off.eval(SOURCE).unwrap();
    let mut on = runtime(true);
    let before = on.baseline_jit_diagnostics();
    boa_gc::force_collect();
    let actual = on.eval(SOURCE).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(
        actual.as_string().unwrap().to_std_string_escaped(),
        r#"["inner","SPAN","outer",true]"#
    );
    let diagnostics = on.baseline_jit_diagnostics();
    if cfg!(all(
        target_arch = "x86_64",
        any(target_os = "linux", target_os = "macos")
    )) {
        assert!(diagnostics.runtime_helper_entries > before.runtime_helper_entries);
        assert!(diagnostics.exception_unwinds > before.exception_unwinds);
        assert!(diagnostics.exception_handler_entries > before.exception_handler_entries);
    } else {
        assert_eq!(diagnostics.runtime_helper_entries, 0);
    }
    assert_eq!(on.eval("21*2").unwrap().as_number(), Some(42.0));
}

#[test]
fn dom_api_error_and_host_error_match_with_jit_on_and_off() {
    const SOURCE: &str = r#"
        function f(fail) {
            let log = '';
            try {
                let node = document.createElement('div');
                if(fail) node.appendChild(node);
                log += node.nodeName;
            } catch(e) { log += e.name; }
            finally { log += ':finally'; }
            return log;
        }
        for(let i=0;i<40;i++)f(false);
        f(true)
    "#;
    let mut off = runtime(false);
    let mut on = runtime(true);
    let expected = off.eval(SOURCE).unwrap();
    assert_eq!(
        expected.as_string().unwrap().to_std_string_escaped(),
        "HierarchyRequestError:finally"
    );
    let before = on.baseline_jit_diagnostics();
    assert_eq!(on.eval(SOURCE).unwrap(), expected);
    let host_error = "function fail(x){if(x)throw new TypeError('boundary');return 1}for(let i=0;i<40;i++)fail(false);fail(true)";
    assert_eq!(
        on.eval(host_error).unwrap_err().to_string(),
        off.eval(host_error).unwrap_err().to_string(),
    );
    if cfg!(all(
        target_arch = "x86_64",
        any(target_os = "linux", target_os = "macos")
    )) {
        assert!(on.baseline_jit_diagnostics().exception_unwinds > before.exception_unwinds);
    }
    boa_gc::force_minor_collect();
    assert_eq!(
        on.eval("document.body.nodeName")
            .unwrap()
            .display()
            .to_string(),
        "\"BODY\""
    );
}
