//! Public DOM interface identity, live collections and static boundary snapshots.
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
fn lifecycle_events_use_intrinsic_event_after_public_constructor_is_deleted() {
    let mut runtime = JsRuntime::with_document(
        TreeBuilder::parse("<!doctype html><html><body></body></html>").document(),
    )
    .unwrap();
    runtime
        .eval(
            r#"globalThis.lifecycleEvents = [];
            document.addEventListener('DOMContentLoaded', event => lifecycleEvents.push(event.type));
            window.addEventListener('load', event => lifecycleEvents.push(event.type));
            delete globalThis.Event;"#,
        )
        .unwrap();
    assert!(runtime.execute_document_scripts(None).is_empty());
    runtime.fire_load().unwrap();
    assert_eq!(
        runtime
            .eval(
                "typeof Event === 'undefined' && lifecycleEvents.join() === 'DOMContentLoaded,load'"
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn geometry_interfaces_preserve_dimensions_private_state_and_mutable_quad_points() {
    check(
        r#"(() => {
        const rect = new DOMRect(10, 20, -4, -8);
        if (Object.getOwnPropertyDescriptor(globalThis, 'DOMMatrix').enumerable ||
            Object.getOwnPropertyDescriptor(DOMMatrix.prototype, Symbol.toStringTag).value !== 'DOMMatrix' ||
            !Object.getOwnPropertyDescriptor(DOMQuad, 'fromQuad').enumerable) return false;
        if (!(rect instanceof DOMRectReadOnly) || rect.left !== 6 || rect.top !== 12) return false;
        rect.width = '8';
        if (rect.right !== 18 || rect.toJSON().width !== 8) return false;
        const immutable = DOMRectReadOnly.fromRect({ x: 1, height: NaN });
        immutable.x = 50;
        if (immutable.x !== 1 || !Number.isNaN(immutable.bottom)) return false;
        const quad = DOMQuad.fromRect({ x: 3, y: 4, width: -2, height: 6 });
        const point = quad.p1;
        if (!(point instanceof DOMPointReadOnly) || point !== quad.p1) return false;
        quad.p1.x = -5;
        const bounds = quad.getBounds();
        if (!(bounds instanceof DOMRect) || bounds.x !== -5 || bounds.width !== 8) return false;
        if (quad.toJSON().p1.x !== -5 || Object.keys(quad).length !== 0) return false;
        const element = document.createElement('div');
        element.style.cssText = 'width:10px;height:20px';
        document.body.append(element);
        const box = element.getBoundingClientRect();
        return box instanceof DOMRect && typeof box.toJSON === 'function' &&
            element.getClientRects() instanceof DOMRectList && element.getClientRects()[0] instanceof DOMRect;
    })()"#,
    );
}

#[test]
fn retained_html_collections_methods_indices_and_named_properties_stay_live() {
    check(
        r#"(() => {
        const parent = document.createElement('section');
        document.body.append(parent);
        const children = parent.children;
        const descendants = parent.getElementsByTagName('*');
        const classed = document.getElementsByClassName('a b');
        if (!(children instanceof HTMLCollection) || parent.children !== children) return false;
        const item = children.item;
        const first = document.createElement('span');
        first.setAttribute('name', 'shared');
        first.className = 'a b';
        parent.append(first);
        const second = document.createElement('span');
        second.id = 'shared';
        parent.append(second);
        if (children.length !== 2 || descendants.length !== 2 || classed.length !== 1) return false;
        if (children.namedItem('shared') !== first || children.shared !== first || item.call(children, 1) !== second) return false;
        if (children[2] !== undefined || children.item(2) !== null || children.namedItem('') !== null) return false;
        if (Object.keys(children).join() !== '0,1') return false;
        const descriptor = Object.getOwnPropertyDescriptor(children, 'shared');
        if (descriptor.enumerable || descriptor.writable || descriptor.value !== first) return false;
        const foreign = document.createElementNS('urn:test', 'span');
        foreign.setAttribute('name', 'foreign');
        parent.append(foreign);
        if (children.namedItem('foreign') !== null || parent.getElementsByTagNameNS('urn:test', 'span')[0] !== foreign) return false;
        first.remove();
        second.className = 'a b';
        return children[0] === second && children.shared === second && classed[0] === second &&
            item.call(children, 0) === second && document.forms instanceof HTMLCollection;
    })()"#,
    );
}

