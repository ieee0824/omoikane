use super::*;
use crate::html::TreeBuilder;

#[test]
fn scalar_dom_mutations_do_not_materialize_utf16_buffers() {
    for items in [1024, 2048] {
        let document = TreeBuilder::parse("<body>").document();
        let mut runtime = JsRuntime::with_document(document).unwrap();
        runtime
            .eval(&format!("globalThis.payload = 'aé😀'.repeat({items});"))
            .unwrap();
        dom_string::reset_materialized_utf16_units();
        assert_eq!(
            runtime
                .eval(
                    r#"(() => {
            const value = payload, host = document.createElement('div');
            const nodes = [document.createTextNode(value), document.createComment(value),
                document.createProcessingInstruction('probe', value)];
            for (const node of nodes) {
                node.data = value; node.nodeValue = value; node.textContent = value;
                if (node.data !== value) throw new Error('CharacterData');
            }
            host.textContent = value;
            host.setAttribute('data-value', value);
            host.setAttributeNS('urn:probe', 'p:value', value);
            host.getAttributeNodeNS('urn:probe', 'value').value = value;
            host.setHTMLUnsafe('<p data-value="' + value + '">' + value + '</p>');
            const parsed = Document.parseHTMLUnsafe('<p>' + value + '</p>');
            document.write('<p id="written">' + value + '</p>'); document.close();
            return host.firstChild.textContent === value && parsed.body.textContent === value &&
                document.getElementById('written').textContent === value;
        })()"#
                )
                .unwrap()
                .as_boolean(),
            Some(true)
        );
        assert_eq!(dom_string::materialized_utf16_units(), 0, "items={items}");
        eprintln!("DOMString staging items={items}, materialized_utf16_units=0");
    }
}

#[test]
fn exact_dom_mutations_reset_to_scalar_and_preserve_unpaired_units() {
    let document = TreeBuilder::parse("<body>").document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
        const values = ['\ud800', '\udfff', '😀\ud800é', '\\uD800', 'aé😀'];
        const host = document.createElement('div');
        const nodes = [document.createTextNode(''), document.createComment(''),
            document.createProcessingInstruction('probe', '')];
        for (const value of values) {
            for (const node of nodes) {
                node.data = value;
                if (node.data !== value || node.cloneNode().data !== value) return false;
            }
            host.textContent = value;
            host.setAttribute('data-value', value);
            host.setAttributeNS('urn:probe', 'p:value', value);
            if (host.textContent !== value || host.getAttribute('data-value') !== value ||
                host.getAttributeNS('urn:probe', 'value') !== value) return false;
        }
        document.open();
        document.write('<p id=pair>\ud83d');
        const text = document.getElementById('pair').firstChild;
        if (text.data !== '\ud83d') return false;
        document.write('\ude00</p><script>document.write("<b>nested</b>");<\/script>');
        document.write('<p id=tail x="\udfff">\ud800<!--\udfff--><?probe \ud800?></p>');
        document.close();
        const tail = document.getElementById('tail');
        return text.data === '😀' && tail.firstChild.data === '\ud800' &&
            tail.getAttribute('x') === '\udfff' && tail.childNodes[1].data === '\udfff' &&
            tail.lastChild.data === '\ud800';
    })()"#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}
