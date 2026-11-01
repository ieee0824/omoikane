//! WebAssembly core module validation and metadata bridge.

use super::*;
use serde::Serialize;
use wasmi::{Config, Engine, Module};
use wasmparser::{ExternalKind, Parser, Payload, TypeRef};

const MAX_MODULE_BYTES: usize = 64 * 1024 * 1024;

/// Runtime-owned WebAssembly engine shared by compiled module objects. Wasmi
/// types and lifetimes do not leak through the JavaScript API.
pub(super) struct State {
    engine: Engine,
}

impl Default for State {
    fn default() -> Self {
        let mut config = Config::default();
        config.consume_fuel(true);
        Self {
            engine: Engine::new(&config),
        }
    }
}

impl State {
    fn validate(&self, bytes: &[u8]) -> bool {
        bytes.len() <= MAX_MODULE_BYTES && Module::validate(&self.engine, bytes).is_ok()
    }

    fn compile(&self, bytes: &[u8]) -> Result<CompiledModule, wasmi::Error> {
        let module = Module::new(&self.engine, bytes)?;
        let metadata =
            ModuleMetadata::parse(bytes).map_err(|error| wasmi::Error::new(error.to_string()))?;
        Ok(CompiledModule { module, metadata })
    }
}

#[derive(Trace, Finalize, boa_engine::JsData)]
struct CompiledModule {
    #[allow(dead_code)]
    #[unsafe_ignore_trace]
    module: Module,
    #[unsafe_ignore_trace]
    metadata: ModuleMetadata,
}

#[derive(Default)]
struct ModuleMetadata {
    imports: Vec<ImportDescriptor>,
    exports: Vec<ExportDescriptor>,
    custom_sections: Vec<CustomSection>,
}

impl ModuleMetadata {
    fn parse(bytes: &[u8]) -> Result<Self, wasmparser::BinaryReaderError> {
        let mut metadata = Self::default();
        for payload in Parser::new(0).parse_all(bytes) {
            match payload? {
                Payload::ImportSection(section) => {
                    for import in section {
                        let import = import?;
                        metadata.imports.push(ImportDescriptor {
                            module: import.module.to_owned(),
                            name: import.name.to_owned(),
                            kind: import_kind(import.ty),
                        });
                    }
                }
                Payload::ExportSection(section) => {
                    for export in section {
                        let export = export?;
                        metadata.exports.push(ExportDescriptor {
                            name: export.name.to_owned(),
                            kind: external_kind(export.kind),
                        });
                    }
                }
                Payload::CustomSection(section) => metadata.custom_sections.push(CustomSection {
                    name: section.name().to_owned(),
                    data: section.data().to_vec(),
                }),
                _ => {}
            }
        }
        Ok(metadata)
    }
}

#[derive(Serialize)]
struct ImportDescriptor {
    module: String,
    name: String,
    kind: &'static str,
}

#[derive(Serialize)]
struct ExportDescriptor {
    name: String,
    kind: &'static str,
}

struct CustomSection {
    name: String,
    data: Vec<u8>,
}

fn import_kind(ty: TypeRef) -> &'static str {
    match ty {
        TypeRef::Func(_) => "function",
        TypeRef::Table(_) => "table",
        TypeRef::Memory(_) => "memory",
        TypeRef::Global(_) => "global",
        TypeRef::Tag(_) => "tag",
    }
}

fn external_kind(kind: ExternalKind) -> &'static str {
    match kind {
        ExternalKind::Func => "function",
        ExternalKind::Table => "table",
        ExternalKind::Memory => "memory",
        ExternalKind::Global => "global",
        ExternalKind::Tag => "tag",
    }
}

fn source_bytes(args: &[JsValue], context: &mut Context) -> JsResult<Vec<u8>> {
    let bytes = text_stream::buffer_source_bytes(
        args.first().unwrap_or(&JsValue::undefined()),
        context,
        "WebAssembly source",
        false,
    )?;
    if bytes.len() > MAX_MODULE_BYTES {
        return Err(JsNativeError::range()
            .with_message("WebAssembly module exceeds the 64 MiB limit")
            .into());
    }
    Ok(bytes)
}

fn validate_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let bytes = match source_bytes(args, context) {
        Ok(bytes) => bytes,
        Err(error) if error.as_native().is_some_and(|native| native.is_range()) => {
            return Ok(false.into());
        }
        Err(error) => return Err(error),
    };
    with_host_state(|host| Ok(host.borrow().webassembly.validate(&bytes).into()))
}

fn compile_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let bytes = source_bytes(args, context)?;
    with_host_state(|host| {
        let module = host
            .borrow()
            .webassembly
            .compile(&bytes)
            .map_err(|error| JsNativeError::syntax().with_message(error.to_string()))?;
        Ok(JsObject::from_proto_and_data(None, module).into())
    })
}

fn metadata_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let module_handle = args
        .first()
        .and_then(JsValue::as_object)
        .ok_or_else(|| JsNativeError::typ().with_message("Invalid WebAssembly.Module handle"))?;
    let operation = args
        .get(1)
        .unwrap_or(&JsValue::undefined())
        .to_string(context)?
        .to_std_string_escaped();
    let name = args
        .get(2)
        .map(|value| value.to_string(context))
        .transpose()?
        .map(|value| value.to_std_string_escaped());
    let module = module_handle
        .downcast_ref::<CompiledModule>()
        .ok_or_else(|| JsNativeError::typ().with_message("Invalid WebAssembly.Module handle"))?;
    let result = (|| {
        let json = match operation.as_str() {
            "imports" => serde_json::to_string(&module.metadata.imports),
            "exports" => serde_json::to_string(&module.metadata.exports),
            "customSections" => serde_json::to_string(
                &module
                    .metadata
                    .custom_sections
                    .iter()
                    .filter(|section| name.as_deref() == Some(section.name.as_str()))
                    .map(|section| section.data.as_slice())
                    .collect::<Vec<_>>(),
            ),
            _ => {
                return Err(JsNativeError::typ()
                    .with_message("Unknown WebAssembly.Module metadata operation")
                    .into());
            }
        }
        .map_err(|error| JsNativeError::error().with_message(error.to_string()))?;
        Ok(js_string!(json).into())
    })();
    drop(module);
    result
}

/// Registers the private hooks consumed by the realm-local WebAssembly wrapper.
pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    for (name, length, function) in [
        (
            js_string!("__omoikane_wasm_validate"),
            1,
            NativeFunction::from_copy_closure(validate_native),
        ),
        (
            js_string!("__omoikane_wasm_compile"),
            1,
            NativeFunction::from_copy_closure(compile_native),
        ),
        (
            js_string!("__omoikane_wasm_module_metadata"),
            3,
            NativeFunction::from_copy_closure(metadata_native),
        ),
    ] {
        register_private_builtin_callable(context, bindings, name, length, function)?;
    }
    Ok(())
}
