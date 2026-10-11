//! Exports independently extracted CLDR era names into ICU's native marker.
//! Usage: export_hijri_eras INPUT_TSV NEW_OUTPUT; existing output is rejected.
use icu_datetime::provider::neo::{DatetimeNamesYearHijriV1, YearNames, marker_attrs};
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
        let [locale, width, ah, bh] = fields.as_slice() else {
            return Err("expected locale, width and AH/BH names".into());
        };
        let attributes = match *width {
            "eraAbbr" => marker_attrs::ABBR,
            "eraNames" => marker_attrs::WIDE,
            "eraNarrow" => marker_attrs::NARROW,
            _ => return Err("invalid source era width".into()),
        };
        let locale: DataLocale = locale.parse()?;
        let names = [*ah, *bh];
        let payload = DataPayload::<DatetimeNamesYearHijriV1>::from_owned(YearNames::FixedEras(
            (&names[..]).into(),
        ));
        let payload: DataPayload<ExportMarker> = UpcastDataPayload::upcast(payload);
        exporter.put_payload(
            DatetimeNamesYearHijriV1::INFO,
            DataIdentifierBorrowed::for_marker_attributes_and_locale(attributes, &locale),
            &payload,
        )?;
        count += 1;
    }
    if count == 0 {
        return Err("no source payloads".into());
    }
    exporter.flush(DatetimeNamesYearHijriV1::INFO, FlushMetadata::default())?;
    exporter.close()?;
    println!("{count} Hijri era payloads exported");
    Ok(())
}
