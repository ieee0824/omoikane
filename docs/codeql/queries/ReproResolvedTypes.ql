/** Lists concrete inferred type renderings for offset constants and loader returns. */
import rust
import codeql.rust.internal.typeinference.TypeMention

from TypeMention mention, string kind, int line
where
  mention.getFile().getAbsolutePath().matches("%/macro-gaps/src/lib.rs") and
  (
    exists(Const item |
      item.getName().getText().matches("%_OFFSET") and
      mention = item.getTypeRepr() and
      kind = "const declared" and line = item.getLocation().getStartLine()
    )
    or
    exists(OffsetOfExpr expr, Const item |
      item.getName().getText().matches("%_OFFSET") and
      expr.getParentNode*() = item.getBody() and
      mention = expr.getTypeRepr() and
      kind = "offset inferred" and line = item.getLocation().getStartLine()
    )
    or
    exists(Function function |
      function.getName().getText() = "load" and
      mention = function.getRetType().getTypeRepr() and
      kind = "loader return" and line = function.getLocation().getStartLine()
    )
  )
select kind, line, mention.toString(), mention.getType().toString()
