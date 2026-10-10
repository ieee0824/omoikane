use crate::{PluralCategory, StringFields};
use zerovec::ZeroVec;

/// Zero-copy CLDR number-range patterns and ordered cardinal plural rules.
///
/// The schema preserves raw miscellaneous patterns and missing rules. It does
/// not decide affix collapse, approximation, or plural fallback policy.
#[derive(Clone, Debug, PartialEq, yoke::Yokeable, zerofrom::ZeroFrom, serde::Deserialize)]
#[cfg_attr(feature = "datagen", derive(serde::Serialize, databake::Bake))]
#[cfg_attr(feature = "datagen", databake(path = boa_intl_data))]
pub struct NumberRangePatterns<'data> {
    /// Raw `range` and `approximately` patterns, in that order.
    #[serde(borrow)]
    pub fields: StringFields<'data>,
    /// Cardinal result indices, indexed by `start * 6 + end`.
    ///
    /// Only indices zero through five, corresponding to the cardinal variants
    /// of [`PluralCategory`], are valid results. Explicit numeric overrides are
    /// not cardinal range categories.
    #[serde(borrow)]
    pub plural_results: ZeroVec<'data, u8>,
    /// Bit `start * 6 + end` distinguishes an explicit rule from a missing one.
    pub plural_present: u64,
}

impl NumberRangePatterns<'_> {
    /// Returns the exact CLDR range pattern, including spacing and quoting.
    pub fn range(&self) -> Option<&str> {
        self.fields.get(0)
    }

    /// Returns the raw approximate pattern, independently of the sign symbol.
    pub fn approximately(&self) -> Option<&str> {
        self.fields.get(1)
    }

    /// Returns an explicit ordered cardinal rule, without applying fallback.
    ///
    /// CLDR 47 defines an absent pair's default as the end category and leaves
    /// negative or nonascending numeric ranges unspecified. A consumer chooses
    /// its policy; this data-only API receives categories, not numeric values.
    /// Invalid custom-provider results and explicit zero/one overrides return
    /// `None`, preserving the distinction from an explicit `other` rule.
    pub fn plural_range(
        &self,
        start: PluralCategory,
        end: PluralCategory,
    ) -> Option<PluralCategory> {
        if start as usize >= 6 || end as usize >= 6 {
            return None;
        }
        let index = start as usize * 6 + end as usize;
        if self.plural_present & (1_u64 << index) == 0 {
            return None;
        }
        let result = self.plural_results.get(index)? as usize;
        if result >= 6 {
            return None;
        }
        PluralCategory::ALL.get(result).copied()
    }
}

icu_provider::data_struct!(NumberRangePatterns<'_>, #[cfg(feature = "datagen")]);
icu_provider::data_marker!(
    /// Number range data with numbering-system attributes. Empty attributes
    /// select the locale default. Bundled explicit systems absent from a locale
    /// follow the CLDR root's same-locale Latin alias, preserving local labels.
    OmoikaneNumberRangePatternsV1, NumberRangePatterns<'static>,
);
