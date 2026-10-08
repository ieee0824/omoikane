//! Platform event payload identity, dictionary conversion and native dispatch.
use omoikane::html::TreeBuilder;
use omoikane::js::JsRuntime;

fn check(script: &str) {
    let mut runtime = JsRuntime::with_document(
        TreeBuilder::parse("<!doctype html><html><body></body></html>").document(),
    )
    .unwrap();
    assert_eq!(runtime.eval(script).unwrap().as_boolean(), Some(true));
}

#[test]
fn window_handlers_are_global_attributes_and_dom_nodes_are_event_targets() {
    check(
        r#"(() => {
        if (!(document instanceof EventTarget) || !(document.body instanceof EventTarget)) return false;
        if ('ontouchstart' in Window.prototype) return false;
        const descriptor = Object.getOwnPropertyDescriptor(window, 'ontouchstart');
        if (!descriptor || !descriptor.enumerable || !descriptor.configurable || descriptor.get.call(undefined) !== null) return false;
        const callback = () => {};
        window.ontouchstart = callback;
        if (window.ontouchstart !== callback) return false;
        window.ontouchstart = null;
        return window.ontouchstart === null &&
            Object.getOwnPropertyDescriptor(window, 'TouchEvent').enumerable === false;
    })()"#,
    );
}

#[test]
fn navigation_and_form_events_keep_platform_payloads_and_required_arguments() {
    check(
        r#"(() => {
        const state = { value: 5 }, pop = new PopStateEvent('popstate', { state, hasUAVisualTransition: true });
        if (!(pop instanceof Event) || pop.state !== state || !pop.hasUAVisualTransition) return false;
        pop.state = null;
        if (pop.state !== state || new PopStateEvent('popstate').state !== null) return false;
        const hash = new HashChangeEvent('hashchange', { oldURL: '\ud800', newURL: '/next' });
        if (hash.oldURL !== '\ufffd' || hash.newURL !== '/next' || hash instanceof PopStateEvent) return false;
        if (!new PageTransitionEvent('pageshow', { persisted: true }).persisted) return false;
        const button = document.createElement('button');
        const submit = new SubmitEvent('submit', { submitter: button });
        if (submit.submitter !== button || new SubmitEvent('submit').submitter !== null) return false;
        const data = new FormData(), form = new FormDataEvent('formdata', { formData: data });
        if (form.formData !== data || !new ContentVisibilityAutoStateChangeEvent('change', { skipped: true }).skipped) return false;
        for (const create of [() => new PopStateEvent(), () => new HashChangeEvent(Symbol()),
            () => new FormDataEvent('formdata'), () => new FormDataEvent('formdata', {}),
            () => new FormDataEvent('formdata', { formData: Object.create(FormData.prototype) }),
            () => new SubmitEvent('submit', { submitter: Object.create(HTMLElement.prototype) }),
            () => new BeforeUnloadEvent('beforeunload')]) {
            try { create(); return false; } catch (error) { if (!(error instanceof TypeError)) return false; }
        }
        const getter = Object.getOwnPropertyDescriptor(PopStateEvent.prototype, 'state').get;
        try { getter.call(hash); return false; } catch (error) { return error instanceof TypeError; }
    })()"#,
    );
}

#[test]
fn event_dictionary_interfaces_use_private_brands_across_realms() {
    check(
        r#"(() => {
        const frame = document.createElement('iframe');
        document.body.append(frame);
        const child = frame.contentWindow;
        const childTarget = child.Function('return new EventTarget()')();
        const childButton = child.Function('return document.createElement("button")')();
        const childWindow = child.Function('return window')();
        const target = new EventTarget(), button = document.createElement('button');
        for (const value of [target, button, document, window, child, childWindow,
            childTarget, childButton, child.document]) {
            if (new Touch({identifier: 1, target: value}).target !== value) return false;
        }
        if (new SubmitEvent('submit', {submitter: childButton}).submitter !== childButton) return false;
        window.parentBrandTarget = target;
        window.parentBrandButton = button;
        if (!child.Function(`
            return new Touch({identifier: 2, target: parent.parentBrandTarget}).target === parent.parentBrandTarget &&
                new SubmitEvent('submit', {submitter: parent.parentBrandButton}).submitter === parent.parentBrandButton;
        `)()) return false;
        for (const value of [Object.create(EventTarget.prototype),
            child.Function('return Object.create(EventTarget.prototype)')(), {}, new Proxy(target, {})]) {
            try { new Touch({identifier: 1, target: value}); return false; }
            catch (error) { if (!(error instanceof TypeError)) throw error; }
        }
        for (const value of [Object.create(HTMLElement.prototype),
            child.Function('return Object.create(HTMLElement.prototype)')(), target,
            document.createElementNS('http://www.w3.org/2000/svg', 'svg')]) {
            try { new SubmitEvent('submit', {submitter: value}); return false; }
            catch (error) { if (!(error instanceof TypeError)) throw error; }
        }
        Object.setPrototypeOf(childTarget, null);
        Object.setPrototypeOf(childButton, null);
        window.EventTarget = window.HTMLElement = function() { throw new Error('Public constructor'); };
        return new Touch({identifier: 3, target: childTarget}).target === childTarget &&
            new SubmitEvent('submit', {submitter: childButton}).submitter === childButton;
    })()"#,
    );
}

