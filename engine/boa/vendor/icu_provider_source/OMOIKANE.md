# ICU source provider ZIP dependency

This is the `icu_provider_source` 2.0.1 crate from crates.io, licensed under
Unicode-3.0; the upstream `LICENSE` and `README.md` are preserved. The copied
crate archive checksum is
`1a0467cd220b7e918b38d1d78baeea317821255776d2afbd0ab31fa80c3138d6`.
The copied files are `Cargo.toml`, `LICENSE`, `README.md`, `src/`, and `data/`.
The upstream integration-test fixtures are omitted because this copy is used
only by Boa's `gen-icu4x-data` development tool.

The dependency change is `zip` 2 to `zip` 9 in `Cargo.toml`. `src/source.rs`
also handles `zip` 9's fallible entry-name iterator. The generator reads ZIP
entries by name and does not extract an archive to the filesystem. `zip` 2 is
affected by RUSTSEC-2026-0367, RUSTSEC-2026-0368, and
RUSTSEC-2026-0369; the fixed major version is 9. Keep this patch until upstream
`icu_provider_source` supports a fixed ZIP version and the Boa ICU toolchain can
move to that release. The original package version is kept for the ICU 2.0
dependency graph.

Three unrelated trailing-whitespace/EOF blank-line spots in the copied source
and data were normalized so the repository's `git diff --check` passes.
