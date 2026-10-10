use zerovec::VarZeroVec;

/// Indexed optional strings with a stable serialized representation.
///
/// Missing strings have a clear presence bit. Present empty strings remain
/// distinct, including unit patterns that deliberately omit a numeric portion.
#[derive(Clone, Debug, PartialEq, yoke::Yokeable, zerofrom::ZeroFrom, serde::Deserialize)]
#[cfg_attr(feature = "datagen", derive(serde::Serialize, databake::Bake))]
#[cfg_attr(feature = "datagen", databake(path = boa_intl_data))]
pub struct StringFields<'data> {
    /// Concatenated zero-copy string fields in schema-defined order.
    #[serde(borrow)]
    pub values: VarZeroVec<'data, str>,
    /// Bit `i` is set when field `i` is present.
    pub present: u64,
}

impl StringFields<'_> {
    /// Returns a field, preserving the distinction between missing and empty.
    pub fn get(&self, index: usize) -> Option<&str> {
        if index >= u64::BITS as usize || self.present & (1_u64 << index) == 0 {
            return None;
        }
        self.values.get(index)
    }

    /// Builds the owned representation used by generators and custom providers.
    /// Returns `None` when more than 64 fields are supplied.
    pub fn from_fields(fields: &[Option<&str>]) -> Option<StringFields<'static>> {
        if fields.len() > u64::BITS as usize {
            return None;
        }
        let values = fields
            .iter()
            .map(|field| field.unwrap_or_default())
            .collect::<alloc::vec::Vec<_>>();
        let present = fields.iter().enumerate().fold(0, |bits, (index, field)| {
            bits | (u64::from(field.is_some()) << index)
        });
        Some(StringFields {
            values: VarZeroVec::from(&values),
            present,
        })
    }
}

/// CLDR plural categories, including explicit zero and one overrides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PluralCategory {
    /// Cardinal zero category.
    Zero,
    /// Cardinal one category.
    One,
    /// Cardinal two category.
    Two,
    /// Cardinal few category.
    Few,
    /// Cardinal many category.
    Many,
    /// Cardinal fallback category.
    Other,
    /// Explicit numeric zero override.
    ExplicitZero,
    /// Explicit numeric one override.
    ExplicitOne,
}

impl PluralCategory {
    /// Source CLDR count name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Zero => "zero",
            Self::One => "one",
            Self::Two => "two",
            Self::Few => "few",
            Self::Many => "many",
            Self::Other => "other",
            Self::ExplicitZero => "0",
            Self::ExplicitOne => "1",
        }
    }

    /// Stable field order in [`PluralPatterns`].
    pub const ALL: [Self; 8] = [
        Self::Zero,
        Self::One,
        Self::Two,
        Self::Few,
        Self::Many,
        Self::Other,
        Self::ExplicitZero,
        Self::ExplicitOne,
    ];
}

/// Borrowed CLDR strings selected by plural category.
#[derive(Clone, Debug, PartialEq, yoke::Yokeable, zerofrom::ZeroFrom, serde::Deserialize)]
#[cfg_attr(feature = "datagen", derive(serde::Serialize, databake::Bake))]
#[cfg_attr(feature = "datagen", databake(path = boa_intl_data))]
pub struct PluralPatterns<'data> {
    /// Patterns or names indexed by [`PluralCategory`].
    #[serde(borrow)]
    pub fields: StringFields<'data>,
}

impl PluralPatterns<'_> {
    /// Returns the exact category without applying fallback.
    pub fn get(&self, category: PluralCategory) -> Option<&str> {
        self.fields.get(category as usize)
    }

    /// Returns the requested category or its CLDR `other` fallback.
    pub fn get_or_other(&self, category: PluralCategory) -> Option<&str> {
        self.get(category)
            .or_else(|| self.get(PluralCategory::Other))
    }
}
