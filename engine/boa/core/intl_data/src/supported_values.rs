use alloc::vec::Vec;
use zerovec::ZeroMap2d;

/// Canonical CLDR inventories used as candidates for provider-backed enumeration.
#[derive(Clone, Debug, PartialEq, yoke::Yokeable, zerofrom::ZeroFrom, serde::Deserialize)]
#[yoke(prove_covariance_manually)]
#[cfg_attr(feature = "datagen", derive(serde::Serialize, databake::Bake))]
#[cfg_attr(feature = "datagen", databake(path = boa_intl_data))]
pub struct SupportedValues<'data> {
    /// Category / canonical identifier / reserved zero value.
    /// `locales` supplies candidate locales for collation availability queries.
    #[serde(borrow)]
    pub values: ZeroMap2d<'data, str, str, u8>,
}

impl SupportedValues<'_> {
    /// Iterates a sorted source inventory without choosing runtime availability.
    pub fn identifiers(&self, category: &str) -> impl Iterator<Item = &str> {
        self.values
            .get0(category)
            .into_iter()
            .flat_map(|cursor| cursor.iter1().map(|(key, _)| key).collect::<Vec<_>>())
    }
}

icu_provider::data_struct!(SupportedValues<'_>, #[cfg(feature = "datagen")]);
icu_provider::data_marker!(
    /// Global canonical CLDR inventories. Locale is `und`, attributes empty.
    OmoikaneSupportedValuesV1, SupportedValues<'static>,
);
