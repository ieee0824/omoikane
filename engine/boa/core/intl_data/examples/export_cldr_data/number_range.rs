use boa_intl_data::{NumberRangePatterns, OmoikaneNumberRangePatternsV1, PluralCategory};
use icu_provider_blob::export::BlobExporter;
use serde_json::Value;
use zerovec::ZeroVec;

use super::support::{Counts, Result, Source, fields, object, put, string, text};

pub fn export(source: &Source, exporter: &BlobExporter<'_>, counts: &mut Counts) -> Result<()> {
    let data = source.read("number_range_patterns.json")?;
    let root = object(&data["root"])?;
    let mut count = 0;
    for (locale, record) in object(&data["locales"])? {
        let patterns = object(&record["patterns"])?;
        let (results, present) = plural_rules(&record["pluralRanges"])?;
        for (nu, root_record) in root {
            let selected = match patterns.get(nu) {
                Some(pattern) => pattern,
                None => {
                    let alias = string(root_record, "localeAliasTarget")
                        .ok_or_else(|| format!("missing range alias: {locale}/{nu}"))?;
                    patterns.get(alias).ok_or_else(|| {
                        format!("missing same-locale range alias target: {locale}/{nu}/{alias}")
                    })?
                }
            };
            put::<OmoikaneNumberRangePatternsV1>(
                exporter,
                locale,
                nu,
                payload(selected, &results, present)?,
            )?;
            count += 1;
        }
        let default = text(&record["defaultNumberingSystem"])?;
        let selected = patterns
            .get(default)
            .ok_or_else(|| format!("missing default range pattern: {locale}/{default}"))?;
        put::<OmoikaneNumberRangePatternsV1>(
            exporter,
            locale,
            "",
            payload(selected, &results, present)?,
        )?;
        count += 1;
    }
    counts.insert("OmoikaneNumberRangePatternsV1", count);
    Ok(())
}

fn payload(
    patterns: &Value,
    results: &[u8; 36],
    present: u64,
) -> Result<NumberRangePatterns<'static>> {
    Ok(NumberRangePatterns {
        fields: fields(&[
            Some(text(&patterns["range"])?),
            Some(text(&patterns["approximately"])?),
        ])?,
        plural_results: ZeroVec::alloc_from_slice(results),
        plural_present: present,
    })
}

fn category(name: &str) -> Result<usize> {
    PluralCategory::ALL[..6]
        .iter()
        .position(|category| category.as_str() == name)
        .ok_or_else(|| format!("invalid cardinal range category: {name}").into())
}

fn plural_rules(rules: &Value) -> Result<([u8; 36], u64)> {
    let mut results = [0; 36];
    let mut present = 0;
    for (start, ends) in object(rules)? {
        let start = category(start)?;
        for (end, result) in object(ends)? {
            let index = start * 6 + category(end)?;
            results[index] = u8::try_from(category(text(result)?)?)?;
            present |= 1_u64 << index;
        }
    }
    Ok((results, present))
}
