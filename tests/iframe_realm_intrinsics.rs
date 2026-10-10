use omoikane::html::TreeBuilder;
use omoikane::js::JsRuntime;

fn runtime() -> JsRuntime {
    let document = TreeBuilder::parse("<html><body></body></html>").document();
    JsRuntime::with_document_and_url(document, "https://example.test/").unwrap()
}

fn check(runtime: &mut JsRuntime, source: &str) {
    let result = runtime
        .eval(source)
        .unwrap_or_else(|error| panic!("iframe check failed: {error}"));
    assert_eq!(result.as_boolean(), Some(true), "{result:?}");
}

#[test]
fn blank_iframe_exposes_its_own_language_constructors_immediately() {
    check(
        &mut runtime(),
        r#"(() => {
        const frame = document.createElement('iframe');
        document.body.appendChild(frame);
        const originalDocument = document;
        const child = frame.contentWindow;
        const date = new child.Date(0);
        const array = new child.Array(1, 2);
        return document === originalDocument && frame.isConnected &&
            child.document !== document && date.toISOString() === '1970-01-01T00:00:00.000Z' &&
            date instanceof child.Date && !(date instanceof Date) &&
            array instanceof child.Array && !(array instanceof Array) &&
            child.Date === child.Date && child.Date !== Date &&
            child.Object !== Object && child.Math !== Math;
    })()"#,
    );
}

#[test]
fn iframe_global_reflection_and_mutation_reach_the_child_realm() {
    check(
        &mut runtime(),
        r#"(() => {
        const frame = document.createElement('iframe');
        document.body.appendChild(frame);
        const child = frame.contentWindow;
        const descriptor = Object.getOwnPropertyDescriptor(child, 'Date');
        if (!descriptor || descriptor.value !== child.Date || descriptor.enumerable) return false;
        if (!('Date' in child) || !Reflect.ownKeys(child).includes('Date')) return false;
        child.marker = 42;
        if (child.Function('return globalThis.marker')() !== 42) return false;
        child.Function('globalThis.fromChild = 17')();
        if (child.fromChild !== 17 || typeof globalThis.fromChild !== 'undefined') return false;
        delete child.marker;
        return child.Function('return typeof globalThis.marker')() === 'undefined';
    })()"#,
    );
}

#[test]
fn iframe_navigation_retargets_constructors_and_preserves_retained_values() {
    let mut runtime = runtime();
    runtime
        .eval(
            r#"
        globalThis.frame = document.createElement('iframe');
        document.body.appendChild(frame);
        globalThis.child = frame.contentWindow;
        globalThis.oldDate = child.Date;
        globalThis.oldValue = new oldDate(123);
        child.marker = 'old';
        frame.srcdoc = '<html><body>new document</body></html>';
    "#,
        )
        .unwrap();
    runtime.run_timers(10, 1, 100);
    check(
        &mut runtime,
        r#"(() => {
        const nextDate = child.Date;
        return child === frame.contentWindow && nextDate !== oldDate &&
            child.marker === undefined && new nextDate(456).getTime() === 456 &&
            oldValue.getTime() === 123 && oldValue instanceof oldDate &&
            !(oldValue instanceof nextDate);
    })()"#,
    );
}

#[test]
fn iframe_intrinsics_stay_unavailable_across_origin_or_after_detach() {
    let mut runtime = runtime();
    runtime
        .eval(
            r#"
        globalThis.frame = document.createElement('iframe');
        document.body.appendChild(frame);
        globalThis.child = frame.contentWindow;
        globalThis.oldDate = child.Date;
        frame.src = 'data:text/html,<html><body>opaque</body></html>';
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    check(
        &mut runtime,
        r#"(() => {
        let denied = false;
        try { void child.Date; } catch (error) { denied = error.name === 'SecurityError'; }
        frame.remove();
        // Discarding the browsing context does not remove its origin boundary.
        let deniedAfterDetach = false;
        try { void child.Date; } catch (error) { deniedAfterDetach = error.name === 'SecurityError'; }
        return denied && deniedAfterDetach && child.closed &&
            new oldDate(123).getTime() === 123;
    })()"#,
    );
}

