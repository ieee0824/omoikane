# length最適化後のarray・closure残差調査（#667）

2026-09-30〜10-01のLinux ARM64診断。実装修正は採用していない。
これは歴史バイナリの調査で、現行main・release・他CPUの速度の保証ではない。
[Issue #667](https://github.com/ieee0824/omoikane/issues/667)を進捗の正本とする。
[共有証跡と検査手順](measurements/issue667/README.md)から、別PCでも測定値と集計を検査できる。
元バイナリとPNG画素はまだ共有しておらず、完全な環境再現はできない。
追加の元バイナリ比較でもclosure/onの遅い側の差が再現したため、Issueはopenを維持する。

## 同条件の再確認

[元の比較条件](string-length-performance.md)を維持した。
基準57fb64f15518cf51dfae9afd2f1719d297c2bf3c、helper版
c2bc5b380971ff319af2127d4c8caff7ca07801dの保存バイナリをhash照合し、
Rust1.98.0、root/Engine lock、test profile、mode別featureを照合した。
fixture v2 SHA256は177eb80ae73d1081eb620f747b4703e2a9f2731e0255bb60131b73b7dcac6464。
全11原body・回数・4pass最小値を維持し、array500000、closure300000回。
各mode10 benchmark pairs/20 page pairsを実行前に固定、新process/runtime、順序交互。
ビルド・profilerを重ねず、全サンプルを残した。良い組への差し替えはしない。

| 対象 | 9月11日の観測 | 最初の再確認 | 遅い組 |
| --- | ---: | ---: | ---: |
| array/off | +2.39% | +0.96% | 7/10 |
| closure/on | +5.87% | +1.97% | 8/10 |

旧観測の大きさ・array全組同符号は再現しなかったが、遅い側の残差はあった。
全体はoff−0.64%/on−0.55%、ページ−2.18%/−0.34%。文字列参照−4.66%/−5.49%、
連結−3.76%/−6.18%。arith/off+0.45%、call/on+0.31%など、他の増加も保持する。
全pairの計算結果・JIT counter（compile時間除外）・page JSON・2枚PNG hashが一致。
[全結果の記録](https://github.com/ieee0824/omoikane/issues/667#issuecomment-5920849644)。

## コード・profileによる切り分け

元変更はprimitive helper化・length直接返却・receiver clone遅延。
arrayはobject property経路を使うが、closure bodyにprimitive string length参照はない。
JsValue::as_objectは所有JsObjectを返す。GcEdge cloneはroot登録ではなく、VM root providerが
register stackをtraceする。clone省略をroot登録省略と混同しない。

原bodyを5000/20000回・1passに分離したCallgrind診断の命令数増分/反復は、
array/off−0.35%、closure/on−0.94%。単純な命令数増加は時間差を説明しなかった。
cache/branchはsimulationで、実CPUイベントではない。symbol aliasとinclusive call treeの
不正確さがあるため、名前だけでGCへ帰属しない。実CPUの配置/cache原因は未確定。

closure単独を原回数・4passにしてもcompile6件すべてreject、native entry0。
先行4workloadを残したprefix診断ではclosure追加でrequests/rejects各+1、
native entry2936・generated code2092 bytes・successful compilation2は不変。
全11合計counterをclosureのnative実行実績とは扱わない。
reject counterは検証失敗・非対応lowering・compile error・resume不一致をまとめる。
[JIT診断と制約](https://github.com/ieee0824/omoikane/issues/667#issuecomment-5922371031)。

元ONバイナリのELF dispatch tableの256ポインタを、同じ開始アドレスのsymbolへ対応付けた。
get_function/call handlerはそれぞれ532/580 bytesで前後同サイズ。
主要な直接呼出し先のcreate_function_object_fast（2080）、CallValue::resolve（408）、
Vm::set_register（184）、CodeBlock::constant_function（216）、unregister_root（320 bytes）も
前後同サイズだった。symbol付き呼出し・分岐アドレスを正規化し、即値を保持した命令を
同じ位置で比較すると、handler2件とconstant_function以外の4関数では、差はADRPと
直後のADDによるアドレス生成だけだった。set_registerは正規化後に完全一致した。
constant_functionにはこれらに加えてLDRのoffset差が1命令あった。
ADRPのpageと即値から求めたslotのELF relocationを照合すると、両版ともpanicメッセージの
usize整形関数を参照していた（同addressのu64 aliasも保持）。通常のconstant取得経路の
演算差ではない。[参照先の証拠](measurements/issue667/constant-function-relocation.json)を参照。
objdumpの近隣symbol表示は参照対象の証明ではなく、データ・間接call先の同一性や
動的coverageも未検証。この範囲の処理命令の追加を支持しないが、配置が時間差の原因とは
断定しない。全calleeを比較したものでもない。

## 不採用の3対照

同じworktree/compiler/locks/profile/featuresでビルドし、変更なしreferenceの4バイナリは
元helper保存バイナリとhash完全一致。各対照を両modeの全11項目10組・ページ20組で比較した。
全計算結果/JIT counter/page JSON/PNGが一致し、関連意味論5testsも成功。
原ソースを復元してから測定し、採用コードには反映しない。

| 対照 | array/off | closure/on | 不採用の主要根拠 |
| --- | ---: | ---: | --- |
| receiver cloneを全経路で前へ戻す | +0.01% | -2.21% | 文字列参照off+1.56%/on+2.87%、連結+2.30%/+2.62% |
| objectのみ早期cloneをOption保持 | -1.44% | -0.70% | 文字列参照on+3.76%、連結on+2.80%、method/off+3.25% |
| 短命register viewから所有object取得、重複clone省略 | -1.90% | -1.47% | 連結off+1.75%、arith/off+1.93%（両方9/10遅い） |

対象2項目だけの短縮で採用しない。各変更は生成コード・配置にも影響し、純粋なclone cost実験ではない。
全11項目と悪化は[全経路対照](https://github.com/ieee0824/omoikane/issues/667#issuecomment-5921482432)、
[object対照](https://github.com/ieee0824/omoikane/issues/667#issuecomment-5921817037)、
[重複clone対照](https://github.com/ieee0824/omoikane/issues/667#issuecomment-5922187860)を参照。

## 同一バイナリとGC診断

同じreferenceバイナリ・同じパスを両labelで用いる対照も各mode10/20組を事前固定した。
array/off中央値−0.35%、範囲−2.45〜+0.96%。closure/on中央値+0.51%、範囲−6.02〜+11.50%。
concat/onも+1.42%・8/10遅い。同一実装でも差が出るが、異なるバイナリの配置差や
元差がすべてnoiseであることは証明しない。
[全項目・順序別記録](https://github.com/ieee0824/omoikane/issues/667#issuecomment-5922327088)。

gc-profile付き同一harnessを元property実装/採用helperでビルドし、array/closure×off/on×10組、
4passの全320passを検査。全pairで各passのcollection回数が一致。
arrayのminor/majorは0、closureのminorは28/27/28/27、major0。
closure/onの4pass合計GC時間比率は+0.37%、同じ最小全体時間passのGCは+0.23%、
GC差し引き時間は+2.17%。中央値は加算せず、純粋なinterpreter CPU時間とも呼ばない。
分離body・各pass別eval・計時追加でfull11のheap履歴とは異なる。
[前後GC全結果](https://github.com/ieee0824/omoikane/issues/667#issuecomment-5922590215)、
[対応pass集計](https://github.com/ieee0824/omoikane/issues/667#issuecomment-5922606178)。

## 証拠の範囲と残る判断

単純な命令数増・closureのnative生成増・GC collection回数増は診断で支持されない。
非collection処理、実CPU cache/branch、コード配置、full11 heap履歴、測定変動の寄与は未確定。
実装修正なしなので文字列改善・既存GC/JIT契約を変更していない。
採用する場合のLinux x86_64/Linux ARM64/macOS ARM64 gateは未実行であり、採用時には別途必要。
本調査を「修正済み」や全環境の速度保証とは扱わない。

ローカルraw/inputs/identity/PNG/build/test証跡は
`/tmp/omoikane-issue667/.artifacts/issue667/` のreproduction、clone-controls、prefix-diagnostic、
closure-four-pass、gc-diagnostic、gc-diagnostic-base、gc-pairedに保持。
静的比較はbinary-inspection-v2、handler-comparison-v2、closure-callee-comparison-v2、
closure-instruction-validation.jsonに保持。初回の失敗した解析も削除せず区別している。
元保存バイナリは `.artifacts/agent-worklog/2026-09-09/issue659/`。

## 追加の元バイナリ比較とclose判断

同じ元保存バイナリ・条件で、追加10 benchmark pairs/20 page pairs per modeを一度だけ
事前固定した。前回のサンプルは置き換えず、全pairの結果・診断・描画一致を確認した。

| workload | off | on |
| --- | ---: | ---: |
| arith | +0.18% | -0.38% |
| prop-mono | -0.29% | -0.07% |
| prop-mega | +0.36% | -0.89% |
| call | -1.03% | +0.54% |
| closure-alloc | -3.68% | +3.00% |
| object-alloc | -5.11% | -1.01% |
| string-concat | -4.68% | -6.10% |
| array | +0.48% | -1.36% |
| primitive-string-property | -4.57% | -6.08% |
| primitive-string-method | -2.02% | -2.09% |
| proto-method | -0.43% | -0.86% |
| 全11項目のプロセス | -1.83% | -1.19% |
| 代表ページ | -1.42% | -0.65% |

closure/onは10/10組で遅い。array/offは6/10組で遅い。
日時ログの長い間隔もあるが、onの各processの単調時計時間は12.75〜13.51秒、CPU時間も
近い値であり、日時間隔だけから実行停滞の原因は断定しない。
ユーザーは再現できなければcloseを許可したが、closure/onの遅い側の差は2回の再確認で
残った。元の+5.87%という大きさが一致しないことだけを「再現なし」とせず、openを維持する。
直接原因は未確定。今後は元実行経路のCPU/配置・非collection処理の証拠が必要であり、
良い結果が出るまで同じ計測を繰り返すことはしない。