#[test]
fn geometry_nested_dictionaries_convert_each_member_before_reading_the_next() {
    check(
        r#"(() => {
        const order = [];
        const point = { get x() { order.push('x'); return 3; } };
        const quad = DOMQuad.fromQuad({
            get p1() { order.push('p1'); return point; },
            get p2() { order.push('p2'); return {}; },
        });
        return order.join() === 'p1,x,p2' && quad instanceof DOMQuad &&
            quad.p1 instanceof DOMPoint && quad.p1 !== point && quad.p1.x === 3;
    })()"#,
    );
}

#[test]
fn document_collections_keep_identity_and_filter_live_html_namespace_members() {
    check(
        r#"(() => {
        const forms = document.forms, images = document.images;
        const links = document.links, anchors = document.anchors;
        if (forms !== document.forms || images !== document.images ||
            links !== document.links || anchors !== document.anchors) return false;
        const form = document.createElement('form'), image = document.createElement('img');
        const link = document.createElement('a');
        link.setAttribute('href', '#probe'); link.setAttribute('name', 'probe');
        const foreign = document.createElementNS('urn:test', 'FORM');
        document.body.append(form, image, link, foreign);
        if (forms.length !== 1 || forms[0] !== form || images[0] !== image ||
            links[0] !== link || anchors[0] !== link) return false;
        link.removeAttribute('href'); link.removeAttribute('name');
        form.remove(); image.remove();
        return forms.length === 0 && images.length === 0 && links.length === 0 &&
            anchors.length === 0 && forms === document.forms && links === document.links;
    })()"#,
    );
}

#[test]
fn matrices_preserve_all_coefficients_for_point_products_and_inverse() {
    check(
        r#"(() => {
        const matrix = new DOMMatrix([2,0,0,0, 0,3,0,0, 0,0,4,0, 10,20,30,5]);
        const point = new DOMPoint(1,2,3,1);
        const transformed = matrix.transformPoint(point);
        if (matrix.is2D || transformed.x !== 12 || transformed.y !== 26 ||
            transformed.z !== 42 || transformed.w !== 5) return false;
        const restored = matrix.inverse().transformPoint(transformed);
        if (Math.abs(restored.x - 1) > 1e-12 || Math.abs(restored.y - 2) > 1e-12 ||
            Math.abs(restored.z - 3) > 1e-12 || Math.abs(restored.w - 1) > 1e-12) return false;
        const identity = new DOMMatrix();
        if (!identity.is2D || identity.m13 !== 0 || identity.m33 !== 1 || identity.m44 !== 1) return false;
        identity.m31 = 7;
        if (identity.is2D || identity.transformPoint(point).x !== 22) return false;
        const translated = new DOMMatrix().translate(1, 2, 3);
        Object.defineProperty(matrix, 'm11', { value: 99 });
        if (matrix.transformPoint(point).x !== 12) return false;
        if (matrix.multiply({m33:2}).transformPoint(point).z !== 54) return false;
        return translated.transformPoint(point).z === 6 &&
            matrix.multiply(new DOMMatrix()).transformPoint(point).z === 42;
    })()"#,
    );
}