#[test]
fn iframe_bootstrap_preserves_parent_closures_and_microtask_order() {
    let mut runtime = runtime();
    check(
        &mut runtime,
        r#"(() => {
        const frame = document.createElement('iframe');
        document.body.appendChild(frame);
        const documentBefore = document;
        const childDocument = frame.contentDocument;
        let events = 0;
        childDocument.addEventListener('probe', () => { events++; });
        globalThis.microtaskRan = false;
        Promise.resolve().then(() => { microtaskRan = true; });
        const childDate = frame.contentWindow.Date;
        childDocument.dispatchEvent(new Event('probe'));
        return !microtaskRan && events === 1 && document === documentBefore &&
            frame.isConnected && frame.contentDocument === childDocument &&
            new childDate(0).getTime() === 0;
    })()"#,
    );
    runtime.run_jobs().unwrap();
    check(&mut runtime, "microtaskRan");
}

#[test]
fn cross_origin_fallback_keys_are_undefined_before_and_after_detach() {
    for sandbox in [false, true] {
        let mut runtime = runtime();
        runtime.eval(if sandbox {
            "globalThis.frame = document.createElement('iframe'); frame.setAttribute('sandbox', ''); frame.srcdoc = '<p>opaque</p>'; document.body.appendChild(frame);"
        } else {
            "globalThis.frame = document.createElement('iframe'); frame.src = 'data:text/html,<p>opaque</p>'; document.body.appendChild(frame);"
        }).unwrap();
        runtime.run_until_idle().unwrap();
        check(
            &mut runtime,
            r#"(() => {
            const child = frame.contentWindow;
            const keys = ['then', Symbol.hasInstance, Symbol.isConcatSpreadable, Symbol.toStringTag];
            for (const phase of ['active', 'closed']) {
                if (phase === 'closed') frame.remove();
                for (const key of keys) {
                    if (child[key] !== undefined || !(key in child)) return false;
                    const descriptor = Object.getOwnPropertyDescriptor(child, key);
                    if (!descriptor || descriptor.value !== undefined || descriptor.writable ||
                        descriptor.enumerable || !descriptor.configurable) return false;
                }
                for (const key of ['Date', 'document', Symbol('hasInstance')]) {
                    let denied = false;
                    try { void child[key]; } catch (error) { denied = error.name === 'SecurityError'; }
                    if (!denied) return false;
                    denied = false;
                    try { Object.getOwnPropertyDescriptor(child, key); }
                    catch (error) { denied = error.name === 'SecurityError'; }
                    if (!denied) return false;
                }
            }
            return child.closed;
        })()"#,
        );
    }
}

#[test]
fn cross_origin_fallback_uses_captured_intrinsic_symbols() {
    let mut runtime = runtime();
    runtime.eval("globalThis.frame = document.createElement('iframe'); frame.src = 'data:text/html,<p>opaque</p>'; document.body.appendChild(frame);").unwrap();
    runtime.run_until_idle().unwrap();
    check(
        &mut runtime,
        r#"(() => {
        const child = frame.contentWindow;
        const originalSymbol = Symbol;
        const keys = ['then', Symbol.hasInstance, Symbol.isConcatSpreadable, Symbol.toStringTag];
        let symbolReads = 0;
        globalThis.Symbol = new Proxy(originalSymbol, {
            get() { symbolReads++; throw new Error('author Symbol getter'); }
        });
        try {
            for (const phase of ['active', 'closed']) {
                if (phase === 'closed') frame.remove();
                for (const key of keys) {
                    if (child[key] !== undefined) return false;
                    const descriptor = Object.getOwnPropertyDescriptor(child, key);
                    if (!descriptor || descriptor.value !== undefined) return false;
                }
            }
            return symbolReads === 0 && child.closed;
        } finally {
            globalThis.Symbol = originalSymbol;
        }
    })()"#,
    );
}

#[test]
fn same_origin_fallback_named_properties_reach_author_getters() {
    check(
        &mut runtime(),
        r#"(() => {
        const frame = document.createElement('iframe');
        document.body.appendChild(frame);
        const child = frame.contentWindow;
        const DateConstructor = child.Date;
        const DocumentConstructor = child.Document;
        child.eval(`globalThis.reads = 0;
            for (const key of ['then', Symbol.hasInstance, Symbol.isConcatSpreadable]) {
                Object.defineProperty(globalThis, key, {
                    configurable: true, get() { globalThis.reads++; return 17; }
                });
            }`);
        for (const key of ['then', Symbol.hasInstance, Symbol.isConcatSpreadable]) {
            if (child[key] !== 17) return false;
        }
        return child.reads === 3 && child.document instanceof DocumentConstructor &&
            child.Date === DateConstructor && new DateConstructor(123).getTime() === 123;
    })()"#,
    );
}
