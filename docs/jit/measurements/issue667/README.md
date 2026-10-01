# Issue #667の共有証跡

全サンプルを保持した6比較（原比較2回・不採用候補3件・同一バイナリ対照）と、
GC・JIT・命令比較のテキスト証跡。gzip内は相対パスから元ファイルのUTF-8内容への
JSON mapで、元テキストを変更せず可逆圧縮した。manifestに各bundleのSHA256を記録する。
inputs内の絶対パスは測定時の記録で、以下の検査ではそのPCのファイルを参照しない。

リポジトリルートで実行する（標準Pythonのみ、読み取り専用）：

```sh
python3 scripts/verify_issue667_evidence.py
```

全720プロセス記録から11項目・全体・ページの全paired ratioと中央値を再計算し、
結果・JIT counter（compile時間除く）・fixture・4pass・PNG hash記録の一致を検査する。
GC全320passのcollection回数も前後で比較する。元バイナリの再実行ではない。

`profile-totals.json`は大きなCallgrind集計からeventsと反復当たり増分を抽出したもの。
元集計のhashを残す。診断bundleはGC全raw log/index・harness・identity、prefix/closure fixture、
handlerと主要calleeのassembly・正規化命令・差分を含む。失敗した初回解析は含まない。
`constant-function-relocation.json`は、命令差として残したLDRの参照先をELF relocationと
exact symbolで照合した追補。diagnostics内の旧「未検証」分類を上書きせず補足している。
`allocation-diagnostics.json.gz`は追加9関数のassembly/差分/集計とperf権限試行の記録。
これは主検査スクリプトの7bundleとは別の追補で、integrityは次で確認する：

```sh
python3 -c 'import gzip,hashlib,json,pathlib; p=pathlib.Path("docs/jit/measurements/issue667"); m=json.loads((p/"allocation-manifest.json").read_text()); b=(p/"allocation-diagnostics.json.gz").read_bytes(); assert len(b)==m["bytes"] and hashlib.sha256(b).hexdigest()==m["sha256"]; assert len(json.loads(gzip.decompress(b)))==m["files"]; print("allocation bundle integrity OK")'
```

実行バイナリ、PNG画素、Callgrindの全eventファイル、build出力は**ここには含まれない**。
これらは元PCにのみ保持され、hashだけで別PCから内容を復元することはできない。
したがって元バイナリの再測定・PNG画素の再検証には、別途それらの移送が必要。
計算結果や測定値の検証と、元環境の完全再現を区別する。

結論と制約は[調査文書](../../array-closure-investigation.md)、進捗は
[Issue #667](https://github.com/ieee0824/omoikane/issues/667)を参照。
