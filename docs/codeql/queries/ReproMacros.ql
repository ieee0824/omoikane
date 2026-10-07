/** Lists macro expansion sizes in the independent Issue #1266 reproduction. */

import rust

from MacroCall call
where call.getFile().getAbsolutePath().matches("%/macro-gaps/src/lib.rs")
select call.getFile().getAbsolutePath(), call.getLocation().getStartLine(),
  call.getLocation().getStartColumn(), call.toString(),
  count(AstNode node | call.getMacroCallExpansion() = node.getParentNode*() | node)
