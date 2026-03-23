---
number: 041-4
slug: intersection-observer
parent: 041-javascript-engine-integration
status: open
---

# IntersectionObserver

## 概要

IntersectionObserver API を実装し、スクロールアニメーション（fade-in 等）のトリガーを可能にする。

## 背景

- モダンサイトで `IntersectionObserver` がスクロール時の要素表示トリガーとして使用
- `--force-opacity` なしでコンテンツを表示するには IO が必要

## 最小実装

- `new IntersectionObserver(callback, options)` コンストラクタ
- `observer.observe(element)` — 要素を監視対象に追加
- ヘッドレスブラウザでは全要素が viewport 内にあると仮定
- `observe()` 時点で即座に callback を呼び出す（isIntersecting: true）

## 修正箇所

- `src/js/mod.rs` — IntersectionObserver クラスの登録
- DOM_BOOTSTRAP に IntersectionObserver のポリフィル追加
