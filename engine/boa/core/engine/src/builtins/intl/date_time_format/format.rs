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
        let object = this
            .as_object()
            .and_then(|object| object.downcast::<Self>().ok())
            .ok_or_else(|| {
                JsNativeError::typ().with_message("receiver is not an Intl.DateTimeFormat")
            })?;
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
                "timeZoneName",
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
        let object = this
            .as_object()
            .and_then(|object| object.downcast::<Self>().ok())
            .ok_or_else(|| {
                JsNativeError::typ().with_message("receiver is not an Intl.DateTimeFormat")
            })?;
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
                    let value = input_epoch(args.get_or_undefined(0), context)?;
                    formatter
                        .borrow_mut()
                        .data_mut()
                        .format_epoch(value, context)
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
        let (datetime, offset_seconds) = self.epoch_datetime(value, context)?;
        let value = self
            .backend
            .format(
                context.intl_provider().erased_provider(),
                &datetime,
                self.components.widths(),
                offset_seconds,
            )
            .map_err(|error| JsNativeError::range().with_message(error))?;
        Ok(js_string!(value).into())
    }

    /// Returns owned parts after clipping the input and resolving its zone offset.
    pub(super) fn parts_epoch(
        &mut self,
        value: f64,
        context: &mut Context,
    ) -> JsResult<Vec<DateTimePart>> {
        let (datetime, offset_seconds) = self.epoch_datetime(value, context)?;
        self.backend
            .format_parts(
                context.intl_provider().erased_provider(),
                &datetime,
                self.components.widths(),
                offset_seconds,
            )
            .map_err(|error| JsNativeError::range().with_message(error).into())
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

/// Performs the one observable numeric conversion before borrowing the formatter.
pub(super) fn input_epoch(value: &JsValue, context: &mut Context) -> JsResult<f64> {
    let _value_root = value.as_object().map(|object| object.root());
    if value.is_undefined() {
        Ok(context.clock().now().millis_since_epoch() as f64)
    } else {
        value.to_number(context)
    }
}
