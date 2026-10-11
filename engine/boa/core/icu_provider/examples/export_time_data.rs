//! Exports named-zone singleton data from the locked ICU4X baked dataset.
//! Usage: export_time_data NEW_OUTPUT; existing files are never replaced.
use icu_provider::{
    dynutil::UpcastDataPayload,
    export::{DataExporter, ExportMarker, FlushMetadata},
    prelude::*,
};
use icu_provider_blob::export::BlobExporter;
use icu_time::provider::{
    Baked, TimezoneVariantsOffsetsV1,
    iana::{TimezoneIdentifiersIanaCoreV1, TimezoneIdentifiersIanaExtendedV1},
};
use std::{error::Error, fs::OpenOptions};

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let output = arguments.next().ok_or("new output path is required")?;
    if arguments.next().is_some() {
        return Err("unexpected extra arguments".into());
    }
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)?;
    let mut exporter = BlobExporter::new_with_sink(Box::new(file));
    macro_rules! export {
        ($($marker:ty),+ $(,)?) => { $(
            let request = DataRequest::default();
            let response = <Baked as DataProvider<$marker>>::load(&Baked, request)?;
            let mut metadata = FlushMetadata::default();
            metadata.checksum = response.metadata.checksum;
            metadata.supports_dry_provider = true;
            let payload: DataPayload<ExportMarker> = UpcastDataPayload::upcast(response.payload);
            exporter.put_payload(<$marker>::INFO, request.id, &payload)?;
            exporter.flush(<$marker>::INFO, metadata)?;
            println!("{:?}: checksum {:?}", <$marker>::INFO.id, metadata.checksum);
        )+ };
    }
    export!(
        TimezoneIdentifiersIanaCoreV1,
        TimezoneIdentifiersIanaExtendedV1,
        TimezoneVariantsOffsetsV1
    );
    exporter.close()?;
    Ok(())
}
