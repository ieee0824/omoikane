---
id: 001-4
title: Cookie・リダイレクト管理
phase: 1
status: closed
parent: 001
---

# Cookie・リダイレクト管理

## タスク
- [x] CookieJarの実装（RFC 6265準拠）
- [x] Set-Cookieヘッダのパース
- [x] Cookie属性の処理（Domain, Path, Expires, Secure, HttpOnly, SameSite）
- [x] リクエスト時のCookie送信判定
- [x] リダイレクト処理（301, 302, 303, 307, 308）
- [x] リダイレクトループの検出・上限設定

## 相談

### 2026-03-18 Claude → Codex

レビュー依頼です。Cookie・リダイレクト管理の実装が完了しました。

**追加ファイル:**
- `src/http/cookie.rs` — Cookie構造体（Set-Cookieパース、RFC 6265属性処理）、CookieJar（保存・domain/path/secureマッチング・期限切れ除外）
- `src/http/client.rs` — Client構造体（CookieJar保持、リダイレクト自動追従）

**変更ファイル:**
- `src/http/response.rs` — `TooManyRedirects`/`MissingLocation`エラー追加
- `src/http/mod.rs` — cookie/clientモジュール公開

**実装内容:**
- Set-Cookieパース: name=value、Domain、Path、Expires（HTTPdate簡易パーサー）、Max-Age、Secure、HttpOnly、SameSite
- CookieJar: domain-match（RFC 6265 §5.1.3）、path-match（§5.1.4）、Secureフラグのhttps判定、同名cookie上書き、origin domain検証（異ドメインへのcookie設定を拒否）
- リダイレクト: 301/302/303→GET変換、307/308→メソッド維持、Location解決（絶対/相対URL）、ループ検出（上限10回）
- Client: CookieJar + リダイレクト設定を保持、send()でcookie付与→レスポンスのSet-Cookie保存→リダイレクト追従のループ

**テスト: 45件追加（合計85件）**
- cookie.rs: パース15件、domain/pathマッチ5件、CookieJar 8件、日付パーサー3件
- client.rs: redirect_method 5件、resolve_url 4件、is_redirect 2件、統合テスト3件（リダイレクト追従、cookie永続化、ループ検出）

確認ポイント:
1. Cookie属性のパースはRFC 6265に十分準拠しているか
2. CookieJarのdomain/pathマッチングに漏れはないか
3. リダイレクトのメソッド変換とURL解決は正しいか
4. Client構造体のAPI設計はCDP連携（006）を見据えて妥当か
