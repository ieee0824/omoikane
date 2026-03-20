---
number: 013-5
slug: external-css-fetch-fixes
parent: 013-real-world-rendering
status: open
---

# 外部CSSフェッチの修正・改善（PR#4コメント対応）

## 概要

PR#4で実装した外部CSSフェッチ機能（013-2）に対して、Copilot reviewerから指摘されたバグ・セキュリティ・パフォーマンス・テストカバレッジの改善項目に対応する。

## 対応項目

### 1. URL解決の絶対URL判定が不完全 (優先度: 高)

**現状:**
- `reference.contains("://")`で判定しているため、`http:foo`のような形式を相対パスと誤認識

**修正内容:**
- `starts_with("http://")` または `starts_with("https://")`で判定に変更
- RFC 3986準拠の厳密な絶対URL判定

**実装箇所**: `src/http/url.rs` - `resolve_url()`内の絶対URL判定ロジック

### 2. 絶対パスが正規化されていない (優先度: 高)

**現状:**
- `/css/../style.css`のようなドット記号を含むパスが正規化されない

**修正内容:**
- 絶対パス処理時に、相対パス処理と同じ`normalize_path`ロジックを適用
- RFC 3986 §5.2.4のdot-segment削除に準拠

**実装箇所**: `src/http/url.rs` - `resolve_url()`内の絶対パス処理

### 3. rel属性の判定が大文字小文字を区別 (優先度: 中)

**現状:**
- `rel.contains("stylesheet")`で大文字小文字を区別
- `rel="StyleSheet"`で誤検知、`rel="notstylesheet"`で誤マッチ

**修正内容:**
- `rel.split_whitespace().any(|token| token.eq_ignore_ascii_case("stylesheet"))`に変更
- HTMLの仕様に従うホワイトスペース区切り + ASCII大文字小文字非区別

**実装箇所**: `src/paint/mod.rs` - `collect_author_stylesheets()`内のrel判定

### 4. SSRF脆弱性 (優先度: **最高** ⚠️)

**現状:**
- 任意の絶対URL（`http://169.254.169.254/...`、内部サービスなど）をフェッチ可能

**修正内容:**
- **相対URLのみをフェッチ対象に制限**
- 絶対URLおよびプロトコル相対URL（`//`）はスキップ
- 条件: `!href.contains("://") && !href.starts_with("//")`を追加

**実装箇所**: `src/paint/mod.rs` - `collect_author_stylesheets()`内のフェッチ条件

**セキュリティ根拠:**
- サーバーサイドレンダリング時の SSRF 攻撃防止
- 外部CSSは相対パスでリソース包含が標準的

### 5. パフォーマンス非効率（不要なクローン） (優先度: 低)

**現状:**
- `String::from_utf8(resp.body().to_vec())`で不必要にバイト列をクローン

**修正内容:**
- `std::str::from_utf8(resp.body())`で験証後に`to_owned()`で所有化
- または`from_utf8_lossy`で一度の割り当てに

**実装箇所**: `src/paint/mod.rs` - `collect_author_stylesheets()`内のUTF-8検証

### 6. テストカバレッジ不足 (優先度: 中)

**現状:**
- 外部CSSのフェッチ・デコード・適用パスがテストされていない
- `base_url=None`の場合のみテスト

**修正内容:**
- 統合テスト追加：`TcpListener`で仮HTTPサーバーを起動
- テストケース：
  - base_url指定時にCSSがフェッチされる
  - フェッチしたCSSが`stylesheets`に追加される
  - 複数の相対CSSリンクがドキュメント順に含まれる
  - フェッチ失敗時はスキップされる

**実装箇所**: `src/paint/mod.rs` - `mod tests` 内

## 実装プラン

1. **修正順序**
   - ① SSRF対応（最優先セキュリティ）
   - ② URL解決バグ修正（絶対URL判定、パス正規化）
   - ③ rel属性判定修正
   - ④ パフォーマンス改善
   - ⑤ テスト追加

2. **テスト戦略**
   - 修正ごとにユニットテスト実行
   - 既存の acid2 テストが通ることを確認
   - 統合テスト追加後に全テスト実行

3. **コミット**
   - 各修正項目ごとに別コミット（関連する修正は1コミット）
   - 例：
     1. "fix: URL解決の絶対URL判定とパス正規化"
     2. "fix: rel属性の大文字小文字非区別判定"
     3. "fix: SSRF脆弱性対応（外部CSSを相対URLのみに制限）"
     4. "perf: UTF-8検証時の不要なクローン削除"
     5. "test: 外部CSSフェッチの統合テスト追加"

## 関連PR
- PR#4: 外部CSSのHTTPフェッチと適用（013-2）

## ✅ 実装済み（第1次対応）

