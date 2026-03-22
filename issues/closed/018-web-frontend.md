---
number: 018
slug: web-frontend
parent:
status: open
---

# Web フロントエンド対応

## 概要

Omoikane のレンダリング結果を Web ブラウザから操作・閲覧できるフロントエンドを提供する。
CDP 互換 API を活用し、URL 指定→スクリーンショット取得→表示の基本フローを実現する。

## 想定スコープ

### Phase 1: 最小 Web API サーバー
- HTTP サーバー（Rust 内蔵 or 軽量クレート）
- `POST /screenshot` — URL を受け取り、レンダリング結果の PNG を返す
- `POST /navigate` — URL を受け取り、DOM/スタイル情報を返す
- `GET /health` — ヘルスチェック

### Phase 2: シンプル Web UI
- URL 入力フォーム
- スクリーンショット表示
- viewport サイズ指定（幅・高さ）
- レンダリング結果のダウンロード

### Phase 3: インタラクティブ機能
- DOM ツリー表示
- 未対応 CSS プロパティのログ表示
- Firefox 等のリファレンスとの並列比較
- レンダリング差分のハイライト

## 技術方針

- バックエンド: 既存の Rust コードベースに HTTP エンドポイントを追加
- フロントエンド: 静的 HTML/CSS/JS（フレームワーク最小限）
- CDP 互換 API との連携を検討
- CORS 対応

## 子 issue（必要に応じて分割）

- 018-1: 最小 HTTP API サーバー
- 018-2: スクリーンショット API エンドポイント
- 018-3: Web UI（HTML/JS）
- 018-4: viewport 制御・オプション

## 受け入れ条件

- ローカルでサーバーを起動し、ブラウザから URL を入力してスクリーンショットが表示される
- `cargo run --example web` 等で起動できる
