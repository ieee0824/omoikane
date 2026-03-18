---
number: 007-2
slug: positioning
status: open
parent: 007
---

# 絶対配置・固定配置

position: absolute / fixed を実装し、containing blockからのオフセット配置を可能にする。

## タスク
- [ ] containing block の決定ロジック（position: relative/absolute/fixed の祖先探索）
- [ ] position: absolute の配置計算（top, right, bottom, left）
- [ ] position: fixed の配置計算（ビューポート基準）
- [ ] 通常フローからの除外（absoluteとfixedは通常フローに影響しない）
- [ ] auto値の解決（top/left が auto の場合の静的位置）
- [ ] width/height が auto の場合のシュリンクトゥフィット
