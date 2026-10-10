//! Native display-name lookup keeps provider backing storage owned by the instance.
mod language;

use super::{
    Service,
    locale::{canonicalize_locale_list, filter_locales, resolve_locale},
    native_names::{load, option},
    options::IntlOptions,
};
use crate::{
    Context, JsArgs, JsData, JsNativeError, JsObject, JsResult, JsString, JsSymbol, JsValue,
    builtins::{
        BuiltInBuilder, BuiltInConstructor, BuiltInObject, IntrinsicObject,
        options::{get_option, get_options_object},
    },
    context::intrinsics::{Intrinsics, StandardConstructor, StandardConstructors},
    js_string,
    object::{ObjectInitializer, internal_methods::get_prototype_from_constructor},
    property::Attribute,
    realm::Realm,
};
use boa_gc::{Finalize, Trace};
use boa_intl_data::{CurrencyTextField, OmoikaneCurrencyTextV1, OmoikaneDisplayNamesV1};
use icu_locale::Locale;
use icu_provider::prelude::*;

#[derive(Debug, Trace, Finalize, JsData)]
// SAFETY: provider payloads and strings contain no JavaScript GC edges.
#[boa_gc(unsafe_empty_trace)]
pub(crate) struct DisplayNames {
    locale: Locale,
    style: String,
    kind: String,
    fallback: String,
    language_display: String,
    names: DataPayload<OmoikaneDisplayNamesV1>,
}

impl Service for DisplayNames {
    type LangMarker = OmoikaneDisplayNamesV1;
    type LocaleOptions = ();

    fn resolve(locale: &mut Locale, _: &mut (), _: &crate::context::icu::IntlProvider) {
        locale.extensions.unicode.clear();
    }
}

impl IntrinsicObject for DisplayNames {
    fn init(realm: &Realm) {
        BuiltInBuilder::from_standard_constructor::<Self>(realm)
            .static_method(
                Self::supported_locales_of,
                js_string!("supportedLocalesOf"),
                1,
            )
            .property(
                JsSymbol::to_string_tag(),
                js_string!("Intl.DisplayNames"),
                Attribute::CONFIGURABLE,
            )
            .method(Self::resolved_options, js_string!("resolvedOptions"), 0)
            .method(Self::of, js_string!("of"), 1)
            .build();
    }

    fn get(intrinsics: &Intrinsics) -> JsObject {
        intrinsics.constructors().display_names().constructor()
    }
}

impl BuiltInObject for DisplayNames {
    const NAME: JsString = js_string!("DisplayNames");
}

impl BuiltInConstructor for DisplayNames {
    const CONSTRUCTOR_ARGUMENTS: usize = 2;
    const PROTOTYPE_STORAGE_SLOTS: usize = 3;
    const CONSTRUCTOR_STORAGE_SLOTS: usize = 1;
    const STANDARD_CONSTRUCTOR: fn(&StandardConstructors) -> &StandardConstructor =
        StandardConstructors::display_names;

