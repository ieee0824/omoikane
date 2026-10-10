use super::support::*;
use boa_intl_data::{CompactPatterns, OmoikaneCompactPatternsV1, PluralCategory};
use icu_provider_blob::export::BlobExporter;
use serde_json::Value;
use zerovec::ZeroMap2d;

fn patterns(value: &Value) -> Result<ZeroMap2d<'static, u64, u8, str>> {
    let mut patterns = ZeroMap2d::<u64, u8, str>::new();
    for (threshold, categories) in object(value)? {
        let threshold: u64 = threshold.parse()?;
        for (category, pattern) in object(categories)? {
            let category = PluralCategory::ALL
                .into_iter()
                .find(|value| value.as_str() == category)
                .ok_or("unknown compact plural category")?;
            patterns.insert(&threshold, &(category as u8), text(pattern)?);
        }
    }
    Ok(patterns)
}

pub fn export(source: &Source, exporter: &BlobExporter<'_>, counts: &mut Counts) -> Result<()> {
    let data = source.read("compact_patterns.json")?;
    let mut count = 0;
    for (locale, formats) in object(&data)? {
        for (attributes, record) in object(formats)? {
            put::<OmoikaneCompactPatternsV1>(
                exporter,
                locale,
                attributes,
                CompactPatterns {
                    normal: patterns(&record["normal"])?,
                    alpha_next_to_number: patterns(&record["alphaNextToNumber"])?,
                },
            )?;
            count += 1;
        }
    }
    counts.insert("OmoikaneCompactPatternsV1", count);
    Ok(())
}