- 1. URL解決の絶対URL判定バグ修正 ✓
- 2. パス正規化の追加 ✓
- 3. rel属性の大文字小文字非区別判定 ✓
- 4. SSRF脆弱性対応（相対URLのみに制限） ✓
- 5. UTF-8検証時の不要なクローン削除 ✓
- 6. 外部CSSフェッチの統合テスト追加 ✓

**提出コミット:**
- a7aede9: fix: PR#4コメント指摘の外部CSSフェッチ関連バグ修正
- 4d68826: test: 外部CSSフェッチの統合テスト追加

## 🆕 新規対応項目（PR#4コメント追加分）

### 7. href属性の空白トリミング (優先度: 中)

**現状:**
- `href.is_empty()`チェックのみで、`href="   "`（空白のみ）が通過してしまう

**修正内容:**
- `let href = href.trim();` で一度トリミング
- その後の各チェックで trimmed value を使用

**実装箇所**: `src/paint/mod.rs` - `collect_author_stylesheets()` 内

### 8. コメント内のエスケープミス (優先度: 低)

**現状:**
- コメント: `protocol-relative URLs ("//"")`  → 余計な `"` がある

**修正内容:**
- コメント修正: `protocol-relative URLs ("//")` に統一

**実装箇所**: `src/paint/mod.rs` - `collect_author_stylesheets()` 内のコメント

### 9. SSRF関連コメントの不正確さ (優先度: 中)

**現状:**
- "Only fetch relative URLs" だが実際には `/css/style.css` のような絶対パスもfetch

**修正内容:**
- コメント改善: "Only fetch same-origin URLs that do not specify a scheme, to prevent SSRF attacks. Absolute URLs (containing "://") and protocol-relative URLs ("//") are skipped; this still allows relative and absolute-path references like "/css/style.css"."

**実装箇所**: `src/paint/mod.rs` - `collect_author_stylesheets()` 内のコメント

### 10. メモリ枯渇DoS対策（CSS サイズ制限） (優先度: **高** ⚠️)

**現状:**
- `HttpResponse::body()`で全体を メモリに読み込む制限がない
- 巨大なCSSで メモリ枯渇DoSの可能性

**修正内容:**
- 定数 `MAX_EXTERNAL_STYLESHEET_BYTES: usize = 1024 * 1024` （1 MiB）を定義
- `body.len() > MAX_EXTERNAL_STYLESHEET_BYTES` の場合はスキップ

**実装箇所**: `src/paint/mod.rs` - `collect_author_stylesheets()` 内

### 11. URLフラグメント（`#`）処理 (優先度: **高** ⚠️)

**現状:**
- `style.css#v2` などのフラグメント付きURLが正規化されない
- フラグメントが path に含まれて HTTP リクエスト送信時に誤った request target になる

**修正内容:**
- `resolve_url()` / `split_path_query()` でフラグメントを事前削除
- `split_path_query()` 後に さらに `?` の前の部分から `#` を削除するロジック追加

**実装箇所**: `src/http/url.rs` - `resolve_url()` / `split_path_query()` 関数

### 12. 非HTTP(S)スキーム処理（RFC 3986準拠） (優先度: **高** ⚠️)

**現状:**
- `mailto:foo` などの非HTTP(S)スキーム を相対パスとして base に merge してしまう
- RFC 3986では `:` の位置で絶対URI判定すべき

**修正内容:**
- RFC 3986 §3.1 に従い、`<scheme>:` の形式（`:` が `/`, `?`, `#` より前）を絶対URI判定
- HTTP(S)の場合は parser に任せ、その他は reject

**実装箇所**: `src/http/url.rs` - `resolve_url()` 内の scheme 判定ロジック

### 13. ドキュメント修正（issue 013-2 の scope 矛盾） (優先度: 中)

**現状:**
- issue 013-2 のスコープ："http/https の hrefをフェッチ"
- 実装 ："相対URL・絶対パスのみfetch、絶対URL・プロトコル相対URLはスキップ"

**修正内容:**
- issue 013-2 の SSRF対応セクションを追記し、実装との矛盾を解消
- または issue 013-5 に明記

**実装箇所**: `issues/open/013-2-external-css-fetch.md`

## 実装計画（継続対応）

**優先度順:**
1. URLフラグメント処理（RFC準拠、requesttargetの正確性）
2. 非HTTP(S)スキーム処理（RFC準拠、security）
3. メモリ制限（DoS対策）
4. href トリミング処理（エッジケース）
5. コメント修正
6. ドキュメント修正
7. テスト追加

**コミット例:**
1. fix: URLフラグメントと非HTTP(S)スキーム対応（RFC 3986準拠）
2. feat: CSSサイズ制限でDoS対策（1 MiBまで）
3. fix: href属性のトリミング処理追加
4. docs: コメントとドキュメント修正
5. test: 新規テストケース追加
