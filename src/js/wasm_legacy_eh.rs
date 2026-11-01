//! Translation from the deployed legacy exception instructions to the
//! standardized `try_table`/`exnref` instruction set.

use std::borrow::Cow;
use std::collections::HashMap;

use wasm_encoder::reencode::{self, Reencode};
use wasm_encoder::{BlockType, Catch, CodeSection, Function, Instruction, Module, TypeSection};
use wasmparser::{CompositeInnerType, FunctionBody, Operator, Parser, Payload, TypeRef, ValType};

#[derive(Clone)]
struct ModuleTypes {
    functions: Vec<Option<wasmparser::FuncType>>,
    defined_functions: Vec<u32>,
    tags: Vec<u32>,
    original_count: u32,
}

impl ModuleTypes {
    fn parse(bytes: &[u8]) -> Result<Self, String> {
        let mut functions = Vec::new();
        let mut defined_functions = Vec::new();
        let mut tags = Vec::new();
        for payload in Parser::new(0).parse_all(bytes) {
            match payload.map_err(|error| error.to_string())? {
                Payload::TypeSection(section) => {
                    for group in section {
                        for subtype in group.map_err(|error| error.to_string())?.types() {
                            let function = match &subtype.composite_type.inner {
                                CompositeInnerType::Func(function) => Some(function.clone()),
                                _ => None,
                            };
                            functions.push(function);
                        }
                    }
                }
                Payload::ImportSection(section) => {
                    for import in section {
                        if let TypeRef::Tag(tag) = import.map_err(|error| error.to_string())?.ty {
                            tags.push(tag.func_type_idx);
                        }
                    }
                }
                Payload::FunctionSection(section) => {
                    for function in section {
                        defined_functions.push(function.map_err(|error| error.to_string())?);
                    }
                }
                Payload::TagSection(section) => {
                    for tag in section {
                        tags.push(tag.map_err(|error| error.to_string())?.func_type_idx);
                    }
                }
                _ => {}
            }
        }
        Ok(Self {
            original_count: functions.len() as u32,
            functions,
            defined_functions,
            tags,
        })
    }

    fn function(&self, index: u32) -> Result<&wasmparser::FuncType, String> {
        self.functions
            .get(index as usize)
            .and_then(Option::as_ref)
            .ok_or_else(|| {
                format!("legacy exception handling references non-function type {index}")
            })
    }

    fn tag_params(&self, tag: u32) -> Result<&[ValType], String> {
        let type_index = *self
            .tags
            .get(tag as usize)
            .ok_or_else(|| format!("legacy exception handling references unknown tag {tag}"))?;
        Ok(self.function(type_index)?.params())
    }
}

enum Node<'a> {
    Operator(Operator<'a>),
    LegacyTry(LegacyTry<'a>),
    Rethrow(u32),
}

struct LegacyTry<'a> {
    id: usize,
    ty: wasmparser::BlockType,
    body: Vec<Node<'a>>,
    handlers: Vec<Handler<'a>>,
}

struct Handler<'a> {
    tag: Option<u32>,
    body: Vec<Node<'a>>,
}

enum Boundary {
    Catch(u32),
    CatchAll,
    Delegate,
    End,
}

struct BodyParser {
    next_try_id: usize,
}