#[test]
fn iframe_native_navigation_events_use_the_child_interface_realm() {
    let mut runtime = JsRuntime::with_document(
        TreeBuilder::parse("<!doctype html><html><body></body></html>").document(),
    )
    .unwrap();
    runtime
        .eval(
            r#"
        const frame = document.createElement('iframe'); document.body.append(frame);
        const child = frame.contentWindow;
        child.Function(`
            const Pop = PopStateEvent, Hash = HashChangeEvent;
            onpopstate = event => { parent.childPopTyped = event instanceof Pop && event.state.n === 1; };
            onhashchange = event => { parent.childHashTyped = event instanceof Hash && event.oldURL === 'about:blank' && event.newURL === 'about:blank#fragment'; };
            PopStateEvent = HashChangeEvent = function() { throw new Error('Public constructor replaced'); };
        `)();
    "#,
        )
        .unwrap();
    boa_gc::force_collect();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
        child.history.pushState({n: 1}, '');
        child.history.pushState({n: 2}, '');
        child.history.back();
        child.location.hash = '#fragment';
        return window.childPopTyped === true && window.childHashTyped === true;
    })()"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn native_navigation_and_load_use_captured_platform_constructors() {
    let mut runtime = JsRuntime::with_document_and_url(
        TreeBuilder::parse("<!doctype html><html><body></body></html>").document(),
        "https://events.example.test/start",
    )
    .unwrap();
    runtime
        .eval(
            r#"
        const Pop = PopStateEvent, Hash = HashChangeEvent, Page = PageTransitionEvent;
        globalThis.records = [];
        onpopstate = event => records.push(event instanceof Pop && event.state === null);
        onhashchange = event => records.push(event instanceof Hash &&
            event.oldURL === 'https://events.example.test/start' && event.newURL === 'https://events.example.test/start#next');
        onpageshow = event => records.push(event instanceof Page && event.persisted === false);
        PopStateEvent = HashChangeEvent = PageTransitionEvent = function() { throw new Error('Public constructor replaced'); };
        __omoikane_commit_same_document_navigation('https://events.example.test/start#next');
    "#,
        )
        .unwrap();
    runtime.fire_load().unwrap();
    assert_eq!(
        runtime
            .eval("records.length === 3 && records.every(Boolean)")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn event_dictionaries_convert_parent_then_lexicographic_members_once() {
    check(
        r#"(() => {
        const reads = [], init = new Proxy({}, { get(_, key) {
            reads.push(key);
            if (key === 'newURL' || key === 'oldURL') return { toString() { reads.push('convert:' + key); return key; } };
            return undefined;
        } });
        const hash = new HashChangeEvent('hashchange', init);
        return hash.newURL === 'newURL' && reads.join() ===
            'bubbles,cancelable,composed,newURL,convert:newURL,oldURL,convert:oldURL';
    })()"#,
    );
}

