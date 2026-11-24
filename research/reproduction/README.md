# Reproduce the turn-timing tests

Requires Rust/Cargo (verified with 1.100.0), Git and `patch`. Place this entire
package at `research/` in the upstream source tree. Its `crates/` contents must
match upstream commit
`0fdade8b40bfa78c0d9782ab58c27ff6f91c3872`.

The separate Cargo workspace uses relative paths to the upstream libraries and
tests, with a checked-in lockfile. See [build scope](../PROVENANCE.md) for the
libraries and tests it includes.
Registry downloads are needed on the first run; add `--offline` if cached.

From a working directory on disk:

```sh
git clone --branch research https://github.com/chrisnestrud/begin.git handoff
git -C handoff diff --exit-code 0fdade8b40bfa78c0d9782ab58c27ff6f91c3872 -- crates
cp -a handoff torpedo
cp -a handoff completion
mkdir -p tmp
export CARGO_TARGET_DIR="$PWD/target-baseline"
export TMPDIR="$PWD/tmp"
```

Check the downloaded `research/SHA256SUMS` before running:

```sh
(cd handoff/research && sha256sum -c SHA256SUMS)
```

The unpatched source should give 6 passing tests and 3 expected
failures (exit 101: one torpedo policy assertion and two completion assertions).
Read the assertions to distinguish expected failures from a build/runtime error.

```sh
cargo test --locked -j 1 --manifest-path handoff/research/reproduction/Cargo.toml \
  -p v165-handoff-checks --no-fail-fast -- --test-threads=1
```

Test the torpedo patch on its own:

```sh
export CARGO_TARGET_DIR="$PWD/target-torpedo"
patch --dry-run -d torpedo -p1 < handoff/research/patches/rust_torpedo_turn.patch
patch -d torpedo -p1 < handoff/research/patches/rust_torpedo_turn.patch
cargo test --locked -j 1 --manifest-path torpedo/research/reproduction/Cargo.toml \
  -p v165-handoff-checks --test torpedo_turn -- --test-threads=1
cargo test --locked -j 1 --manifest-path torpedo/research/reproduction/Cargo.toml \
  --workspace --exclude v165-handoff-checks -- --test-threads=1
```

Test the completion patch on its own:

```sh
export CARGO_TARGET_DIR="$PWD/target-completion"
patch --dry-run -d completion -p1 < handoff/research/patches/rust_completion_window.patch
patch -d completion -p1 < handoff/research/patches/rust_completion_window.patch
cargo test --locked -j 1 --manifest-path completion/research/reproduction/Cargo.toml \
  -p v165-handoff-checks --test completion_window -- --test-threads=1
cargo test --locked -j 1 --manifest-path completion/research/reproduction/Cargo.toml \
  --workspace --exclude v165-handoff-checks -- --test-threads=1
```

Expected results: 4 torpedo tests or 5 completion tests pass, plus 36 upstream
tests with one AI-watch test ignored. Each patch fixes only its own test failures.
See the [recorded results](../VALIDATION.md).

Keep a separate build directory for each source copy. Sharing one can reuse a
stale executable when relative package paths and timestamps match.