impl BodyParser {
    fn parse<'a>(&mut self, body: &FunctionBody<'a>) -> Result<Vec<Node<'a>>, String> {
        let mut reader = body
            .get_operators_reader()
            .map_err(|error| error.to_string())?;
        let (nodes, boundary) = self.parse_until_boundary(&mut reader)?;
        if !matches!(boundary, Boundary::End) || !reader.eof() {
            return Err("malformed WebAssembly function body".to_string());
        }
        Ok(nodes)
    }

    fn parse_until_boundary<'a>(
        &mut self,
        reader: &mut wasmparser::OperatorsReader<'a>,
    ) -> Result<(Vec<Node<'a>>, Boundary), String> {
        let mut nodes = Vec::new();
        let mut control_depth = 0usize;
        loop {
            let operator = reader.read().map_err(|error| error.to_string())?;
            match operator {
                Operator::Try { blockty } => {
                    nodes.push(Node::LegacyTry(self.parse_try(reader, blockty)?));
                }
                Operator::Rethrow { relative_depth } => {
                    nodes.push(Node::Rethrow(relative_depth));
                }
                Operator::Block { .. }
                | Operator::Loop { .. }
                | Operator::If { .. }
                | Operator::TryTable { .. } => {
                    control_depth += 1;
                    nodes.push(Node::Operator(operator));
                }
                Operator::End if control_depth > 0 => {
                    control_depth -= 1;
                    nodes.push(Node::Operator(operator));
                }
                Operator::End => return Ok((nodes, Boundary::End)),
                Operator::Catch { tag_index } if control_depth == 0 => {
                    return Ok((nodes, Boundary::Catch(tag_index)));
                }
                Operator::CatchAll if control_depth == 0 => {
                    return Ok((nodes, Boundary::CatchAll));
                }
                Operator::Delegate { .. } if control_depth == 0 => {
                    return Ok((nodes, Boundary::Delegate));
                }
                _ => nodes.push(Node::Operator(operator)),
            }
        }
    }

    fn parse_try<'a>(
        &mut self,
        reader: &mut wasmparser::OperatorsReader<'a>,
        ty: wasmparser::BlockType,
    ) -> Result<LegacyTry<'a>, String> {
        let id = self.next_try_id;
        self.next_try_id += 1;
        let (body, mut boundary) = self.parse_until_boundary(reader)?;
        let mut handlers = Vec::new();
        loop {
            let tag = match boundary {
                Boundary::Catch(tag) => Some(tag),
                Boundary::CatchAll => None,
                Boundary::End => break,
                Boundary::Delegate => {
                    return Err(
                        "legacy WebAssembly delegate lowering is not implemented".to_string()
                    );
                }
            };
            let (handler_body, next) = self.parse_until_boundary(reader)?;
            handlers.push(Handler {
                tag,
                body: handler_body,
            });
            boundary = next;
        }
        Ok(LegacyTry {
            id,
            ty,
            body,
            handlers,
        })
    }
}

struct LegacyReencoder {
    types: ModuleTypes,
    body_index: usize,
    branch_types: HashMap<(u32, bool), u32>,
}

impl LegacyReencoder {
    fn new(types: ModuleTypes) -> Self {
        let mut branch_types = HashMap::new();
        let mut next = types.original_count;
        for tag in 0..types.tags.len() as u32 {
            branch_types.insert((tag, false), next);
            next += 1;
            branch_types.insert((tag, true), next);
            next += 1;
        }
        Self {
            types,
            body_index: 0,
            branch_types,
        }
    }

    fn append_branch_types(&mut self, section: &mut TypeSection) -> Result<(), String> {
        for tag in 0..self.types.tags.len() as u32 {
            let tag_params = self.types.tag_params(tag)?.to_vec();
            let params = tag_params
                .into_iter()
                .map(|ty| self.val_type(ty).map_err(|error| error.to_string()))
                .collect::<Result<Vec<_>, _>>()?;
            section.ty().function([], params.iter().copied());
            let mut with_ref = params;
            with_ref.push(wasm_encoder::ValType::EXNREF);
            section.ty().function([], with_ref);
        }
        Ok(())
    }

    fn branch_type(&mut self, tag: Option<u32>, with_ref: bool) -> Result<BlockType, String> {
        let Some(tag) = tag else {
            return Ok(if with_ref {
                BlockType::Result(wasm_encoder::ValType::EXNREF)
            } else {
                BlockType::Empty
            });
        };
        let params = self.types.tag_params(tag)?;
        if !with_ref && params.len() == 1 {
            return self
                .val_type(params[0])
                .map(BlockType::Result)
                .map_err(|error| error.to_string());
        }
        Ok(BlockType::FunctionType(self.branch_types[&(tag, with_ref)]))
    }
}

impl Reencode for LegacyReencoder {
    type Error = String;

    fn parse_type_section(
        &mut self,
        section: &mut TypeSection,
        types: wasmparser::TypeSectionReader<'_>,
    ) -> Result<(), reencode::Error<Self::Error>> {
        reencode::utils::parse_type_section(self, section, types)?;
        self.append_branch_types(section)
            .map_err(reencode::Error::UserError)
    }

