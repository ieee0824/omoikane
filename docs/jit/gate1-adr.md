# ADR: Gate 1の採用経路

- Status: Accepted
- Date: 2026-08-02
- Scope: #307 / #521
- Decision: Boa fork adapterをGate 2以降の唯一の実装経路として採用する

## 決定

Omoikaneが現在使っているBoa forkをengine-owned dependencyとして維持し、
Gate 2〜4ではそのforkにJIT境界を実装する。Omoikane側には安定したadapter
契約だけを公開し、VM・bytecode・shape/IC・GCのJIT依存部分はfork側で所有する。

自前engine新設はこのロードマップの実装経路として採用しない。特に、Gate 2で
新engineを並行して作るdual-engine本番経路は作らない。必要な差分実行はテスト用
のfeature flagまたは別プロセスのdifferential harnessに限定する。

現行Boaのproduction経路はGate 1〜3の間も変更しない。JITを実行するのは、
Gate 3の限定ベンチマークとGate 4のsoundness検証が合格した後だけにする。
Gate 4でGC・deopt・例外・interruptのいずれかが証明できなければ、JITを
無効化した現行Boaへ戻せることを必須とする。

## 根拠

Gate 1のprobeは、現在のforkには次の性質があることを確認した。

1. 公開APIからScript/CodeBlockのcompile・evaluateは実行できる。
2. Opcode、Instruction、VM frame、InlineCache、bytecode内部表現は外部embedding
   から直接patchできない。JITにはfork側のprivate API adapterが必要である。
3. shape/ICはreceiverとprototypeのshape guard、miss時のreset/fallbackまで成立
   している。これはprop-monoの最初のfast pathの基礎になる。
4. boa_gcのRootProvider、世代別GC、remembered parent、ephemeronは確認できるが、
   native JIT frameのstack map、register slot、safepoint、deopt ownershipはない。
5. OmoikaneのembeddingはScript/VM、host rooting、module/suspension、DOM/Web API、
   realm/task sourceの5層に及び、自前engineのfull migrationはGate 2の最小subsetを
   超えて大きい。既存の動作を保ったままJITを検証するには、fork adapterの方が
   rollback境界を小さくできる。

根拠の再実行点は次の通りである。

- Gate 1 baseline: [gate1-baseline.md](gate1-baseline.md)
- scope manifest: [gate1-scope.json](gate1-scope.json)
- Omoikane runtime probe: [tests/jit_gate1.rs](../../tests/jit_gate1.rs)
- interpreter IC regression oracle: [tests/boa_inline_cache.rs](../../tests/boa_inline_cache.rs)
- Boa revision: `1674beed49e671b991d092a9c4448fd019c275f5`

PR #555でCopilotレビューとGitHub CIを通過したことも、Gate 1の観測結果が
production runtimeを壊していないことの確認材料とする。ただし、これはnative
JIT frameのsoundnessを証明するものではない。

## Gate 2以降のscope

既存のGate 2〜6のIssueは自前engineを暗黙に前提にせず、次のように読む。

| Gate | 採用経路での責務 | 開始条件 | 停止条件 |
| --- | --- | --- | --- |
| Gate 2 / #511 | fork-owned bytecode・frame・shape/IC adapterのJITなし契約を固める | #521の本ADRがmainに入り、既存runtimeを変更しないprobeがある | semantic mismatch、既存suite回帰、またはSM-int相当域への見通し喪失 |
| Gate 3 / #512 | x86_64のarithとprop-monoだけをbaseline JIT化する | Gate 2のcontract/verifierとfallbackが安定 | 限定benchで効果なし、またはinterpreter fallbackとの差分が再現不能 |
| Gate 4 / #513 | JIT frame、GC root、deopt、例外、interruptを統合する | Gate 3の限定fast pathと完全なfallback | GC/use-after-free、誤ったdeopt/例外、timeout不能、WPT/Acid3/全suite回帰 |
| Gate 5 / #514 | aarch64と4配布targetへ同じ契約を移植する | Gate 4のsoundness成立 | target間の意味論差分、CI/release gate失敗 |
| Gate 6 / #515 | 必要なら全面切替を評価する | full embedding parity、性能、release gateを全て満たす | 1つでも未達ならBoaをdefaultから外さない |

Gate 6は「切替を行う権利」を得るgateであり、切替を予約するものではない。
Boa依存の削除、default engine変更、production realmでのJIT有効化は、Gate 6の
完了条件を別PRで満たし、Copilotレビュー・CI・compatibility gateを通過するまで
禁止する。

## Gate 2 Issueの読み替え

