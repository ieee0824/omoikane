use omoikane::html::TreeBuilder;
use omoikane::js::JsRuntime;

fn check(source: &str) {
    let document = TreeBuilder::parse("<html><body></body></html>").document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let result = runtime.eval(source).unwrap();
    assert_eq!(result.as_boolean(), Some(true), "{result:?}");
}

#[test]
fn append_accepts_nodes_strings_and_fragments_in_one_mutation() {
    check(
        r#"(() => {
        const parent = document.createElement('div');
        document.body.append(parent);
        const a = document.createElement('a');
        const b = document.createElement('b');
        const fragment = document.createDocumentFragment();
        fragment.append(b, 'tail');
        const observer = new MutationObserver(() => {});
        observer.observe(parent, {childList:true});
        const result = parent.append('head', a, fragment, 42, null, undefined);
        const records = observer.takeRecords();
        return result === undefined && parent.isConnected &&
            parent.textContent === 'headtail42nullundefined' &&
            parent.childNodes[1] === a && parent.childNodes[2] === b &&
            fragment.childNodes.length === 0 && records.length === 1 &&
            records[0].addedNodes.length === 7 &&
            Array.from(parent.childNodes).every(node => node.ownerDocument === document);
    })()"#,
    );
}

#[test]
fn append_converts_all_strings_before_moving_nodes_and_handles_repetition() {
    check(
        r#"(() => {
        const a = document.createElement('a');
        const b = document.createElement('b');
        const parent = document.createElement('div');
        parent.append(a, b);
        const before = parent.innerHTML;
        let rejected = false;
        try { parent.append(a, Symbol('invalid')); }
        catch (error) { rejected = error instanceof TypeError; }
        if (!rejected || parent.innerHTML !== before) return false;
        parent.append(a, b, a);
        return parent.childNodes.length === 2 && parent.firstChild === b &&
            parent.lastChild === a && parent.append() === undefined;
    })()"#,
    );
}

#[test]
fn append_enforces_document_and_ancestor_constraints() {
    check(
        r#"(() => {
        const doc = document.implementation.createDocument(null, '');
        const root = doc.createElement('root');
        doc.append(root);
        const child = doc.createElement('child');
        root.append(child);
        const fragment = doc.createDocumentFragment();
        fragment.append(doc.createElement('extra'));
        const attempts = [() => doc.append('text'), () => doc.append(fragment),
            () => child.append(root), () => root.append(doc),
            () => root.append(doc.implementation.createDocumentType('html', '', ''))];
        for (const attempt of attempts) {
            try { attempt(); return false; }
            catch (error) { if (error.name !== 'HierarchyRequestError') return false; }
        }
        return doc.documentElement === root && root.firstChild === child &&
            fragment.childNodes.length === 1 &&
            typeof document.createTextNode('').append === 'undefined' &&
            Element.prototype[Symbol.unscopables].append === true;
    })()"#,
    );
}
