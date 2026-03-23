---
number: 027
slug: unsupported-html-tag-logging
parent:
status: open
---

# 未対応HTMLタグの観測ログ（SQLite）

## 概要

レンダリング時に未対応のHTMLタグを検出し、SQLiteに記録する。
CSS未対応プロパティログ（`OMOIKANE_UNSUPPORTED_CSS_SQLITE`）と同様の仕組み。

## 背景

- 実サイトレンダリングで未対応HTMLタグがどの程度使われているか把握できない
- 優先実装の判断材料として、出現頻度と使用コンテキストを収集したい

## 対応内容

### 環境変数
- `OMOIKANE_UNSUPPORTED_HTML_SQLITE=<path>` — SQLiteファイルパスを指定
- `OMOIKANE_LOG_UNSUPPORTED_HTML=1` — stderrにもログ出力

### SQLiteスキーマ
```sql
CREATE TABLE IF NOT EXISTS unsupported_html_log (
    tag TEXT NOT NULL,
    parent_tag TEXT,
    first_seen_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    last_seen_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    occurrences INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (tag, parent_tag)
);
CREATE INDEX IF NOT EXISTS idx_unsupported_html_log_occurrences
ON unsupported_html_log (occurrences DESC);
```

### 対応済みHTMLタグリスト
レイアウト/描画に影響するタグのうち、実装済みのもの:
- 構造: `html`, `head`, `body`, `div`, `span`, `section`, `article`, `aside`, `main`, `nav`, `header`, `footer`
- テキスト: `p`, `h1`-`h6`, `br`, `strong`, `em`, `b`, `i`, `u`, `a`, `pre`, `code`
- リスト: `ul`, `ol`, `li`
- テーブル: `table`, `thead`, `tbody`, `tfoot`, `tr`, `td`, `th`
- メディア: `img`, `object`
- フォーム: （未対応）
- その他: `style`, `link`, `meta`, `title`, `script`, `font`, `blockquote`, `hr`

上記以外のタグが出現した場合にログに記録する。

### 実装箇所
- `src/layout/mod.rs` または `src/layout/inline.rs` のノード走査時に検出
- CSS未対応ログと同じ SQLite 接続キャッシュを再利用

## 受け入れ条件

- 未対応タグが SQLite に記録される
- 対応済みタグは記録されない
- 出現回数が UPSERT でカウントされる
- stderr ログオプションが動作する
- 既存テスト全通過
