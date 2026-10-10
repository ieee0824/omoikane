use crate::PluralCategory;
use zerovec::{ZeroMap2d, ule::AsULE};

/// Raw CLDR compact-number patterns for one locale, family and numbering system.
///
/// First keys are positive source thresholds (10^n), rather than exponents n;
/// second keys are the stable [`PluralCategory`] indices. Raw `0` sentinels,
/// explicit counts, quotes and patterns omitting the number remain unchanged.
#[derive(Clone, Debug, PartialEq, yoke::Yokeable, zerofrom::ZeroFrom, serde::Deserialize)]
// ZeroMap2d storage projections obscure covariance; project the lifetime using
// each field's existing Yokeable implementation, as in ICU4X map payloads.
#[yoke(prove_covariance_manually)]
#[cfg_attr(feature = "datagen", derive(serde::Serialize, databake::Bake))]
#[cfg_attr(feature = "datagen", databake(path = boa_intl_data))]
pub struct CompactPatterns<'data> {
    /// Normal patterns by source threshold and plural category index.
    #[serde(borrow)]
    pub normal: ZeroMap2d<'data, u64, u8, str>,
    /// Currency patterns used beside alphabetic currency text, when present.
    /// A category need not also have a normal pattern at the same threshold.
    #[serde(borrow)]
    pub alpha_next_to_number: ZeroMap2d<'data, u64, u8, str>,
}

impl CompactPatterns<'_> {
    /// Returns the exact normal pattern without plural or threshold fallback.
    pub fn get(&self, magnitude: u64, category: PluralCategory) -> Option<&str> {
        self.normal.get_2d(&magnitude, &(category as u8))
    }

    /// Returns the exact alphabetic-currency variant, without normal fallback.
    pub fn get_alpha_next_to_number(
        &self,
        magnitude: u64,
        category: PluralCategory,
    ) -> Option<&str> {
        self.alpha_next_to_number
            .get_2d(&magnitude, &(category as u8))
    }

    /// Iterates normal-pattern source thresholds in ascending order.
    /// Alpha variants use a subset of these thresholds, but may add categories.
    pub fn magnitudes(&self) -> impl Iterator<Item = u64> + '_ {
        self.normal
            .iter0()
            .map(|cursor| u64::from_unaligned(*cursor.key0()))
    }
}

icu_provider::data_struct!(CompactPatterns<'_>, #[cfg(feature = "datagen")]);
icu_provider::data_marker!(
    /// Compact patterns. Attributes are `decimal-short-<nu>`,
    /// `decimal-long-<nu>` or `currency-short-<nu>`. Only actual CLDR records
    /// are exported; numbering-system fallback belongs to the consumer.
    OmoikaneCompactPatternsV1, CompactPatterns<'static>,
);
