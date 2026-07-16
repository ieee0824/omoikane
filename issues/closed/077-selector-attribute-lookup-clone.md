---
number: 077
slug: selector-attribute-lookup-clone
status: closed
---

# selector属性参照時のMap clone除去

## 概要

selector照合で属性1件を読むたびに要素の属性`BTreeMap`全体をcloneしていた経路を、DOMの直接参照APIへ置換する。

## 対応

- CSS matcherの属性取得を`NodeHandle::get_attribute`へ変更
- 1,000要素×1,000 class ruleのstyle benchmarkを追加
- 既存selectorテストで照合結果を維持する

## 結果（2026-07-16）

Linux aarch64、rustc 1.97.0、release build、10回計測。

| style resolve median | 変更前 | 変更後 | 改善率 |
| --- | ---: | ---: | ---: |
| 1,000要素×1,000 rule | 100.640 ms | 31.588 ms | 68.6% |

## 関連issue

- 076 CSS rule候補indexによるstyle解決高速化
