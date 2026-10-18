/** Checks the extracted AST and types of the eight JIT ABI offset constants. */

import rust
import codeql.rust.internal.typeinference.TypeMention

from Const item
where
  (
    item.getFile().getAbsolutePath().matches("%/engine/boa/core/engine/src/jit/arithmetic.rs") and
    item.getName().getText().matches("NATIVE_FRAME_%_OFFSET")
    or
    item.getFile().getAbsolutePath().matches("%/engine/boa/core/engine/src/jit/runtime_call.rs") and
    item.getName().getText() = "RUNTIME_CALL_TRAMPOLINE_OFFSET"
  )
select item.getFile().getAbsolutePath(), item.getLocation().getStartLine(), item.getName().getText(),
  count(OffsetOfExpr offset | offset.getParentNode*() = item.getBody() | offset),
  count(OffsetOfExpr offset |
    offset.getParentNode*() = item.getBody() and
    exists(offset.getTypeRepr().(TypeMention).getType())
  | offset),
  count(TypeMention mention | mention = item.getTypeRepr() and exists(mention.getType()) | mention)
