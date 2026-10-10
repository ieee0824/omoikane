//! Export pinned, hash-verified CLDR extension data without loading the bundle.
//! Usage: export_cldr_data JSON_DIR OUTPUT_POSTCARD [OUTPUT_REPORT]
//! JSON exists only in this generator; Intl runtime consumers load typed payloads.
mod calendar_intervals;
mod compact;
mod currency;
mod datetime;
mod names;
mod number;
mod number_range;
mod root_currency;
mod support;
mod units;

use boa_intl_data::MARKERS;
use icu_provider::export::{DataExporter, FlushMetadata};
use icu_provider_blob::export::BlobExporter;
use std::{error::Error, fs::OpenOptions, path::PathBuf};
use support::Source;

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let directory = PathBuf::from(arguments.next().ok_or("JSON directory is required")?);
    let output = PathBuf::from(
        arguments
            .next()
            .ok_or("new postcard output path is required")?,
    );
    let report = arguments
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| output.with_extension("provenance.json"));
    if arguments.next().is_some() {
        return Err("unexpected extra arguments".into());
    }
    if output.exists() || report.exists() {
        return Err("output and report paths must be new".into());
    }
    let source = Source::open(directory)?;
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)?;
    let mut exporter = BlobExporter::new_with_sink(Box::new(file));
    let mut counts = std::collections::BTreeMap::new();
    number::export(&source, &exporter, &mut counts)?;
    currency::export(&source, &exporter, &mut counts)?;
    root_currency::export(&source, &exporter, &mut counts)?;
    units::export(&source, &exporter, &mut counts)?;
    datetime::export(&source, &exporter, &mut counts)?;
    compact::export(&source, &exporter, &mut counts)?;
    number_range::export(&source, &exporter, &mut counts)?;
    calendar_intervals::export(&source, &exporter, &mut counts)?;
    names::export(&source, &exporter, &mut counts)?;
    for marker in MARKERS {
        exporter.flush(*marker, FlushMetadata::default())?;
    }
    exporter.close()?;
    source.write_report(&output, &report, &counts)?;
    println!(
        "{} bytes; marker counts {counts:?}",
        std::fs::metadata(&output)?.len()
    );
    Ok(())
}
