//! HTML `inert` reflection and interaction suppression.

use omoikane::html::TreeBuilder;
use omoikane::js::{FindInPageAction, JsRuntime};

fn runtime(source: &str) -> JsRuntime {
    JsRuntime::with_document(TreeBuilder::parse(source).document()).unwrap()
}

#[test]
fn inert_reflects_on_html_elements_only() {
    let mut runtime = runtime("<!doctype html><body><div id='target'></div></body>");
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
                  const descriptor = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'inert');
                  const target = document.getElementById('target');
                  if (!descriptor?.enumerable || typeof descriptor.get !== 'function' ||
                      typeof descriptor.set !== 'function' || target.inert) return false;
                  target.inert = true;
                  if (!target.hasAttribute('inert') || !target.inert) return false;
                  target.inert = 0;
                  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
                  return !target.hasAttribute('inert') && !('inert' in svg);
                })()"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn inert_flat_tree_subtree_cannot_receive_focus_or_pointer_hit() {
    let mut runtime = runtime(
        "<!doctype html><style>body{margin:0} #back,#host{position:absolute;left:0;top:0;width:80px;height:40px} #back{background:red} #host{background:blue}</style><body><button id='back'>back</button><div id='host' inert><button id='front'>front</button></div></body>",
    );
    runtime.set_viewport(100.0, 60.0);
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
                  const front = document.getElementById('front');
                  front.focus();
                  return document.activeElement !== front && document.elementFromPoint(10, 10).id === 'back';
                })()"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn inertness_follows_assigned_slots_in_the_flat_tree() {
    let mut runtime = runtime(
        "<!doctype html><body><div id='host'><button id='light'>light</button></div></body>",
    );
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
                  const host = document.getElementById('host');
                  const root = host.attachShadow({ mode: 'open' });
                  const slot = document.createElement('slot');
                  slot.inert = true;
                  root.appendChild(slot);
                  const light = document.getElementById('light');
                  light.focus();
                  return light.assignedSlot === slot && document.activeElement !== light;
                })()"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn inert_text_is_excluded_from_find_in_page() {
    let mut runtime = runtime(
        "<!doctype html><body><p inert>needle</p><p>needle</p><p inert><span>needle</span></p></body>",
    );
    let result = runtime
        .find_in_page(FindInPageAction::Start, "needle")
        .unwrap();
    assert_eq!(result.match_count, 1);
}

#[test]
fn inert_text_is_excluded_from_user_selection_text() {
    let mut runtime =
        runtime("<!doctype html><body><p inert>hidden</p><p id='shown'>shown</p></body>");
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
                  const selection = getSelection();
                  selection.selectAllChildren(document.body);
                  return selection.toString().trim();
                })()"#,
            )
            .unwrap()
            .as_string()
            .map(|value| value.to_std_string_escaped()),
        Some("shown".to_string())
    );
}

#[test]
fn modal_dialog_text_escapes_an_inert_ancestor_for_find_in_page() {
    let mut runtime = runtime(
        "<!doctype html><body><main inert>hidden<dialog id='dialog'>needle</dialog></main></body>",
    );
    runtime
        .eval("document.getElementById('dialog').showModal()")
        .unwrap();
    let result = runtime
        .find_in_page(FindInPageAction::Start, "needle")
        .unwrap();
    assert_eq!(result.match_count, 1);
}

#[test]
fn modal_dialog_escapes_an_inert_ancestor_unless_it_is_itself_inert() {
    let mut runtime = runtime(
        "<!doctype html><body><main inert><dialog id='dialog'><button id='inside'>inside</button></dialog></main></body>",
    );
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
                  const dialog = document.getElementById('dialog');
                  const inside = document.getElementById('inside');
                  dialog.showModal();
                  inside.focus();
                  if (document.activeElement !== inside) return false;
                  dialog.inert = true;
                  inside.blur();
                  inside.focus();
                  return document.activeElement !== inside;
                })()"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn topmost_modal_dialog_makes_the_rest_of_the_document_inert() {
    let mut runtime = runtime(
        "<!doctype html><body><button id='outside'>outside needle</button><dialog id='dialog'><button id='inside'>inside needle</button></dialog></body>",
    );
    runtime
        .eval("document.getElementById('dialog').showModal()")
        .unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
                  const outside = document.getElementById('outside');
                  const inside = document.getElementById('inside');
                  outside.focus();
                  if (document.activeElement === outside) return false;
                  inside.focus();
                  return document.activeElement === inside;
                })()"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    let result = runtime
        .find_in_page(FindInPageAction::Start, "needle")
        .unwrap();
    assert_eq!(result.match_count, 1);
}
