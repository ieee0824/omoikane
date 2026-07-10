---
number: 016-7
slug: document-write
parent: 016
status: open
---

# document.write 実装

## 目的

`document.write` を実装する。Acid3 の body 内スクリプトが
`document.write('<map>…<iframe id="selectors">…')` で selectors iframe 等を生成する。

## 背景（GAP_ANALYSIS.md セクション1 P8、セクション3 領域E）

- body 末尾で `document.write(...)` により map / area / iframe(#selectors) / form / table を生成する。
- これらは `getTestDocument()` 依存の約 35 テストの前提の一つ。
- 現状 `document.write` は未実装。

## スコープ

- `document.write` / （必要に応じて `document.open` / `document.close`）
- 書き込まれた HTML 断片のトークナイズ・ツリー挿入（同期）
- 生成される map / area / iframe / form / table 等の要素化

## 受け入れ条件

- `document.write('<iframe id="selectors">…')` で selectors iframe 要素が生成される
- 生成要素が `getElementById` 等で参照できる
- 016-9（iframe/contentDocument）と併せて getTestDocument が機能する前提を満たす

## 進捗（2026-07-10）

### 実装内容

`document.write` / `document.writeln` / `document.open` / `document.close` を実装した。

- **アーキテクチャ上の制約**: 本エンジンはパース完了後に `execute_document_scripts`
  で `<script>` を一括実行する（ストリーミングパーサではない）。そのため HTML 仕様の
  「トークナイザ挿入ポイント」を、実行中スクリプト要素を基準に近似した。
- **挿入ポイントの追従** (`src/js/mod.rs`):
  - `HostState` に `write_insertion_ref: Option<NodeHandle>` を追加。
  - `execute_document_scripts` が各インラインスクリプト実行の直前に、その `<script>`
    要素を挿入基準として設定し、実行後に `None` へ戻す。
  - `document.write` は書き込み断片を「実行中スクリプトの直後の兄弟」として挿入するため、
    スクリプトより後ろにあった要素（Acid3 の `#instructions` / `#remove-last-child-test`）は
    書き込み内容の後ろに残り、ソース順が仕様どおり再現される。
  - 連続する複数回の write は基準を最後に挿入したノードへ前進させ、呼び出し順を保つ。
  - スクリプト実行外（タイマー等）からの write は破壊的な `document.open()` リセットを
    避け `<body>` へ追記するフォールバックにした（対象ページに影響なし）。
- **断片のツリー構築**: `TreeBuilder::parse("<body>{text}</body>")` で断片をパースし、
  body の子ノードを実ツリーへ移設（innerHTML と同じ方式）。挿入したサブツリーは
  `register_tree` で登録し、後続の `getElementById(...).setAttribute(...)` 等が動作する。
- **書き込まれた `<script>` の同期実行**: native `__omoikane_document_write` が挿入した
  `<script>` 要素の id 配列を返し、JS 側 `write()` が `(0, eval)(code)` でグローバル
  スコープで同期実行する（Acid3 の主 document.write には script は含まれないが、
  古典的挙動として対応・テスト済み）。
- **native/バインディング**: `__omoikane_document_write` を追加、`dom_bootstrap.js` の
  `Document` クラスに `write` / `writeln` / `open` / `close` を追加。

### テスト（`src/js/mod.rs` の `tests` に7件追加、いずれも具体アサーション）

- `document_write_creates_iframe_findable_by_id`: 書いた iframe#selectors が
  `getElementById` で引け、`tagName === "IFRAME"`。
- `document_write_inserts_at_script_position`: 書き込み内容がスクリプトと後続要素の
  間に入る（body 子が `[script, div, p]`）。
- `document_write_multiple_calls_stay_in_order`: 複数 write が呼び出し順に連結
  （`[script, i, b, span]`）。
- `document_write_executes_written_script`: 書き込んだ `<script>` が実行される。
- `document_writeln_appends_newline`: writeln が末尾に改行テキストノードを追加。
- `document_write_without_active_script_appends_to_body`: スクリプト外 write は
  既存内容を消さず body へ追記。
- `document_write_acid3_fragment_builds_selectors_scaffold`: Acid3 実物の断片を書き、
  map/area/iframe#selectors/form/table が生成され、iframe が後から setAttribute 可能。

### スコア変化

- 着手前: **43/100**
- 完了時: **52/100**（+9）
- `document.write` で map/iframe/form/table/area 等が生成されるようになり、
  contentDocument を必要としない form(52)/area(63) 系などが通過。
- 残りの `getTestDocument` 依存テスト（約30件）は `iframe.contentDocument` が
  null を返す（**016-9 の担当範囲**）ため引き続き失敗。document.write 自体は
  機能しており、getElementById('selectors') は iframe を返せている
  （エラーが「iframe が無い」から「contentDocument が null」へ変化）。

### レビュー指摘の修正（2026-07-10, PR #103）

- **insert_before 失敗の黙殺を解消**: `insert_or_append` ヘルパを追加し、
  `insert_before` 失敗時は `append_child` へフォールバック。挿入されたノードのみ
  登録・挿入ポイント前進を行い、ツリー外ノードを登録しない。
- **defer スクリプトの write アンカー**: 遅延スクリプトをコードと script ノードの
  タプルで保存し、実行時に `write_insertion_ref` を設定/クリア（インラインと同一挙動）。
- **document.write が返す script id をインライン classic のみに限定**:
  `is_inline_classic_script` で `src` 付き / `type="module"` / 非 JS 型を除外。
  それらは DOM には挿入されるが同期実行されない。
- **document.open() のリセット実装**: native `__omoikane_document_reset` が
  文書児を全除去。JS `open()` から配線。任意の Document ノード id で動作するため
  016-9 のサブ文書にも流用可能。open() 後の write は空文書への追記として動く。
- doc コメントを実挙動（インライン classic のみ同期実行）に合わせて修正。

### 残課題（016-7 の範囲外）

- **016-9**: `iframe.contentDocument` / サブブラウジングコンテキスト。これが入ると
  `getTestDocument` が完成し、bucket3/5 の約30テストが一気に解放される見込み。
- 断片が単一 write 呼び出し内で完結する前提（Acid3 は1回で全断片を書くため問題なし）。
  タグをまたいで分割 write する厳密な入力ストリーム連結は未対応。
- **フォローアップ（オーケストレーター側で issue 化予定、コードにコメント記載済み）**:
  - 1つの write 断片に `<script>` と後続ノードが混在した場合、仕様のストリーミング
    挿入ポイントと実行・挿入順が乖離する（全ノードを挿入後にスクリプトを実行するため）。
  - write が write する `<script>` の再帰深度ガードが無い（スタックオーバーフロー可能）。