#[test]
fn touch_snapshots_validate_target_and_convert_numeric_and_enum_members() {
    check(
        r#"(() => {
        const target = document.body;
        const touch = new Touch({ identifier: 4294967295, target, clientX: '12.5', force: 0.1, touchType: 'stylus' });
        if (touch.identifier !== -1 || touch.target !== target || touch.clientX !== 12.5 ||
            touch.force !== Math.fround(0.1) || touch.touchType !== 'stylus' || touch.pageX !== 0) return false;
        touch.clientX = 3;
        if (touch.clientX !== 12.5 || Object.prototype.toString.call(touch) !== '[object Touch]') return false;
        for (const init of [{ target }, { identifier: 1 }, { identifier: 1, target: {} },
            { identifier: 1, target, clientX: Infinity }, { identifier: 1, target, force: 1e100 },
            { identifier: 1, target, touchType: 'unknown' }]) {
            try { new Touch(init); return false; } catch (error) { if (!(error instanceof TypeError)) return false; }
        }
        return new Touch({ identifier: 0, target: window }).target === window;
    })()"#,
    );
}

#[test]
fn request_submit_dispatches_submit_event_with_actual_submitter() {
    check(
        r#"(() => {
        const form = document.createElement('form'), button = document.createElement('button');
        form.append(button); document.body.append(form);
        let dispatched = null;
        form.addEventListener('submit', event => { dispatched = event; event.preventDefault(); });
        form.requestSubmit(button);
        return dispatched instanceof SubmitEvent && dispatched.submitter === button &&
            dispatched.bubbles && dispatched.cancelable && dispatched.defaultPrevented;
    })()"#,
    );
}

#[test]
fn native_resize_and_viewport_scroll_invoke_handlers_once_in_listener_order() {
    check(
        r#"(() => {
        const calls = [], slots = [];
        window.addEventListener('resize', () => calls.push('resize-listener'));
        window.onresize = event => { calls.push('resize-handler'); slots.push(window.event === event); };
        document.addEventListener('scroll', () => calls.push('document-listener'));
        window.addEventListener('scroll', () => calls.push('scroll-listener'));
        window.onscroll = event => { calls.push('scroll-handler'); slots.push(window.event === event); };
        __omoikane_dispatch_viewport_events(true, false, false);
        __omoikane_dispatch_scroll_event(document.__id, true);
        return calls.join() === 'resize-listener,resize-handler,document-listener,scroll-listener,scroll-handler' && slots.length === 2 && slots.every(Boolean) && window.event === undefined;
    })()"#,
    );
}

#[test]
fn nonmixin_event_handlers_set_and_restore_the_current_window_event() {
    let mut runtime = JsRuntime::with_document(
        TreeBuilder::parse("<!doctype html><html><body></body></html>").document(),
    )
    .unwrap();
    runtime
        .eval(
            r#"
        globalThis.handlerSlots = [];
        const record = event => handlerSlots.push(window.event === event);
        const request = new XMLHttpRequest();
        request.onload = record;
        request.upload.onload = record;
        request.dispatchEvent(new Event('load'));
        request.upload.dispatchEvent(new Event('load'));
        const controller = new AbortController();
        controller.signal.onabort = record;
        controller.abort();
        globalThis.reader = new FileReader();
        reader.onload = record;
        reader.readAsText(new Blob(['text']));
    "#,
        )
        .unwrap();
    runtime.tick(0).unwrap();
    assert_eq!(
        runtime
            .eval("handlerSlots.length === 4 && handlerSlots.every(Boolean) && reader.result === 'text' && window.event === undefined")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn handler_replacement_preserves_listener_order_and_cancellation() {
    check(
        r#"(() => {
        const target = document.createElement('button'), calls = [];
        const first = () => calls.push('old');
        target.onclick = first;
        target.addEventListener('click', () => calls.push('listener'));
        const next = () => { calls.push('new'); return false; };
        target.onclick = next;
        if (target.onclick !== next || target.dispatchEvent(new Event('click', { cancelable: true })) ||
            calls.join() !== 'new,listener') return false;
        calls.length = 0;
        target.onclick = null;
        target.dispatchEvent(new Event('click'));
        if (calls.join() !== 'listener' || target.onclick !== null) return false;
        for (const name of ['onpopstate','onhashchange','onbeforeunload','onresize','onscroll','onscrollend',
            'onformdata','onanimationend','onpointerdown','ontouchstart','onselectionchange']) {
            if (!(name in window) || window[name] !== null) return false;
        }
        return Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'onclick').enumerable;
    })()"#,
    );
}

