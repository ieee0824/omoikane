//! CSS selector support queries at the JavaScript and cascade boundaries.

use omoikane::{html::TreeBuilder, js::JsRuntime};

#[test]
fn stylesheet_selectors_are_reported_by_css_supports() {
    let document = TreeBuilder::parse(
        "<style>\
         @supports selector(::marker) { #probe { color: green; } }\
         @supports selector(::unknown) { #probe { color: red; } }\
         </style><div id=probe></div>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).expect("runtime");
    let result = runtime
        .eval(
            r#"(() => {
              const supported = ['::marker', '::first-letter', '::placeholder',
                'x-card::part(label)', '::slotted(.item)', ':host', ':host(.active)',
                ':host(:is(.first, .second))'];
              const unsupported = ['::unknown', 'div, span', ':host(> span)',
                ':is(.known, :unknown)', ':is(.known, ::before)',
                ':has(::before)', ':has(:has(.item))',
                ':host(:is(.known, :unknown))',
                '::slotted(:is(.known, :unknown))'];
              return supported.every(selector => CSS.supports(`selector(${selector})`)) &&
                unsupported.every(selector => !CSS.supports(`selector(${selector})`)) &&
                getComputedStyle(document.getElementById('probe')).color === 'rgb(0, 128, 0)';
            })()"#,
        )
        .expect("CSS.supports evaluation");
    assert_eq!(result.as_boolean(), Some(true));
}
