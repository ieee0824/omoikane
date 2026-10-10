use super::support::*;
use boa_intl_data::{
    CurrencyDigits, CurrencyPatterns, CurrencyText, OmoikaneCurrencyDigitsV1,
    OmoikaneCurrencyPatternsV1, OmoikaneCurrencyTextV1,
};
use icu_provider_blob::export::BlobExporter;
use serde_json::Value;
use zerovec::ZeroMap;

pub fn patterns(record: &Value) -> Result<CurrencyPatterns<'static>> {
    let patterns = &record["patterns"];
    let before = &record["currencySpacing"]["beforeCurrency"];
    let after = &record["currencySpacing"]["afterCurrency"];
    Ok(CurrencyPatterns {
        fields: fields(&[
            string(&patterns["standard"], "raw"),
            string(&patterns["accounting"], "raw"),
            string(&patterns["standard-alphaNextToNumber"], "raw"),
            string(&patterns["accounting-alphaNextToNumber"], "raw"),
            string(&patterns["standard-noCurrency"], "raw"),
            string(&patterns["accounting-noCurrency"], "raw"),
            string(record, "currencyPatternAppendISO"),
            string(before, "currencyMatch"),
            string(before, "surroundingMatch"),
            string(before, "insertBetween"),
            string(after, "currencyMatch"),
            string(after, "surroundingMatch"),
            string(after, "insertBetween"),
        ])?,
        unit_patterns: plurals(&record["unitPatterns"], "")?,
    })
}

fn currency_text(record: &Value) -> Result<CurrencyText<'static>> {
    Ok(CurrencyText {
        fields: fields(&[
            string(record, "symbol"),
            string(record, "symbol-alt-narrow"),
            string(record, "symbol-alt-formal"),
            string(record, "symbol-alt-variant"),
            string(record, "displayName"),
            string(record, "decimal"),
            string(record, "group"),
            string(&record["patternOverride"], "raw"),
        ])?,
        plural_names: plurals(&record["pluralNames"], "")?,
    })
}

fn integer(record: &Value, key: &str) -> Result<u16> {
    let integer = record
        .get(key)
        .and_then(Value::as_u64)
        .ok_or("missing precision integer")?;
    Ok(u16::try_from(integer)?)
}

fn precision(record: &Value) -> Result<CurrencyDigits<'static>> {
    let mut normal_digits = ZeroMap::<str, u8>::new();
    let mut normal_rounding = ZeroMap::<str, u16>::new();
    let mut cash_digits = ZeroMap::<str, u8>::new();
    let mut cash_rounding = ZeroMap::<str, u16>::new();
    for (code, item) in object(&record["currencies"])? {
        let code = code.as_str();
        normal_digits.insert(code, &u8::try_from(integer(item, "effectiveDigits")?)?);
        if item.get("rounding").is_some() {
            normal_rounding.insert(code, &integer(item, "rounding")?);
        }
        if item.get("cashDigits").is_some() {
            cash_digits.insert(code, &u8::try_from(integer(item, "cashDigits")?)?);
        }
        if item.get("cashRounding").is_some() {
            cash_rounding.insert(code, &integer(item, "cashRounding")?);
        }
    }
    Ok(CurrencyDigits {
        default_digits: u8::try_from(integer(record, "defaultDigits")?)?,
        normal_digits,
        normal_rounding,
        cash_digits,
        cash_rounding,
    })
}

pub fn export(source: &Source, exporter: &BlobExporter<'_>, counts: &mut Counts) -> Result<()> {
    let data = source.read("currency_patterns.json")?;
    let metadata = source.read("locale_metadata.json")?;
    let mut count = 0;
    for (locale, systems) in object(&data)? {
        for (nu, record) in object(systems)? {
            put::<OmoikaneCurrencyPatternsV1>(exporter, locale, nu, patterns(record)?)?;
            count += 1;
        }
        let default = text(&metadata["locales"][locale]["defaultNumberingSystem"])?;
        let record = systems
            .get(default)
            .ok_or("default currency patterns are missing")?;
        put::<OmoikaneCurrencyPatternsV1>(exporter, locale, "", patterns(record)?)?;
        count += 1;
    }
    counts.insert("OmoikaneCurrencyPatternsV1", count);
    let data = source.read("currency_names.json")?;
    let mut count = 0;
    for (locale, currencies) in object(&data)? {
        for (code, record) in object(currencies)? {
            put::<OmoikaneCurrencyTextV1>(exporter, locale, code, currency_text(record)?)?;
            count += 1;
        }
    }
    counts.insert("OmoikaneCurrencyTextV1", count);
    put::<OmoikaneCurrencyDigitsV1>(
        exporter,
        "und",
        "",
        precision(&source.read("currency_digits.json")?)?,
    )?;
    counts.insert("OmoikaneCurrencyDigitsV1", 1);
    Ok(())
}
