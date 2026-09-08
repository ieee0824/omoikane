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
    check(
        &mut runtime(),
        r#"(() => {
        const frame = document.createElement('iframe');
        document.body.appendChild(frame);
        const child = frame.contentWindow;
        const oldDate = child.Date;
        frame.src = 'data:text/html,<html><body>opaque</body></html>';
        let denied = false;
        try { void child.Date; } catch (error) { denied = error.name === 'SecurityError'; }
        frame.remove();
        return denied && child.closed && child.Date === undefined &&
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
