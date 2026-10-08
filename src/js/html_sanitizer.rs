//! Owned sanitizer configurations and filtering of newly parsed, inert DOM trees.
//!
//! Conversion from author objects happens in `html_sanitizer.js`; this module
//! does not call JavaScript while filtering or borrow the active document.
use super::*;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

mod configuration;
mod filtering;
mod modifiers;
use configuration::{Config, Name};

const HTML: &str = "http://www.w3.org/1999/xhtml";
const SVG: &str = "http://www.w3.org/2000/svg";
const MATHML: &str = "http://www.w3.org/1998/Math/MathML";
const XLINK: &str = "http://www.w3.org/1999/xlink";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Constants {
    default: Config,
    unsafe_elements: Vec<Name>,
    navigating_attributes: Vec<(Name, Name)>,
    event_attributes: Vec<String>,
}

static CONSTANTS: LazyLock<Constants> = LazyLock::new(|| {
    serde_json::from_str(include_str!("html_sanitizer/constants.json"))
        .expect("checked-in normative sanitizer constants")
});

pub(super) fn configuration_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let source = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let permissive = args.get(1).is_some_and(JsValue::to_boolean);
    let mut config = if source == "default" {
        CONSTANTS.default.clone()
    } else {
        serde_json::from_str::<Config>(&source)
            .map_err(|_| JsNativeError::typ().with_message("Invalid sanitizer configuration"))?
    };
    config.canonicalize(permissive);
    config
        .validate()
        .map_err(|message| JsNativeError::typ().with_message(message))?;
    Ok(js_string!(serde_json::to_string(&config).expect("owned configuration")).into())
}

pub(super) fn modify_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let source = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let operation = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let value = args
        .get(2)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let mut config: Config = serde_json::from_str(&source)
        .map_err(|_| JsNativeError::typ().with_message("Invalid sanitizer configuration"))?;
    let argument = serde_json::from_str(&value)
        .map_err(|_| JsNativeError::typ().with_message("Invalid sanitizer argument"))?;
    let changed = config
        .modify(&operation, argument)
        .map_err(|message| JsNativeError::typ().with_message(message))?;
    debug_assert!(config.validate().is_ok());
    Ok(js_string!(serde_json::json!({"configuration": if operation == "get" { config.json() } else { serde_json::to_string(&config).expect("owned configuration") }, "changed": changed}).to_string()).into())
}

/// Decodes already converted private data before any parsed tree is attached.
pub(super) fn from_argument(
    value: Option<&JsValue>,
    context: &mut Context,
) -> JsResult<Option<Config>> {
    let Some(value) = value.filter(|value| !value.is_null_or_undefined()) else {
        return Ok(None);
    };
    let source = value.to_string(context)?.to_std_string_escaped();
    let config: Config = serde_json::from_str(&source)
        .map_err(|_| JsNativeError::typ().with_message("Invalid sanitizer configuration"))?;
    config
        .validate()
        .map_err(|message| JsNativeError::typ().with_message(message))?;
    Ok(Some(config))
}

pub(super) fn sanitize(tree: &NodeHandle, config: &Config, safe: bool) {
    let mut effective = config.clone();
    if safe {
        effective.remove_unsafe();
    }
    filtering::sanitize_children(tree, &effective);
}
