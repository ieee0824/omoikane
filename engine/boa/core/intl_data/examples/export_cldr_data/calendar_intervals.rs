use super::support::*;
use boa_intl_data::{CalendarIntervalPatterns, OmoikaneCalendarIntervalPatternsV1, StringFields};
use icu_provider_blob::export::BlobExporter;
use zerovec::ZeroMap2d;

pub fn export(source: &Source, exporter: &BlobExporter<'_>, counts: &mut Counts) -> Result<()> {
    let data = source.read("calendar_interval_patterns.json")?;
    let mut count = 0;
    for (locale, calendars) in object(&data)? {
        for (calendar, record) in object(calendars)? {
            let mut patterns = ZeroMap2d::<str, str, str>::new();
            for (skeleton, differences) in object(&record["patterns"])? {
                for (difference, raw) in object(differences)? {
                    patterns.insert(skeleton.as_str(), difference.as_str(), text(raw)?);
                }
            }
            let fields = StringFields::from_fields(&[Some(text(&record["fallback"])?)])
                .ok_or("interval fallback storage")?;
            put::<OmoikaneCalendarIntervalPatternsV1>(
                exporter,
                locale,
                calendar,
                CalendarIntervalPatterns { patterns, fields },
            )?;
            count += 1;
        }
    }
    counts.insert("OmoikaneCalendarIntervalPatternsV1", count);
    Ok(())
}
