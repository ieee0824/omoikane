//! XPath 1.0 DOM bindings and evaluation behavior.

use omoikane::js::JsRuntime;

fn evaluate_boolean(source: &str) -> bool {
    JsRuntime::new()
        .expect("create XPath runtime")
        .eval(source)
        .expect("evaluate XPath probe")
        .as_boolean()
        .expect("XPath probe must return a boolean")
}

#[test]
fn evaluates_xpath_operators_functions_axes_and_predicates() {
    assert!(evaluate_boolean(
        r#"(() => {
            document.body.innerHTML = '<section><p class="hit"> one </p><p>two</p><p class="hit">three</p></section>';
            const section = document.querySelector('section');
            const nodes = document.evaluate(
                './/p[@class="hit"][position() = last()]', section, null,
                XPathResult.ORDERED_NODE_SNAPSHOT_TYPE, null
            );
            return nodes.snapshotLength === 1 && nodes.snapshotItem(0).textContent === 'three' &&
                document.evaluate('count(.//p) + 2 * 3', section, null,
                    XPathResult.NUMBER_TYPE, null).numberValue === 9 &&
                document.evaluate('normalize-space(string(.//p[1]))', section, null,
                    XPathResult.STRING_TYPE, null).stringValue === 'one' &&
                document.evaluate('.//p[1]/following-sibling::p[1] = "two"', section, null,
                    XPathResult.BOOLEAN_TYPE, null).booleanValue;
        })()"#,
    ));
}

#[test]
fn resolves_namespaces_and_returns_attribute_nodes() {
    assert!(evaluate_boolean(
        r#"(() => {
            const doc = new DOMParser().parseFromString(
                '<r xmlns="urn:r" xmlns:x="urn:x"><x:item x:key="value"/></r>',
                'application/xml'
            );
            const resolver = prefix => ({r: 'urn:r', x: 'urn:x'})[prefix] || null;
            const element = doc.evaluate('/r:r/x:item', doc, resolver,
                XPathResult.FIRST_ORDERED_NODE_TYPE, null).singleNodeValue;
            const attribute = doc.evaluate('@x:key', element, resolver,
                XPathResult.FIRST_ORDERED_NODE_TYPE, null).singleNodeValue;
            return element.localName === 'item' && attribute instanceof Attr &&
                attribute.localName === 'key' && attribute.namespaceURI === 'urn:x' &&
                attribute.ownerElement === element && attribute.value === 'value';
        })()"#,
    ));
}

#[test]
fn exposes_result_modes_and_invalidates_iterators_after_mutation() {
    assert!(evaluate_boolean(
        r#"(() => {
            document.body.innerHTML = '<ul><li>a</li><li>b</li></ul>';
            const iterator = document.evaluate('//li', document, null,
                XPathResult.ORDERED_NODE_ITERATOR_TYPE, null);
            const first = iterator.iterateNext();
            document.querySelector('ul').appendChild(document.createElement('li'));
            let invalid = iterator.invalidIteratorState;
            let threw = false;
            try { iterator.iterateNext(); } catch (error) {
                threw = error instanceof DOMException && error.name === 'InvalidStateError';
            }
            const snapshot = document.evaluate('//li', document, null,
                XPathResult.ORDERED_NODE_SNAPSHOT_TYPE, null);
            const any = document.evaluate('true()', document, null, XPathResult.ANY_TYPE, null);
            return first.textContent === 'a' && invalid && threw &&
                snapshot.snapshotLength === 3 && snapshot.snapshotItem(3) === null &&
                any.resultType === XPathResult.BOOLEAN_TYPE && any.booleanValue;
        })()"#,
    ));
}

