use super::support::*;
use boa_intl_data::{
    CANONICAL_COMPOUND_UNIT_IDENTIFIERS, CompoundUnitPatterns, DurationDigital,
    OmoikaneCompoundUnitPatternsV1, OmoikaneDurationDigitalV1, OmoikaneUnitPatternsV1,
    SANCTIONED_SIMPLE_UNIT_IDENTIFIERS, UnitPatterns,
};
use icu_provider_blob::export::BlobExporter;
use serde_json::Value;

fn unit_patterns(record: &Value) -> Result<UnitPatterns<'static>> {
    Ok(UnitPatterns {
        patterns: plurals(record, "unitPattern-count-")?,
        fields: fields(&[
            string(record, "displayName"),
            string(record, "perUnitPattern"),
        ])?,
    })
}

fn export_units(source: &Source, exporter: &BlobExporter<'_>, counts: &mut Counts) -> Result<()> {
    let data = source.read("all_unit_patterns.json")?;
    let mut unit_count = 0;
    let mut compound_count = 0;
    for (locale, widths) in object(&data)? {
        for width in ["long", "short", "narrow"] {
            let source_units = object(&widths[width])?;
            for &(_, unit) in SANCTIONED_SIMPLE_UNIT_IDENTIFIERS
                .iter()
                .chain(CANONICAL_COMPOUND_UNIT_IDENTIFIERS)
            {
                let record = source_units.get(unit).ok_or_else(|| {
                    format!("missing reachable CLDR unit {locale}/{width}/{unit}")
                })?;
                put::<OmoikaneUnitPatternsV1>(
                    exporter,
                    locale,
                    &format!("{width}-{unit}"),
                    unit_patterns(record)?,
                )?;
                unit_count += 1;
            }
            let compound = &widths["compound"][width];
            put::<OmoikaneCompoundUnitPatternsV1>(
                exporter,
                locale,
                width,
                CompoundUnitPatterns {
                    fields: fields(&[
                        string(&compound["per"], "compoundUnitPattern"),
                        string(&compound["times"], "compoundUnitPattern"),
                    ])?,
                },
            )?;
            compound_count += 1;
        }
    }
    counts.insert("OmoikaneUnitPatternsV1", unit_count);
    counts.insert("OmoikaneCompoundUnitPatternsV1", compound_count);
    Ok(())
}

fn digital(record: &Value, root_symbols: &Value, nu: &str) -> Result<DurationDigital<'static>> {
    let locale_separator = record["timeSeparators"].get(nu).and_then(Value::as_str);
    let global_separator = string(&root_symbols[nu]["resolvedSymbols"], "timeSeparator");
    if global_separator.is_none() {
        return Err("global numeric time separator is missing".into());
    }
    Ok(DurationDigital {
        fields: fields(&[locale_separator.or(global_separator), Some(nu)])?,
    })
}

fn export_digital(source: &Source, exporter: &BlobExporter<'_>, counts: &mut Counts) -> Result<()> {
    let data = source.read("duration_unit_patterns.json")?;
    let metadata = source.read("locale_metadata.json")?;
    let root = source.read("root_number_symbols.json")?;
    let root_symbols = &root["numberingSystemSymbols"];
    let mut count = 0;
    for (locale, record) in object(&data)? {
        for nu in object(root_symbols)?.keys() {
            put::<OmoikaneDurationDigitalV1>(
                exporter,
                locale,
                nu,
                digital(record, root_symbols, nu)?,
            )?;
            count += 1;
        }
        let default = text(&metadata["locales"][locale]["defaultNumberingSystem"])?;
        put::<OmoikaneDurationDigitalV1>(
            exporter,
            locale,
            "",
            digital(record, root_symbols, default)?,
        )?;
        count += 1;
    }
    counts.insert("OmoikaneDurationDigitalV1", count);
    Ok(())
}

pub fn export(source: &Source, exporter: &BlobExporter<'_>, counts: &mut Counts) -> Result<()> {
    export_units(source, exporter, counts)?;
    export_digital(source, exporter, counts)
}
