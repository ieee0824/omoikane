---
number: 059
slug: boa-patch-not-applied
status: open
---

# Boa フォーク patch が適用されず IC バグが再発する

## 概要

058 で `[patch.crates-io]` により Boa フォーク(IC バグ修正、rev 6b9778e)を差し込んだが、
**patch が crate graph に適用されず**、環境によっては素の boa_engine 0.21.0 が使われて
IC バグ(`parseDecls` の "not a callable function" 等)が再発する。

## 根本原因

- omoikane の依存は `boa_engine = "0.21.0"`(= `^0.21.0`)。Cargo.lock は 0.21.0 で固定されうる
- フォークは boa v0.21.1 タグ起点のため patched crate の version が **0.21.1**
- `[patch.crates-io]` は**グラフ内の同一バージョンのみ置換**する。0.21.1 の patch は
  0.21.0 に固定されたグラフには使われず `warning: patch ... was not used in the crate graph`
- Cargo.lock は .gitignore 対象のため lock 状態がマシン依存。コンテナでは新規解決で
  0.21.1 + patch が効いていたが、既存 lock が 0.21.0 のマシン(ユーザーの Mac)では未適用

## 対応方針

`[patch.crates-io]` をやめ、`boa_engine` / `boa_gc` を **直接 git 依存**(rev 固定)にする:

```toml
boa_engine = { git = "https://github.com/ieee0824/boa", rev = "6b9778e", features = ["annex-b"] }
boa_gc = { git = "https://github.com/ieee0824/boa", rev = "6b9778e" }
```

- version 照合が不要になり、rev 固定で全マシン決定的
- `[patch.crates-io]` セクションは削除
- boa_engine の transitive な兄弟クレート(boa_gc/boa_interner 等)は同一 git workspace から解決される

## 受け入れ条件

- `cargo build` で "was not used in the crate graph" 警告が出ない
- `cargo test --test boa_inline_cache` が PASS(合成再現で IC バグが直っていることを確認)
- tokyo6.tokyo のレンダリングで `parseDecls` の "not a callable function" が出ない
- Acid3 97/100 維持、全テスト維持

## 検証結果 (2026-07-13)

- `cargo build -j1`: PASS。`was not used in the crate graph` 警告なし
- `cargo tree -i boa_engine`: `boa_engine v0.21.1 (https://github.com/ieee0824/boa?rev=6b9778e#6b9778ef)`
- `cargo test --test boa_inline_cache -j1`: 1 passed
- `cargo test --lib -j1`: 1048 passed、0 failed、26 ignored
- `cargo run --example acid3 -j1`: faithful load / direct drive ともに 97/100、test index 100
- `cargo run --example screenshot -j1 -- "https://tokyo6.tokyo/" /tmp/tokyo6-059.png`: 正常終了。ログに `parseDecls` および `not a callable function` なし

## 関連

- 058 Boa フォークによる IC バグ根治(patch 方式が不発だった)
