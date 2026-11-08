# Contributing to Deadwood

Deadwood is small and deliberate, and the process below keeps it that way.
Participation is covered by the [code of conduct](CODE_OF_CONDUCT.md).
Questions and ideas are welcome in
[GitHub issues](https://github.com/rlorenzo/deadwood-rs/issues). This is a
spare-time project, so expect responses in days, not hours.

## The quality gate

Every change passes the same gate locally and in CI:

```console
$ scripts/check.sh       # fmt --check, clippy -D warnings, doc links, tests
$ scripts/check.sh --fix # apply rustfmt + clippy fixes instead of checking
```

CI (`.github/workflows/ci.yml`) runs exactly this on every push and PR. The
toolchain is pinned to `stable` via `rust-toolchain.toml`, and `unsafe_code` is
forbidden crate-wide.

To run the gate before every commit, opt into the tracked hook once:

```console
$ git config core.hooksPath .githooks
```

`git commit --no-verify` skips it in an emergency. CI runs the identical
script either way, so nothing rides on the hook.

## Before writing code

- **File an issue first.** The roadmap in [`docs/SCOPE.md`](docs/SCOPE.md) and
  the issue tracker say the same thing, and a change starts by being filed so
  neither can quietly rot. For a bug, the ideal issue has a reproducing
  fixture.
- **Read the tenets** at the bottom of [`docs/SCOPE.md`](docs/SCOPE.md). The
  load-bearing one is to prefer false negatives to false positives. A
  dead-code tool that reports live code as dead gets uninstalled, so anything
  unresolvable (macro input, attribute arguments, ambiguity of any kind)
  counts as a use, not a finding.
- **New dependencies need a confirmed problem.** The current set (`std`,
  `syn`, `proc-macro2`, `serde`, `serde_json`, `clap`, `anyhow`, `toml`) is a
  ceiling.

## Shape of a change

One slice per PR: a single check, fix, or behavior change, with the tests
that pin it. Fixtures under `tests/fixtures/` are self-contained cargo
packages the integration tests analyze. A new behavior usually wants a new
fixture, or a new case in an existing one, exercised from `tests/analyze.rs`.

A change to analysis behavior also updates the documentation that describes
it: the README's check table, `docs/ANALYSIS.md`, `docs/LIMITATIONS.md` and,
for a shipped slice, the phase record in [`docs/SCOPE.md`](docs/SCOPE.md) and
[`docs/HISTORY.md`](docs/HISTORY.md). Those record what each phase changed,
measured and rejected.

## Releasing

Set the version in `Cargo.toml`, date its `CHANGELOG.md` section, and merge.
Then push a matching tag, such as `v1.2.3`. The release workflow builds the
binaries, runs the semver check, publishes to crates.io through trusted
publishing, and only then makes the GitHub release public.

## License

MIT. By contributing you agree your contributions are licensed under the same
terms; see [LICENSE](LICENSE).
