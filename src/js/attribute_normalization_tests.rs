use super::*;
use crate::html::TreeBuilder;

fn runtime() -> JsRuntime {
    JsRuntime::with_document_and_url(
        TreeBuilder::parse("<html><body><iframe id=f></iframe></body></html>").document(),
        "https://attributes.example/parent.html",
    )
    .unwrap()
}

#[test]
fn attribute_case_uses_native_element_namespace_and_document_type() {
    let mut runtime = runtime();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
                    const html = document.createElementNS('http://www.w3.org/1999/xhtml', 'p');
                    html.setAttribute('MiXeD', 'html');
                    const xml = document.createElementNS(null, 'Plain');
                    xml.setAttribute('MiXeD', 'xml');
                    const parsed = new DOMParser().parseFromString(
                        '<html xmlns="http://www.w3.org/1999/xhtml"/>', 'application/xhtml+xml');
                    const xhtml = parsed.documentElement;
                    xhtml.setAttribute('MiXeD', 'xhtml');
                    return html.getAttribute('mixed') === 'html' &&
                        html.hasAttribute('MIXED') && html.getAttributeNode('mixed').name === 'mixed' &&
                        xml.getAttribute('MiXeD') === 'xml' && xml.getAttribute('mixed') === null &&
                        xhtml.getAttribute('MiXeD') === 'xhtml' && xhtml.getAttribute('mixed') === null &&
                        parsed.contentType === 'application/xhtml+xml' &&
                        parsed.cloneNode(true).contentType === 'application/xhtml+xml' &&
                        new Document().contentType === 'application/xml';
                })()"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn element_creation_preserves_xhtml_namespace_and_xml_name_case() {
    let mut runtime = runtime();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
                    const xhtml = new DOMParser().parseFromString(
                        '<html xmlns="http://www.w3.org/1999/xhtml"/>', 'application/xhtml+xml');
                    const script = xhtml.createElement('script');
                    const frame = xhtml.createElement('IFRAME');
                    const xml = new DOMParser().parseFromString('<root/>', 'application/xml');
                    const xmlScript = xml.createElement('script');
                    const html = document.createElement('MiXeD');
                    script.setAttribute('MiXeD', 'xhtml');
                    const check = (label, condition, actual) => {
                        if (!condition) throw new Error(label + ': ' + String(actual));
                    };
                    check('XHTML script interface', script instanceof HTMLScriptElement,
                        script.constructor.name);
                    check('XHTML script namespace', script.namespaceURI === 'http://www.w3.org/1999/xhtml',
                        script.namespaceURI);
                    check('XHTML script local name', script.localName === 'script', script.localName);
                    check('XHTML script tag name', script.tagName === 'script', script.tagName);
                    check('XHTML script node name', script.nodeName === 'script', script.nodeName);
                    check('XHTML attribute case', script.getAttribute('MiXeD') === 'xhtml',
                        script.getAttribute('MiXeD'));
                    check('XHTML lowercase attribute absent', script.getAttribute('mixed') === null,
                        script.getAttribute('mixed'));
                    check('XHTML iframe namespace', frame.namespaceURI === 'http://www.w3.org/1999/xhtml',
                        frame.namespaceURI);
                    check('XHTML iframe local name', frame.localName === 'IFRAME', frame.localName);
                    check('XHTML iframe tag name', frame.tagName === 'IFRAME', frame.tagName);
                    check('XHTML iframe node name', frame.nodeName === 'IFRAME', frame.nodeName);
                    check('XML script namespace', xmlScript.namespaceURI === null, xmlScript.namespaceURI);
                    check('XML script local name', xmlScript.localName === 'script', xmlScript.localName);
                    check('XML script interface', !(xmlScript instanceof HTMLScriptElement),
                        xmlScript.constructor.name);
                    check('HTML namespace', html.namespaceURI === 'http://www.w3.org/1999/xhtml',
                        html.namespaceURI);
                    check('HTML lowercase local name', html.localName === 'mixed', html.localName);
                    return true;
                })()"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn element_clone_preserves_local_names_and_namespace_prefixes() {
    let mut runtime = runtime();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
                    const namespace = 'http://www.w3.org/1999/xhtml';
                    const xhtml = new DOMParser().parseFromString(
                        '<html xmlns="http://www.w3.org/1999/xhtml"/>', 'application/xhtml+xml');
                    const xml = document.implementation.createDocument(null, 'root', null);
                    const check = (label, actual, expected) => {
                        if (actual !== expected) throw new Error(label + ': ' + String(actual));
                    };
                    for (const [owner, localNamespace, htmlDocument] of [
                        [document, namespace, true], [xhtml, namespace, false], [xml, null, false],
                    ]) {
                        const local = owner.createElement('X:Y');
                        const qualified = owner.createElementNS(namespace, 'X:Y');
                        const unicode = owner.createElement('äBC');
                        check('createElement namespace', local.namespaceURI, localNamespace);
                        check('createElement prefix', local.prefix, null);
                        check('createElement local name', local.localName, htmlDocument ? 'x:y' : 'X:Y');
                        check('createElementNS prefix', qualified.prefix, 'X');
                        check('createElementNS local name', qualified.localName, 'Y');
                        check('ASCII-only local name folding', unicode.localName, htmlDocument ? 'äbc' : 'äBC');
                        check('ASCII-only node name folding', unicode.nodeName, 'äBC');
                        for (const original of [local, qualified, unicode]) {
                            const clone = original.cloneNode();
                            check('cloned namespace', clone.namespaceURI, original.namespaceURI);
                            check('cloned prefix for ' + original.localName, clone.prefix, original.prefix);
                            check('cloned local name', clone.localName, original.localName);
                            check('cloned node name', clone.nodeName, original.nodeName);
                            check('cloned owner', clone.ownerDocument, original.ownerDocument);
                            check('cloned node equality', clone.isEqualNode(original), true);
                        }
                    }
                    return true;
                })()"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn element_clone_preserves_xhtml_attribute_case_and_utf16() {
    let mut runtime = runtime();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
                    const xhtml = new DOMParser().parseFromString(
                        '<html xmlns="http://www.w3.org/1999/xhtml"/>', 'application/xhtml+xml');
                    const xml = document.implementation.createDocument(null, 'root', null);
                    const script = xhtml.createElement('script');
                    const child = xhtml.createElement('span');
                    const value = '\uD800A\uDC00';
                    script.setAttribute('MiXeD', value);
                    script.setAttributeNS('urn:attribute', 'a:CaSe', value);
                    child.setAttribute('ChIlD', value);
                    script.appendChild(child);
                    const check = (label, condition) => {
                        if (!condition) throw new Error(label);
                    };
                    check('source lone-surrogate UTF-16', value.length === 3 &&
                        value.charCodeAt(0) === 0xD800 && value.charCodeAt(2) === 0xDC00);
                    check('source XHTML attribute', script.getAttribute('MiXeD') === value);
                    const shallow = script.cloneNode();
                    const deep = script.cloneNode(true);
                    const imported = xml.importNode(script, true);
                    const adopted = xml.adoptNode(script.cloneNode(true));
                    for (const [copy, owner, nested] of [
                        [shallow, xhtml, false], [deep, xhtml, true],
                        [imported, xml, true], [adopted, xml, true],
                    ]) {
                        check('cloned mixed-case attribute value', copy.getAttribute('MiXeD') === value);
                        check('cloned lone-surrogate UTF-16', copy.getAttribute('MiXeD').charCodeAt(0) === 0xD800 &&
                            copy.getAttribute('MiXeD').charCodeAt(2) === 0xDC00);
                        check('cloned lowercase attribute absent', copy.getAttribute('mixed') === null);
                        check('cloned attribute name', copy.getAttributeNode('MiXeD').name === 'MiXeD');
                        check('cloned namespaced UTF-16', copy.getAttributeNS('urn:attribute', 'CaSe') === value);
                        check('cloned owner', copy.ownerDocument === owner);
                        check('clone depth', copy.childNodes.length === Number(nested));
                        if (nested) {
                            check('deep attribute case and UTF-16', copy.firstChild.getAttribute('ChIlD') === value);
                            check('deep lowercase attribute absent', copy.firstChild.getAttribute('child') === null);
                            check('deep owner', copy.firstChild.ownerDocument === owner);
                        }
                    }
                    document.adoptNode(adopted);
                    check('HTML adoption preserves raw attribute name', adopted.getAttributeNS(null, 'MiXeD') === value);
                    check('HTML adoption adds no lowercase attribute', adopted.getAttributeNS(null, 'mixed') === null);
                    check('HTML adoption preserves descendant', adopted.firstChild.getAttributeNS(null, 'ChIlD') === value);
                    return true;
                })()"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn attribute_case_ignores_author_getters_and_html_instance_checks() {
    let mut runtime = runtime();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
                    const element = document.createElement('p');
                    const set = Element.prototype.setAttribute;
                    const get = Element.prototype.getAttribute;
                    const has = Element.prototype.hasAttribute;
                    const remove = Element.prototype.removeAttribute;
                    const nodeName = Object.getOwnPropertyDescriptor(Node.prototype, 'nodeName').get;
                    const fail = () => { throw new Error('author state must not be read'); };
                    Object.defineProperty(element, 'ownerDocument', { get: fail });
                    Object.defineProperty(document, 'contentType', { get: fail });
                    Object.defineProperty(HTMLElement, Symbol.hasInstance, { value: fail });
                    document.__contentType = 'application/xml';
                    Object.setPrototypeOf(element, null);
                    set.call(element, 'DaTa-MiXeD', 'value');
                    const present = get.call(element, 'DATA-MIXED') === 'value' &&
                        has.call(element, 'data-mixed') && nodeName.call(element) === 'P';
                    remove.call(element, 'DATA-MIXED');
                    return present && get.call(element, 'data-mixed') === null;
                })()"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn attribute_case_tracks_detached_adoption_between_html_and_xml_documents() {
    let mut runtime = runtime();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
                    const element = document.createElement('p');
                    const xml = document.implementation.createDocument(null, 'root', null);
                    element.setAttribute('BeFoRe', 'html');
                    xml.adoptNode(element);
                    element.setAttribute('MiXeD', 'xml');
                    const detachedXML = element.ownerDocument === xml &&
                        element.nodeName === 'p' && element.tagName === 'p' &&
                        element.getAttribute('MiXeD') === 'xml' && element.getAttribute('mixed') === null &&
                        element.hasAttribute('MiXeD') && !element.hasAttribute('mixed');
                    xml.documentElement.appendChild(element);
                    element.setAttribute('AtTaChEd', 'xml');
                    const attachedXML = element.getAttribute('AtTaChEd') === 'xml' &&
                        element.getAttribute('attached') === null;
                    document.adoptNode(element);
                    element.setAttribute('AfTeR', 'html');
                    return detachedXML && attachedXML && element.parentNode === null &&
                        element.nodeName === 'P' && element.tagName === 'P' &&
                        element.ownerDocument === document && element.getAttribute('after') === 'html' &&
                        element.getAttributeNode('before').name === 'before' &&
                        element.getAttributeNode('after').name === 'after';
                })()"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn attribute_case_normalizes_cross_realm_html_wrappers() {
    let mut runtime = runtime();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
                    const child = document.getElementById('f').contentWindow;
                    const element = child.document.createElement('p');
                    const foreignPrototype = Object.getPrototypeOf(element);
                    const set = Element.prototype.setAttribute;
                    const get = Element.prototype.getAttribute;
                    set.call(element, 'CrOsS-ReAlM', 'value');
                    const foreign = foreignPrototype === child.HTMLParagraphElement.prototype &&
                        element.ownerDocument === child.document && get.call(element, 'CROSS-REALM') === 'value';
                    document.adoptNode(element);
                    set.call(element, 'AdOpTeD', 'yes');
                    return foreign && Object.getPrototypeOf(element) === foreignPrototype &&
                        element.ownerDocument === document && get.call(element, 'adopted') === 'yes';
                })()"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn native_document_content_type_is_not_an_author_expando() {
    let mut runtime = runtime();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
                    const xml = new DOMParser().parseFromString('<root/>', 'text/xml');
                    const html = document.implementation.createHTMLDocument('inert');
                    xml.__contentType = 'text/html';
                    html.__contentType = 'application/xml';
                    document.__contentType = 'application/xml';
                    const element = html.createElement('p');
                    element.setAttribute('MiXeD', 'value');
                    return document.contentType === 'text/html' && xml.contentType === 'text/xml' &&
                        xml.cloneNode(true).contentType === 'text/xml' && html.contentType === 'text/html' &&
                        element.getAttribute('mixed') === 'value' &&
                        typeof globalThis.__omoikane_html_element_in_html_document === 'undefined' &&
                        typeof globalThis.__omoikane_set_document_content_type === 'undefined';
                })()"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}
