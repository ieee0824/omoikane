use super::support::*;
use boa_intl_data::{
    NumberPatternField, NumberPatterns, NumberSymbol, NumberSymbols,
    OmoikaneGlobalNumberPatternsV1, OmoikaneGlobalNumberSymbolsV1, OmoikaneNumberPatternsV1,
    OmoikaneNumberSymbolsV1,
};
use icu_provider_blob::export::BlobExporter;
use serde_json::Value;

pub fn symbols(value: &Value, numbering_system: &str) -> Result<NumberSymbols<'static>> {
    let values = NumberSymbol::ALL.map(|field| match field {
        NumberSymbol::NumberingSystem => Some(numbering_system),
        NumberSymbol::CurrencyDecimal => {
            string(value, "currencyDecimal").or_else(|| string(value, "decimal"))
        }
        NumberSymbol::CurrencyGroup => {
            string(value, "currencyGroup").or_else(|| string(value, "group"))
        }
        field => string(value, field.as_str()),
    });
    Ok(NumberSymbols {
        fields: fields(&values)?,
    })
}

pub fn export(source: &Source, exporter: &BlobExporter<'_>, counts: &mut Counts) -> Result<()> {
    let data = source.read("number_symbols.json")?;
    let metadata = source.read("locale_metadata.json")?;
    let mut count = 0;
    for (locale, systems) in object(&data)? {
        for (nu, record) in object(systems)? {
            put::<OmoikaneNumberSymbolsV1>(exporter, locale, nu, symbols(record, nu)?)?;
            count += 1;
        }
        let default = text(&metadata["locales"][locale]["defaultNumberingSystem"])?;
        let record = systems
            .get(default)
            .ok_or("default numbering-system symbols are missing")?;
        put::<OmoikaneNumberSymbolsV1>(exporter, locale, "", symbols(record, default)?)?;
        count += 1;
    }
    counts.insert("OmoikaneNumberSymbolsV1", count);
    let root = source.read("root_number_symbols.json")?;
    let root = object(&root["numberingSystemSymbols"])?;
    for (nu, record) in root {
        put::<OmoikaneGlobalNumberSymbolsV1>(
            exporter,
            "und",
            nu,
            symbols(&record["resolvedSymbols"], nu)?,
        )?;
    }
    counts.insert("OmoikaneGlobalNumberSymbolsV1", root.len());
    export_patterns(source, exporter, counts)?;
    Ok(())
}

fn patterns(record: &Value) -> Result<NumberPatterns<'static>> {
    let values = NumberPatternField::ALL.map(|field| string(record, field.as_str()));
    Ok(NumberPatterns {
        fields: fields(&values)?,
    })
}

fn export_patterns(
    source: &Source,
    exporter: &BlobExporter<'_>,
    counts: &mut Counts,
) -> Result<()> {
    let data = source.read("number_patterns.json")?;
    let metadata = source.read("locale_metadata.json")?;
    let mut count = 0;
    for (locale, systems) in object(&data)? {
        for (nu, record) in object(systems)? {
            put::<OmoikaneNumberPatternsV1>(exporter, locale, nu, patterns(record)?)?;
            count += 1;
        }
        let default = text(&metadata["locales"][locale]["defaultNumberingSystem"])?;
        let record = systems
            .get(default)
            .ok_or("default numbering-system patterns are missing")?;
        put::<OmoikaneNumberPatternsV1>(exporter, locale, "", patterns(record)?)?;
        count += 1;
    }
    counts.insert("OmoikaneNumberPatternsV1", count);
    super::root_currency::export_number_patterns(source, exporter, counts)
}
