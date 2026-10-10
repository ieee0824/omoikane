use super::support::*;
use boa_intl_data::{CalendarDatePatterns, OmoikaneCalendarDatePatternsV1};
use icu_provider_blob::export::BlobExporter;
use zerovec::ZeroMap;

pub fn export(source: &Source, exporter: &BlobExporter<'_>, counts: &mut Counts) -> Result<()> {
    let data = source.read("calendar_date_patterns.json")?;
    let mut count = 0;
    for (locale, calendars) in object(&data)? {
        for (calendar, record) in object(calendars)? {
            let mut available_formats = ZeroMap::<str, str>::new();
            for (skeleton, pattern) in object(&record["availableFormats"])? {
                available_formats.insert(skeleton.as_str(), text(pattern)?);
            }
            put::<OmoikaneCalendarDatePatternsV1>(
                exporter,
                locale,
                calendar,
                CalendarDatePatterns { available_formats },
            )?;
            count += 1;
        }
    }
    counts.insert("OmoikaneCalendarDatePatternsV1", count);
    Ok(())
}
