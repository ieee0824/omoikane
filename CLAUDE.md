# Omoikane - Headless Browser (Rust)

フルスクラッチで実装するヘッドレスブラウザ。Rustで基盤を構築し、CDP互換APIで他言語から利用可能にする。

## プロジェクト構成

```
issues/          タスク管理
  open/          未完了のissue
  closed/        完了したissue
```

## タスク管理ルール

- タスクは `issues/open/` にmarkdownファイルとして作成する
- 完了時は `issues/closed/` へ移動する
- ファイル名は `{番号}-{slug}.md` 形式（例: `001-http-client.md`）

## 開発フェーズ

1. HTTPクライアント + HTMLパーサー + DOM構築
2. CSSパーサー + スタイル計算
3. レイアウトエンジン
4. JavaScriptエンジン統合
5. CDP互換API + 他言語連携（C FFI, WebSocket JSON-RPC）

## 技術方針

- 言語: Rust
- 他言語連携: CDP(Chrome DevTools Protocol)互換 + C FFI薄レイヤー
- テスト: Web Platform Testsを参考にする
- 依存: 可能な限り最小限に抑え、コア部分は自前実装
