//! The browser and standalone engine must expose identical binary16 semantics.

use omoikane::{dom::NodeHandle, js::JsRuntime};

#[test]
fn browser_float16_conversions_preserve_values_and_order() {
    let mut runtime = JsRuntime::with_document(NodeHandle::document()).unwrap();
    let result = runtime
        .eval(include_str!(
            "../engine/boa/core/engine/tests/assets/float16_rounding.js"
        ))
        .expect("browser binary16 conversion contract");
    assert_eq!(result.as_boolean(), Some(true));
}
