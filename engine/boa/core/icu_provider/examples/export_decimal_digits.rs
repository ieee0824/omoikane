//! Exports all pinned CLDR numeric digits with the standard ICU marker.
//! Usage: export_decimal_digits DIGITS_TSV EXISTING_DECIMAL_BLOB NEW_OUTPUT
//! TSV has one lowercase numbering-system identifier and ten Unicode scalars
//! per line, separated by a tab. Inputs are validated before creating output.
use icu_decimal::provider::DecimalDigitsV1;
use icu_provider::{
    dynutil::UpcastDataPayload,
    export::{DataExporter, ExportMarker, FlushMetadata},
    prelude::*,
};
use icu_provider_blob::{BlobDataProvider, export::BlobExporter};
use std::{collections::BTreeMap, error::Error, fs::OpenOptions, path::Path};

type Digits = BTreeMap<String, [char; 10]>;
type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn read_digits(path: &Path) -> Result<Digits> {
    let input = std::fs::read_to_string(path)?;
    let mut records = Digits::new();
    for line in input.lines() {
        let (nu, raw) = line.split_once('\t').ok_or("invalid digits TSV row")?;
        DataMarkerAttributes::try_from_str(nu)
            .map_err(|_| "invalid numbering-system attributes")?;
        if nu.is_empty() || !nu.bytes().all(|byte| byte.is_ascii_lowercase()) {
            return Err("numeric numbering-system names must be lowercase ASCII".into());
        }
        let digits: [char; 10] = raw
            .chars()
            .collect::<Vec<_>>()
            .try_into()
            .map_err(|_| "a numeric mapping must contain ten Unicode scalars")?;
        if records.insert(nu.to_owned(), digits).is_some() {
            return Err("duplicate numbering-system mapping".into());
        }
    }
    if records.len() != 77 {
        return Err("the pinned CLDR47 input must contain all 77 numeric mappings".into());
    }
    Ok(records)
}

fn verify_existing(records: &Digits, path: &Path) -> Result<usize> {
    let provider = BlobDataProvider::try_new_from_blob(std::fs::read(path)?.into_boxed_slice())?;
    let ids = provider.iter_ids_for_marker(DecimalDigitsV1::INFO)?;
    if ids.is_empty() {
        return Err("the original digits marker must contain data".into());
    }
    for id in &ids {
        if id.locale != DataLocale::default() {
            return Err("decimal digits must be stored in the und locale".into());
        }
        let expected = records
            .get(id.marker_attributes.as_str())
            .ok_or("the new input omits an existing numbering system")?;
        let payload = DataProvider::<DecimalDigitsV1>::load(
            &provider.as_deserializing(),
            DataRequest {
                id: id.as_borrowed(),
                ..Default::default()
            },
        )?
        .payload;
        if payload.get() != expected {
            return Err("the new input changes an existing digit mapping".into());
        }
    }
    Ok(ids.len())
}

fn export_digits(records: &Digits, output: &Path) -> Result<()> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)?;
    let mut exporter = BlobExporter::new_with_sink(Box::new(file));
    let locale = DataLocale::default();
    for (nu, digits) in records {
        let attributes = DataMarkerAttributes::try_from_str(nu)
            .map_err(|_| "invalid numbering-system attributes")?;
        let id = DataIdentifierBorrowed::for_marker_attributes_and_locale(attributes, &locale);
        let payload: DataPayload<ExportMarker> =
            UpcastDataPayload::upcast(DataPayload::<DecimalDigitsV1>::from_owned(*digits));
        exporter.put_payload(DecimalDigitsV1::INFO, id, &payload)?;
    }
    exporter.flush(DecimalDigitsV1::INFO, FlushMetadata::default())?;
    exporter.close()?;
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() != 4 {
        return Err("expected DIGITS_TSV EXISTING_DECIMAL_BLOB NEW_OUTPUT".into());
    }
    let records = read_digits(Path::new(&args[1]))?;
    let existing = verify_existing(&records, Path::new(&args[2]))?;
    export_digits(&records, Path::new(&args[3]))?;
    println!("exported={} preserved_existing={existing}", records.len());
    Ok(())
}
