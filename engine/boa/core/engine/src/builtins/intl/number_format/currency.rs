//! Currency text, precision and spacing come from typed CLDR payloads.

use super::{
    compact_pattern::CompactPattern,
    data,
    options::{Currency, CurrencyDisplay, CurrencySign},
    output::{Edge, NumberOutput},
    pattern::{NumberPattern, placeholders},
};
use crate::{JsNativeError, JsResult, context::icu::IntlProvider};
use boa_intl_data::{
    CurrencyPatternField as Field, CurrencyTextField as Text, NumberSymbol, NumberSymbols,
    OmoikaneCurrencyDigitsV1, OmoikaneCurrencyPatternsV1, OmoikaneCurrencyTextV1,
    OmoikaneGlobalCurrencyPatternsV1, PluralCategory,
};
use fixed_decimal::Sign;
use icu_locale::Locale;
use icu_properties::{
    CodePointMapData,
    props::{GeneralCategory, GeneralCategoryGroup},
};
use icu_provider::{DataErrorKind, DataPayload};

#[derive(Debug)]
pub(super) struct CurrencyFormatter {
    code: String,
    display: CurrencyDisplay,
    label: String,
    pattern: NumberPattern,
    patterns: DataPayload<OmoikaneCurrencyPatternsV1>,
    text: Option<DataPayload<OmoikaneCurrencyTextV1>>,
    categories: CodePointMapData<GeneralCategory>,
    before: Spacing,
    after: Spacing,
}

impl CurrencyFormatter {
    pub(super) fn new(
        provider: &IntlProvider,
        locale: &Locale,
        attributes: &str,
        currency: Currency,
        display: CurrencyDisplay,
        sign: CurrencySign,
    ) -> JsResult<Self> {
        let patterns = data::locale_or_global::<
            OmoikaneCurrencyPatternsV1,
            OmoikaneGlobalCurrencyPatternsV1,
        >(provider, locale, attributes)?;
        let text =
            match data::try_load::<OmoikaneCurrencyTextV1>(provider, locale, currency.as_str()) {
                Ok(payload) => Some(payload),
                Err(error) if error.kind == DataErrorKind::IdentifierNotFound => None,
                Err(error) => return Err(data::data_error(error)),
            };
        let data = patterns.get();
        let categories = CodePointMapData::<GeneralCategory>::new().static_to_owned();
        let label = currency_label(
            currency.as_str(),
            display,
            text.as_ref().map(DataPayload::get),
        );
        let (normal, alpha) = if sign == CurrencySign::Accounting {
            (Field::Accounting, Field::AccountingAlphaNextToNumber)
        } else {
            (Field::Standard, Field::StandardAlphaNextToNumber)
        };
        let override_pattern = text
            .as_ref()
            .and_then(|text| text.get().get(Text::PatternOverride));
        let raw = override_pattern
            .or_else(|| data.get(normal))
            .ok_or_else(|| JsNativeError::typ().with_message("missing CLDR currency pattern"))?;
        let mut pattern = NumberPattern::parse(raw)?;
        if override_pattern.is_none()
            && pattern.currency_adjacent_to_letter(&label, |character| {
                GeneralCategoryGroup::Letter.contains(categories.as_borrowed().get(character))
            })
        {
            if let Some(alpha) = data.get(alpha) {
                pattern = NumberPattern::parse(alpha)?;
            }
        }
        if display == CurrencyDisplay::Name
            && data.unit_patterns.get(PluralCategory::Other).is_none()
        {
            return Err(JsNativeError::typ()
                .with_message("missing CLDR currency name pattern")
                .into());
        }
        let before = Spacing::new(
            data.get(Field::BeforeCurrencyMatch),
            data.get(Field::BeforeSurroundingMatch),
            data.get(Field::BeforeInsertBetween),
        )?;
        let after = Spacing::new(
            data.get(Field::AfterCurrencyMatch),
            data.get(Field::AfterSurroundingMatch),
            data.get(Field::AfterInsertBetween),
        )?;
        Ok(Self {
            code: currency.as_str().to_owned(),
            display,
            label,
            pattern,
            patterns,
            text,
            categories,
            before,
            after,
        })
    }

    pub(super) fn format<O: NumberOutput>(
        &self,
        number: O,
        sign: Sign,
        category: PluralCategory,
        symbols: &NumberSymbols<'_>,
        decimal: &NumberPattern,
    ) -> O {
        if self.display == CurrencyDisplay::Name {
            let number = decimal.render(number, sign, symbols, "");
            return self.format_name(number, category);
        }
        self.pattern
            .render_currency(number, sign, symbols, &self.label, self)
    }

    pub(super) fn format_compact<O: NumberOutput>(
        &self,
        number: O,
        sign: Sign,
        category: PluralCategory,
        symbols: &NumberSymbols<'_>,
        normal: &CompactPattern,
        alpha: Option<&CompactPattern>,
    ) -> O {
        if self.display == CurrencyDisplay::Name {
            return self.format_name(normal.render(number, sign, symbols, ""), category);
        }
        let pattern = if normal.currency_adjacent_to_letter(&self.label, |character| {
            GeneralCategoryGroup::Letter.contains(self.categories.as_borrowed().get(character))
        }) {
            alpha.unwrap_or(normal)
        } else {
            normal
        };
        pattern.render_currency(number, sign, symbols, &self.label, self)
    }

