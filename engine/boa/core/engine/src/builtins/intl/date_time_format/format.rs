//! Native bound format functions and conversion of explicit epoch input.
use super::{DateTimeFormat, parts::DateTimePart};
use crate::object::FunctionObjectBuilder;
use crate::{Context, JsArgs, JsNativeError, JsResult, JsValue, NativeFunction, js_string};

impl DateTimeFormat {
    pub(super) fn resolved_options(
        this: &JsValue,
        _: &[JsValue],
        context: &mut Context,
    ) -> JsResult<JsValue> {
        let object = super::legacy::unwrap(this, context)?;
        let _object_root = object.clone().root();
        let data = object.borrow();
        let data = data.data();
        let selected_fields = data.backend.selected_fields();
        let resolved = selected_fields.map_or_else(
            || {
                super::resolution::ResolvedPattern::from_pattern(
                    data.backend.resolved_pattern(data.components.widths()),
                )
            },
            |fields| {
                super::resolution::ResolvedPattern::from_components(
                    fields,
                    data.backend.hour_cycle(),
                )
            },
        );
        let mut result = crate::object::ObjectInitializer::new(context);
        let attributes = crate::property::Attribute::all();
        result.property(
            js_string!("locale"),
            js_string!(data.locale.to_string()),
            attributes,
        );
        result.property(
            js_string!("calendar"),
            js_string!(data.backend.calendar_name()),
            attributes,
        );
        result.property(
            js_string!("numberingSystem"),
            js_string!(data.backend.numbering_system()),
            attributes,
        );
        result.property(
            js_string!("timeZone"),
            data.time_zone.identifier().clone(),
            attributes,
        );
        if resolved.hour_cycle.is_some() {
            let cycle = data.backend.hour_cycle();
            result.property(js_string!("hourCycle"), js_string!(cycle), attributes);
            result.property(
                js_string!("hour12"),
                matches!(cycle, "h11" | "h12"),
                attributes,
            );
        }
        if data.components.date_style.is_none() && data.components.time_style.is_none() {
            for name in [
                "weekday",
                "era",
                "year",
                "month",
                "day",
                "dayPeriod",
                "hour",
                "minute",
                "second",
            ] {
                if name == "dayPeriod"
                    && selected_fields.is_none()
                    && data.components.day_period.is_none()
                {
                    continue;
                }
                if let Some((_, value)) = resolved.fields.iter().find(|(field, _)| field == &name) {
                    result.property(js_string!(name), js_string!(*value), attributes);
                }
            }
            if let Some(digits) = resolved.fractional_digits {
                result.property(js_string!("fractionalSecondDigits"), digits, attributes);
            }
            if let Some((_, value)) = resolved
                .fields
                .iter()
                .find(|(field, _)| *field == "timeZoneName")
            {
                result.property(js_string!("timeZoneName"), js_string!(*value), attributes);
            }
            if let Some(zone) = data
                .components
                .zone_name
                .as_ref()
                .filter(|_| selected_fields.is_none())
            {
                result.property(js_string!("timeZoneName"), zone.clone(), attributes);
            }
        }
        for (name, value) in [
            ("dateStyle", &data.components.date_style),
            ("timeStyle", &data.components.time_style),
        ] {
            if let Some(value) = value {
                result.property(js_string!(name), value.clone(), attributes);
            }
        }
        Ok(result.build().into())
    }

    pub(super) fn get_format(
        this: &JsValue,
        _: &[JsValue],
        context: &mut Context,
    ) -> JsResult<JsValue> {
        let object = super::legacy::unwrap(this, context)?;
        let rooted = object.clone().root();
        if let Some(format) = &object.borrow().data().bound_format {
            return Ok(format.clone().root().into());
        }
        let format = FunctionObjectBuilder::new(
            context.realm(),
            NativeFunction::from_copy_closure_with_captures(
                |_, args, formatter, context| {
                    let _root = formatter.clone().root();
                    // Coercion can execute author code. Release all object borrows first.
                    let value = super::input::read(args.get_or_undefined(0), context)?;
                    formatter
                        .borrow_mut()
                        .data_mut()
                        .format_input(value, context)
                },
                object.clone(),
            ),
        )
        .name(js_string!())
        .length(1)
        .build();
        object.borrow_mut().data_mut().bound_format = Some(format.clone().into_edge());
        drop(rooted);
        Ok(format.into())
    }

