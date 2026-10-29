use super::*;

fn parsed_bootstrap_entries(
    runtime: &JsRuntime,
) -> Vec<Rc<boa_engine::module::ParsedModuleSource>> {
    runtime
        .host_state
        .borrow()
        .parsed_bootstrap_modules
        .values()
        .cloned()
        .collect()
}

#[test]
fn iframe_realms_reuse_the_parsed_bootstrap_module_and_stay_independent() {
    let mut runtime = JsRuntime::with_document(
        crate::html::TreeBuilder::parse("<!doctype html><html><body></body></html>").document(),
    )
    .unwrap();
    // The main Realm and iframe Realms receive different private bindings,
    // so each kind parses its generated source once.
    runtime
        .eval("document.body.appendChild(document.createElement('iframe')).contentWindow.document")
        .unwrap();
    let before = parsed_bootstrap_entries(&runtime);
    let result = runtime
        .eval(
            "(() => {
            const frames = [0, 1, 2].map(() =>
                document.body.appendChild(document.createElement('iframe')));
            frames[0].contentWindow.marker = 'first';
            frames[0].contentDocument.body.append(frames[0].contentDocument.createElement('p'));
            return frames.every(frame => frame.contentWindow.HTMLElement !== HTMLElement) &&
                frames[1].contentWindow.marker === undefined &&
                frames[0].contentDocument.querySelectorAll('p').length === 1 &&
                frames[1].contentDocument.querySelectorAll('p').length === 0 &&
                frames[2].contentWindow.document.createElement('div') instanceof
                    frames[2].contentWindow.HTMLDivElement;
        })()",
        )
        .unwrap();
    assert_eq!(result.as_boolean(), Some(true));
    // Later iframe Realms reused the existing parses instead of adding one.
    let after = parsed_bootstrap_entries(&runtime);
    assert_eq!(after.len(), before.len());
    assert!(
        after
            .iter()
            .all(|parsed| before.iter().any(|old| Rc::ptr_eq(old, parsed)))
    );
}
