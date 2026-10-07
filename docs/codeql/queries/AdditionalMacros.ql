/** Lists macros in the file newly reported by extractor 2.27.2. */

import rust

from MacroCall call
where call.getFile().getAbsolutePath().matches("%/engine/boa/core/engine/src/jit/platform.rs")
select call.getFile().getAbsolutePath(), call.getLocation().getStartLine(),
  call.getLocation().getStartColumn(), call.toString(),
  count(AstNode node | call.getMacroCallExpansion() = node.getParentNode*() | node)
