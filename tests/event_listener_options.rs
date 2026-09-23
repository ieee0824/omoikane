use omoikane::{html::TreeBuilder, js::JsRuntime};

fn check(source: &str) {
    let mut runtime = JsRuntime::with_document(
        TreeBuilder::parse("<html><body><button></button></body></html>").document(),
    )
    .expect("create runtime");
    let result = runtime
        .eval(source)
        .unwrap_or_else(|error| panic!("{source}: {error}"));
    assert!(result.to_boolean(), "{source}");
}

#[test]
fn null_listener_options_are_defaulted_across_targets() {
    check(
        r#"(() => {
            const targets = [new EventTarget(), document.querySelector('button'), window,
                new AbortController().signal, new MessageChannel().port1,
                new XMLHttpRequest(), new XMLHttpRequestUpload()];
            return targets.every(target => {
                let calls = 0;
                const listener = () => calls++;
                target.addEventListener('probe', listener, null);
                target.dispatchEvent(new Event('probe'));
                target.removeEventListener('probe', listener, null);
                target.dispatchEvent(new Event('probe'));
                return calls === 1;
            });
        })()"#,
    );
}

#[test]
fn callback_object_is_looked_up_when_the_event_is_dispatched() {
    check(
        r#"(() => {
            const targets = [new EventTarget(), document.querySelector('button'),
                new XMLHttpRequest()];
            return targets.every(target => {
                const listener = {};
                let calls = 0;
                target.addEventListener('probe', listener);
                listener.handleEvent = () => calls++;
                target.dispatchEvent(new Event('probe'));
                target.removeEventListener('probe', listener);
                return calls === 1;
            });
        })()"#,
    );
}

#[test]
fn noncallable_handle_event_reports_an_error_without_stopping_dispatch() {
    check(
        r#"(() => {
            const target = new EventTarget();
            const errors = [];
            const onError = event => errors.push(event.message);
            window.addEventListener('error', onError);
            let reachedNextListener = false;
            target.addEventListener('probe', {});
            target.addEventListener('probe', () => reachedNextListener = true);
            const result = target.dispatchEvent(new Event('probe'));
            window.removeEventListener('error', onError);
            return result && reachedNextListener && errors.length === 1 &&
                errors[0].includes('handleEvent');
        })()"#,
    );
}

#[test]
fn abort_signal_removes_listener_and_aborted_signal_never_registers() {
    check(
        r#"(() => {
            const targets = [new EventTarget(), document.querySelector('button'), window,
                new XMLHttpRequest(), new XMLHttpRequestUpload()];
            return targets.every(target => {
                const controller = new AbortController();
                let calls = 0;
                target.addEventListener('probe', () => calls++, {signal: controller.signal});
                target.dispatchEvent(new Event('probe'));
                controller.abort();
                target.dispatchEvent(new Event('probe'));
                target.addEventListener('probe', () => calls += 10,
                    {signal: AbortSignal.abort()});
                target.dispatchEvent(new Event('probe'));
                const detached = new AbortController();
                const noop = () => {};
                target.addEventListener('probe', noop, {signal: detached.signal});
                target.removeEventListener('probe', noop);
                const noAbortHandler = !(detached.signal._listeners.get('abort') || []).length;
                let rejected = false;
                try { target.addEventListener('probe', () => {}, {signal: 1}); }
                catch (error) { rejected = error instanceof TypeError; }
                return calls === 1 && rejected && noAbortHandler;
            });
        })()"#,
    );
}

#[test]
fn passive_listeners_cannot_cancel_events() {
    check(
        r#"(() => {
            const targets = [new EventTarget(), document.querySelector('button'), window,
                new XMLHttpRequest(), new XMLHttpRequestUpload()];
            return targets.every(target => {
                const listener = event => event.preventDefault();
                target.addEventListener('probe', listener, {passive: true});
                const event = new Event('probe', {cancelable: true});
                const allowed = target.dispatchEvent(event) && !event.defaultPrevented;
                target.removeEventListener('probe', listener);
                return allowed;
            });
        })()"#,
    );
}

#[test]
fn passive_listener_ignores_legacy_return_value_cancellation() {
    check(
        r#"(() => {
            const target = new EventTarget();
            target.addEventListener('probe', event => event.returnValue = false,
                {passive: true});
            const event = new Event('probe', {cancelable: true});
            return target.dispatchEvent(event) && event.returnValue &&
                !event.defaultPrevented;
        })()"#,
    );
}

#[test]
fn explicit_null_signal_is_rejected_even_for_null_listener() {
    check(
        r#"(() => {
            const target = new EventTarget();
            for (const listener of [null, () => {}]) {
                try { target.addEventListener('probe', listener, {signal: null}); }
                catch (error) { if (error instanceof TypeError) continue; }
                return false;
            }
            return true;
        })()"#,
    );
}

#[test]
fn root_wheel_listeners_default_to_passive_but_can_opt_out() {
    check(
        r#"(() => {
            const button = document.querySelector('button');
            const listener = event => event.preventDefault();
            document.addEventListener('wheel', listener);
            const passiveEvent = new Event('wheel', {bubbles: true, cancelable: true});
            const passiveResult = button.dispatchEvent(passiveEvent);
            const passiveInput = __omoikane_dispatch_wheel_input(button.__id,
                {deltaX: 0, deltaY: 1});
            document.removeEventListener('wheel', listener);
            document.addEventListener('wheel', listener, {passive: false});
            const activeEvent = new Event('wheel', {bubbles: true, cancelable: true});
            const activeResult = button.dispatchEvent(activeEvent);
            const activeInput = __omoikane_dispatch_wheel_input(button.__id,
                {deltaX: 0, deltaY: 1});
            return passiveResult && !passiveEvent.defaultPrevented &&
                (passiveInput & 1) === 1 && !activeResult &&
                activeEvent.defaultPrevented && activeInput === 0;
        })()"#,
    );
}

#[test]
fn listener_options_apply_to_every_event_target_family() {
    check(
        r#"(() => {
            const channel = new BroadcastChannel('listener-options-test');
            const host = document.createElement('div');
            const targets = [new EventTarget(), document.querySelector('button'),
                document.createTextNode('text'), document, window,
                host.attachShadow({mode: 'open'}), new XMLHttpRequest(),
                new XMLHttpRequestUpload(), new AbortController().signal,
                new MessageChannel().port1, new FileReader(), channel,
                matchMedia('(min-width: 0px)')];
            const result = targets.every(target => {
                const controller = new AbortController();
                let calls = 0;
                target.addEventListener('probe', event => {
                    calls++;
                    event.preventDefault();
                }, {signal: controller.signal, passive: true});
                const first = new Event('probe', {cancelable: true});
                const passive = target.dispatchEvent(first) && !first.defaultPrevented;
                controller.abort();
                target.dispatchEvent(new Event('probe'));
                let invalidSignal = false;
                try { target.addEventListener('probe', () => {}, {signal: 1}); }
                catch (error) { invalidSignal = error instanceof TypeError; }
                return passive && calls === 1 && invalidSignal;
            });
            channel.close();
            return result;
        })()"#,
    );
}