#[test]
fn static_range_preserves_boundaries_without_live_mutation_or_offset_validation() {
    check(
        r#"(() => {
        const text = document.createTextNode('abcd');
        document.body.append(text);
        const snapshot = new StaticRange({startContainer:text, startOffset:1, endContainer:text, endOffset:3});
        const range = document.createRange();
        range.setStart(text, 1); range.setEnd(text, 3);
        if (!(snapshot instanceof AbstractRange) || !(range instanceof AbstractRange)) return false;
        text.deleteData(0, 4);
        if (snapshot.startOffset !== 1 || snapshot.endOffset !== 3 || range.endOffset !== 0) return false;
        snapshot.startOffset = 8;
        if (snapshot.startOffset !== 1) return false;
        const beyond = new StaticRange({startContainer:text, startOffset:99, endContainer:text, endOffset:2});
        if (beyond.startOffset !== 99 || beyond.collapsed) return false;
        for (const invalid of [document.createAttribute('a'), document.implementation.createDocumentType('html', '', '')]) {
            try { new StaticRange({startContainer:invalid, startOffset:0, endContainer:text, endOffset:0}); return false; }
            catch (error) { if (error.name !== 'InvalidNodeTypeError') return false; }
        }
        return Object.getPrototypeOf(Range.prototype) === AbstractRange.prototype;
    })()"#,
    );
}

#[test]
fn composed_selection_boundaries_survive_cross_root_collapse_and_shadow_mutations() {
    check(
        r#"(() => {
        const a = document.createElement('div'), b = document.createElement('div');
        document.body.append(a, b);
        const c = a.attachShadow({mode:'closed'}), d = b.attachShadow({mode:'open'});
        c.textContent = 'C'; d.textContent = 'xy';
        const selection = getSelection();
        selection.setBaseAndExtent(c.firstChild, 0, d.firstChild, 2);
        if (selection.anchorNode !== null || selection.focusNode !== null || selection.rangeCount !== 0 ||
            selection.type !== 'None' || selection.isCollapsed) return false;
        try { selection.getRangeAt(0); return false; }
        catch (error) { if (error.name !== 'IndexSizeError') return false; }
        // An externally retained ordinary Range still tracks composed setter changes.
        const ordinary = document.createRange();
        ordinary.selectNodeContents(document.body);
        selection.addRange(ordinary);
        ordinary.setStart(c.firstChild, 0); ordinary.setEnd(d.firstChild, 2);
        if (!ordinary.collapsed || ordinary.startContainer !== d.firstChild) return false;
        const snapshot = selection.getComposedRanges({shadowRoots:[c,d]})[0];
        if (snapshot.startContainer !== c.firstChild || snapshot.endOffset !== 2) return false;
        const outer = selection.getComposedRanges()[0];
        if (outer.startContainer !== document.body || outer.startOffset !== 0 || outer.endOffset !== 2) return false;
        d.firstChild.deleteData(0, 1);
        if (selection.getComposedRanges({shadowRoots:[c,d]})[0].endOffset !== 1 || snapshot.endOffset !== 2) return false;
        a.remove();
        const after = selection.getComposedRanges({shadowRoots:[d]})[0];
        if (after.startContainer !== document.body || after.startOffset !== 0 || after.endContainer !== d.firstChild) return false;
        if (snapshot.startContainer !== c.firstChild) return false;
        ordinary.setStart(document.createElement('span'), 0);
        return selection.getComposedRanges().length === 0 && selection.anchorNode === null;
    })()"#,
    );
}

#[test]
fn composed_ranges_rescope_shadow_boundaries_and_preserve_authorized_nested_roots() {
    check(
        r#"(() => {
        const host = document.createElement('div');
        document.body.append(host);
        const outer = host.attachShadow({mode:'closed'});
        const innerHost = document.createElement('div');
        outer.append(innerHost);
        const inner = innerHost.attachShadow({mode:'open'});
        const text = document.createTextNode('abcd');
        inner.append(text);
        const range = document.createRange();
        range.setStart(text, 1); range.setEnd(text, 3);
        const selection = document.getSelection();
        selection.addRange(range);
        const opaque = selection.getComposedRanges()[0];
        if (!(opaque instanceof StaticRange) || opaque.startContainer !== document.body ||
            opaque.endOffset !== opaque.startOffset + 1) return false;
        const partial = selection.getComposedRanges({shadowRoots:[outer]})[0];
        if (partial.startContainer !== outer || partial.endOffset !== partial.startOffset + 1) return false;
        const exact = selection.getComposedRanges({shadowRoots:[inner]})[0];
        if (exact.startContainer !== text || exact.startOffset !== 1 || exact.endOffset !== 3) return false;
        range.setEnd(text, 2);
        if (exact.endOffset !== 3) return false;
        selection.removeAllRanges();
        return selection.getComposedRanges().length === 0;
    })()"#,
    );
}

