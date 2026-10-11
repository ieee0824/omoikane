use super::support::*;
use boa_intl_data::{
    DateTimeStylePatterns, DayPeriods, OmoikaneDateTimeStylePatternsV1, OmoikaneDayPeriodsV1,
};
use icu_provider_blob::export::BlobExporter;
use zerovec::ZeroMap2d;

pub fn export(source: &Source, exporter: &BlobExporter<'_>, counts: &mut Counts) -> Result<()> {
    export_styles(source, exporter, counts)?;
    let data = source.read("day_periods.json")?;
    let mut count = 0;
    for (locale, record) in object(&data)? {
        let names = map(&record["names"])?;
        let rules = map(&record["rules"])?;
        let fields = fields(&[Some(text(&record["sourceRuleLocale"])?)])?;
        put::<OmoikaneDayPeriodsV1>(
            exporter,
            locale,
            "",
            DayPeriods {
                names,
                rules,
                fields,
            },
        )?;
        count += 1;
    }
    counts.insert("OmoikaneDayPeriodsV1", count);
    Ok(())
}

fn export_styles(source: &Source, exporter: &BlobExporter<'_>, counts: &mut Counts) -> Result<()> {
    let data = source.read("datetime_style_patterns.json")?;
    let mut count = 0;
    for (locale, calendars) in object(&data)? {
        for (calendar, record) in object(calendars)? {
            let mut patterns = ZeroMap2d::<str, str, str>::new();
            let mut numbering_overrides = ZeroMap2d::<str, str, str>::new();
            for (family, styles) in object(&record["patterns"])? {
                let styles = if family == "atTime" {
                    &styles["standard"]
                } else {
                    styles
                };
                for (style, value) in object(styles)? {
                    let pattern = if value.is_object() {
                        numbering_overrides.insert(family, style, text(&value["_numbers"])?);
                        text(&value["_value"])?
                    } else {
                        text(value)?
                    };
                    patterns.insert(family, style, pattern);
                }
            }
            put::<OmoikaneDateTimeStylePatternsV1>(
                exporter,
                locale,
                calendar,
                DateTimeStylePatterns {
                    patterns,
                    numbering_overrides,
                },
            )?;
            count += 1;
        }
    }
    counts.insert("OmoikaneDateTimeStylePatternsV1", count);
    Ok(())
}

fn map(value: &serde_json::Value) -> Result<ZeroMap2d<'static, str, str, str>> {
    let mut result = ZeroMap2d::<str, str, str>::new();
    for (first, records) in object(value)? {
        for (second, value) in object(records)? {
            result.insert(first, second, text(value)?);
        }
    }
    Ok(result)
}