#[test]
fn body_frameset_content_and_idl_handlers_share_window_slot() {
    check(
        r#"(() => {
        for (const element of [document.body, document.createElement('frameset')]) {
            if (element.localName === 'frameset' && !(element instanceof HTMLFrameSetElement)) return false;
            element.setAttribute('onpopstate', 'globalThis.handledState = event.state');
            if (typeof element.onpopstate !== 'function' || element.onpopstate !== window.onpopstate) return false;
            window.dispatchEvent(new PopStateEvent('popstate', { state: 7 }));
            if (globalThis.handledState !== 7) return false;
            const replacement = () => {};
            element.onpopstate = replacement;
            if (window.onpopstate !== replacement) return false;
            element.removeAttribute('onpopstate');
            if (window.onpopstate !== null || element.onpopstate !== null) return false;
        }
        return true;
    })()"#,
    );
}

#[test]
fn repeated_inline_initialization_preserves_idl_assignments() {
    check(
        r#"(() => {
        const body = document.body;
        body.setAttribute('onload', 'globalThis.inlineCalls = (globalThis.inlineCalls || 0) + 1');
        let scriptCalls = 0;
        window.onload = () => { scriptCalls++; };
        __omoikane_wire_inline_handlers();
        window.dispatchEvent(new Event('load'));
        if (scriptCalls !== 1 || globalThis.inlineCalls !== undefined) return false;
        body.setAttribute('onload', body.getAttribute('onload'));
        window.dispatchEvent(new Event('load'));
        return scriptCalls === 1 && globalThis.inlineCalls === 1;
    })()"#,
    );
}

#[test]
fn window_event_is_set_before_object_lookup_and_restored_after_nested_throw() {
    check(
        r#"(() => {
        const target = new EventTarget(), outer = new Event('outer'), nested = new Event('nested');
        const observed = [];
        target.addEventListener('outer', { get handleEvent() {
            observed.push(window.event === outer);
            return () => {
                window.dispatchEvent(nested);
                observed.push(window.event === outer);
            };
        } });
        window.addEventListener('nested', () => { observed.push(window.event === nested); throw new Error('expected'); });
        target.dispatchEvent(outer);
        return observed.length === 3 && observed.every(Boolean) && window.event === undefined;
    })()"#,
    );
}

#[test]
fn formdata_event_exposes_live_entries_and_guards_recursive_construction() {
    check(
        r#"(() => {
        const form = document.createElement('form'), input = document.createElement('input');
        input.name = 'name'; input.value = 'before'; form.append(input); document.body.append(form);
        let eventData, recursive = false, bubbled = false;
        document.addEventListener('formdata', event => { bubbled = event.bubbles && event instanceof FormDataEvent; });
        form.onformdata = event => {
            eventData = event.formData;
            event.formData.append('added', 'yes');
            try { new FormData(form); } catch (error) { recursive = error.name === 'InvalidStateError'; }
        };
        const data = new FormData(form);
        return eventData === data && data.get('name') === 'before' && data.get('added') === 'yes' && recursive && bubbled;
    })()"#,
    );
}

#[test]
fn touch_event_lists_are_snapshots_and_native_input_uses_touch_instances() {
    check(
        r#"(() => {
        const target = document.body, touch = new Touch({ identifier: 3, target, clientX: 9 });
        const source = [touch], event = new TouchEvent('touchstart', { touches: source, changedTouches: source, ctrlKey: true });
        source.length = 0;
        if (!(event instanceof UIEvent) || !(event.touches instanceof TouchList) || event.touches.length !== 1 ||
            event.touches[0] !== touch || event.touches.item(0) !== touch || event.touches.item(1) !== null ||
            !event.getModifierState('Control') || event.targetTouches.length !== 0) return false;
        event.touches[0] = null;
        if (event.touches[0] !== touch) return false;
        if (Reflect.deleteProperty(event.touches, '0') || event.touches[0] !== touch || event.touches.item(0) !== touch) return false;
        try { event.getModifierState(Symbol()); return false; }
        catch (error) { if (!(error instanceof TypeError)) return false; }
        let received;
        target.addEventListener('touchstart', value => received = value);
        __omoikane_dispatch_touch_input(target.__id, 'touchstart', {
            touches: [{ identifier: 4, clientX: 12 }], changedTouches: [{ identifier: 4, clientX: 12 }],
        });
        return received instanceof TouchEvent && received.touches[0] instanceof Touch &&
            received.touches[0].target === target && received.touches[0].identifier === 4;
    })()"#,
    );
}