#[test]
fn dataset_implementation_and_text_metrics_expose_platform_interface_instances() {
    check(
        r#"(() => {
        const node = document.createElement('div');
        const dataset = node.dataset;
        if (!(dataset instanceof DOMStringMap) || dataset !== node.dataset) return false;
        dataset.fooBar = 'one';
        node.setAttribute('data-other', 'two');
        if (dataset.fooBar !== 'one' || dataset.other !== 'two' || JSON.stringify(dataset) !== '{"fooBar":"one","other":"two"}') return false;
        delete dataset.fooBar;
        try { dataset['bad-name'] = 'x'; return false; } catch (error) { if (error.name !== 'SyntaxError') return false; }
        const implementation = document.implementation;
        if (!(implementation instanceof DOMImplementation) || implementation !== document.implementation) return false;
        if (implementation.createHTMLDocument('Title').title !== 'Title') return false;
        const metric = document.createElement('canvas').getContext('2d').measureText('abc');
        if (!(metric instanceof TextMetrics) || metric.width <= 0) return false;
        const width = metric.width;
        metric.width = -1;
        return metric.width === width && dataset.fooBar === undefined;
    })()"#,
    );
}

#[test]
fn implementation_converts_web_idl_strings_before_creating_documents() {
    check(
        r#"(() => {
        const implementation = document.implementation;
        const doctype = implementation.createDocumentType('html', null, undefined);
        if (doctype.publicId !== 'null' || doctype.systemId !== 'undefined') return false;
        if (implementation.createDocument(null, null).documentElement !== null) return false;
        if (implementation.createHTMLDocument(null).querySelector('title').textContent !== 'null') return false;
        const calls = [
            () => implementation.createDocumentType(Symbol(), '', ''),
            () => implementation.createDocumentType('html', Symbol(), ''),
            () => implementation.createDocument(Symbol(), ''),
            () => implementation.createDocument(null, Symbol()),
            () => implementation.createDocument(null, '', document.body),
            () => implementation.createHTMLDocument(Symbol()),
        ];
        for (const call of calls) {
            try { call(); return false; }
            catch (error) { if (!(error instanceof TypeError)) return false; }
        }
        return true;
    })()"#,
    );
}