    fn format_name<O: NumberOutput>(&self, number: O, category: PluralCategory) -> O {
        let text = self.text.as_ref().map(DataPayload::get);
        let label = text
            .and_then(|text| {
                text.plural_names
                    .get_or_other(category)
                    .or_else(|| text.get(Text::DisplayName))
            })
            .unwrap_or(&self.code);
        let pattern = self
            .patterns
            .get()
            .unit_patterns
            .get_or_other(category)
            .expect("validated currency name pattern");
        placeholders(pattern, number, label, "currency")
    }

    pub(super) fn separator<'a>(
        &'a self,
        symbol: NumberSymbol,
        symbols: &'a NumberSymbols<'_>,
    ) -> &'a str {
        let text = self.text.as_ref().map(DataPayload::get);
        match symbol {
            NumberSymbol::Decimal => text
                .and_then(|text| text.get(Text::DecimalOverride))
                .or_else(|| symbols.get(NumberSymbol::CurrencyDecimal)),
            NumberSymbol::Group => text
                .and_then(|text| text.get(Text::GroupOverride))
                .or_else(|| symbols.get(NumberSymbol::CurrencyGroup)),
            _ => None,
        }
        .or_else(|| symbols.get(symbol))
        .expect("validated numeric symbol")
    }

    /// Applies CLDR spacing from semantic edges, without scanning numeric text.
    pub(super) fn spacing_between(&self, left: Edge, right: Edge) -> Option<&str> {
        let (spacing, currency, number) = if left.kind == "currency" && numeric(right.kind) {
            (&self.after, left.character, right.character)
        } else if numeric(left.kind) && right.kind == "currency" {
            (&self.before, right.character, left.character)
        } else {
            return None;
        };
        (!spacing.insert.is_empty() && spacing.matches(currency, number, &self.categories))
            .then_some(spacing.insert.as_str())
    }
}

pub(super) fn fraction_digits(provider: &IntlProvider, currency: Currency) -> JsResult<u8> {
    Ok(data::singleton::<OmoikaneCurrencyDigitsV1>(provider)?
        .get()
        .digits(currency.as_str()))
}

fn currency_label(
    code: &str,
    display: CurrencyDisplay,
    text: Option<&boa_intl_data::CurrencyText<'_>>,
) -> String {
    let field = match display {
        CurrencyDisplay::Symbol => Text::Symbol,
        CurrencyDisplay::NarrowSymbol => Text::NarrowSymbol,
        CurrencyDisplay::Name => Text::DisplayName,
        CurrencyDisplay::Code => return code.to_owned(),
    };
    text.and_then(|text| text.get(field).or_else(|| text.get(Text::Symbol)))
        .unwrap_or(code)
        .to_owned()
}

fn numeric(kind: &str) -> bool {
    matches!(
        kind,
        "integer" | "fraction" | "decimal" | "nan" | "infinity"
    )
}

#[derive(Debug)]
struct Spacing {
    currency: UnicodeMatch,
    surrounding: UnicodeMatch,
    insert: String,
}

impl Spacing {
    fn new(
        currency: Option<&str>,
        surrounding: Option<&str>,
        insert: Option<&str>,
    ) -> JsResult<Self> {
        Ok(Self {
            currency: UnicodeMatch::parse(currency)?,
            surrounding: UnicodeMatch::parse(surrounding)?,
            insert: insert.unwrap_or_default().to_owned(),
        })
    }

    fn matches(
        &self,
        currency: char,
        number: char,
        categories: &CodePointMapData<GeneralCategory>,
    ) -> bool {
        let categories = categories.as_borrowed();
        self.currency.matches(categories.get(currency))
            && self.surrounding.matches(categories.get(number))
    }
}

#[derive(Debug)]
enum UnicodeMatch {
    None,
    NotSymbol,
    NotSeparator,
    NotSymbolOrSeparator,
    Digit,
    Letter,
}

impl UnicodeMatch {
    fn parse(pattern: Option<&str>) -> JsResult<Self> {
        match pattern {
            None => Ok(Self::None),
            Some("[:^S:]") => Ok(Self::NotSymbol),
            Some("[:^Z:]") => Ok(Self::NotSeparator),
            Some("[[:^S:]&[:^Z:]]") => Ok(Self::NotSymbolOrSeparator),
            Some("[:digit:]") | Some("[:Nd:]") => Ok(Self::Digit),
            Some("[:letter:]") | Some("[:L:]") => Ok(Self::Letter),
            Some(_) => Err(JsNativeError::typ()
                .with_message("unsupported CLDR currency spacing UnicodeSet")
                .into()),
        }
    }

    fn matches(&self, category: GeneralCategory) -> bool {
        match self {
            Self::None => false,
            Self::NotSymbol => !GeneralCategoryGroup::Symbol.contains(category),
            Self::NotSeparator => !GeneralCategoryGroup::Separator.contains(category),
            Self::NotSymbolOrSeparator => {
                !GeneralCategoryGroup::Symbol.contains(category)
                    && !GeneralCategoryGroup::Separator.contains(category)
            }
            Self::Digit => category == GeneralCategory::DecimalNumber,
            Self::Letter => GeneralCategoryGroup::Letter.contains(category),
        }
    }
}
