//! Native `Intl.DurationFormat`, with owned locale data and exact arithmetic.

use crate::{
    Context, JsArgs, JsData, JsNativeError, JsResult, JsString, JsValue,
    builtins::{BuiltInBuilder, BuiltInConstructor, BuiltInObject, IntrinsicObject},
    context::intrinsics::{Intrinsics, StandardConstructor, StandardConstructors},
    js_string,
    object::{JsObject, internal_methods::get_prototype_from_constructor},
    property::Attribute,
    realm::Realm,
};
use boa_gc::{Finalize, Trace};

mod backend;
mod initialization;
mod options;
mod parts;
mod record;
#[cfg(test)]
mod tests;

use options::DurationOptions;
use record::DurationRecord;

/// A duration formatter owns its options, numeric formatters and list data.
#[derive(Debug, Trace, Finalize, JsData)]
#[boa_gc(unsafe_empty_trace)]
pub(crate) struct DurationFormat {
    locale: icu_locale::Locale,
    numbering_system: String,
    options: DurationOptions,
    backend: backend::DurationBackend,
}

impl IntrinsicObject for DurationFormat {
    fn init(realm: &Realm) {
        BuiltInBuilder::from_standard_constructor::<Self>(realm)
            .property(
                crate::JsSymbol::to_string_tag(),
                js_string!("Intl.DurationFormat"),
                Attribute::CONFIGURABLE,
            )
            .method(Self::format, js_string!("format"), 1)
            .method(Self::format_to_parts, js_string!("formatToParts"), 1)
            .method(Self::resolved_options, js_string!("resolvedOptions"), 0)
            .static_method(
                Self::supported_locales_of,
                js_string!("supportedLocalesOf"),
                1,
            )
            .build();
    }

    fn get(intrinsics: &Intrinsics) -> JsObject {
        Self::STANDARD_CONSTRUCTOR(intrinsics.constructors()).constructor()
    }
}

impl BuiltInObject for DurationFormat {
    const NAME: JsString = js_string!("DurationFormat");
}

impl BuiltInConstructor for DurationFormat {
    const CONSTRUCTOR_ARGUMENTS: usize = 0;
    const PROTOTYPE_STORAGE_SLOTS: usize = 4;
    const CONSTRUCTOR_STORAGE_SLOTS: usize = 1;
    const STANDARD_CONSTRUCTOR: fn(&StandardConstructors) -> &StandardConstructor =
        StandardConstructors::duration_format;

    fn constructor(
        new_target: &JsValue,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<JsValue> {
        if new_target.is_undefined() {
            return Err(JsNativeError::typ()
                .with_message("Intl.DurationFormat requires new")
                .into());
        }
        let prototype = get_prototype_from_constructor(
            new_target,
            StandardConstructors::duration_format,
            context,
        )?;
        let _prototype_root = prototype.clone().root();
        let data = Self::new(args.get_or_undefined(0), args.get_or_undefined(1), context)?;
        Ok(
            JsObject::from_proto_and_data_with_shared_shape(context.root_shape(), prototype, data)
                .into(),
        )
    }
}

impl DurationFormat {
    fn receiver(this: &JsValue) -> JsResult<JsObject<Self>> {
        this.as_object()
            .and_then(|object| object.downcast::<Self>().ok())
            .ok_or_else(|| {
                JsNativeError::typ()
                    .with_message("receiver is not an Intl.DurationFormat")
                    .into()
            })
    }

    fn format(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
        let object = Self::receiver(this)?;
        let _object_root = object.clone().root();
        // Duration getters and coercions can re-enter this formatter or collect.
        let record = DurationRecord::from_input(args.get_or_undefined(0), context)?;
        let value = object.borrow().data().format_record(&record)?;
        Ok(js_string!(value).into())
    }

    fn format_to_parts(
        this: &JsValue,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<JsValue> {
        let object = Self::receiver(this)?;
        let _object_root = object.clone().root();
        let record = DurationRecord::from_input(args.get_or_undefined(0), context)?;
        let parts = object.borrow().data().partition(&record)?;
        parts::to_array(parts, context).map(Into::into)
    }

    fn partition(&self, record: &DurationRecord) -> JsResult<Vec<parts::DurationPart>> {
        self.backend.partition(record, &self.options)
    }

    fn format_record(&self, record: &DurationRecord) -> JsResult<String> {
        self.backend.format(record, &self.options)
    }

    /// Formats internal Temporal fields without consulting author constructors.
    #[cfg(feature = "temporal")]
    pub(crate) fn format_temporal(
        duration: &temporal_rs::Duration,
        locales: &JsValue,
        options: &JsValue,
        context: &mut Context,
    ) -> JsResult<JsValue> {
        let formatter = Self::new(locales, options, context)?;
        let record = DurationRecord::from_temporal(duration)?;
        Ok(js_string!(formatter.format_record(&record)?).into())
    }
}