#[test]
fn motion_events_have_readonly_nullable_numbers_and_nested_interface_instances() {
    check(
        r#"(() => {
        const orientation = new DeviceOrientationEvent('deviceorientation', { alpha: '12', beta: null, absolute: true });
        if (orientation.alpha !== 12 || orientation.beta !== null || !orientation.absolute) return false;
        orientation.alpha = 2;
        if (orientation.alpha !== 12 || typeof DeviceOrientationEvent.requestPermission !== 'function') return false;
        const event = new DeviceMotionEvent('devicemotion', { acceleration: { x: '3' }, rotationRate: { gamma: 4 }, interval: null });
        if (!(event.acceleration instanceof DeviceMotionEventAcceleration) || event.acceleration.x !== 3 ||
            event.acceleration.y !== null || event.accelerationIncludingGravity !== null ||
            !(event.rotationRate instanceof DeviceMotionEventRotationRate) || event.rotationRate.gamma !== 4 || event.interval !== 0) return false;
        try { new DeviceOrientationEvent('event', { alpha: NaN }); return false; }
        catch (error) { return error instanceof TypeError; }
    })()"#,
    );
}

#[test]
fn window_event_belongs_to_the_callback_realm() {
    check(
        r#"(() => {
        const frame = document.createElement('iframe');
        document.body.append(frame);
        const child = frame.contentWindow;
        const callback = child.Function('event', `
            parent.callbackEventMatches = window.event === event;
            parent.dispatcherEventIsUnset = parent.event === undefined;
        `);
        for (const listener of [callback, callback.bind(null)]) {
            window.addEventListener('callback-realm', listener);
            window.dispatchEvent(new Event('callback-realm'));
            window.removeEventListener('callback-realm', listener);
            if (!window.callbackEventMatches || !window.dispatcherEventIsUnset ||
                window.event !== undefined || child.event !== undefined) return false;
        }
        return true;
    })()"#,
    );
}

#[test]
fn iframe_handler_attributes_forward_to_the_active_window() {
    check(
        r#"(() => {
        const frame = document.createElement('iframe');
        document.body.append(frame);
        const child = frame.contentWindow;
        for (const name of ['onclick', 'onload', 'onpopstate', 'onscroll', 'ondeviceorientation']) {
            if (child[name] !== null) return false;
            const callback = () => {};
            child[name] = callback;
            if (child[name] !== callback || child.Function('name', 'return globalThis[name]')(name) !== callback) return false;
            child.Function('name', 'globalThis[name] = null')(name);
            if (child[name] !== null || window[name] !== null) return false;
            try { Reflect.get(child, name, {}); return false; }
            catch (error) { if (error.name !== 'TypeError') return false; }
        }
        return true;
    })()"#,
    );
}

#[test]
fn borrowed_window_event_accessors_use_the_receiver_window() {
    check(
        r#"(() => {
        const frame = document.createElement('iframe'), sibling = document.createElement('iframe');
        document.body.append(frame, sibling);
        const child = frame.contentWindow, other = sibling.contentWindow;
        const own = Object.getOwnPropertyDescriptor(window, 'event');
        const childGet = child.Function("return Object.getOwnPropertyDescriptor(globalThis, 'event').get")();
        let parentMatches = false;
        window.borrowedParentGet = own.get;
        window.childProxy = child;
        window.siblingProxy = other;
        window.addEventListener('borrowed', event => {
            parentMatches = childGet.call(window) === event && childGet.call(undefined) === undefined;
        });
        window.addEventListener('borrowed', child.Function('event', `
            parent.borrowedMatches = parent.borrowedParentGet.call(globalThis) === event &&
                parent.borrowedParentGet.call(parent.childProxy) === event &&
                parent.borrowedParentGet.call(parent.siblingProxy) === undefined;
        `));
        window.dispatchEvent(new Event('borrowed'));
        if (!parentMatches || !window.borrowedMatches || own.get.call(undefined) !== undefined) return false;
        for (const receiver of [{}, Object.create(Window.prototype), new EventTarget(), 1]) {
            for (const method of [own.get, own.set]) {
                try { method.call(receiver, 1); return false; }
                catch (error) { if (!(error instanceof TypeError)) return false; }
            }
        }
        const popup = window.open('about:blank');
        if (own.get.call(popup) !== undefined) return false;
        own.set.call(popup, 17);
        const popupDescriptor = Object.getOwnPropertyDescriptor(popup, 'event');
        if (popup.event !== 17 || popupDescriptor.value !== 17 ||
            !popupDescriptor.writable || !popupDescriptor.configurable || !popupDescriptor.enumerable) return false;
        if (!Reflect.ownKeys(popup).includes('event')) return false;
        delete popup.event;
        if (popup.event !== undefined || Object.getOwnPropertyDescriptor(popup, 'event') !== undefined) return false;
        popup.close();
        own.set.call(child, 23);
        if (child.event !== 23 || window.event !== undefined) return false;
        delete child.event;
        const descriptor = child.Function("return Object.getOwnPropertyDescriptor(globalThis, 'event')")();
        return descriptor === undefined;
    })()"#,
    );
}

