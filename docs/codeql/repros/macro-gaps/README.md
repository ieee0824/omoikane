# Rust macro extraction reproduction

Omoikane [Issue #1266](https://github.com/ieee0824/omoikane/issues/1266) の独立した
再現crate。ブラウザのworkspaceや実行コードは変更しない。

`dynify 0.1.2`、`static_assertions 1.1.0`と間接依存を、基準main
`903227c78811cdc22dbf473845d17b72ce116cf8` の `engine/boa/Cargo.lock` と揃える。
`Cargo.lock`を使い、Rust 1.98.1、CodeQL CLI / Rust libraryの版、hostと解析targetを
結果に記録する。CLI 2.27.1 / `codeql/rust-all 0.2.22`が初期測定の組合せ。

## 入力と対照

- `NativeFrame`の先頭2フィールドと`RuntimeCallFrame`の先頭の関数ポインタを
  `repr(C)`と`const_assert!(offset_of!(...))`で検査する。元の全JIT構造体の
  ABI再検証ではなく、問題のマクロ入力を残した縮小例である。
- `DynLoader`のtraitとimplの戻り値型で`Fn!`を使う。所有する`Rc<Self>`、
  2つの借用ライフタイム、`Future<Output = u32>`の契約を残し、
  `from_fn!`で構築したFutureをpollして値42を確認する。ブラウザ固有の
  Referrer・Module・Contextは除き、同じ依存版のマクロを使う。
- 同じ`offset_of!`を通常のテスト式にも置き、展開ASTの正の対照にする。
  assertionの外側のASTだけで内側マクロの成功を判定しない。
- ARM64のassertionは、元のJITテストと同様に**文単位**のcfgを持つケースと、
  関数全体にcfgを付けた対照を分ける。解析targetだけを変えて比較する。
  x86_64向けDBの結果をARM64でのRustテスト実行と混同しない。

## Rust検証

リポジトリの容量guardを使い、worktree専用targetへ出力する。

```sh
export CARGO_TARGET_DIR="/target/$(basename "$PWD")"
python3 scripts/cargo-space-guard.py run -- \
  cargo test --locked --manifest-path docs/codeql/repros/macro-gaps/Cargo.toml
python3 scripts/cargo-space-guard.py run -- \
  cargo build --locked --manifest-path docs/codeql/repros/macro-gaps/Cargo.toml
```

このcrateのテストは縮小したABIとFuture契約を検査する。ブラウザや本体の
Native JITテストの代わりにはならない。

## CodeQL再現

`OUT`は新しい証跡ディレクトリ、`ANALYSIS_TARGET`は
`aarch64-unknown-linux-gnu`または`x86_64-unknown-linux-gnu`。
両targetの出力を別々に保持する。CLIは`PATH`に追加しておく。

```sh
mkdir -p "$OUT"
python3 scripts/cargo-space-guard.py run -- \
  codeql database create "$OUT/database" --language=rust --build-mode=none \
    --source-root docs/codeql/repros/macro-gaps --threads=2 --ram=2048 \
    --extractor-option="cargo_target_dir=$CARGO_TARGET_DIR" \
    --extractor-option="cargo_target=$ANALYSIS_TARGET"
codeql pack ci docs/codeql/queries
codeql query run --database="$OUT/database" --threads=2 --ram=2048 \
  --output="$OUT/diagnostics.bqrs" docs/codeql/queries/RawDiagnostics.ql
codeql bqrs decode --format=csv --output="$OUT/diagnostics.csv" \
  "$OUT/diagnostics.bqrs"
codeql query run --database="$OUT/database" --threads=2 --ram=2048 \
  --output="$OUT/macros.bqrs" docs/codeql/queries/ReproMacros.ql
codeql bqrs decode --format=csv --output="$OUT/macros.csv" "$OUT/macros.bqrs"
```

同じDBへのqueryは直列に実行する。診断のseverity・位置と内側マクロの
展開ASTを照合し、ソース・lock・元CSV・抽出ログのハッシュを保存する。
初期例で警告が出なくなっても、ASTと型情報を確認してから改善と判断する。

上流追跡先は [標準マクロの警告](https://redirect.github.com/github/codeql/issues/19982)
と [マクロ警告の説明](https://redirect.github.com/github/codeql/issues/19966)。
一般的な関連議論であり、この例の修正や同じ原因を証明するリンクではない。
再現例は本Issueに添付できる成果物として用意し、上流への投稿は別途確認する。

## 2026-10-07の結果

上記固定依存・Rust 1.98.1・CLI 2.27.1・library 0.2.22で測定した。
両DBのソースarchive内の`src/lib.rs`ハッシュはこの入力と一致した。

| 解析target | warning | マクロ照会行 | 展開AST 0 |
| --- | ---: | ---: | ---: |
| ARM64 Linux | 5 | 26 | 5 |
| x86_64 Linux | 6 | 24 | 6 |

両方で`offset_of!`3箇所、`__Fn`2箇所がwarningかつAST 0。
通常のテスト式の`offset_of!`3箇所にはそれぞれ8 AST nodeがある。
`Fn!`外側の9 nodeや`from_fn!`の成功を、内側`__Fn`の成功とは扱わない。

文単位cfgのARM64 assertionは、x86_64解析で内側`panic_2021`がwarningかつ
AST 0になり、ARM64解析では展開される。関数全体cfgの対照には同じwarningが
出なかった。cfgを削除して警告を消す対策は採らない。

ARM64 hostで`cargo test --locked`の4テストと`cargo build --locked`が成功。
x86_64はCodeQL解析targetの比較であり、このcrateのRustテストをx86_64で
実行したという結果ではない。

[版・入力・元CSVのハッシュ](../../results/2026-10-07-reproduction.json)、
[ARM64診断](../../results/2026-10-07-repro-arm64-diagnostics.csv)、
[ARM64マクロ](../../results/2026-10-07-repro-arm64-macros.csv)、
[x86_64診断](../../results/2026-10-07-repro-x64-diagnostics.csv)、
[x86_64マクロ](../../results/2026-10-07-repro-x64-macros.csv)を保持した。
元DB・BQRS・CSV・ログは`.artifacts/issue1266/`に保存している。

最新版APIとpack取得を確認した時点では、基準測定後の新しいextractor/libraryは
未公開だった。これらは同じ版での再現結果であり、更新版での改善確認ではない。
