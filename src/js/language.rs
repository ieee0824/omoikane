//! Host language values passed privately to Navigator bootstrap.

use super::{BootstrapBindings, register_private_property};
use boa_engine::{
    Context, JsResult, JsValue, js_string, object::builtins::JsArray, property::Attribute,
};

pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    let preferences = crate::language::preferences();
    register_private_property(
        context,
        bindings,
        js_string!("__omoikane_navigator_language"),
        js_string!(preferences.primary()),
        Attribute::all(),
    )?;
    let languages = JsArray::from_iter(
        preferences
            .languages()
            .iter()
            .map(|language| JsValue::from(js_string!(language.as_str()))),
        context,
    );
    register_private_property(
        context,
        bindings,
        js_string!("__omoikane_navigator_languages"),
        languages,
        Attribute::all(),
    )
}