#[test]
fn selection_public_boundaries_hide_shadow_nodes_without_losing_composed_ranges() {
    check(
        r#"(() => {
        const host = document.createElement('div'), light = document.createTextNode('light');
        document.body.append(light, host);
        const root = host.attachShadow({mode:'closed'});
        root.textContent = 'secret';
        const text = root.firstChild, selection = getSelection();
        if (Object.keys(selection).length !== 0) return false;
        selection.__range = {startContainer:document.body}; selection.__doc = document;
        selection.collapse(text, 2);
        if (selection.anchorNode !== null || selection.focusNode !== null || selection.anchorOffset !== 0 ||
            selection.focusOffset !== 0 || selection.rangeCount !== 0 || selection.type !== 'None' || !selection.isCollapsed) return false;
        try { selection.getRangeAt(0); return false; }
        catch (error) { if (error.name !== 'IndexSizeError') return false; }
        const captured = selection.getComposedRanges({shadowRoots:[root]})[0];
        if (captured.startContainer !== text || captured.startOffset !== 2 || !captured.collapsed) return false;
        Object.defineProperty(text, 'parentNode', {value:document.body, configurable:true});
        Object.defineProperty(root, 'host', {value:light, configurable:true});
        if (selection.anchorNode !== null || selection.rangeCount !== 0) return false;
        const outer = selection.getComposedRanges()[0];
        if (outer.startContainer !== document.body || outer.startOffset !== 1 || outer.endOffset !== 2) return false;
        delete text.parentNode; delete root.host;
        selection.setBaseAndExtent(light, 1, text, 3);
        if (selection.anchorNode !== light || selection.anchorOffset !== 1 || selection.focusNode !== null || selection.isCollapsed) return false;
        selection.setBaseAndExtent(text, 3, light, 1);
        if (selection.anchorNode !== null || selection.focusNode !== light || selection.focusOffset !== 1) return false;
        selection.collapse(light, 1);
        const range = selection.getRangeAt(0);
        if (selection.rangeCount !== 1 || selection.type !== 'Caret' || selection.getRangeAt(4294967296) !== range) return false;
        for (const bad of [1, -1, Symbol(), 0n]) {
            try { selection.getRangeAt(bad); return false; } catch (_) {}
        }
        try { selection.getRangeAt(); return false; }
        catch (error) { if (!(error instanceof TypeError)) return false; }
        return selection.getComposedRanges()[0].startContainer === light;
    })()"#,
    );
}

#[test]
fn composed_range_options_convert_iterables_once_and_close_on_invalid_roots() {
    check(
        r#"(() => {
        const host = document.createElement('div'); document.body.append(host);
        const root = host.attachShadow({mode:'closed'}); root.textContent = 'value';
        const selection = getSelection(); selection.collapse(root.firstChild, 1);
        let reads = 0, iterations = 0, closed = false;
        const roots = {
            get [Symbol.iterator]() { reads++; return function* () { iterations++; yield root; }; }
        };
        const range = selection.getComposedRanges({shadowRoots:roots})[0];
        if (reads !== 1 || iterations !== 1 || range.startContainer !== root.firstChild) return false;
        const invalid = { *[Symbol.iterator]() { try { yield document; throw new Error('advanced invalid sequence'); } finally { closed = true; } } };
        try { selection.getComposedRanges({shadowRoots:invalid}); return false; }
        catch (error) { if (!(error instanceof TypeError) || !closed) return false; }
        for (const value of [null, 'text', {0:root,length:1}]) {
            try { selection.getComposedRanges({shadowRoots:value}); return false; }
            catch (error) { if (!(error instanceof TypeError)) return false; }
        }
        let touched = false;
        try { Selection.prototype.getComposedRanges.call({}, {get shadowRoots(){touched=true;return []}}); return false; }
        catch (error) { if (!(error instanceof TypeError) || touched) return false; }
        return selection.getComposedRanges(null)[0].startContainer === document.body;
    })()"#,
    );
}

#[test]
fn selection_private_state_survives_weak_map_method_replacement() {
    check(
        r#"(() => {
        const host = document.createElement('div'); document.body.append(host);
        const root = host.attachShadow({mode:'closed'}); root.textContent = 'secret';
        const selection = getSelection(); selection.collapse(root.firstChild, 1);
        const original = WeakMap.prototype.get;
        let calls = 0;
        WeakMap.prototype.get = function (key) { calls++; return original.call(this, key); };
        try {
            const visible = selection.anchorNode === null && selection.focusNode === null &&
                selection.rangeCount === 0 && selection.type === 'None' && selection.isCollapsed;
            const range = selection.getComposedRanges()[0];
            return visible && range.startContainer === document.body && calls === 0;
        } finally { WeakMap.prototype.get = original; }
    })()"#,
    );
}

