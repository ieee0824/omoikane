# DOM / Geometry のインターフェースと Selection の境界

## インターフェースの接続

Issue [#1285](https://github.com/ieee0824/omoikane/issues/1285) の実装は、既存の DOM・layout・canvas の値を対応するプラットフォームオブジェクトで返す。

| API / 値 | インターフェース | 状態の性質 |
| --- | --- | --- |
| `children`、`getElementsBy*`、document の `forms/images/links/anchors` | `HTMLCollection` | 呼び出し元の木に追随する live collection。`children` と document の四属性は同じオブジェクトを返す |
| `getBoundingClientRect()` | `DOMRect`（`DOMRectReadOnly` を継承） | 独立した可変の結果。負の幅・高さも保持する |
| `getClientRects()` | `DOMRectList`、要素は `DOMRect` | snapshot。リストの添字と長さは読み取り専用 |
| `DOMQuad` の点と境界 | `DOMPoint`、`DOMRect` | 四つの点の同一性を保持し、点の変更後に境界を計算する |
| `Range` / `StaticRange` | `AbstractRange` を継承 | `Range` は live、`StaticRange` は端点・offset の snapshot |
| `document.implementation` / `element.dataset` | `DOMImplementation` / `DOMStringMap` | 所有する document / element ごとに同じオブジェクトを返す |
| canvas `measureText()` | `TextMetrics` | 既存の測定結果を読み取り専用属性として返す |

Geometry の辞書は規定の順にメンバーを読み、各メンバーを次の読み取りより前に変換する。Node・ShadowRoot の引数は内部の登録情報で検証し、公開 `nodeType` や prototype の偽装を判定に使わない。DOMMatrixInit の別名係数の整合性と 3D 係数を検証し、DOMPoint の変換では 16 係数を使う。型付けは、別 Issue で扱う font の測定精度や Canvas の未対応機能の実装を意味しない。

## Selection の公開境界

[Selection API](https://www.w3.org/TR/selection-api/) と [editor’s draft](https://w3c.github.io/selection-api/) の公開境界に従う。内部の composed 端点は、通常の Range の同一 root 制約による collapse より前に保持し、Range setter、テキスト変更、shadow を含む木からの削除へ接続する。Selection の状態と composed Range は WeakMap に保持し、ページから参照できる追加フィールドに保存しない。

- `anchorNode/focusNode` と各 offset は、それぞれ document tree 外なら `null/0` を返す。
- どちらかの端点が document tree 外なら `rangeCount` は 0、`type` は `None`、`getRangeAt(0)` は `IndexSizeError`。`isCollapsed` は内部端点の一致を判定する。
- `getComposedRanges({shadowRoots})` は許可された root（指定 root の shadow-including ancestor を含む）の端点を保持し、それ以外を host の親の前後に rescope して `StaticRange` を返す。
- 公開の `parentNode/host` や WeakMap メソッドを上書きしても、この判定と rescope を変更しない。
- options の root 列は iterable として変換し、無効な root で iterator を閉じる。閉じた root の端点を取得するには、その root の参照を明示的に渡す必要がある。

## 固定 WPT と参考ブラウザ

WPT revision は `dc97e7bed3096ac9e0e591ab5fa22e7fb8844ead`。DOM・Geometry 20 ファイルと Selection 6 ファイルを対象にする。固定 revision に `dom/interfaces.html` はなく、`dom/interface-objects.html` を実行する。

Selection の tentative テストには、shadow tree 内の通常 Range や公開 anchor を返す旧挙動への期待がある。同じ revision の range-update の最初のサブテストは逆に `getRangeAt()` の例外を期待しており、すべてをそのまま満たす公開境界にはならない。Firefox 157.0.1 は collapsed の旧期待には成功するが、range-update の例外期待には失敗する。ブラウザとの一致のみを仕様適合の証明には使わない。

仕様と異なる期待を持つ tentative サブテストは、実行結果と具体的な名前を記録して区別する。型の実装や composed 端点・mutation の検証は、これらの旧公開境界への期待で代替せず、`tests/dom_interfaces.rs` と `web_api_surface` で独立して確認する。全体テストと最終 WPT 結果は PR の検証記録を参照する。

### 記録した tentative の期待差分

| 固定 WPT ファイル | 失敗するサブテスト | 仕様との差分 |
| --- | ---: | --- |
| `Selection-getComposedRanges-collapsed.html` | 1 | shadow root の通常 Range の公開を期待する |
| `Selection-getComposedRanges-range-update.html` | 6 | 公開 shadow anchor、または通常 Range の collapse と composed 選択の collapse の一致を期待する |
| `Selection-getComposedRanges-slot.html` | 2 | shadow をまたぐ通常 Range の取得を composed 検証の前提にする |

`tests/wpt/manifest.json` は FAIL のサブテスト名をこの 9 件に限定する。他の FAIL、ERROR、TIMEOUT、または件数の変化は既知の差分として扱わない。2026-11-07 を再確認の目安として記録する。これは 3 ファイルの成功を意味しない。後続の composed 端点検証が前段の旧公開 API 期待で実行されなくなる箇所については、root 境界・slot 順序・setter mutation の独立した回帰テストで確認する。

WPT runner 自身の失敗経路の検証は synthetic fixture の manifest を使う。外部から指定する `WPT_MANIFEST` を子プロセスへ引き継がず、限定 WPT 実行時にも synthetic な回帰・予期しない成功・report 書き込み失敗を検出できるようにする。

通常の ASCII タグ名の live collection はネイティブの木探索を再実行して返す。CSS selector として表せない qualified name はネイティブの子 ID を使った探索へ戻る。XML の qualified name は prefix と localName で比較し、HTML の ASCII case folding と混同しない。`document.body/head` は最初の一致をネイティブ探索し、XML case / qualified name の再判定が必要な場合は保持した live collection を再評価する。各取得で collection の状態・proxy を再生成せず、body/head の削除・追加にも追随する。
