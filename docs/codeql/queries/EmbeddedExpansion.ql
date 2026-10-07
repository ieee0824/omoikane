/**
 * Checks that both embedded-module fixtures expand into file-entry arrays.
 * An outer macro or compile_error node alone does not satisfy this contract.
 */

import rust

from MacroCall call
where
  call.getFile().getAbsolutePath().matches("%/engine/boa/tests/macros/tests/embedded.rs") and
  call.toString().matches("%__embed_module_inner!%")
select call.getLocation().getStartLine(),
  count(ArrayListExpr array | array = call.getMacroCallExpansion() | array),
  count(TupleExpr entry | call.getMacroCallExpansion() = entry.getParentNode*() | entry),
  count(StringLiteralExpr path |
    call.getMacroCallExpansion() = path.getParentNode*() and
    path.getTextValue().matches("%file%.js%")
  |
    path
  ),
  count(MacroCall error |
    call.getMacroCallExpansion() = error.getParentNode*() and
    error.toString().matches("%compile_error!%")
  |
    error
  )
