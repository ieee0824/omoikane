---
number: 064
slug: object-data-url-reflection
parent: 016
status: open
---

# HTMLObjectElement.data の URL 反射（Acid3 test 64）

## 概要

`object.data` IDL 属性を URL 反射（owner document の base URL に対する相対→絶対解決）にする。

## 失敗内容

```
Test 64 failed: expected './test.html' but got 'test.html' - object elements didn't resolve URIs correctly
```

```js
obj1.setAttribute('data', 'test.html');
obj2.setAttribute('data', './test.html');
assertEquals(obj1.data, obj2.data);          // 同一の絶対 URL に解決されるべき
assert(obj1.data.match(/^http:/));            // 絶対 http: URL であるべき
```

## 原因分析（調査済み）

- `data` getter（`src/js/dom_bootstrap.js:2517-2519`）が `__omoikane_get_attribute` の生値をそのまま返す
- Rust 側には `resolve_url`（`src/http/url.rs:219-286`）と `HostState.base_url` があるが、**JS へ URL 解決を露出する native binding が存在しない**
- テスト後半の「非存在属性の非漏洩」（`'foo' in el` が false）は、ラッパが Proxy でなく素のクラスインスタンス（`wrapNode`, `src/js/dom_bootstrap.js:24-54`）のため**既に成立しており対応不要**

## 実装方針

1. **native binding `__omoikane_resolve_url(reference)` を追加**（`src/js/mod.rs`）
   - `with_host_state` で `base_url` を読み、`Some(base)` なら `crate::http::url::resolve_url(&base, &reference)` → Ok なら `to_string()`、Err なら reference 原文を返す（spec の「解決失敗時は属性値を返す」に一致）
   - `base_url` が `None` の場合も原文を返す
   - native 関数登録配列（`src/js/mod.rs:1318-1562`）の末尾に追加
2. **`data` getter を差し替え**（`src/js/dom_bootstrap.js:2517-2519`）
   - `raw === null` なら `""`、それ以外は `__omoikane_resolve_url(raw)` を返す。setter は変更不要
3. Rust 側のロード経路（`resolve_resource_ref` / `iframe_content_document`）は触らない（ロードは Rust 側で解決済み。二重解決を避ける）
4. `iframe.src` / `img.src` / `a.href` の URL 反射は今回のスコープ外（回帰リスク回避のため据え置き）

## テスト計画

`src/js/mod.rs` の `#[cfg(test)] mod tests` に、`iframe_relative_src_resolves_against_base_url`（src/js/mod.rs:8874）の書式で追加:

- `object_data_reflects_as_absolute_url_resolved_against_base_url`: `set_base_url("http://example.com/dir/page.html")` 後、`data='test.html'` と `data='./test.html'` の両方が `"http://example.com/dir/test.html"` になること
- `object_data_absent_reflects_as_empty_string`: data 属性なしで `.data === ""`
- `element_setattribute_does_not_leak_to_js_property`: `p.setAttribute('foo','x')` 後に `!('foo' in p) && p.foo === undefined && p.getAttribute('foo') === 'x'`（回帰防止）

## 回帰リスク

- `embedded_svg_documents_are_exposed_by_iframe_and_object`（src/js/mod.rs:8222）は `object.data` を読み戻さないため無影響
- `resolve_url` は非 http(s) スキームを reject する → フォールバックで原文を返すこと（mailto:/data: 等）
- `location.href` / `document.URL` は既定 `http://localhost/` のままで今回は変更しない（既存の別問題）

## 受け入れ条件

- Acid3 test 64 が FAITHFUL/DIRECT 両モードで PASS（97→98）
- 上記単体テストの追加と既存テスト全通過
