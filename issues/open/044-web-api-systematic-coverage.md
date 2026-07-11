---
number: 044
slug: web-api-systematic-coverage
status: open
---

# Web API 体系的カバレッジ

## 概要

実サイトの JS が完走できるよう、不足している Web API をカテゴリ別に体系的に整備する。
jQuery 等のエラーを個別に追うのではなく、よく使われる API を優先度付きで一括洗い出し・実装する。

## 背景

- 043 で基本 DOM 操作 API を追加したが、実サイト（jQuery/React 等）はさらに多くの API に依存
- エラーを1つ潰しても次が出る場当たり的な対応では進まない
- カテゴリ別に整理し、フェーズを分けて段階的に実装する

## 現状の実装済み API

### Rust ネイティブバインディング
- Document: `getElementById`, `createElement`, `createTextNode`, `createDocumentFragment`
- Node 走査: `parentNode`, `nextSibling`, `previousSibling`, `childNodeIds`
- DOM 操作: `appendChild`, `removeChild`, `insertBefore`, `cloneNode`
- セレクタ: `querySelector`, `querySelectorAll`
- 属性: `getAttribute`, `setAttribute`, `removeAttribute`
- コンテンツ: `textContent`, `innerHTML`
- メタ: `nodeType`, `nodeName`
- ネットワーク: `fetch`（基本）
- スタイル/レイアウト: `__omoikane_computed_style`（カスケード実値）, `__omoikane_layout_metrics`（offset*/client*/scroll*/getBoundingClientRect、forced reflow 付き）— 044-2/PR #105

### JS ポリフィル
- Event system（capture/bubble/target phases）
- `className`, `classList`, `style`（Proxy）, `tagName`
- `getElementsByTagName`, `getElementsByClassName`
- `IntersectionObserver`
- `getComputedStyle`（カスケード実値を返す実装。044-2/PR #105 で空 Proxy stub を置換）
- `window.addEventListener/removeEventListener`
- `Element`, `HTMLElement`（Node class のエイリアス）

## 不足 API カテゴリ

### Phase 1: レイアウトメトリクス（最優先）— ✅ 完了（2026-07-11、[044-2](../closed/044-2-layout-metrics-bindings.md) / PR #105）

実サイトの JS がレイアウト情報を取得するために必須。
下表すべて実装済み（forced reflow 付き）。残課題は 047（インライン style カスケード）と 048（メトリクスキャッシュ）で追跡。

| API | 用途 |
|-----|------|
| `getBoundingClientRect()` | 要素の位置・サイズ取得 |
| `offsetWidth/Height` | レイアウト幅・高さ |
| `offsetTop/Left` | 親からのオフセット |
| `clientWidth/Height` | padding 含むサイズ |
| `scrollWidth/Height/Top/Left` | スクロール情報 |
| `getComputedStyle()` 実値返却 | 計算済みスタイル取得 |

### Phase 2: イベント・タイマー

インタラクティブな JS コードの実行に必要。

| API | 用途 |
|-----|------|
| `MouseEvent`, `KeyboardEvent` 等 | イベントクラス |
| `preventDefault()` | デフォルト動作抑制 |
| `stopImmediatePropagation()` | 即時伝播停止 |
| `CustomEvent` | カスタムイベント |
| `requestAnimationFrame` | アニメーションフレーム |
| `alert/confirm/prompt`（stub） | ダイアログ |

### Phase 3: DOM 補完

ライブラリが依存する追加 DOM API。

| API | 用途 |
|-----|------|
| `attributes` (NamedNodeMap) | 属性コレクション |
| `dataset` | data-* 属性アクセス |
| `innerText` | 表示テキスト取得 |
| `isConnected` | DOM 接続状態 |
| `document.createEvent()` | レガシーイベント生成 |
| `document.createRange()` | 範囲オブジェクト |
| `document.readyState` | ドキュメント状態 |
| `document.createComment()` | コメントノード生成 |
| `window.innerWidth/innerHeight` | ビューポートサイズ |

### Phase 4: Observer・メディア

高度な機能を使うサイト向け。

| API | 用途 |
|-----|------|
| `MutationObserver` | DOM 変更監視 |
| `ResizeObserver` | リサイズ監視 |
| `matchMedia()` | メディアクエリ判定 |
| `localStorage/sessionStorage`（stub） | ストレージ |

### Phase 5: ネットワーク・データ

| API | 用途 |
|-----|------|
| `XMLHttpRequest` | レガシー通信 |
| `Headers/Request/Response` | Fetch API 完全版 |
| `URL/URLSearchParams` | URL 操作 |
| `TextEncoder/TextDecoder` | テキスト変換 |

### Phase 6: フォーム・入力

| API | 用途 |
|-----|------|
| `value/checked/disabled` | フォーム要素プロパティ |
| `submit()/reset()` | フォーム操作 |

## 実装方針

- 各 Phase を子 issue に分割して進める
- Rust ネイティブバインディングが必要なもの（レイアウト情報取得等）と JS 側だけで完結するもの（Event クラス拡張等）を区別
- stub でも「存在する」ことが重要な API（`matchMedia` 等）は先にスタブを入れる
- `console.warn/error` 等の軽量なものは随時追加
