/** Checks that the module-loader alias and both return paths resolve. */

import rust
import codeql.rust.internal.PathResolution

from AstNode owner, TypeRepr repr, string kind
where
  owner.getFile().getAbsolutePath().matches("%/engine/boa/core/engine/src/module/loader/mod.rs") and
  (
    exists(TypeAlias alias |
      alias = owner and
      alias.getName().getText() = "LoadImportedModuleFn" and
      repr = alias.getTypeRepr() and
      kind = "alias"
    )
    or
    exists(Function function |
      function = owner and
      function.getName().getText() = "load_imported_module" and
      repr = function.getRetType().getTypeRepr() and
      kind = "return"
    )
  )
select kind, owner.getLocation().getStartLine(), repr.toString(),
  count(ItemNode target |
    target = resolvePath(repr.(PathTypeRepr).getPath())
  | target)
