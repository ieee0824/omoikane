//! Shared coercion and typed-data loading for the native name services.
use crate::{Context, JsNativeError, JsObject, JsResult, js_string};
use icu_locale::Locale;
use icu_provider::prelude::*;

pub(super) fn option(
    options: &JsObject,
    key: &str,
    allowed: &[&str],
    fallback: Option<&str>,
    context: &mut Context,
) -> JsResult<String> {
    let value = options.get(js_string!(key), context)?;
    if value.is_undefined() {
        return fallback.map(str::to_owned).ok_or_else(|| {
            JsNativeError::typ()
                .with_message(format!("missing required option {key}"))
                .into()
        });
    }
    let value = value.to_string(context)?.to_std_string_escaped();
    if allowed.contains(&value.as_str()) {
        Ok(value)
    } else {
        Err(JsNativeError::range()
            .with_message(format!("invalid option {key}"))
            .into())
    }
}

pub(super) fn load<M: DataMarker>(context: &Context, locale: &Locale) -> JsResult<DataPayload<M>>
where
    crate::context::icu::IntlProvider: DataProvider<M>,
{
    let locale = DataLocale::from(locale.id.clone());
    DataProvider::<M>::load(
        context.intl_provider(),
        DataRequest {
            id: DataIdentifierBorrowed::for_locale(&locale),
            metadata: Default::default(),
        },
    )
    .map(|response| response.payload)
    .map_err(|error| JsNativeError::typ().with_message(error.to_string()).into())
}
