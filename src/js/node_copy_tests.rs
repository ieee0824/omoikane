use super::*;
use crate::html::TreeBuilder;

fn runtime() -> JsRuntime {
    JsRuntime::with_document(TreeBuilder::parse("<body></body>").document()).unwrap()
}

fn assert_contract(runtime: &mut JsRuntime, source: &str) {
    let result = runtime.eval(source).unwrap_or_else(|error| {
        let message = error
            .to_opaque(&mut runtime.context)
            .to_string(&mut runtime.context)
            .unwrap()
            .to_std_string_escaped();
        panic!("DOM copy contract failed: {message}");
    });
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn id_lookup_preserves_exact_values_and_duplicate_tree_order() {
    let mut runtime = runtime();
    assert_contract(
        &mut runtime,
        r#"(() => {
            const first = document.createElement('span');
            const second = document.createElement('span');
            first.id = second.id = 'duplicate';
            document.body.appendChild(first);
            document.body.appendChild(second);
            if (document.getElementById('duplicate') !== first) return false;
            first.remove();
            if (document.getElementById('duplicate') !== second) return false;
            document.body.insertBefore(first, second);
            const special = document.createElement('em');
            const exact = 'name.with[selector] #punctuation';
            special.id = exact;
            document.body.appendChild(special);
            return document.getElementById('duplicate') === first &&
                document.getElementById(exact) === special &&
                document.getElementById(exact.toUpperCase()) === null &&
                document.getElementById('') === null;
        })()"#,
    );
}

#[test]
fn ordinary_node_relations_follow_reparenting_and_shadow_boundaries() {
    let mut runtime = runtime();
    assert_contract(
        &mut runtime,
        r#"(() => {
            const parent = document.createElement('section');
            const text = document.createTextNode('text');
            const comment = document.createComment('comment');
            const element = document.createElement('em');
            if (parent.firstChild !== null || parent.lastChild !== null) return false;
            parent.appendChild(text); parent.appendChild(comment); parent.appendChild(element);
            Object.defineProperty(parent, 'childNodes', {
                get() { throw new Error('node relations must not read author childNodes'); },
                configurable: true
            });
            if (parent.firstChild !== text || parent.lastChild !== element ||
                text.previousSibling !== null || text.nextSibling !== comment ||
                comment.previousSibling !== text || comment.nextSibling !== element ||
                element.nextSibling !== null) return false;
            delete parent.childNodes;
            parent.removeChild(comment);
            if (text.nextSibling !== element || element.previousSibling !== text ||
                comment.nextSibling !== null || comment.previousSibling !== null) return false;
            const fragment = document.createDocumentFragment();
            fragment.appendChild(element);
            if (parent.lastChild !== text || fragment.firstChild !== element ||
                fragment.lastChild !== element || element.previousSibling !== null) return false;
            const host = document.createElement('div');
            const shadow = host.attachShadow({ mode: 'open' });
            shadow.appendChild(element); shadow.appendChild(comment);
            return fragment.firstChild === null && fragment.lastChild === null &&
                shadow.firstChild === element && shadow.lastChild === comment &&
                element.nextSibling === comment && comment.previousSibling === element &&
                element.parentNode === shadow && host.firstChild === null &&
                host.lastChild === null && shadow.previousSibling === null &&
                shadow.nextSibling === null && element.ownerDocument === document;
        })()"#,
    );
}

#[test]
fn child_edges_discover_native_insertions_and_preserve_owner_document() {
    let mut runtime = runtime();
    let body = runtime.document().query_selector("body").unwrap();
    body.append_child(NodeHandle::text("native first"));
    let last = NodeHandle::element("em");
    last.append_child(NodeHandle::text("native last"));
    body.append_child(last);
    assert_contract(
        &mut runtime,
        r#"(() => {
            const first = document.body.firstChild;
            const last = document.body.lastChild;
            return first.data === 'native first' && last.nodeName === 'EM' &&
                last.firstChild.data === 'native last' &&
                first.ownerDocument === document && last.ownerDocument === document &&
                first.parentNode === document.body && last.parentNode === document.body;
        })()"#,
    );
}