    fn constructor(
        new_target: &JsValue,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<JsValue> {
        if new_target.is_undefined() {
            return Err(JsNativeError::typ()
                .with_message("Intl.DisplayNames requires new")
                .into());
        }
        let proto = get_prototype_from_constructor(
            new_target,
            StandardConstructors::display_names,
            context,
        )?;
        let _proto_root = proto.clone().root();
        let requested = canonicalize_locale_list(args.get_or_undefined(0), context)?;
        let value = args.get_or_undefined(1);
        if value.is_undefined() {
            return Err(JsNativeError::typ()
                .with_message("Intl.DisplayNames requires options")
                .into());
        }
        let options = get_options_object(value)?;
        let _options_root = options.clone().root();
        let matcher =
            get_option(&options, js_string!("localeMatcher"), context)?.unwrap_or_default();
        let locale = resolve_locale::<Self>(
            requested,
            &mut IntlOptions {
                matcher,
                service_options: (),
            },
            context.intl_provider(),
        )?;
        let style = option(
            &options,
            "style",
            &["long", "short", "narrow"],
            Some("long"),
            context,
        )?;
        let kind = option(
            &options,
            "type",
            &[
                "language",
                "script",
                "region",
                "currency",
                "calendar",
                "dateTimeField",
            ],
            None,
            context,
        )?;
        let fallback = option(
            &options,
            "fallback",
            &["code", "none"],
            Some("code"),
            context,
        )?;
        // This getter is required even when the selected type is not language.
        let language_display = option(
            &options,
            "languageDisplay",
            &["dialect", "standard"],
            Some("dialect"),
            context,
        )?;
        let names = load::<OmoikaneDisplayNamesV1>(context, &locale)?;
        let data = Self {
            locale,
            style,
            kind,
            fallback,
            language_display,
            names,
        };
        Ok(
            JsObject::from_proto_and_data_with_shared_shape(context.root_shape(), proto, data)
                .into(),
        )
    }
}

impl DisplayNames {
    /// Brand checks precede author coercion; no state borrow survives ToString.
    fn of(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
        let object = this
            .as_object()
            .and_then(|object| object.downcast::<Self>().ok())
            .ok_or_else(|| {
                JsNativeError::typ().with_message("receiver is not Intl.DisplayNames")
            })?;
        let _object_root = object.clone().root();
        let source = args
            .get_or_undefined(0)
            .to_string(context)?
            .to_std_string_escaped();
        let kind = object.borrow().data().kind.clone();
        let code = canonical_code(&kind, source, context)?;
        let result = {
            let borrow = object.borrow();
            let data = borrow.data();
            if kind == "language" {
                data.language(&code)
            } else if kind == "currency" {
                data.currency(&code, context)?
            } else {
                data.label(&kind, &code).map(str::to_owned)
            }
            .or_else(|| (data.fallback == "code").then_some(code))
        };
        Ok(result
            .map(|value| js_string!(value).into())
            .unwrap_or_else(JsValue::undefined))
    }

    pub(super) fn label(&self, kind: &str, code: &str) -> Option<&str> {
        let records = &self.names.get().records;
        let key = format!("{kind}/{}/{code}", self.style);
        records
            .get(key.as_str())
            .or_else(|| {
                if self.style == "narrow" {
                    records.get(format!("{kind}/short/{code}").as_str())
                } else {
                    None
                }
            })
            .or_else(|| records.get(format!("{kind}/long/{code}").as_str()))
    }

    /// Currency names already live in the per-code number-format marker. Load
    /// that zero-copy payload directly instead of duplicating the same strings
    /// and keys in every locale's DisplayNames map.
    fn currency(&self, code: &str, context: &Context) -> JsResult<Option<String>> {
        let locale = DataLocale::from(self.locale.id.clone());
        let attributes = DataMarkerAttributes::try_from_str(code).map_err(|error| {
            JsNativeError::typ().with_message(format!("currency attributes: {error:?}"))
        })?;
        let payload = match DataProvider::<OmoikaneCurrencyTextV1>::load(
            context.intl_provider(),
            DataRequest {
                id: DataIdentifierBorrowed::for_marker_attributes_and_locale(attributes, &locale),
                metadata: Default::default(),
            },
        ) {
            Ok(response) => response.payload,
            Err(error) if error.kind == DataErrorKind::IdentifierNotFound => return Ok(None),
            Err(error) => {
                return Err(JsNativeError::typ().with_message(error.to_string()).into());
            }
        };
        let data = payload.get();
        let value = match self.style.as_str() {
            "narrow" => data
                .get(CurrencyTextField::NarrowSymbol)
                .or_else(|| data.get(CurrencyTextField::Symbol))
                .or_else(|| data.get(CurrencyTextField::DisplayName)),
            "short" => data
                .get(CurrencyTextField::Symbol)
                .or_else(|| data.get(CurrencyTextField::DisplayName)),
            _ => data.get(CurrencyTextField::DisplayName),
        };
        Ok(value.map(str::to_owned))
    }