#[test]
fn compiles_expressions_and_reports_xpath_errors() {
    assert!(evaluate_boolean(
        r#"(() => {
            const evaluator = new XPathEvaluator();
            const expression = evaluator.createExpression('ancestor-or-self::*[1]');
            const result = expression.evaluate(document.body, XPathResult.FIRST_ORDERED_NODE_TYPE, null);
            let syntax = false;
            let namespace = false;
            let nodeType = false;
            try { document.evaluate('//*[', document, null); }
            catch (error) { syntax = error instanceof DOMException && error.name === 'SyntaxError'; }
            try { document.evaluate('//missing:item', document, null); }
            catch (error) { namespace = error instanceof DOMException && error.name === 'NamespaceError'; }
            try { document.evaluate('.', document.doctype, null); }
            catch (error) { nodeType = error instanceof DOMException && error.name === 'NotSupportedError'; }
            return result.singleNodeValue === document.body && syntax && namespace && nodeType &&
                evaluator.createNSResolver(document.body) === document.body;
        })()"#,
    ));
}

#[test]
fn preserves_tree_order_across_unions_and_shadow_roots() {
    assert!(evaluate_boolean(
        r#"(() => {
            const doc = document.implementation.createHTMLDocument('xpath');
            doc.documentElement.innerHTML = '<body><div></div></body>';
            const div = doc.documentElement.querySelector('div');
            const union = doc.evaluate('(.//div)[1] | .', doc.documentElement);
            const unionNodes = [union.iterateNext(), union.iterateNext(), union.iterateNext()];
            const host = document.createElement('div');
            const shadow = host.attachShadow({mode: 'open'});
            const span = document.createElement('span');
            shadow.appendChild(span);
            const shadowResult = new XPathEvaluator().evaluate('//span', span, null,
                XPathResult.FIRST_ORDERED_NODE_TYPE, null);
            return unionNodes[0] === doc.documentElement &&
                unionNodes[1] === div && unionNodes[2] === null &&
                shadowResult.singleNodeValue === span;
        })()"#,
    ));
}

#[test]
fn uses_xpath_ascii_case_and_xml_whitespace_rules() {
    assert!(evaluate_boolean(
        r#"(() => {
            const context = document.createElement('div');
            context.setAttribute('nonÄsciiAttribute', '');
            context.textContent = '\u00a0  \u3000';
            document.body.appendChild(context);
            const matches = document.evaluate('//*[@nonÄsciiAttribute]', document, null,
                XPathResult.ORDERED_NODE_SNAPSHOT_TYPE, null);
            const misses = document.evaluate('//*[@nonäsciiattribute]', document, null,
                XPathResult.ORDERED_NODE_SNAPSHOT_TYPE, null);
            const normalized = document.evaluate('normalize-space(.)', context, null,
                XPathResult.STRING_TYPE, null).stringValue;
            return matches.snapshotLength === 1 && misses.snapshotLength === 0 &&
                normalized === '\u00a0 \u3000';
        })()"#,
    ));
}

#[test]
fn evaluates_all_xpath_axes_and_remaining_core_functions() {
    assert!(evaluate_boolean(
        r#"(() => {
            document.body.innerHTML = '<main><i id="before"></i><section id="context" a="4"><b><em></em></b><u></u></section><i id="after"></i></main>';
            const context = document.getElementById('context');
            const truth = expression => document.evaluate(expression, context, null,
                XPathResult.BOOLEAN_TYPE, null).booleanValue;
            const number = expression => document.evaluate(expression, context, null,
                XPathResult.NUMBER_TYPE, null).numberValue;
            return truth('self::section') && truth('parent::main') &&
                truth('ancestor::html') && truth('ancestor-or-self::section') &&
                truth('child::b') && truth('descendant::em') &&
                truth('descendant-or-self::section') && truth('attribute::a') &&
                truth('preceding-sibling::i[@id="before"]') &&
                truth('following-sibling::i[@id="after"]') &&
                truth('preceding::i[@id="before"]') && truth('following::i[@id="after"]') &&
                number('sum(@a)') === 4 && number('floor(1.9)') === 1 &&
                number('ceiling(1.1)') === 2 && number('round(-1.5)') === -1 &&
                document.evaluate('translate("bar", "ab", "AB")', context, null,
                    XPathResult.STRING_TYPE, null).stringValue === 'BAr';
        })()"#,
    ));
}
