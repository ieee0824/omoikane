//! Exports singleton calendar data from the locked ICU4X baked dataset.
//! Usage: cargo run -p boa_icu_provider --example export_calendar_data -- OUTPUT
//! Existing output files are preserved; generation never removes service blobs.
use icu_calendar::provider::{
    Baked, CalendarChineseV1, CalendarDangiV1, CalendarHijriSimulatedMeccaV1,
    CalendarJapaneseExtendedV1, CalendarJapaneseModernV1,
};
use icu_provider::{
    dynutil::UpcastDataPayload,
    export::{DataExporter, ExportMarker, FlushMetadata},
    prelude::*,
};
use icu_provider_blob::export::BlobExporter;
use std::{error::Error, fs::OpenOptions};

fn main() -> Result<(), Box<dyn Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("output path is required")?;
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let mut exporter = BlobExporter::new_with_sink(Box::new(file));
    macro_rules! export {
        ($($marker:ty),+ $(,)?) => { $(
            let request = DataRequest::default();
            let response = <Baked as DataProvider<$marker>>::load(&Baked, request)?;
            let payload: DataPayload<ExportMarker> = UpcastDataPayload::upcast(response.payload);
            exporter.put_payload(<$marker>::INFO, request.id, &payload)?;
            exporter.flush(<$marker>::INFO, FlushMetadata::default())?;
        )+ };
    }
    export!(
        CalendarChineseV1,
        CalendarDangiV1,
        CalendarHijriSimulatedMeccaV1,
        CalendarJapaneseModernV1,
        CalendarJapaneseExtendedV1,
    );
    exporter.close()?;
    Ok(())
}