    fn parse_function_body(
        &mut self,
        code: &mut CodeSection,
        body: FunctionBody<'_>,
    ) -> Result<(), reencode::Error<Self::Error>> {
        let function_type = *self
            .types
            .defined_functions
            .get(self.body_index)
            .ok_or_else(|| reencode::Error::UserError("missing function type".to_string()))?;
        self.body_index += 1;
        let param_count = self
            .types
            .function(function_type)
            .map_err(reencode::Error::UserError)?
            .params()
            .len() as u32;
        let mut parser = BodyParser { next_try_id: 0 };
        let nodes = parser.parse(&body).map_err(reencode::Error::UserError)?;
        let original_locals = body
            .get_locals_reader()?
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        let local_count = original_locals.iter().map(|(count, _)| count).sum::<u32>();
        let mut locals = original_locals
            .into_iter()
            .map(|(count, ty)| self.val_type(ty).map(|ty| (count, ty)))
            .collect::<Result<Vec<_>, _>>()?;
        if parser.next_try_id > 0 {
            locals.push((parser.next_try_id as u32, wasm_encoder::ValType::EXNREF));
        }
        let mut function = Function::new(locals);
        let local_base = param_count + local_count;
        emit_nodes(self, &mut function, &nodes, local_base, &mut Vec::new())
            .map_err(reencode::Error::UserError)?;
        function.instruction(&Instruction::End);
        code.function(&function);
        Ok(())
    }
}

