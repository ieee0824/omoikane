# JavaScript engine ownership

Omoikane maintains the existing Boa implementation in [`boa/`](boa/). Changes to
the engine and its browser embedding can be reviewed, tested and released in one
Omoikane revision. Development does not depend on merging changes upstream or
publishing another fork revision. Crate names and public APIs are retained.

[`boa-origin.json`](boa-origin.json) records the imported fork revision
`ee98fdacffb38093d9d220c2ac21a4ed6839ce37`, Git tree, and every original tracked
file's hash and mode. All 886 files were copied from verified Git blobs, including
the workspace manifests, fixtures, development tools and original workflows.
The original metadata and copyright notices remain intact. The preserved
[`LICENSE-MIT`](boa/LICENSE-MIT) and [`LICENSE-UNLICENSE`](boa/LICENSE-UNLICENSE)
describe the imported code's licensing.

The root `Cargo.toml` uses path dependencies for `boa_engine` and `boa_gc`.
Their seven related dependencies also resolve inside this tree. The root
`Cargo.lock` fixes the browser dependency graph; `boa/Cargo.lock` independently
fixes the complete engine workspace and its development tools. Keep both under
version control. The initial conversion preserves all 384 root package versions
and only removes the nine former Git source entries.

Run `python3 scripts/check-engine-source.py` from the repository root to check
source retention and the resolved dependency graph. The report lists changes
since import; it does not forbid reviewed engine development. The initial import
also passes `--pristine`, which requires byte-identical files. Do not rewrite the
origin manifest to hide subsequent changes: use normal Omoikane commits to record
them. Engine source identity after import is the Omoikane commit plus this origin.

The nested workspace remains usable directly:

```sh
cd engine/boa
cargo test --locked -p boa_parser -p boa_gc -p boa_engine
cargo test --locked -p boa_engine --features baseline-jit jit:: -- --nocapture
```

The original `boa/.github/workflows/` files are retained as history, but GitHub
does not execute nested workflows. Omoikane's root workflows must exercise the
engine alongside browser compatibility and release gates. Source placement and
successful dependency resolution alone are not completion of #552/#515.