#[test]
fn native_touch_input_supplies_missing_contact_identifiers() {
    check(
        r#"(() => {
        const element = document.createElement('div');
        document.body.append(element);
        let event;
        element.ontouchstart = value => { event = value; };
        __omoikane_dispatch_touch_input(element.__id, 'touchstart', {
            touches: [{}, {}, {identifier: 7}], changedTouches: [{}]
        });
        return event instanceof TouchEvent && event.touches.length === 3 &&
            [...event.touches].every(touch => touch instanceof Touch && touch.target === element) &&
            [...event.touches].map(touch => touch.identifier).join() === '0,1,7' &&
            event.changedTouches[0].identifier === 0;
    })()"#,
    );
}

#[test]
fn window_event_proxy_registry_survives_collection_and_checks_navigation_origin() {
    let mut runtime =
        JsRuntime::with_document(TreeBuilder::parse("<html><body></body></html>").document())
            .unwrap();
    runtime
        .eval(
            r#"
        globalThis.savedFrame = document.createElement('iframe');
        document.body.append(savedFrame);
        globalThis.savedProxy = savedFrame.contentWindow;
        globalThis.savedGet = Object.getOwnPropertyDescriptor(window, 'event').get;
    "#,
        )
        .unwrap();
    boa_gc::force_collect();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
        const original = WeakMap.prototype.get;
        WeakMap.prototype.get = () => { throw new Error('author method must not run'); };
        try { if (savedGet.call(savedProxy) !== undefined) return false; }
        finally { WeakMap.prototype.get = original; }
        savedFrame.srcdoc = '<html><body>next document</body></html>';
        if (savedGet.call(savedProxy) !== undefined) return false;
        savedFrame.setAttribute('sandbox', 'allow-scripts');
        savedFrame.srcdoc = '<html><body>opaque document</body></html>';
        try { savedGet.call(savedProxy); return false; }
        catch (error) { return error.name === 'SecurityError'; }
    })()"#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn intrinsic_object_listeners_use_their_creation_realm() {
    check(
        r#"(() => {
        const frame = document.createElement('iframe');
        document.body.append(frame);
        const child = frame.contentWindow;
        for (const name of ['Object.prototype', 'Array.prototype', 'Map.prototype', 'Math', 'JSON', 'Reflect']) {
            const listener = child.Function(`
                const listener = ${name};
                Object.defineProperty(listener, 'handleEvent', {configurable:true, get() {
                    parent.intrinsicGetterMatches = window.event === parent.intrinsicEvent;
                    return event => { parent.intrinsicCallbackMatches = window.event === event; };
                }});
                return listener;
            `)();
            window.intrinsicGetterMatches = window.intrinsicCallbackMatches = false;
            window.addEventListener('intrinsic', listener);
            window.intrinsicEvent = new Event('intrinsic');
            window.dispatchEvent(intrinsicEvent);
            window.removeEventListener('intrinsic', listener);
            delete listener.handleEvent;
            if (!intrinsicGetterMatches || !intrinsicCallbackMatches || child.event !== undefined) return false;
        }
        return true;
    })()"#,
    );
}