fn references_handler(nodes: &[Node<'_>], target_depth: usize) -> bool {
    let mut control_depth = 0usize;
    for node in nodes {
        match node {
            Node::Rethrow(depth) if *depth as usize == target_depth + control_depth => return true,
            Node::LegacyTry(inner) => {
                if references_handler(&inner.body, target_depth + control_depth + 1)
                    || inner.handlers.iter().any(|handler| {
                        references_handler(&handler.body, target_depth + control_depth + 1)
                    })
                {
                    return true;
                }
            }
            Node::Operator(
                Operator::Block { .. }
                | Operator::Loop { .. }
                | Operator::If { .. }
                | Operator::TryTable { .. },
            ) => control_depth += 1,
            Node::Operator(Operator::End) => control_depth = control_depth.saturating_sub(1),
            _ => {}
        }
    }
    false
}

fn handler_uses_current_rethrow(nodes: &[Node<'_>]) -> bool {
    references_handler(nodes, 0)
}

fn emit_nodes(
    reencoder: &mut LegacyReencoder,
    function: &mut Function,
    nodes: &[Node<'_>],
    local_base: u32,
    source_labels: &mut Vec<Option<u32>>,
) -> Result<(), String> {
    for node in nodes {
        match node {
            Node::Operator(operator) => {
                let instruction = reencoder
                    .instruction(operator.clone())
                    .map_err(|error| error.to_string())?;
                function.instruction(&instruction);
                match operator {
                    Operator::Block { .. }
                    | Operator::Loop { .. }
                    | Operator::If { .. }
                    | Operator::TryTable { .. } => source_labels.push(None),
                    Operator::End => {
                        source_labels.pop();
                    }
                    _ => {}
                }
            }
            Node::Rethrow(depth) => {
                let local = source_labels
                    .len()
                    .checked_sub(*depth as usize + 1)
                    .and_then(|index| source_labels.get(index))
                    .copied()
                    .flatten()
                    .ok_or_else(|| "legacy rethrow has no matching catch".to_string())?;
                function.instruction(&Instruction::LocalGet(local));
                function.instruction(&Instruction::ThrowRef);
            }
            Node::LegacyTry(legacy) => {
                emit_legacy_try(reencoder, function, legacy, local_base, source_labels)?;
            }
        }
    }
    Ok(())
}

fn emit_legacy_try(
    reencoder: &mut LegacyReencoder,
    function: &mut Function,
    legacy: &LegacyTry<'_>,
    local_base: u32,
    source_labels: &mut Vec<Option<u32>>,
) -> Result<(), String> {
    let ty = reencoder
        .block_type(legacy.ty)
        .map_err(|error| error.to_string())?;
    function.instruction(&Instruction::Block(ty));
    let uses_ref = legacy
        .handlers
        .iter()
        .map(|handler| handler_uses_current_rethrow(&handler.body))
        .collect::<Vec<_>>();
    for (handler, with_ref) in legacy.handlers.iter().zip(&uses_ref).rev() {
        let sent = reencoder.branch_type(handler.tag, *with_ref)?;
        function.instruction(&Instruction::Block(sent));
    }
    let catches = legacy
        .handlers
        .iter()
        .enumerate()
        .map(|(label, handler)| match (handler.tag, uses_ref[label]) {
            (Some(tag), false) => Catch::One {
                tag,
                label: label as u32,
            },
            (Some(tag), true) => Catch::OneRef {
                tag,
                label: label as u32,
            },
            (None, false) => Catch::All {
                label: label as u32,
            },
            (None, true) => Catch::AllRef {
                label: label as u32,
            },
        })
        .collect::<Vec<_>>();
    function.instruction(&Instruction::TryTable(ty, Cow::Owned(catches)));
    source_labels.push(None);
    emit_nodes(reencoder, function, &legacy.body, local_base, source_labels)?;
    source_labels.pop();
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::Br(legacy.handlers.len() as u32));
    let exn_local = local_base + legacy.id as u32;
    for (index, (handler, with_ref)) in legacy.handlers.iter().zip(uses_ref).enumerate() {
        function.instruction(&Instruction::End);
        if with_ref {
            function.instruction(&Instruction::LocalSet(exn_local));
        }
        source_labels.push(with_ref.then_some(exn_local));
        emit_nodes(
            reencoder,
            function,
            &handler.body,
            local_base,
            source_labels,
        )?;
        source_labels.pop();
        if index + 1 < legacy.handlers.len() {
            function.instruction(&Instruction::Br((legacy.handlers.len() - index - 1) as u32));
        }
    }
    function.instruction(&Instruction::End);
    Ok(())
}

/// Lowers legacy exception instructions to the standardized exception model.
pub(super) fn lower(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let types = ModuleTypes::parse(bytes)?;
    let mut reencoder = LegacyReencoder::new(types);
    let mut module = Module::new();
    reencode::utils::parse_core_module(&mut reencoder, &mut module, Parser::new(0), bytes)
        .map_err(|error| error.to_string())?;
    Ok(module.finish())
}

#[cfg(test)]
mod tests {
    use super::lower;
    use wasm_encoder::{
        BlockType, CodeSection, ExportKind, ExportSection, Function, FunctionSection, Instruction,
        Module, TagKind, TagSection, TagType, TypeSection,
    };

    fn legacy_module(rethrow: bool) -> Vec<u8> {
        let mut module = Module::new();
        let mut types = TypeSection::new();
        types.ty().function([wasm_encoder::ValType::I32], []);
        types.ty().function([], [wasm_encoder::ValType::I32]);
        module.section(&types);
        let mut functions = FunctionSection::new();
        functions.function(1);
        module.section(&functions);
        let mut tags = TagSection::new();
        tags.tag(TagType {
            kind: TagKind::Exception,
            func_type_idx: 0,
        });
        module.section(&tags);
        let mut exports = ExportSection::new();
        exports.export("run", ExportKind::Func, 0);
        module.section(&exports);
        let mut function = Function::new([]);
        function.instruction(&Instruction::Try(BlockType::Result(
            wasm_encoder::ValType::I32,
        )));
        if rethrow {
            function.instruction(&Instruction::Try(BlockType::Result(
                wasm_encoder::ValType::I32,
            )));
        }
        function.instruction(&Instruction::I32Const(if rethrow { 41 } else { 37 }));
        function.instruction(&Instruction::Throw(0));
        function.instruction(&Instruction::Catch(0));
        if rethrow {
            function.instruction(&Instruction::Block(BlockType::Empty));
            function.instruction(&Instruction::Rethrow(1));
            function.instruction(&Instruction::End);
            function.instruction(&Instruction::End);
            function.instruction(&Instruction::Catch(0));
        }
        function.instruction(&Instruction::End);
        function.instruction(&Instruction::End);
        let mut code = CodeSection::new();
        code.function(&function);
        module.section(&code);
        module.finish()
    }

    fn run_i32(bytes: &[u8]) -> i32 {
        let lowered = lower(&bytes).expect("legacy exception module must lower");
        let mut config = wasmtime::Config::new();
        config.wasm_exceptions(true);
        let engine = wasmtime::Engine::new(&config).expect("exception engine must initialize");
        let module = wasmtime::Module::new(&engine, lowered).expect("lowered module must validate");
        let mut store = wasmtime::Store::new(&engine, ());
        let instance = wasmtime::Instance::new(&mut store, &module, &[])
            .expect("lowered module must instantiate");
        instance
            .get_typed_func::<(), i32>(&mut store, "run")
            .expect("test export must exist")
            .call(&mut store, ())
            .expect("test export must return")
    }

    #[test]
    fn catches_payload_from_wasm_throw() {
        assert_eq!(run_i32(&legacy_module(false)), 37);
    }

    #[test]
    fn rethrows_the_same_exception_to_an_outer_catch() {
        assert_eq!(run_i32(&legacy_module(true)), 41);
    }
}
