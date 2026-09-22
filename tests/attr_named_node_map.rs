//! Live `Attr` nodes and `NamedNodeMap` behavior.

use omoikane::js::JsRuntime;

fn evaluate_string(source: &str) -> String {
    JsRuntime::new()
        .expect("create Attr runtime")
        .eval(source)
        .expect("evaluate Attr probe")
        .as_string()
        .expect("Attr probe must return a string")
        .to_std_string_escaped()
}

#[test]
fn attr_nodes_and_named_node_maps_are_live_and_stable() {
    let actual = evaluate_string(
        r#"(() => {
            const element = document.createElement('div');
            const other = document.createElement('div');
            const attributes = element.attributes;
            element.setAttribute('alpha', 'one');
            const alpha = attributes.getNamedItem('alpha');
            const stable = attributes === element.attributes &&
                alpha === element.getAttributeNode('alpha') &&
                alpha === attributes.item(0);
            alpha.value = 'two';
            element.setAttribute('alpha', 'three');
            element.removeAttribute('alpha');
            const detached = alpha.ownerElement === null && alpha.value === 'three';
            element.setAttributeNode(alpha);
            let inUse;
            try { other.setAttributeNode(alpha); } catch (error) { inUse = error.name; }

            const replacement = document.createAttribute('alpha');
            replacement.value = 'replacement';
            const replaced = attributes.setNamedItem(replacement);
            const removed = element.removeAttributeNode(replacement);

            const firstNs = document.createAttributeNS('urn:first', 'item');
            firstNs.value = 'first';
            const secondNs = document.createAttributeNS('urn:second', 'item');
            secondNs.value = 'second';
            attributes.setNamedItemNS(firstNs);
            attributes.setNamedItemNS(secondNs);
            const namespacePair = attributes.getNamedItemNS('urn:first', 'item') === firstNs &&
                attributes.getNamedItemNS('urn:second', 'item') === secondNs;
            const removedNs = attributes.removeNamedItemNS('urn:first', 'item');

            const nullNamespace = document.createAttributeNS(null, 'collision');
            const stringNamespace = document.createAttributeNS('null', 'collision');
            attributes.setNamedItemNS(nullNamespace);
            attributes.setNamedItemNS(stringNamespace);
            const namespaceCacheIsDistinct =
                attributes.getNamedItemNS(null, 'collision') === nullNamespace &&
                attributes.getNamedItemNS('null', 'collision') === stringNamespace &&
                nullNamespace !== stringNamespace;

            const popover = document.createElement('div');
            document.body.appendChild(popover);
            popover.setAttribute('popover', 'manual');
            popover.showPopover();
            popover.setAttributeNS(null, 'popover', 'auto');
            const namespaceSetClosedPopover = !popover.matches(':popover-open');
            popover.showPopover();
            popover.removeAttributeNS(null, 'popover');
            const namespaceRemovalClosedPopover = !popover.matches(':popover-open');

            const movingElement = document.createElement('moving');
            movingElement.setAttribute('moving', 'value');
            const movingAttribute = movingElement.getAttributeNode('moving');
            const foreignDocument = new Document();
            foreignDocument.appendChild(movingElement);
            const attachedOwnerDocumentMoved = movingAttribute.ownerDocument === foreignDocument;
            const importedAttribute = foreignDocument.importNode(
                document.createAttribute('imported'), false
            );
            const importedOwnerDocumentMoved = importedAttribute.ownerDocument === foreignDocument;

            const checkbox = document.createElement('input');
            checkbox.type = 'checkbox';
            const checked = document.createAttribute('checked');
            checkbox.attributes.setNamedItem(checked);
            const checkedAfterAttach = checkbox.checked;
            checkbox.attributes.removeNamedItem('checked');
            const checkedAfterDetach = checkbox.checked;

            return [
                attributes instanceof NamedNodeMap, stable,
                detached, alpha.ownerElement === null,
                inUse, replaced === alpha, alpha.ownerElement === null,
                removed === replacement, replacement.ownerElement === null,
                namespacePair, removedNs === firstNs, firstNs.ownerElement === null,
                attributes.length, secondNs.ownerDocument === document,
                checkedAfterAttach, checkedAfterDetach,
                namespaceCacheIsDistinct, namespaceSetClosedPopover,
                namespaceRemovalClosedPopover,
                attachedOwnerDocumentMoved, importedOwnerDocumentMoved
            ].join('|');
        })()"#,
    );
    assert_eq!(
        actual,
        "true|true|true|true|InUseAttributeError|true|true|true|true|true|true|true|3|true|true|false|true|true|true|true|true"
    );
}

#[test]
fn namespace_replacement_preserves_identity_order_and_method_names() {
    let actual = evaluate_string(
        r#"(() => {
            const element = document.createElement('div');
            element.setAttributeNS('urn:first', 'p:item', 'first');
            element.setAttributeNS('urn:second', 'item', 'second');
            element.setAttribute('tail', 'tail');
            const first = element.getAttributeNodeNS('urn:first', 'item');
            element.setAttributeNS('urn:first', 'q:item', 'updated');
            const map = element.attributes;
            const clone = first.cloneNode();
            element.setAttributeNS('urn:method', 'item', 'method');
            return [
                first === element.getAttributeNodeNS('urn:first', 'item'),
                first.name, first.prefix, first.value,
                map[0] === first, map[1].namespaceURI, map[2].name,
                typeof map.item, map.item === NamedNodeMap.prototype.item,
                clone.ownerElement === null, clone.ownerDocument === document,
                clone.isEqualNode(first),
                element.getAttributeNS('urn:second', 'item'),
                element.getAttribute('item')
            ].join('|');
        })()"#,
    );
    assert_eq!(
        actual,
        "true|p:item|p|updated|true|urn:second|tail|function|true|true|true|true|second|second"
    );
}
