/**
 * Lists original macro locations and expanded AST sizes for the tracked files.
 * A nonzero AST count is evidence of expansion, not proof of complete analysis.
 */

import rust

predicate tracked(File file) {
  file.getAbsolutePath().matches("%/engine/boa/core/engine/src/jit/arithmetic.rs") or
  file.getAbsolutePath().matches("%/engine/boa/core/engine/src/jit/runtime_call.rs") or
  file.getAbsolutePath().matches("%/engine/boa/core/engine/src/jit/mod.rs") or
  file.getAbsolutePath().matches("%/engine/boa/core/engine/src/module/loader/mod.rs") or
  file.getAbsolutePath().matches("%/engine/boa/tests/macros/tests/embedded.rs") or
  file.getAbsolutePath().matches("%/tests/jit_code_memory.rs")
}

from MacroCall call
where tracked(call.getFile())
select call.getFile().getAbsolutePath(), call.getLocation().getStartLine(),
  call.getLocation().getStartColumn(), call.toString(),
  count(AstNode node | call.getMacroCallExpansion() = node.getParentNode*() | node)
