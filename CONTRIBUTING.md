# Contributing to Deadwood

Thanks for your interest. Deadwood is small and deliberate, and the process
below is what keeps it that way.

## The quality gate

Every change passes the same gate locally and in CI:

```console
$ scripts/check.sh       # fmt --check, clippy -D warnings, doc links, tests
$ scripts/check.sh --fix # apply rustfmt + clippy fixes instead of checking
```

CI (`.github/workflows/ci.yml`) runs exactly this on every push and PR. The
toolchain is pinned to `stable` via `rust-toolchain.toml`; `unsafe_code` is
forbidden crate-wide.

## Before writing code

- **File an issue first.** The roadmap in
  [`docs/SCOPE.md`](docs/SCOPE.md) and the issue tracker are kept saying the
  same thing — a change starts by being filed, so that neither can quietly
  rot. For a bug, an issue with a reproducing fixture is the ideal shape.
- **Read the tenets** at the bottom of [`docs/SCOPE.md`](docs/SCOPE.md). The
  load-bearing one: prefer false negatives to false positives. A dead-code
  tool that reports live code as dead gets uninstalled, so anything
  unresolvable — macro input, attribute arguments, ambiguity of any kind —
  counts as a use, not a finding.
- **New dependencies need a confirmed problem.** The current set (`std`,
  `syn`, `proc-macro2`, `serde`, `serde_json`, `clap`, `anyhow`, `toml`) is a
  ceiling, not a floor.

## Shape of a change

One slice per PR: a single check, fix, or behavior change, with the tests
that pin it. Fixtures under `tests/fixtures/` are self-contained cargo
packages the integration tests analyze — a new behavior usually wants a new
fixture or a new case in an existing one, exercised from `tests/analyze.rs`.

A change to analysis behavior also updates the documentation that claims to
describe it: the README's check table and known-limitations section, and —
for a shipped slice — the phase record in [`docs/SCOPE.md`](docs/SCOPE.md)
and [`docs/HISTORY.md`](docs/HISTORY.md), which record what each phase
changed, what it measured, and what it rejected.

## License

MIT. By contributing you agree your contributions are licensed under the same
terms — see [LICENSE](LICENSE).
