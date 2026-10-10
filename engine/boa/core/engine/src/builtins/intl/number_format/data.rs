//! Typed provider lookups occur once, during native formatter initialization.

use crate::{JsNativeError, JsResult, context::icu::IntlProvider};
use icu_locale::Locale;
use icu_provider::prelude::*;

pub(super) fn try_load<M: DataMarker>(
    provider: &IntlProvider,
    locale: &Locale,
    attributes: &str,
) -> Result<DataPayload<M>, DataError>
where
    IntlProvider: DataProvider<M>,
{
    let locale = DataLocale::from(locale.id.clone());
    let attributes = DataMarkerAttributes::try_from_str(attributes)
        .map_err(|_| DataErrorKind::InvalidRequest.into_error())?;
    let request = DataRequest {
        id: DataIdentifierBorrowed::for_marker_attributes_and_locale(attributes, &locale),
        metadata: Default::default(),
    };
    DataProvider::<M>::load(provider, request).map(|response| response.payload)
}

pub(super) fn load<M: DataMarker>(
    provider: &IntlProvider,
    locale: &Locale,
    attributes: &str,
) -> JsResult<DataPayload<M>>
where
    IntlProvider: DataProvider<M>,
{
    try_load::<M>(provider, locale, attributes).map_err(data_error)
}

pub(super) fn locale_or_global<M, G>(
    provider: &IntlProvider,
    locale: &Locale,
    attributes: &str,
) -> JsResult<DataPayload<M>>
where
    M: DataMarker,
    G: DataMarker<DataStruct = M::DataStruct>,
    IntlProvider: DataProvider<M> + DataProvider<G>,
{
    match try_load::<M>(provider, locale, attributes) {
        Ok(payload) => Ok(payload),
        Err(error) if error.kind == DataErrorKind::IdentifierNotFound && !attributes.is_empty() => {
            load::<G>(provider, &icu_locale::locale!("und"), attributes).map(DataPayload::cast)
        }
        Err(error) => Err(data_error(error)),
    }
}

pub(super) fn data_error(error: DataError) -> crate::JsError {
    JsNativeError::typ().with_message(error.to_string()).into()
}

pub(super) fn singleton<M: DataMarker>(provider: &IntlProvider) -> JsResult<DataPayload<M>>
where
    IntlProvider: DataProvider<M>,
{
    DataProvider::<M>::load(provider, Default::default())
        .map(|response| response.payload)
        .map_err(|error| JsNativeError::typ().with_message(error.to_string()).into())
}