Gate 2の作業項目は、新しいGCやDOM runtimeを作る作業ではない。現在のBoaの
意味論とfallbackを再利用し、forkとOmoikaneの境界を検証可能にする。

- #522: dual-engine本番境界ではなく、JIT on/offとinterpreter fallbackの
  test-only differential harnessにする。
- #523: compilerを書き直さず、fork-owned bytecodeのread-only contract、
  verifier、versioning、hotness/compiled-entry metadataを定義する。
- #524: 既存VMをbaseline interpreter/fallbackとして使い、frameとcontrol-flow
  の観測・復元契約を追加する。
- #525: 既存shape transition・prototype invalidationを再利用し、guardが
  interpreter pathと同じ意味論を持つことを固定する。
- #526: まずprop-monoとmiss/fallbackを対象にする。poly/megaは設計上の状態を
  先に追加せず、monoの正しさと測定が成立した後に必要性を再評価する。
- #527: 既存boa_gcとallocationを再利用し、runtime call/slow pathとrootの
  観測を追加する。new-engine heapは作らない。
- #528: 現行11 workloadのsemantic parityと回帰を先に判定し、SM-intとの
  性能比較は既存benchmark条件・複数回測定・環境情報付きの補助gateとする。
  一時的な速度値だけでproduction切替を決めない。

これにより、元のGate 2にあった「自前VMを作ってから本番へdual-engineで移行する」
含意を除去する。#522〜#528のタイトル・本文がこの決定と食い違う場合は、着手前に
この対応表をIssueへ転記してscopeを更新する。

## Omoikane動作保証の境界

このADRの期間中、次を不変条件とする。

- productionのdefault engineは現行Boaのままにする。
- JIT probeが失敗したときはinterpreterへfallbackし、プロセスを不正なJIT
  frameのまま継続しない。
- JIT変更を含むPRは、少なくとも`cargo build`、全Rust test、Acid3、WPT smoke、
  Web API surface、既存benchmark、Gate 1 runtime probeを通過する。
- GC・deopt・例外・interruptのテストが1つでも不安定なら、そのJIT経路は
  disabledのままとする。
- Boa pinとOmoikane embeddingのrollbackを別々にできるよう、変更は小さいPRと
  feature flagに分ける。

Gate 1時点で確認済みの動作根拠は、`jit_gate1` 3件、Acid3 4件、Web API surface
96/97 supported・error 0、WPT smoke 69件中66 pass・regression 0、および
PR #555のGitHub CI成功である。これは現行runtimeの保証範囲であり、将来のJITを
安全とみなす根拠にはしない。

## 代替案と不採用理由

### 自前engine新設

Gate 2の11 workloadだけならbounded prototypeにできるが、Omoikaneのfull
embeddingにはhost ABI、Promise/jobs、modules、DOM/Web API、workers、realms、
event loop、structured clone、CDP値などの移行が必要になる。Boaをproductionに
残したままこの経路を並行して持つと、意味論とGCの二重管理が生じ、今回の目的で
ある性能検証の前にcompatibility riskが増えるため採用しない。

### Gate 1からproductionでJITを有効化

現時点ではmachine-code allocator、safepoint、JIT root map、deopt、例外
unwindがない。動作保証と相反するため採用しない。

### Boaの公開APIだけでJITを実装

Gate 1 probeでbytecode・frame・IC internalsがprivateであることが確認済みで、
adapterなしでは安定した契約を作れない。fork側で明示的に契約を所有する方を選ぶ。

## Rollbackと再判断

各Gateの実験は専用feature flagと小さいPRで行い、defaultを常にinterpreterに
置く。次のいずれかでそのGateを停止し、Boa forkのpinへ戻す。

- Omoikaneの既存semantic test、Acid3、WPT smoke、Web API surface、または全suiteに
  regressionが出る。
- JITとinterpreterのdifferential結果が再現できない。
- GC root map、deopt frame、例外、interruptの所有権をレビュー可能な形で定義できない。
- Gate 3の性能効果が測定ノイズを超えて確認できない。

この停止は自前engineを自動的に開始する合図ではない。再判断が必要な場合は
#307で新しい証拠とともにADRを更新し、別経路を開始する前に再承認する。

## 追跡Issue

- Gate 2: #511, #522〜#528
- Gate 3: #512, #317, #529〜#533
- Gate 4: #513, #534〜#540
- Gate 5: #514, #541〜#545
- Gate 6: #515, #546〜#553

このADRにより、#521の完了条件である「採用経路を一つに決め、#511着手時に
設計選択をやり直さない」を満たす。#511の実装開始前に、Gate 2 Issueへこの
scopeと停止条件を反映する。