#[test]
fn next_sibling_discovers_native_insertions_without_reading_other_child_edges() {
    let mut runtime = runtime();
    let body = runtime.document().query_selector("body").unwrap();
    body.append_child(NodeHandle::text("native first"));
    body.append_child(NodeHandle::text("native second"));
    assert_contract(
        &mut runtime,
        r#"(() => {
            const first = document.body.firstChild;
            const second = first.nextSibling;
            return first.data === 'native first' && second.data === 'native second' &&
                second.ownerDocument === document && second.parentNode === document.body &&
                second.previousSibling === first && second.nextSibling === null;
        })()"#,
    );
}

#[test]
fn previous_sibling_discovers_native_insertions_without_reading_other_child_edges() {
    let mut runtime = runtime();
    let body = runtime.document().query_selector("body").unwrap();
    body.append_child(NodeHandle::text("native first"));
    body.append_child(NodeHandle::text("native second"));
    assert_contract(
        &mut runtime,
        r#"(() => {
            const second = document.body.lastChild;
            const first = second.previousSibling;
            return first.data === 'native first' && second.data === 'native second' &&
                first.ownerDocument === document && first.parentNode === document.body &&
                first.nextSibling === second && first.previousSibling === null;
        })()"#,
    );
}

#[test]
fn character_data_reads_preserve_utf16_across_native_clones() {
    let mut runtime = runtime();
    assert_contract(
        &mut runtime,
        r#"(() => {
            const values = ['', 'ASCII', 'é日本😀', '\uD800x\uDC00'];
            for (const value of values) {
                const nodes = [document.createTextNode(value), document.createComment(value),
                    document.createProcessingInstruction('target', value)];
                for (const node of nodes) {
                    const clone = node.cloneNode();
                    if (node.data !== value || node.textContent !== value ||
                        clone.data !== value || clone.textContent !== value) return false;
                }
                const xml = document.implementation.createDocument(null, 'root', null);
                const cdata = xml.createCDATASection(value);
                if (cdata.data !== value || cdata.cloneNode().data !== value) return false;
            }
            const parent = document.createElement('div');
            parent.appendChild(document.createTextNode('\uD800x\uDC00'));
            return parent.cloneNode(true).firstChild.data === '\uD800x\uDC00';
        })()"#,
    );
}

#[test]
fn descendant_text_preserves_code_units_and_excludes_non_text_trees() {
    let mut runtime = runtime();
    assert_contract(
        &mut runtime,
        r#"(() => {
            const fragment = document.createDocumentFragment();
            fragment.appendChild(document.createTextNode('A\uD800'));
            fragment.appendChild(document.createComment('excluded comment'));
            fragment.appendChild(document.createProcessingInstruction('target', 'excluded pi'));
            const nested = document.createElement('span');
            nested.appendChild(document.createTextNode('\uDC00日本😀'));
            fragment.appendChild(nested);
            const host = document.createElement('div');
            host.attachShadow({ mode: 'open' }).appendChild(document.createTextNode('excluded shadow'));
            fragment.appendChild(host);
            const template = document.createElement('template');
            template.content.appendChild(document.createTextNode('excluded template'));
            fragment.appendChild(template);
            const expected = 'A\uD800\uDC00日本😀';
            const xml = new DOMParser().parseFromString('<root><![CDATA[é]]><!--ignore--><?target ignore?>日本</root>', 'text/xml');
            const doctype = document.implementation.createDocumentType('html', '', '');
            return fragment.textContent === expected &&
                fragment.cloneNode(true).textContent === expected &&
                host.textContent === '' && host.shadowRoot.textContent === 'excluded shadow' &&
                xml.documentElement.textContent === 'é日本' &&
                doctype.textContent === null && document.textContent === null;
        })()"#,
    );
}
