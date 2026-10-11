//! Exports independently extracted CLDR month names into ICU's native marker.
//! Usage: export_hebrew_months INPUT_TSV NEW_OUTPUT; existing output is rejected.
use icu_datetime::provider::neo::{DatetimeNamesMonthHebrewV1, MonthNames, marker_attrs};
use icu_provider::{
    dynutil::UpcastDataPayload,
    export::{DataExporter, ExportMarker, FlushMetadata},
    prelude::*,
};
use icu_provider_blob::export::BlobExporter;
use std::{error::Error, fs::OpenOptions};

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let input = arguments.next().ok_or("input TSV path is required")?;
    let output = arguments.next().ok_or("new output path is required")?;
    if arguments.next().is_some() {
        return Err("unexpected extra arguments".into());
    }
    let source = std::fs::read_to_string(input)?;
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)?;
    let mut exporter = BlobExporter::new_with_sink(Box::new(file));
    let mut count = 0;
    for line in source.lines() {
        let fields: Vec<_> = line.split('\t').collect();
        if fields.len() != 27 {
            return Err("expected locale, context, width and 24 month-code names".into());
        }
        let attributes = match (fields[1], fields[2]) {
            ("format", "abbreviated") => marker_attrs::ABBR,
            ("format", "wide") => marker_attrs::WIDE,
            ("format", "narrow") => marker_attrs::NARROW,
            ("stand-alone", "abbreviated") => marker_attrs::ABBR_STANDALONE,
            ("stand-alone", "wide") => marker_attrs::WIDE_STANDALONE,
            ("stand-alone", "narrow") => marker_attrs::NARROW_STANDALONE,
            _ => return Err("invalid source month width/context".into()),
        };
        let locale: DataLocale = fields[0].parse()?;
        let names = &fields[3..];
        let payload = DataPayload::<DatetimeNamesMonthHebrewV1>::from_owned(
            MonthNames::LeapLinear((&names[..]).into()),
        );
        let payload: DataPayload<ExportMarker> = UpcastDataPayload::upcast(payload);
        exporter.put_payload(
            DatetimeNamesMonthHebrewV1::INFO,
            DataIdentifierBorrowed::for_marker_attributes_and_locale(attributes, &locale),
            &payload,
        )?;
        count += 1;
    }
    if count == 0 {
        return Err("no source payloads".into());
    }
    exporter.flush(DatetimeNamesMonthHebrewV1::INFO, FlushMetadata::default())?;
    exporter.close()?;
    println!("{count} Hebrew month payloads exported");
    Ok(())
}