    fn resolved_options(this: &JsValue, _: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
        let object = this
            .as_object()
            .and_then(|object| object.downcast::<Self>().ok())
            .ok_or_else(|| {
                JsNativeError::typ().with_message("receiver is not Intl.DisplayNames")
            })?;
        let (locale, style, kind, fallback, language) = {
            let borrow = object.borrow();
            let data = borrow.data();
            (
                data.locale.to_string(),
                data.style.clone(),
                data.kind.clone(),
                data.fallback.clone(),
                data.language_display.clone(),
            )
        };
        let is_language = kind == "language";
        let mut result = ObjectInitializer::new(context);
        result
            .property(js_string!("locale"), js_string!(locale), Attribute::all())
            .property(js_string!("style"), js_string!(style), Attribute::all())
            .property(js_string!("type"), js_string!(kind), Attribute::all())
            .property(
                js_string!("fallback"),
                js_string!(fallback),
                Attribute::all(),
            );
        if is_language {
            result.property(
                js_string!("languageDisplay"),
                js_string!(language),
                Attribute::all(),
            );
        }
        Ok(result.build().into())
    }

    fn supported_locales_of(
        _: &JsValue,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<JsValue> {
        let requested = canonicalize_locale_list(args.get_or_undefined(0), context)?;
        filter_locales::<Self>(requested, args.get_or_undefined(1), context).map(JsValue::from)
    }
}

fn canonical_code(kind: &str, code: String, context: &mut Context) -> JsResult<String> {
    let letters = |value: &str| value.bytes().all(|byte| byte.is_ascii_alphabetic());
    let invalid = || JsNativeError::range().with_message("invalid display-name code");
    match kind {
        "language" => {
            // Language IDs have no extensions, private use, or legacy underscores.
            let language = code.split('-').next().unwrap_or_default();
            if !((2..=3).contains(&language.len()) || (5..=8).contains(&language.len()))
                || !letters(language)
                || code.split('-').any(|part| part.len() == 1)
                || code.contains('_')
            {
                return Err(invalid().into());
            }
            let mut locale = language::parse_locale(&code).ok_or_else(invalid)?;
            // ICU sorts/deduplicates variants; ECMA requires duplicates to throw.
            let source_variants = code
                .split('-')
                .skip(1)
                .filter(|part| {
                    part.len() >= 5 || (part.len() == 4 && part.as_bytes()[0].is_ascii_digit())
                })
                .count();
            if source_variants != locale.id.variants.iter().count() {
                return Err(invalid().into());
            }
            context
                .intl_provider()
                .locale_canonicalizer()?
                .canonicalize(&mut locale);
            let canonical = locale.id.to_string();
            if language.len() > 3 {
                // The placeholder is only for parsing; never expose it as a name.
                let tail = canonical.find('-').map_or("", |index| &canonical[index..]);
                Ok(format!("{}{tail}", language.to_ascii_lowercase()))
            } else {
                Ok(canonical)
            }
        }
        "region"
            if (code.len() == 2 && letters(&code))
                || (code.len() == 3 && code.bytes().all(|byte| byte.is_ascii_digit())) =>
        {
            Ok(code.to_ascii_uppercase())
        }
        "script" if code.len() == 4 && letters(&code) => {
            let mut code = code.to_ascii_lowercase();
            code[..1].make_ascii_uppercase();
            Ok(code)
        }
        "currency" if code.len() == 3 && letters(&code) => Ok(code.to_ascii_uppercase()),
        "calendar"
            if code.split('-').all(|part| {
                (3..=8).contains(&part.len())
                    && part.bytes().all(|byte| byte.is_ascii_alphanumeric())
            }) =>
        {
            Ok(code.to_ascii_lowercase())
        }
        "dateTimeField"
            if [
                "era",
                "year",
                "quarter",
                "month",
                "weekOfYear",
                "weekday",
                "day",
                "dayPeriod",
                "hour",
                "minute",
                "second",
                "timeZoneName",
            ]
            .contains(&code.as_str()) =>
        {
            Ok(code)
        }
        _ => Err(invalid().into()),
    }
}
