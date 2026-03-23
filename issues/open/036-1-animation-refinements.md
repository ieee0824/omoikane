---
number: 036-1
slug: animation-refinements
parent: 036-css-animation-final-state
status: open
---

# CSS animation 実装の改善

## Copilot/codex レビューからの残課題

1. **`var()` が @keyframes 内で未解決** — custom properties の解決パスを keyframe 適用時にも通す
2. **`@media` 内の `@keyframes` が収集されない** — `collect_keyframes` を再帰的に @media ブロック内も探索
3. **em/rem 解決の context** — keyframe 適用時の font-size 解決を要素の computed font-size に合わせる