#[test]
fn composed_root_boundaries_preserve_offsets_when_public_ranges_are_unavailable() {
    check(
        r#"(() => {
        const parent = document.createElement('section'); document.body.append(parent);
        const first = document.createElement('div'), second = document.createElement('div'); parent.append(first, second);
        const a = first.attachShadow({mode:'closed'}), b = second.attachShadow({mode:'open'});
        a.textContent = 'a'; b.textContent = 'b';
        const selection = getSelection(); selection.setBaseAndExtent(a, 0, b, 0);
        const full = selection.getComposedRanges({shadowRoots:[a,b]})[0];
        if (full.startContainer !== a || full.endContainer !== b || full.startOffset !== 0 || full.endOffset !== 0) return false;
        const outer = selection.getComposedRanges()[0];
        if (outer.startContainer !== parent || outer.startOffset !== 0 || outer.endContainer !== parent || outer.endOffset !== 2) return false;
        const onlyA = selection.getComposedRanges({shadowRoots:[a]})[0];
        const onlyB = selection.getComposedRanges({shadowRoots:[b]})[0];
        if (onlyA.startContainer !== a || onlyA.endContainer !== parent || onlyA.endOffset !== 2 ||
            onlyB.startContainer !== parent || onlyB.startOffset !== 0 || onlyB.endContainer !== b) return false;
        selection.setBaseAndExtent(a, 0, b, 0);
        const repeated = selection.getComposedRanges({shadowRoots:[a,b]})[0];
        return repeated.startContainer === a && repeated.endContainer === b && selection.rangeCount === 0 && !selection.isCollapsed;
    })()"#,
    );
}

#[test]
fn composed_slot_boundaries_use_dom_order_for_slotted_and_unslotted_content() {
    check(
        r#"(() => {
        const selection = getSelection();
        for (const slotted of [true, false]) {
            const parent = document.createElement('section'), host = document.createElement('div');
            host.textContent = 'Second'; parent.append(host); document.body.append(parent);
            const root = host.attachShadow({mode:'closed'});
            root.innerHTML = slotted ? 'First <slot></slot> Third' : '<span>First</span><span>Third</span>';
            const light = host.firstChild, shadow = slotted ? root.lastChild : root.lastChild.firstChild;
            for (const reverse of [false, true]) {
                if (reverse) selection.setBaseAndExtent(shadow, 4, light, 3);
                else selection.setBaseAndExtent(light, 3, shadow, 4);
                const range = selection.getComposedRanges({shadowRoots:[root]})[0];
                if (range.startContainer !== shadow || range.startOffset !== 4 || range.endContainer !== light || range.endOffset !== 3) return false;
                const outer = selection.getComposedRanges()[0];
                if (outer.startContainer !== parent || outer.startOffset !== 0 || outer.endContainer !== light || outer.endOffset !== 3) return false;
            }
            parent.remove(); selection.removeAllRanges();
        }
        return true;
    })()"#,
    );
}

