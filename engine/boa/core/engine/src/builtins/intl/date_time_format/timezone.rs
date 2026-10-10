//! Owned time-zone state resolved once during formatter initialization.
use crate::builtins::options::get_option;
use crate::{Context, JsNativeError, JsObject, JsResult, JsString, js_string};
use temporal_rs::{Instant, TimeZone};

#[derive(Debug, Clone)]
pub(super) struct ResolvedTimeZone {
    identifier: JsString,
    zone: TimeZone,
}

impl ResolvedTimeZone {
    /// Validates minute-precision offsets or an available IANA identifier.
    /// The provider fixes case and resolves IANA links to their primary ID.
    fn resolve(identifier: &str, context: &Context) -> JsResult<Self> {
        let zone =
            TimeZone::try_from_identifier_str_with_provider(identifier, context.tz_provider())
                .map_err(time_zone_error)?;
        let zone = zone
            .primary_identifier_with_provider(context.tz_provider())
            .map_err(time_zone_error)?;
        let identifier = zone
            .identifier_with_provider(context.tz_provider())
            .map_err(time_zone_error)?;
        // ECMA-402 represents these equivalent named zones as UTC. Fixed
        // offsets, including +00:00, retain their offset identifier.
        let identifier = if ["Etc/UTC", "Etc/GMT", "GMT"].contains(&identifier.as_str()) {
            "UTC".to_owned()
        } else {
            identifier
        };
        Ok(Self {
            identifier: js_string!(identifier),
            zone,
        })
    }

    pub(super) fn identifier(&self) -> &JsString {
        &self.identifier
    }

    /// Calculates the offset for an explicit clipped epoch using the TZDB.
    pub(super) fn offset_millis(&self, epoch: f64, context: &Context) -> JsResult<f64> {
        let instant = Instant::from_epoch_milliseconds(epoch as i64).map_err(time_zone_error)?;
        let datetime = instant
            .to_zoned_date_time_iso_with_provider(self.zone, context.tz_provider())
            .map_err(time_zone_error)?;
        Ok(datetime.offset_nanoseconds() as f64 / 1_000_000.0)
    }
}

/// Reads the observable option once. Only an absent option consults the host
/// system zone; the resulting formatter owns its resolved identifier and ID.
pub(super) fn read_time_zone(
    options: &JsObject,
    context: &mut Context,
) -> JsResult<ResolvedTimeZone> {
    let identifier = get_option::<JsString>(options, js_string!("timeZone"), context)?;
    let identifier = identifier.map_or_else(
        || iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".to_owned()),
        |identifier| identifier.to_std_string_escaped(),
    );
    ResolvedTimeZone::resolve(&identifier, context)
}

fn time_zone_error(error: temporal_rs::TemporalError) -> crate::JsError {
    JsNativeError::range()
        .with_message(error.to_string())
        .into()
}
