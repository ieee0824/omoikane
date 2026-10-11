//! Owned zone identity and provider data for native named-zone pattern fields.
use icu_calendar::Iso;
use icu_datetime::input::{DateTime, TimeZone, TimeZoneInfo, UtcOffset};
use icu_provider::prelude::*;
use icu_time::zone::{IanaParser, VariantOffsetsCalculator, models};
use std::sync::Arc;

pub(super) type FullZone = TimeZoneInfo<models::Full>;

#[derive(Debug, Clone)]
pub(super) struct ZoneInput {
    identifier: TimeZone,
    variants: Arc<VariantOffsetsCalculator>,
}

impl ZoneInput {
    pub(super) fn load<P: BufferProvider + ?Sized>(
        provider: &P,
        identifier: &str,
    ) -> Result<Self, String> {
        let parser = IanaParser::try_new_with_buffer_provider(provider)
            .map_err(|error| format!("named-zone identifiers: {error:?}"))?;
        let identifier = parser.as_borrowed().parse(identifier);
        let variants = VariantOffsetsCalculator::try_new_with_buffer_provider(provider)
            .map_err(|error| format!("named-zone variants: {error:?}"))?;
        Ok(Self {
            identifier,
            variants: Arc::new(variants),
        })
    }

    pub(super) fn at(
        &self,
        datetime: &DateTime<Iso>,
        offset_seconds: i32,
    ) -> Result<FullZone, String> {
        let offset = UtcOffset::try_from_seconds(offset_seconds)
            .map_err(|error| format!("named-zone offset: {error:?}"))?;
        Ok(self
            .identifier
            .with_offset(Some(offset))
            .at_date_time_iso(datetime.clone())
            .infer_variant(self.variants.as_borrowed()))
    }
}

pub(super) fn utc_at(datetime: &DateTime<Iso>) -> FullZone {
    TimeZoneInfo::utc()
        .at_date_time_iso(datetime.clone())
        .with_variant(icu_time::zone::TimeZoneVariant::Standard)
}
