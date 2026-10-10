//! Finalization registries own holdings while keeping targets and tokens weak.
use crate::{
    Context, JsArgs, JsData, JsNativeError, JsResult, JsString, JsValue,
    builtins::{
        BuiltInBuilder, BuiltInConstructor, BuiltInObject, IntrinsicObject, symbol::Symbol,
    },
    context::intrinsics::{Intrinsics, StandardConstructor, StandardConstructors},
    js_string,
    object::{ErasedVTableObject, JsObject, internal_methods::get_prototype_from_constructor},
    property::Attribute,
    realm::{Realm, RealmEdge},
    string::StaticJsStrings,
    symbol::WeakJsSymbol,
};
use boa_gc::{Finalize, Trace, WeakGcEdge};

mod cleanup;

#[derive(Trace, Finalize)]
enum WeakTarget {
    Object(WeakGcEdge<ErasedVTableObject>),
    Symbol(WeakJsSymbol),
}
impl std::fmt::Debug for WeakTarget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Object(_) => "WeakObject",
            Self::Symbol(_) => "WeakSymbol",
        })
    }
}
impl WeakTarget {
    fn new(value: &JsValue, context: &mut Context) -> JsResult<Self> {
        if let Some(object) = value.as_object() {
            return Ok(Self::Object(WeakGcEdge::new_rooted(&object.root_inner())));
        }
        if let Some(symbol) = value.as_symbol() {
            if Symbol::key_for(&JsValue::undefined(), &[value.clone()], context)?.is_undefined() {
                return Ok(Self::Symbol(symbol.downgrade()));
            }
        }
        Err(JsNativeError::typ()
            .with_message("value cannot be held weakly")
            .into())
    }
    fn is_alive(&self) -> bool {
        match self {
            Self::Object(object) => object.is_upgradable(),
            Self::Symbol(symbol) => symbol.is_alive(),
        }
    }
    fn matches(&self, value: &JsValue) -> bool {
        match self {
            Self::Object(weak) => weak
                .upgrade_edge()
                .zip(value.as_object())
                .is_some_and(|(left, right)| JsObject::equals(&JsObject::from(left), &right)),
            Self::Symbol(weak) => value
                .as_symbol()
                .is_some_and(|symbol| weak.matches(&symbol)),
        }
    }
}

#[derive(Debug, Trace, Finalize)]
struct Cell {
    target: WeakTarget,
    holding: JsValue,
    token: Option<WeakTarget>,
}

#[derive(Debug, Trace, Finalize, JsData)]
pub(crate) struct FinalizationRegistry {
    callback: JsObject,
    realm: RealmEdge,
    // Cleanup leaves removed entries as tombstones while callbacks can call
    // `register` or `unregister`. This keeps the scan cursor stable and avoids
    // shifting the tail of the vector once for every collected target.
    cells: Vec<Option<Cell>>,
    cleanup_passes: usize,
    cleanup_scheduled: bool,
}
impl BuiltInObject for FinalizationRegistry {
    const NAME: JsString = StaticJsStrings::FINALIZATION_REGISTRY;
}
impl IntrinsicObject for FinalizationRegistry {
    fn init(realm: &Realm) {
        BuiltInBuilder::from_standard_constructor::<Self>(realm)
            .method(Self::register, js_string!("register"), 2)
            .method(Self::unregister, js_string!("unregister"), 1)
            .property(
                crate::JsSymbol::to_string_tag(),
                Self::NAME,
                Attribute::CONFIGURABLE,
            )
            .build();
    }
    fn get(intrinsics: &Intrinsics) -> JsObject {
        intrinsics
            .constructors()
            .finalization_registry()
            .constructor()
    }
}
impl BuiltInConstructor for FinalizationRegistry {
    const CONSTRUCTOR_ARGUMENTS: usize = 1;
    const PROTOTYPE_STORAGE_SLOTS: usize = 3;
    const CONSTRUCTOR_STORAGE_SLOTS: usize = 0;
    const STANDARD_CONSTRUCTOR: fn(&StandardConstructors) -> &StandardConstructor =
        StandardConstructors::finalization_registry;
    fn constructor(
        new_target: &JsValue,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<JsValue> {
        if new_target.is_undefined() {
            return Err(JsNativeError::typ()
                .with_message("FinalizationRegistry requires new")
                .into());
        }
        let callback = args.get_or_undefined(0).as_callable().ok_or_else(|| {
            JsNativeError::typ().with_message("cleanup callback must be callable")
        })?;
        let _callback_root = callback.clone().root();
        let prototype =
            get_prototype_from_constructor(new_target, Self::STANDARD_CONSTRUCTOR, context)?;
        let registry = JsObject::from_proto_and_data_with_shared_shape(
            context.root_shape(),
            prototype,
            Self {
                callback,
                realm: context.realm().to_edge(),
                cells: Vec::new(),
                cleanup_passes: 0,
                cleanup_scheduled: false,
            },
        );
        let _root = registry.clone().root();
        context.track_finalization_registry(&registry);
        Ok(registry.into())
    }
}
impl FinalizationRegistry {
    fn require(this: &JsValue) -> JsResult<JsObject<Self>> {
        this.as_object()
            .and_then(|object| object.downcast::<Self>().ok())
            .ok_or_else(|| {
                JsNativeError::typ()
                    .with_message("receiver must be a FinalizationRegistry")
                    .into()
            })
    }
    fn register(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
        let registry = Self::require(this)?;
        let _registry_root = registry.clone().root();
        let target_value = args.get_or_undefined(0);
        let target = WeakTarget::new(target_value, context)?;
        let _target_root = match &target {
            WeakTarget::Object(edge) => Some(edge.root()),
            _ => None,
        };
        let holding = args.get_or_undefined(1);
        if JsValue::same_value(target_value, holding) {
            return Err(JsNativeError::typ()
                .with_message("target and holding must differ")
                .into());
        }
        let token_value = args.get_or_undefined(2);
        let token = if token_value.is_undefined() {
            None
        } else {
            Some(WeakTarget::new(token_value, context)?)
        };
        registry.borrow_mut().data_mut().cells.push(Some(Cell {
            target,
            holding: holding.clone(),
            token,
        }));
        context.track_finalization_registry(&registry.clone().upcast());
        Ok(JsValue::undefined())
    }
    fn unregister(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
        let registry = Self::require(this)?;
        let _root = registry.clone().root();
        let token = args.get_or_undefined(0);
        let _validation = WeakTarget::new(token, context)?;
        let mut object = registry.borrow_mut();
        let state = object.data_mut();
        let mut removed = false;
        if state.cleanup_passes == 0 {
            state.cells.retain(|cell| {
                let matches = cell.as_ref().is_some_and(|cell| {
                    cell.token.as_ref().is_some_and(|weak| weak.matches(token))
                });
                removed |= matches;
                !matches
            });
        } else {
            for cell in &mut state.cells {
                if cell
                    .as_ref()
                    .is_some_and(|cell| cell.token.as_ref().is_some_and(|weak| weak.matches(token)))
                {
                    *cell = None;
                    removed = true;
                }
            }
        }
        Ok(removed.into())
    }
}
