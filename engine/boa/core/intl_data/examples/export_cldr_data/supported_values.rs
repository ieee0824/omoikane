use super::support::*;
use boa_intl_data::{OmoikaneSupportedValuesV1, SupportedValues};
use icu_provider_blob::export::BlobExporter;
use zerovec::ZeroMap2d;

pub(crate) fn export(
    source: &Source,
    exporter: &BlobExporter<'_>,
    counts: &mut Counts,
) -> Result<()> {
    let source = source.read("supported_values.json")?;
    let mut values = ZeroMap2d::<str, str, u8>::new();
    for (category, identifiers) in object(&source)? {
        for identifier in identifiers.as_array().ok_or("expected identifier array")? {
            values.insert(category, text(identifier)?, &0);
        }
    }
    put::<OmoikaneSupportedValuesV1>(exporter, "und", "", SupportedValues { values })?;
    counts.insert("OmoikaneSupportedValuesV1", 1);
    Ok(())
}
