//! Unit widths and grammatical patterns are selected from typed CLDR data.

use super::{
    data,
    options::{Unit, UnitDisplay},
    output::NumberOutput,
    pattern::placeholders,
};
use crate::{JsNativeError, JsResult, context::icu::IntlProvider};
use boa_intl_data::{
    OmoikaneCompoundUnitPatternsV1, OmoikaneUnitPatternsV1, PluralCategory, UnitWidth,
    canonical_compound_unit_cldr_id, sanctioned_simple_unit,
};
use icu_locale::Locale;
use icu_provider::{DataErrorKind, DataPayload};

#[derive(Debug)]
pub(super) struct UnitFormatter {
    numerator: DataPayload<OmoikaneUnitPatternsV1>,
    denominator: Option<DataPayload<OmoikaneUnitPatternsV1>>,
    compound: Option<DataPayload<OmoikaneCompoundUnitPatternsV1>>,
}

impl UnitFormatter {
    pub(super) fn new(
        provider: &IntlProvider,
        locale: &Locale,
        unit: &Unit,
        display: UnitDisplay,
    ) -> JsResult<Self> {
        let width = match display {
            UnitDisplay::Long => UnitWidth::Long,
            UnitDisplay::Short => UnitWidth::Short,
            UnitDisplay::Narrow => UnitWidth::Narrow,
        };
        let name = unit.to_js_string().to_std_string_escaped();
        if let Some(numerator) = load_canonical_compound(provider, locale, width, &name)? {
            return Ok(Self {
                numerator,
                denominator: None,
                compound: None,
            });
        }
        let (numerator, denominator) = name
            .split_once("-per-")
            .map_or((name.as_str(), None), |(numerator, denominator)| {
                (numerator, Some(denominator))
            });
        let numerator = load_unit(provider, locale, width, numerator)?;
        let denominator = denominator
            .map(|name| load_unit(provider, locale, width, name))
            .transpose()?;
        let compound = if denominator.is_some() {
            let compound =
                data::load::<OmoikaneCompoundUnitPatternsV1>(provider, locale, width.as_str())?;
            if compound.get().per_pattern().is_none() {
                return Err(JsNativeError::typ()
                    .with_message("missing CLDR compound unit pattern")
                    .into());
            }
            Some(compound)
        } else {
            None
        };
        Ok(Self {
            numerator,
            denominator,
            compound,
        })
    }

    pub(super) fn format<O: NumberOutput>(&self, number: O, category: PluralCategory) -> O {
        let pattern = self
            .numerator
            .get()
            .patterns
            .get_or_other(category)
            .expect("validated unit pattern");
        let numerator = placeholders(pattern, number, "", "unit");
        let Some(denominator) = &self.denominator else {
            return numerator;
        };
        let denominator = denominator.get();
        let mut parts = if let Some(pattern) = denominator.per_unit_pattern() {
            placeholders(pattern, numerator, "", "unit")
        } else {
            let label = denominator
                .patterns
                .get_or_other(PluralCategory::One)
                .expect("validated unit pattern")
                .replace("{0}", "");
            let pattern = self
                .compound
                .as_ref()
                .expect("compound payload")
                .get()
                .per_pattern()
                .expect("validated compound pattern");
            placeholders(pattern, numerator, label.trim(), "unit")
        };
        parts.merge_unit_labels();
        parts
    }
}

/// Prefer CLDR's precomposed unit over separately translated numerator/denominator.
fn load_canonical_compound(
    provider: &IntlProvider,
    locale: &Locale,
    width: UnitWidth,
    name: &str,
) -> JsResult<Option<DataPayload<OmoikaneUnitPatternsV1>>> {
    let Some(id) = canonical_compound_unit_cldr_id(name) else {
        return Ok(None);
    };
    let attributes = format!("{}-{id}", width.as_str());
    match data::try_load::<OmoikaneUnitPatternsV1>(provider, locale, &attributes) {
        Ok(payload) => validate_unit(payload).map(Some),
        Err(error) if error.kind == DataErrorKind::IdentifierNotFound => Ok(None),
        Err(error) => Err(data::data_error(error)),
    }
}

fn load_unit(
    provider: &IntlProvider,
    locale: &Locale,
    width: UnitWidth,
    name: &str,
) -> JsResult<DataPayload<OmoikaneUnitPatternsV1>> {
    let attributes = format!(
        "{}-{}",
        width.as_str(),
        sanctioned_simple_unit(name)
            .map(|(_, cldr)| cldr)
            .ok_or_else(|| JsNativeError::range().with_message("unsupported unit"))?
    );
    let payload = data::load::<OmoikaneUnitPatternsV1>(provider, locale, &attributes)?;
    validate_unit(payload)
}

fn validate_unit(
    payload: DataPayload<OmoikaneUnitPatternsV1>,
) -> JsResult<DataPayload<OmoikaneUnitPatternsV1>> {
    if payload.get().patterns.get(PluralCategory::Other).is_none() {
        return Err(JsNativeError::typ()
            .with_message("missing CLDR unit pattern")
            .into());
    }
    Ok(payload)
}
