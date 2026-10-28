# Clipboard のイベントとデータ表現

`ClipboardEvent` と `ClipboardItem` は、ページが構築するイベントとデータ表現を提供する。
編集ホストのネイティブな copy/cut/paste と contenteditable の既定編集処理は
[#1288](https://github.com/ieee0824/omoikane/issues/1288) で継続する。
Async Clipboard の `read()` / `write()` / `readText()` / `writeText()` は、
プロセス内の共有ストレージを使う。

## Async Clipboard の公開とテキスト操作

`Clipboard` は EventTarget を継承する。secure context の Window に公開し、
`Navigator.prototype.clipboard` は readonly な SameObject 属性として同じ Clipboard を返す。
Worker では constructor と navigator の属性を公開しない。
同一originの別realmからメソッドを借用しても、receiver が属する Window のネイティブ操作を使う。

`writeText()` の DOMString 変換は呼び出し時に一度だけ行う。
引数不足・Symbol・不正receiverのエラーは TypeError の rejected Promise とし、
実際の読み書き時にはネイティブ側でも許可を確認する。

`write()` は1個の ClipboardItem が持つ全representationを所有バイト列へ変換してから、
共有ストレージを一度だけ置き換える。`read()` はその時点の全representationをsnapshotし、
呼び出したrealmの ClipboardItem と Blob に復元する。`writeText()` は他形式を置き換え、
`readText()` は `text/plain` がなければ空文字列を返す。

## ClipboardEvent

Window で `new ClipboardEvent(type, init)` を構築できる。`clipboardData` は nullable な
DataTransfer で、指定されたオブジェクトを保持する読み取り専用属性である。
未指定時は null となり、偽の prototype だけを持つオブジェクトは受け付けない。
同一originの別realmで構築した DataTransfer と、別realmの ClipboardEvent getter を扱う。

EventInit と ClipboardEventInit のメンバーは WebIDL の順序で一度ずつ読み取る。
合成イベントは untrusted であり、dispatch しても入力値や共有クリップボードを変更しない。
`bubbles`、`cancelable`、`composed` は明示した値を反映し、既定値は false となる。

## ClipboardItem

ClipboardItem は secure context の Window に公開し、Worker には公開しない。
constructor は MIME type をキー、Blob / DOMString またはその Promise を値とするレコードを受け取る。

| メンバー | 動作 |
| --- | --- |
| `types` | MIME type を正規化した、構築時の frozen array を返す。同じ配列を保持する。 |
| `presentationStyle` | `unspecified` / `inline` / `attachment`。既定値は `unspecified`。 |
| `getType(type)` | 対応する Promise を待ち、Blob は同じオブジェクトを返す。文字列は UTF-8 の Blob に変換する。 |
| `supports(type)` | mandatory type、`text/uri-list`、`image/svg+xml`、パラメーターのない有効な `web ` custom type を判定する。 |

constructor は任意の有効な MIME type を保持できる。`supports()` が false の型でも
構築と `getType()` は可能であり、書き込み可能な形式の判定とは区別する。
`web ` prefix は通常形式と区別して保持し、MIME type の正規化後に同じ表現が重複すれば TypeError とする。
MIME parameter の名前を小文字化し、値と必要な引用符を保持する。

`getType()` は不正な引数やreceiverを TypeError の rejected Promise にし、
存在しない表現や拒否されたデータを NotFoundError の rejected Promise にする。
レコード、options、Promise の解決値はそれぞれの受け入れ境界で変換し、内側では変換済みの値を扱う。
Promise が返す内部レコードは null prototype とし、author の `Object.prototype.then` に渡さない。

## 検証と参考実装

- `tests/clipboard_events.rs`: readonly payload、WebIDL の読み取り順、別realm、Worker、合成イベントの既定動作抑止。
- `tests/clipboard_items.rs`: MIME 正規化・重複、metadata snapshot、Promise、公開範囲、別realm、内部 thenable の露出抑止。
- 固定 WPT: `tests/wpt/clipboard-events.json` の2ファイル。revision は `tests/wpt/revision.txt` に従う。原本と期待値を変更しない。

Firefox 158.0.1 の loopback secure context で、正規化、重複拒否、PresentationStyle、
getType の Promise エラー、文字列変換のタイミング、未読データの Promise rejection を比較した。
Firefox は web custom format を constructor で拒否し、optional な SVG / URI 形式も supports では false とする。
拒否されたデータに対しては DataError を返すが、この実装は現行仕様の NotFoundError に従う。

文字列から作る web custom format の Blob.type は、固定 WPT と Chromium の現在の adapter に合わせて
`web ` prefix を保持する。現行 draft の MIME serialization の記述とは差があるため、
Firefox との一致や仕様完全適合を主張しない。

参照: [Clipboard API and events](https://w3c.github.io/clipboard-apis/)、
[MIME Sniffing](https://mimesniff.spec.whatwg.org/)、
[Chromium ClipboardItem](https://source.chromium.org/chromium/chromium/src/+/main:third_party/blink/renderer/modules/clipboard/clipboard_item.cc)、
[Ladybird ClipboardItem](https://github.com/LadybirdBrowser/ladybird/blob/master/Libraries/LibWeb/Clipboard/ClipboardItem.cpp)。
MIME parser は `whatwg-mimetype` の revision `9b2fc150869a9f46154379ed24e3a1226d4d8482` と
198ケースを単体比較し、不一致0件だった。これは Boa 上のAPI検証を代替しない。