    pub(crate) fn format_epoch(&mut self, value: f64, context: &mut Context) -> JsResult<JsValue> {
        self.format_input(super::input::Input::Number(value), context)
    }

    pub(super) fn format_input(
        &mut self,
        value: super::input::Input,
        context: &mut Context,
    ) -> JsResult<JsValue> {
        let (datetime, offset_seconds, kind) = self.prepare_input(value, context)?;
        let (components, backend) = self.native_format(kind, context)?;
        let value = backend
            .format(
                context.intl_provider().erased_provider(),
                &datetime,
                components.widths(),
                offset_seconds,
            )
            .map_err(|error| JsNativeError::range().with_message(error))?;
        Ok(js_string!(value).into())
    }

    /// Returns owned parts after converting the already observed input.
    pub(super) fn parts_input(
        &mut self,
        value: super::input::Input,
        context: &mut Context,
    ) -> JsResult<Vec<DateTimePart>> {
        let (datetime, offset_seconds, kind) = self.prepare_input(value, context)?;
        let (components, backend) = self.native_format(kind, context)?;
        backend
            .format_parts(
                context.intl_provider().erased_provider(),
                &datetime,
                components.widths(),
                offset_seconds,
            )
            .map_err(|error| JsNativeError::range().with_message(error).into())
    }

    pub(super) fn prepare_input(
        &self,
        value: super::input::Input,
        context: &Context,
    ) -> JsResult<(
        icu_datetime::input::DateTime<icu_calendar::Iso>,
        i32,
        Option<super::input::Kind>,
    )> {
        use super::input::Input;
        let kind = value.kind();
        match value {
            Input::Number(epoch) => self
                .epoch_datetime(epoch, context)
                .map(|(date, offset)| (date, offset, kind)),
            #[cfg(feature = "temporal")]
            Input::Instant(epoch) => self
                .epoch_datetime(epoch, context)
                .map(|(date, offset)| (date, offset, kind)),
            #[cfg(feature = "temporal")]
            Input::Plain {
                kind: plain_kind,
                datetime,
                calendar,
            } => {
                if let Some(calendar) = calendar {
                    let exact = matches!(
                        plain_kind,
                        super::input::Kind::YearMonth | super::input::Kind::MonthDay
                    );
                    if calendar != self.backend.calendar_name() && (exact || calendar != "iso8601")
                    {
                        return Err(JsNativeError::range()
                            .with_message("Temporal calendar does not match formatter calendar")
                            .into());
                    }
                }
                Ok((datetime, 0, kind))
            }
            #[cfg(feature = "temporal")]
            Input::UnsupportedTemporal => Err(JsNativeError::typ()
                .with_message("Temporal.ZonedDateTime cannot be formatted by Intl.DateTimeFormat")
                .into()),
        }
    }

    pub(super) fn native_format(
        &mut self,
        _kind: Option<super::input::Kind>,
        context: &Context,
    ) -> JsResult<(
        &super::options::Components,
        &mut super::backend::DateTimeBackend,
    )> {
        #[cfg(feature = "temporal")]
        if let Some(kind) = _kind {
            let (components, backend) =
                self.temporal_formats
                    .get(kind, &self.backend, &self.components, context)?;
            return Ok((components, backend));
        }
        let _ = context;
        Ok((&self.components, &mut self.backend))
    }

    pub(super) fn epoch_datetime(
        &self,
        value: f64,
        context: &Context,
    ) -> JsResult<(icu_datetime::input::DateTime<icu_calendar::Iso>, i32)> {
        if !value.is_finite() || value.abs() > 8.64e15 {
            return Err(JsNativeError::range()
                .with_message("invalid time value")
                .into());
        }
        let value = value.trunc();
        let offset = self.time_zone.offset_millis(value, context)?;
        let datetime = crate::builtins::date::Date::intl_utc_datetime(value + offset)?;
        Ok((datetime, (offset / 1_000.0) as i32))
    }
}
