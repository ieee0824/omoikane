//! HTMLElement `outerText` reflection and replacement behavior.

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
fn outer_text_is_an_enumerable_html_element_attribute_with_inner_text_getter() {
    check(
        r#"(() => {
          const descriptor = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'outerText');
          if (!descriptor || !descriptor.enumerable || typeof descriptor.get !== 'function' ||
              typeof descriptor.set !== 'function') return false;
          const element = document.createElement('div');
          element.append('visible   ');
          return element.outerText === element.innerText && element.outerText === 'visible';
        })()"#,
    );
}

#[test]
fn outer_text_replaces_with_text_and_breaks_and_merges_only_adjacent_text() {
    check(
        r#"(() => {
          const container = document.createElement('div');
          container.append('A', 'B', document.createElement('span'), 'D', 'E');
          container.childNodes[2].outerText = 'x\r\ny\rz';
          if (container.innerHTML !== 'ABx<br>y<br>zDE') return false;

          const merged = document.createElement('div');
          merged.append('1', '2', document.createElement('span'), '3', '4');
          merged.childNodes[2].outerText = '';
          if (merged.childNodes.length !== 3 || merged.childNodes[0].data !== '1' ||
              merged.childNodes[1].data !== '23' || merged.childNodes[2].data !== '4') return false;

          const detached = document.createElement('span');
          try { detached.outerText = ''; return false; }
          catch (error) { return error.name === 'NoModificationAllowedError'; }
        })()"#,
    );
}

#[test]
fn outer_text_preserves_dom_string_data_and_is_not_exposed_on_svg_or_mathml() {
    check(
        r#"(() => {
          const container = document.createElement('div');
          const element = document.createElement('span');
          container.append(element);
          element.outerText = null;
          if (container.textContent !== '') return false;
          const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
          const math = document.createElementNS('http://www.w3.org/1998/Math/MathML', 'math');
          return !('outerText' in svg) && !('outerText' in math);
        })()"#,
    );
}

#[test]
fn outer_text_converts_once_at_the_setter_boundary_before_tree_mutation() {
    check(
        r#"(() => {
          const container = document.createElement('div');
          const element = document.createElement('span');
          container.append(element);
          let conversions = 0;
          element.outerText = { toString() { conversions++; return 'value'; } };
          if (conversions !== 1 || container.textContent !== 'value') return false;

          const detached = document.createElement('span');
          container.replaceChildren(detached);
          const value = { toString() { detached.remove(); return 'detached'; } };
          try { detached.outerText = value; return false; }
          catch (error) {
            if (error.name !== 'NoModificationAllowedError' || container.textContent !== '') return false;
          }

          const symbolTarget = document.createElement('span');
          container.append(symbolTarget);
          try { symbolTarget.outerText = Symbol('invalid'); return false; }
          catch (error) { return error instanceof TypeError && symbolTarget.parentNode === container; }
        })()"#,
    );
}