#[test]
fn composed_setter_mutations_preserve_both_edges_and_collapse_only_out_of_order() {
    check(
        r#"(() => {
        const before = document.createTextNode('before text'), host = document.createElement('div'), after = document.createTextNode('after text');
        document.body.append(before, host, after);
        const root = host.attachShadow({mode:'closed'}); root.textContent = 'shadow text';
        const inside = root.firstChild, selection = getSelection();
        const select = (node, start, end) => { selection.removeAllRanges(); const range=document.createRange();range.setStart(node,start);range.setEnd(node,end);selection.addRange(range);return range; };
        let range = select(before, 1, 4); range.setEnd(inside, 3);
        let composed = selection.getComposedRanges({shadowRoots:[root]})[0];
        if (!range.collapsed || composed.startContainer !== before || composed.startOffset !== 1 || composed.endContainer !== inside || composed.endOffset !== 3 || selection.isCollapsed) return false;
        range = select(after, 1, 4); range.setStart(inside, 3);
        composed = selection.getComposedRanges({shadowRoots:[root]})[0];
        if (!range.collapsed || composed.startContainer !== inside || composed.startOffset !== 3 || composed.endContainer !== after || composed.endOffset !== 4 || selection.isCollapsed) return false;
        range = select(before, 1, 4); range.setStart(inside, 3);
        composed = selection.getComposedRanges({shadowRoots:[root]})[0];
        if (!composed.collapsed || composed.startContainer !== inside || composed.startOffset !== 3 || !selection.isCollapsed) return false;
        range.selectNode(host);
        composed = selection.getComposedRanges()[0];
        if (composed.startContainer !== document.body || composed.startOffset !== 1 || composed.endOffset !== 2) return false;
        range.collapse(false);
        if (!selection.getComposedRanges()[0].collapsed || selection.focusOffset !== 2) return false;
        selection.removeAllRanges(); range = document.createRange(); selection.addRange(range);
        range.setEnd(inside, 3); range.setStart(before, 1);
        composed = selection.getComposedRanges({shadowRoots:[root]})[0];
        return composed.startContainer === before && composed.startOffset === 1 && composed.endContainer === inside && composed.endOffset === 3 &&
            selection.anchorNode === before && selection.focusNode === null && !selection.isCollapsed;
    })()"#,
    );
}

#[test]
fn node_interface_arguments_use_native_brands_and_ignore_node_type_overrides() {
    check(
        r#"(() => {
        const fake = Object.create(Node.prototype);
        Object.defineProperties(fake, {nodeType:{value:1}, __id:{value:document.body.__id}});
        for (const value of [fake, new Proxy(document.body, {})]) {
            try { new StaticRange({startContainer:value,startOffset:0,endContainer:document.body,endOffset:0}); return false; }
            catch (error) { if (!(error instanceof TypeError)) return false; }
        }
        const element = document.createElement('div');
        Object.defineProperty(element, 'nodeType', {value:10});
        const range = new StaticRange({startContainer:element,startOffset:0,endContainer:element,endOffset:0});
        if (range.startContainer !== element) return false;
        try { document.implementation.createDocument(null, '', element); return false; }
        catch (error) { if (!(error instanceof TypeError)) return false; }
        const doctype = document.implementation.createDocumentType('html', '', '');
        Object.defineProperty(doctype, 'nodeType', {value:1});
        try { new StaticRange({startContainer:doctype,startOffset:0,endContainer:element,endOffset:0}); return false; }
        catch (error) { if (error.name !== 'InvalidNodeTypeError') return false; }
        const doc = document.implementation.createDocument(null, '', doctype);
        return doc.doctype === doctype;
    })()"#,
    );
}

#[test]
fn composed_root_arguments_require_native_shadow_roots_and_survive_prototype_changes() {
    check(
        r#"(() => {
        const host = document.createElement('div'); document.body.append(host);
        const root = host.attachShadow({mode:'closed'}); root.textContent = 'secret';
        const text = root.firstChild, selection = getSelection(); selection.collapse(text,1);
        const fake = Object.create(ShadowRoot.prototype);
        Object.defineProperty(fake, '__id', {value:root.__id});
        for (const value of [fake, new Proxy(root, {}), document.createDocumentFragment()]) {
            try { selection.getComposedRanges({shadowRoots:[value]}); return false; }
            catch (error) { if (!(error instanceof TypeError)) return false; }
        }
        Object.setPrototypeOf(root, Object.prototype);
        if (selection.anchorNode !== null || selection.getComposedRanges()[0].startContainer !== document.body) return false;
        return selection.getComposedRanges({shadowRoots:[root]})[0].startContainer === text;
    })()"#,
    );
}

