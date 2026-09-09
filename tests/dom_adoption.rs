//! Insertion preserves ownership within a document and adopts entire subtrees.

use omoikane::{html::TreeBuilder, js::JsRuntime};

#[test]
fn repeated_moves_preserve_light_shadow_and_template_owners() {
    let document = TreeBuilder::parse("<html><body></body></html>").document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let result = runtime
        .eval(
            r#"(() => {
                const other = document.implementation.createHTMLDocument('other');
                const host = document.createElement('div');
                host.innerHTML = '<span><b></b></span><template><i></i></template>';
                const light = host.firstChild;
                const template = host.lastChild;
                const shadow = host.attachShadow({mode: 'open'});
                shadow.innerHTML = '<em></em>';
                const shadowChild = shadow.firstChild;
                const owner = Object.getOwnPropertyDescriptor(Node.prototype, 'ownerDocument').get;
                Object.defineProperty(host, 'ownerDocument', {
                    get() { throw new Error('author accessor must not affect adoption'); }
                });
                for (const doc of [document, document, other, other, document]) {
                    doc.body.appendChild(host);
                    doc.body.insertBefore(host, doc.body.firstChild);
                    host.remove();
                    const inert = doc.createElement('template').content.ownerDocument;
                    if (owner.call(host) !== doc || light.ownerDocument !== doc ||
                        light.firstChild.ownerDocument !== doc || shadow.ownerDocument !== doc ||
                        shadowChild.ownerDocument !== doc || template.ownerDocument !== doc ||
                        template.content.ownerDocument !== inert ||
                        template.content.firstChild.ownerDocument !== inert) return false;
                }
                return true;
            })()"#,
        )
        .expect("same-document insertion and adoption must preserve internal ownership");
    assert_eq!(result.as_boolean(), Some(true));
}
