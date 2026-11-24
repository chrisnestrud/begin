# Test results — 2026-11-24

Tests ran with Rust 1.100.0 and the relative-path Cargo manifests in this package.
Commands used `cargo test --offline --locked -j 1` and `--test-threads=1`.

| Source | Behavior tests | Upstream library tests |
| --- | --- | --- |
| Unchanged source | 6 pass, 3 expected failures; exit 101 | Not rerun |
| Torpedo patch only | 4 pass | 36 pass, 1 ignored |
| Completion patch only | 5 pass | 36 pass, 1 ignored |

The original source fails one torpedo assertion and two completion assertions.
The baseline command uses `--no-fail-fast` to run both test files. Each patch
was tested in a separate source copy with its own build directory. The ignored
upstream test is AI-watch.

Logs: [baseline](logs/baseline.log),
[torpedo tests](logs/torpedo-focused.log),
[torpedo upstream tests](logs/torpedo-upstream.log),
[completion tests](logs/completion-focused.log),
[completion upstream tests](logs/completion-upstream.log).
Local path prefixes have been replaced with `verification/`.

All 68 captured source files match the
[source hashes](evidence/upstream-source-sha256.txt), whose paths are relative to
the upstream repository root. Each patched copy differs only in the files named
by its patch. Both patches passed `patch --dry-run`; the patches and tests match
the 2026-11-19 copies. All eleven registry dependency versions and checksums match
the lockfile used in those experiments.

See [reproduction commands](reproduction/README.md) to repeat these checks.