#[test]
fn tag_collections_match_xml_qualified_names_and_cached_document_tags_remain_live() {
    check(
        r#"(() => {
        const documentTag = 'http://www.w3.org/1999/xhtml';
        const xml = document.implementation.createDocument(documentTag,'html');
        const head = xml.createElementNS(documentTag, 'head'), body = xml.createElementNS(documentTag,'body');
        xml.documentElement.append(head, body);
        const titles = xml.getElementsByTagName('title');
        const title = xml.createElementNS(documentTag, 'title'); title.textContent = 'Sparrow'; head.append(title);
        if (xml.head !== head || xml.body !== body || xml.title !== 'Sparrow' || titles[0] !== title || titles.length !== 1) return false;
        const upper = xml.createElementNS(documentTag, 'Title'); head.append(upper);
        if (titles.length !== 1 || xml.getElementsByTagName('Title')[0] !== upper) return false;
        const prefixed = xml.createElementNS('urn:test', 'p:tag'); body.append(prefixed);
        if (xml.getElementsByTagName('p:tag')[0] !== prefixed || xml.getElementsByTagName('tag').length !== 0) return false;
        body.remove(); head.remove();
        if (xml.body !== null || xml.head !== null || titles.length !== 0) return false;
        const nextBody = xml.createElementNS(documentTag,'body'), nextHead = xml.createElementNS(documentTag,'head');
        xml.documentElement.append(nextHead,nextBody);
        return xml.body === nextBody && xml.head === nextHead;
    })()"#,
    );
}

#[test]
fn selection_methods_ignore_overridden_public_counts_and_range_getters() {
    check(
        r#"(() => {
        const host = document.createElement('div'); document.body.append(host);
        const root = host.attachShadow({mode:'closed'}); root.textContent = 'secret';
        const text = root.firstChild, selection = getSelection(); selection.collapse(text, 2);
        Object.defineProperty(selection, 'rangeCount', {value:1,configurable:true});
        Object.defineProperty(selection, 'isCollapsed', {value:false,configurable:true});
        Object.defineProperty(Range.prototype,'startContainer',{get(){return document.body},configurable:true});
        Object.defineProperty(Range.prototype,'endContainer',{get(){return document.body},configurable:true});
        try {
            try { selection.getRangeAt(0); return false; }
            catch (error) { if (error.name !== 'IndexSizeError') return false; }
            const type = Object.getOwnPropertyDescriptor(Selection.prototype, 'type').get.call(selection);
            const anchor = Object.getOwnPropertyDescriptor(Selection.prototype, 'anchorNode').get.call(selection);
            const composed = selection.getComposedRanges({shadowRoots:[root]})[0];
            if (type !== 'None' || anchor !== null || composed.startContainer !== text || composed.startOffset !== 2) return false;
            let converted = false;
            try { Selection.prototype.getRangeAt.call({}, {valueOf(){converted=true;return 0}}); return false; }
            catch (error) { if (!(error instanceof TypeError) || converted) return false; }
            return true;
        } finally {
            delete Range.prototype.startContainer; delete Range.prototype.endContainer;
            delete selection.rangeCount; delete selection.isCollapsed;
        }
    })()"#,
    );
}

#[test]
fn selection_collapse_uses_true_composed_start_and_end_across_shadow_roots() {
    check(
        r#"(() => {
        const a = document.createElement('div'), b = document.createElement('div'); document.body.append(a,b);
        const first = a.attachShadow({mode:'closed'}), last = b.attachShadow({mode:'open'});
        first.textContent='first';last.textContent='last';
        const selection=getSelection();selection.setBaseAndExtent(first.firstChild,1,last.firstChild,3);
        selection.collapseToStart();
        const start=selection.getComposedRanges({shadowRoots:[first,last]})[0];
        if (!start.collapsed || start.startContainer!==first.firstChild || start.startOffset!==1) return false;
        selection.setBaseAndExtent(first.firstChild,1,last.firstChild,3);selection.collapseToEnd();
        const end=selection.getComposedRanges({shadowRoots:[first,last]})[0];
        return end.collapsed && end.startContainer===last.firstChild && end.startOffset===3 && selection.rangeCount===0;
    })()"#,
    );
}
