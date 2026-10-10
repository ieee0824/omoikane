use boa_intl_data::{
    DisplayNamesData, OmoikaneDisplayNamesV1, OmoikaneRelativeTimePatternsV1, RelativeTimePatterns,
};
use icu_provider_blob::export::BlobExporter;
use serde_json::Value;
use zerovec::ZeroMap;

use super::support::{Counts, Result, Source, object, put, text};

pub fn export(source: &Source, exporter: &BlobExporter<'_>, counts: &mut Counts) -> Result<()> {
    let relative = source.read("relative_time_patterns.json")?;
    let names = source.read("display_names.json")?;
    for (locale, row) in object(&relative)? {
        put::<OmoikaneRelativeTimePatternsV1>(
            exporter,
            locale,
            "",
            RelativeTimePatterns {
                records: records(row)?,
            },
        )?;
    }
    for (locale, row) in object(&names)? {
        put::<OmoikaneDisplayNamesV1>(
            exporter,
            locale,
            "",
            DisplayNamesData {
                records: records(row)?,
            },
        )?;
    }
    counts.insert("OmoikaneRelativeTimePatternsV1", object(&relative)?.len());
    counts.insert("OmoikaneDisplayNamesV1", object(&names)?.len());
    Ok(())
}

fn records(row: &Value) -> Result<ZeroMap<'static, str, str>> {
    let mut records = ZeroMap::<str, str>::new();
    for (key, value) in object(row)? {
        records.insert(key.as_str(), text(value)?);
    }
    Ok(records)
}
