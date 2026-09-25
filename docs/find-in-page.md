# ページ内検索

GUIでは `Ctrl+F`（macOSでは `Command+F`）で検索を始め、文字を入力します。ウィンドウタイトルに検索語と現在位置・件数を表示します。`Enter` で次、`Shift+Enter` で前、`Escape` で終了します。検索中のキー入力はページへ渡しません。

CDP接続では独自メソッド `Omoikane.findInPage` を使用します。`action` は `start`、`next`、`previous`、`status`、`stop` のいずれかで、`start` には `query` も必要です。結果は `query`、`matchCount`、1から始まる `activeMatchOrdinal` を返し、該当なしの場合の位置は0です。

```json
{"method":"Omoikane.findInPage","params":{"action":"start","query":"example"}}
```

C API の `omoikane_find_in_page(browser, action, query)` は同じ結果をJSON文字列で返します。`start` 以外の `query` は `NULL` にできます。戻り値は `omoikane_string_free()` で解放してください。エラー時は `NULL` で、正しいbrowserハンドルがあれば `omoikane_last_error()` から理由を取得できます。

検索対象は現在のdocumentの可視テキスト、open shadow root、アクセス可能なiframeです。`display:none`、`content-visibility:hidden`、非表示のテキストは除外します。画面外の `content-visibility:auto` は検索でき、選択時にスクロールして表示します。DOMやスタイルが変わった場合は次の検索操作で結果を更新します。

Firefoxとの比較用fixtureは [tests/fixtures/find-in-page/README.md](../tests/fixtures/find-in-page/README.md) を参照してください。
