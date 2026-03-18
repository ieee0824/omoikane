---
id: 001-1
title: TCP接続 & HTTP/1.1基本実装
phase: 1
status: open
parent: 001
---

# TCP接続 & HTTP/1.1基本実装

## タスク
- [ ] TCP接続の確立（std::net::TcpStream）
- [ ] HTTPリクエスト構造体の設計（メソッド、ヘッダ、ボディ）
- [ ] HTTPリクエストのシリアライズ（バイト列への変換）
- [ ] HTTPレスポンスのパース（ステータス行、ヘッダ、ボディ）
- [ ] Content-Length / Transfer-Encoding: chunked の処理
- [ ] 基本的なテスト（GETリクエストの送受信）
