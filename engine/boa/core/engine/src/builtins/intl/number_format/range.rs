//! Native range formatting uses exact endpoints and owned semantic parts.

use boa_intl_data::{NumberSymbol, OmoikaneNumberRangePatternsV1, OmoikaneNumberSymbolsV1};
use icu_locale::Locale;
use icu_provider::{DataErrorKind, DataPayload};

use super::{
    MathematicalValue, NativeNumberFormatter, NumberFormat, NumericFormatOptions, data, input,
    options::UnitFormatOptions,
    parts::{NumberPart, push_symbol},
    range_affixes::{self, Collapse},
    range_pattern::RangePattern,
};
use crate::{
    Context, JsArgs, JsNativeError, JsResult, JsValue, builtins::Array, context::icu::IntlProvider,
    js_string, object::ObjectInitializer, property::Attribute,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Source {
    Start,
    End,
    Shared,
}

impl Source {
    fn as_str(self) -> &'static str {
        match self {
            Self::Start => "startRange",
            Self::End => "endRange",
            Self::Shared => "shared",
        }
    }
}

#[derive(Debug)]
pub(super) struct RangePart {
    kind: &'static str,
    value: String,
    source: Source,
}

impl RangePart {
    pub(super) fn shared(kind: &'static str, value: String) -> Self {
        Self {
            kind,
            value,
            source: Source::Shared,
        }
    }

    pub(super) fn from_shared(part: NumberPart) -> Self {
        Self::shared(part.kind, part.value)
    }

    fn endpoint(part: NumberPart, source: Source) -> Self {
        Self {
            kind: part.kind,
            value: part.value,
            source,
        }
    }
}

/// Provider payloads and parsed templates are owned; partitioning has no I/O.
#[derive(Debug)]
pub(super) struct NativeNumberRangeFormatter {
    patterns: DataPayload<OmoikaneNumberRangePatternsV1>,
    range: RangePattern,
    approximately_sign: String,
    collapse: Collapse,
}

impl NativeNumberRangeFormatter {
    pub(super) fn new(
        provider: &IntlProvider,
        locale: &Locale,
        formatter: &NativeNumberFormatter,
        style: &UnitFormatOptions,
    ) -> JsResult<Self> {
        let patterns = data::load::<OmoikaneNumberRangePatternsV1>(
            provider,
            locale,
            formatter.numbering_system(),
        )?;
        let missing = || JsNativeError::typ().with_message("missing CLDR number range pattern");
        let range = RangePattern::parse(patterns.get().range().ok_or_else(missing)?, 2)?;
        let approximately_sign = approximate_symbol(provider, locale, formatter)?;
        Ok(Self {
            patterns,
            range,
            approximately_sign,
            collapse: Collapse::for_style(style),
        })
    }

    fn partition(
        &self,
        formatter: &NativeNumberFormatter,
        options: &NumericFormatOptions,
        start: MathematicalValue,
        end: MathematicalValue,
    ) -> JsResult<Vec<RangePart>> {
        if matches!(start, MathematicalValue::NaN) || matches!(end, MathematicalValue::NaN) {
            return Err(JsNativeError::range()
                .with_message("number range endpoint is NaN")
                .into());
        }
        let requires_plural_reformat = matches!(self.collapse, Collapse::Unit);
        let (mut left, mut right, retained_endpoints) = if requires_plural_reformat {
            (
                formatter.format_endpoint(start.clone(), options, None),
                formatter.format_endpoint(end.clone(), options, None),
                Some((start, end)),
            )
        } else {
            (
                formatter.format_endpoint(start, options, None),
                formatter.format_endpoint(end, options, None),
                None,
            )
        };
        if equal_rendered_values(&left.parts, &right.parts) {
            return Ok(self.approximate(left.parts));
        }
        // The raw category-pair relation preserves argument order, including
        // descending and negative ranges. No positive-range ICU API is used.
        if let Some((start, end)) = retained_endpoints {
            let category = self
                .patterns
                .get()
                .plural_range(left.category, right.category)
                .unwrap_or(right.category);
            if left.category != category {
                left = formatter.format_endpoint(start, options, Some(category));
            }
            if right.category != category {
                right = formatter.format_endpoint(end, options, Some(category));
            }
        }
        let shared = range_affixes::collapse(&mut left.parts, &mut right.parts, self.collapse);
        let space = range_affixes::needs_spacing(&left.parts, &right.parts);
        let start = left
            .parts
            .into_iter()
            .map(|part| RangePart::endpoint(part, Source::Start))
            .collect();
        let end = right
            .parts
            .into_iter()
            .map(|part| RangePart::endpoint(part, Source::End))
            .collect();
        let mut result: Vec<_> = shared
            .prefix
            .into_iter()
            .map(RangePart::from_shared)
            .collect();
        result.extend(self.range.range(start, end, space));
        result.extend(shared.suffix.into_iter().map(RangePart::from_shared));
        Ok(result)
    }

