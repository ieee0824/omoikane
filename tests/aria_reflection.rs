//! ARIAMixin string and element-reference reflection.

use omoikane::html::TreeBuilder;
use omoikane::js::JsRuntime;

fn runtime(source: &str) -> JsRuntime {
    JsRuntime::with_document(TreeBuilder::parse(source).document()).unwrap()
}

#[test]
fn aria_string_properties_reflect_nullable_content_attributes() {
    let mut runtime = runtime("<!doctype html><body><div id='target' role='button'></div></body>");
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
                  const target = document.getElementById('target');
                  const descriptor = Object.getOwnPropertyDescriptor(Element.prototype, 'ariaLabel');
                  if (!descriptor?.enumerable || target.role !== 'button') return false;
                  target.role = null;
                  target.ariaLabel = 'Save';
                  if (target.hasAttribute('role') || target.getAttribute('aria-label') !== 'Save') return false;
                  target.ariaLabel = undefined;
                  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
                  svg.ariaHidden = 'true';
                  return target.ariaLabel === null && svg.getAttribute('aria-hidden') === 'true';
                })()"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn aria_element_references_preserve_identity_and_follow_attribute_mutations() {
    let mut runtime = runtime(
        "<!doctype html><body><input id='control' aria-labelledby='first'><span id='first'>One</span><span id='second'>Two</span></body>",
    );
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
                  const control = document.getElementById('control');
                  const first = document.getElementById('first');
                  const second = document.getElementById('second');
                  const parsed = control.ariaLabelledByElements;
                  if (parsed.length !== 1 || parsed[0] !== first || parsed !== control.ariaLabelledByElements) return false;
                  control.ariaLabelledByElements = [second, first];
                  const explicit = control.ariaLabelledByElements;
                  if (control.getAttribute('aria-labelledby') !== '' || explicit[0] !== second || explicit[1] !== first || !Object.isFrozen(explicit)) return false;
                  control.setAttribute('aria-labelledby', 'second');
                  return control.ariaLabelledByElements[0] === second;
                })()"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn aria_element_references_enforce_tree_scope_without_losing_explicit_targets() {
    let mut runtime = runtime(
        "<!doctype html><body><div id='owner'></div><div id='host'></div><span id='label'>Label</span></body>",
    );
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
                  const owner = document.getElementById('owner');
                  const host = document.getElementById('host');
                  const label = document.getElementById('label');
                  const root = host.attachShadow({mode:'open'});
                  root.append(label);
                  owner.ariaActiveDescendantElement = label;
                  if (owner.ariaActiveDescendantElement !== null || owner.getAttribute('aria-activedescendant') !== '') return false;
                  owner.append(label);
                  if (owner.ariaActiveDescendantElement !== label) return false;
                  root.append(label);
                  if (owner.ariaActiveDescendantElement !== null) return false;
                  const shadowOwner = document.createElement('div');
                  root.append(shadowOwner);
                  shadowOwner.ariaActiveDescendantElement = owner;
                  return shadowOwner.ariaActiveDescendantElement === owner;
                })()"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn aria_element_setter_notifies_attribute_observers_and_custom_elements() {
    let mut runtime = runtime("<!doctype html><body><span id='label'>Label</span></body>");
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
                  const calls = [];
                  class AriaTest extends HTMLElement {
                    static get observedAttributes() { return ['aria-labelledby']; }
                    attributeChangedCallback(name, oldValue, newValue) {
                      calls.push([oldValue, newValue, this.ariaLabelledByElements?.length]);
                    }
                  }
                  customElements.define('aria-test', AriaTest);
                  const element = document.createElement('aria-test');
                  document.body.append(element);
                  const observer = new MutationObserver(() => {});
                  observer.observe(element, {attributes: true, attributeOldValue: true});
                  element.ariaLabelledByElements = [document.getElementById('label')];
                  element.ariaLabelledByElements = null;
                  const records = observer.takeRecords();
                  return calls.length === 2 && calls[0][0] === null && calls[0][1] === '' &&
                    calls[0][2] === 1 && calls[1][0] === '' && calls[1][1] === null &&
                    records.length === 2 && records[0].attributeName === 'aria-labelledby' &&
                    records[0].oldValue === null && records[1].oldValue === '';
                })()"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}
