# x86 interpreter CI consolidation: follow-up measurements

Tracking: [#1065](https://github.com/ieee0824/omoikane/issues/1065).
The implementation was merged in [PR #1205](https://github.com/ieee0824/omoikane/pull/1205).
This follow-up adds the second post-change measurement, including the main push,
and rechecks test identities, cache inputs and diagnostic artifacts against the
downloaded GitHub Actions evidence. Collected on 2026-10-07.

The [machine-readable result](results/issue1065-2026-10-07.json) records all run
and relevant job IDs, cache-hit keys, the 193 Browser test IDs, artifact paths,
and the timing definitions. Full downloaded logs and artifacts are retained
locally at `/workspace/.artifacts/p2-ci-measurements/`.

## Measurements

| Head / condition | CI + Browser runner minutes | CI test minutes | Separate x86 interpreter minutes | All workflows elapsed minutes |
| --- | ---: | ---: | ---: | ---: |
| `92125ac6`, before / push | 122.52 | 13.57 | 6.77 | 19.18, censored |
| `c3099977`, before / push | 118.30 | 10.80 | 5.48 | 23.02 |
| `0c3a33f6`, after / PR | 111.12 | 12.58 | — | 21.77 |
| `196579e4`, after / main push | 113.33 | 13.23 | — | 21.18 |

Sources: before [CI](https://github.com/ieee0824/omoikane/actions/runs/36788656572)
and [Browser](https://github.com/ieee0824/omoikane/actions/runs/36788656539),
second before [CI](https://github.com/ieee0824/omoikane/actions/runs/36790299601)
and [Browser](https://github.com/ieee0824/omoikane/actions/runs/36790299636),
after PR [CI](https://github.com/ieee0824/omoikane/actions/runs/36791289694)
and [Browser](https://github.com/ieee0824/omoikane/actions/runs/36791289972),
after main [CI](https://github.com/ieee0824/omoikane/actions/runs/36793241005)
and [Browser](https://github.com/ieee0824/omoikane/actions/runs/36793240985).

Runner minutes sum each non-skipped job's started-to-completed duration;
parallel jobs contribute separately. Elapsed time includes queueing and spans
the earliest workflow creation to the latest job completion across **all**
workflows at the head. The first sample's CodeQL and release gate were cancelled,
so its elapsed time is not a successful completion measurement. The other three
samples completed all their triggered workflows successfully.

All four CI test jobs restored the same `workspace-v1` cache key and 569 MB
archive; both removed Browser jobs restored the same separate interpreter key
and 381 MB archive. They used Rust 1.98.1. The before/after-main font inventory
JSON is byte-identical. These are comparable warm-cache observations, rather
than a cold-cache/warm-cache comparison. Workflow trigger sets differ (for
example the second before sample includes GUI Pointer Lock), so the total
elapsed values are observational, not a controlled estimate of PR latency.

The two-sample mean CI + Browser runner use decreased from 120.41 to 112.23
minutes, an observed saving of 8.18 minutes (6.8%). The directly removed job cost
5.48–6.77 minutes per run. Remaining timing variation prevents attributing the
entire mean difference to consolidation. The successful before sample took
23.02 minutes across all workflows; the successful after samples took 21.77 and
21.18 minutes. The critical path remained CodeQL/release, so the removed job's
runner minutes are not a promise of an equal reduction in waiting time.

## Contract and artifact correspondence

| Condition | Before Browser x86 interpreter | Consolidated CI test |
| --- | --- | --- |
| Test IDs | 193: font filter 174, font selection 1, benchmark results 5, journeys 7, Acid3 5, float16 1 | Every one of the 193 IDs appears in all four CI logs; full suite 3350 passed, plus separate Gate 2 differential tests 3 passed |
| Ignored / feature | Font filter includes ignored; default features | Whole suite includes ignored; default features; zero harness-ignored tests in measured logs |
| Runner / fonts | ubuntu-24.04; DejaVu, Liberation, Noto Core | Same image selection and font packages; selection inventory unchanged |
| Threads | Explicit one test thread | Default test parallelism; case-specific report files and scoped font diagnostics preserve independent outputs |
| Reports / images | Journey JSON, font diagnostics, painted frames, Acid3 report | All 31 existing report/image paths retained; four additional paint images |
| Text logs | tests.log plus font-tests.log and system-font-tests.log | tests.log contains the full suite, including the font tests; the two separate logs are replaced by this combined log |
| Build inputs | Revision, Rust version, lockfile, dependency metadata | Same four input records |

The retrieved logs contain 193 Browser test IDs, correcting the earlier PR's
189-count snapshot. Successful harness counts do not imply that every optional
font case exercised its assertions: the old logs explicitly report missing CJK
test fonts and an Arabic-glyph fixture skip. Consolidation does not add such
skips or claim to solve font-fixture coverage.

Both downloaded Acid3 reports scored 100/100 in faithful and direct modes with
empty task/drive errors. The after-main artifact retains the original
`browser-x86_64-unknown-linux-gnu-interpreter` name. Its 40 files comprise the
31 preserved report/image files, four additional images and five build/log
files; the old artifact's 38 files include seven build/log files.

PR #1205 verified that the removed Browser check was not required by a ruleset;
the ordinary `test` check remains. No additional check or validation is removed
in this follow-up. These measurements support retaining the consolidation.