#[test]
fn object_listener_event_uses_creation_realm_before_getter_lookup() {
    check(
        r#"(() => {
        const frame = document.createElement('iframe');
        document.body.append(frame);
        const child = frame.contentWindow;
        for (const factory of [
            '({})', 'Object.create(null)', 'Object.create(parent.Object.prototype)',
            '[]', 'new (class Listener {})()', 'new Proxy({}, {})',
            'new Map()', 'new Set()', 'new Date()', 'new RegExp("a")',
            'new ArrayBuffer(8)', 'new Number(1)',
        ]) {
            const listener = child.Function(`
                const listener = ${factory};
                Object.defineProperty(listener, 'handleEvent', {get() {
                    parent.objectLookupMatches = window.event === parent.activeEvent;
                    return event => {
                        parent.objectCallbackMatches = window.event === event;
                        parent.objectParentUnset = parent.event === undefined;
                    };
                }});
                return listener;
            `)();
            Object.setPrototypeOf(listener, Object.prototype);
            const event = window.activeEvent = new Event('object-realm');
            window.addEventListener('object-realm', listener);
            window.dispatchEvent(event);
            window.removeEventListener('object-realm', listener);
            if (!window.objectLookupMatches || !window.objectCallbackMatches ||
                !window.objectParentUnset || window.event !== undefined || child.event !== undefined) {
                throw new Error(factory + ': ' + JSON.stringify({
                    lookup: window.objectLookupMatches, callback: window.objectCallbackMatches,
                    parentUnset: window.objectParentUnset,
                    restored: window.event === undefined && child.event === undefined,
                }));
            }
        }
        return true;
    })()"#,
    );
}

#[test]
fn generated_handler_inventory_exposes_and_dispatches_every_attribute() {
    let inventory: serde_json::Value =
        serde_json::from_str(include_str!("../src/js/event_handler_inventory.json")).unwrap();
    let groups = serde_json::json!({
        "global": inventory["global"],
        "window": inventory["window"],
        "secureWindow": inventory["secureWindow"],
    });
    check(&format!(
        r#"(() => {{
        const inventory = {groups};
        const element = document.createElement('div');
        const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
        for (const target of [window, document, element, svg]) {{
            for (const type of inventory.global) {{
                const name = 'on' + type;
                if (!(name in target) || target[name] !== null) return false;
                let received;
                const callback = event => {{ received = event; }};
                target[name] = callback;
                const event = new Event(type);
                target.dispatchEvent(event);
                if (target[name] !== callback || received !== event) return false;
                target[name] = null;
                received = undefined;
                target.dispatchEvent(new Event(type));
                if (received !== undefined || target[name] !== null) return false;
            }}
        }}
        for (const type of inventory.window) {{
            const name = 'on' + type;
            if (!(name in window) || name in document || name in element) return false;
            const body = document.createElement('body'), frameset = document.createElement('frameset');
            const callback = () => {{}};
            body[name] = callback;
            if (window[name] !== callback || frameset[name] !== callback) return false;
            frameset[name] = null;
            if (window[name] !== null || body[name] !== null) return false;
        }}
        return inventory.global.length > 100 && inventory.window.length >= 20 &&
            inventory.secureWindow.length === 3;
    }})()"#,
    ));
}

#[test]
fn secure_sensor_handlers_use_window_listener_slots() {
    check(
        r#"(() => {
        for (const type of ['deviceorientation', 'deviceorientationabsolute', 'devicemotion']) {
            const name = 'on' + type;
            if (!(name in window) || window[name] !== null || name in document.body) return false;
            const calls = [];
            window[name] = event => calls.push(event);
            const event = new Event(type);
            window.dispatchEvent(event);
            if (calls.length !== 1 || calls[0] !== event) return false;
            window[name] = null;
            window.dispatchEvent(new Event(type));
            if (calls.length !== 1) return false;
        }
        return true;
    })()"#,
    );
}

#[test]
fn motion_interfaces_are_not_exposed_to_insecure_documents() {
    let mut runtime = JsRuntime::with_document_and_url(
        TreeBuilder::parse("<html><body></body></html>").document(),
        "http://example.test/",
    )
    .unwrap();
    assert_eq!(
        runtime
            .eval("typeof DeviceMotionEvent === 'undefined' && typeof DeviceOrientationEvent === 'undefined' && !('ondeviceorientation' in window) && !('ondeviceorientationabsolute' in window) && !('ondevicemotion' in window)")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}
