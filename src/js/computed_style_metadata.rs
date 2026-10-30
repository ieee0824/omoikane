//! CSSOM enumeration reads names and counts without materializing CSS values.
use super::*;
use boa_engine::object::builtins::JsArray;

/// Resolves the current declaration using the same document, origin, layout and
/// logical aliases as a complete CSSOM read, then exports only its metadata.
pub(super) fn read_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    let style = resolve_native_computed_style(node_id, args, true, true, context)?;
    let operation = args.get(2).and_then(JsValue::as_string);
    match operation
        .as_ref()
        .map(|name| name.to_std_string_escaped())
        .as_deref()
    {
        Some("length") => Ok(style
            .as_ref()
            .map_or(0, |style| style.property_count())
            .into()),
        Some("item") => {
            // The private JS caller supplies only an already validated u32;
            // object coercion stays in JS after its names snapshot is taken.
            let index = args
                .get(3)
                .and_then(JsValue::as_number)
                .unwrap_or(f64::INFINITY);
            let name = if index.is_finite() && index >= 0.0 && index.fract() == 0.0 {
                style
                    .as_ref()
                    .and_then(|style| style.property_names().nth(index as usize))
            } else {
                None
            };
            Ok(js_string!(name.unwrap_or("")).into())
        }
        _ => {
            let names = style
                .as_ref()
                .into_iter()
                .flat_map(|style| style.property_names())
                .map(|name| JsValue::from(js_string!(name)));
            Ok(JsArray::from_iter(names, context).into())
        }
    }
}