    fn approximate(&self, parts: Vec<NumberPart>) -> Vec<RangePart> {
        // Intl uses the resolved number symbol, rather than the legacy
        // miscPatterns.approximately template (which can add obsolete spaces).
        let mut sign = Vec::new();
        push_symbol(&mut sign, "approximatelySign", &self.approximately_sign);
        sign.into_iter()
            .chain(parts)
            .map(RangePart::from_shared)
            .collect()
    }
}

/// Part boundaries and empty values do not affect the rendered UTF-8 stream.
pub(super) fn equal_rendered_values(start: &[NumberPart], end: &[NumberPart]) -> bool {
    start
        .iter()
        .flat_map(|part| part.value.bytes())
        .eq(end.iter().flat_map(|part| part.value.bytes()))
}

/// Joins owned range parts while retaining the leading part's allocation.
pub(super) fn parts_into_string(parts: Vec<RangePart>) -> String {
    let mut parts = parts.into_iter();
    let Some(first) = parts.next() else {
        return String::new();
    };
    let additional = parts.as_slice().iter().map(|part| part.value.len()).sum();
    let mut output = first.value;
    output.reserve(additional);
    for part in parts {
        output.push_str(&part.value);
    }
    output
}

fn approximate_symbol(
    provider: &IntlProvider,
    locale: &Locale,
    formatter: &NativeNumberFormatter,
) -> JsResult<String> {
    let local =
        data::try_load::<OmoikaneNumberSymbolsV1>(provider, locale, formatter.numbering_system());
    let missing = || JsNativeError::typ().with_message("missing CLDR approximately sign");
    match local {
        Ok(_) => formatter
            .approximately_sign()
            .map(str::to_owned)
            .ok_or_else(|| missing().into()),
        Err(error) if error.kind == DataErrorKind::IdentifierNotFound => {
            // Numeric digits may use global data, while an approximate word
            // remains in the same locale's default symbol family.
            let symbols = data::load::<OmoikaneNumberSymbolsV1>(provider, locale, "")?;
            symbols
                .get()
                .get(NumberSymbol::ApproximatelySign)
                .map(str::to_owned)
                .ok_or_else(|| missing().into())
        }
        Err(error) => Err(data::data_error(error)),
    }
}

/// Require the ordinary brand before either endpoint is converted.
fn partition(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<Vec<RangePart>> {
    let object = this
        .as_object()
        .and_then(|object| object.downcast::<NumberFormat>().ok())
        .ok_or_else(|| JsNativeError::typ().with_message("receiver is not an Intl.NumberFormat"))?;
    let _object_root = object.clone().root();
    let start = args.get_or_undefined(0);
    let end = args.get_or_undefined(1);
    if start.is_undefined() || end.is_undefined() {
        return Err(JsNativeError::typ()
            .with_message("number range endpoint is undefined")
            .into());
    }
    let start = input::to_mathematical_value(start, context)?;
    let end = input::to_mathematical_value(end, context)?;
    let data = object.borrow();
    let data = data.data();
    data.range_formatter.partition(
        &data.formatter,
        &NumericFormatOptions {
            digits: data.digit_options.clone(),
            sign_display: data.sign_display,
        },
        start,
        end,
    )
}

pub(super) fn format_range(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let parts = partition(this, args, context)?;
    Ok(js_string!(parts_into_string(parts)).into())
}

pub(super) fn format_range_to_parts(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let parts = partition(this, args, context)?;
    let array = Array::array_create(0, None, context)?;
    let _array_root = array.clone().root();
    for (index, part) in parts.into_iter().enumerate() {
        let object = ObjectInitializer::new(context)
            .property(js_string!("type"), js_string!(part.kind), Attribute::all())
            .property(
                js_string!("value"),
                js_string!(part.value),
                Attribute::all(),
            )
            .property(
                js_string!("source"),
                js_string!(part.source.as_str()),
                Attribute::all(),
            )
            .build();
        let _object_root = object.clone().root();
        array.create_data_property_or_throw(index, object, context)?;
    }
    Ok(array.into())
}
