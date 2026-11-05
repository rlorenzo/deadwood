# Deadwood

Deadwood finds the dead code in a Rust workspace that `rustc` and `clippy` don't
report: orphaned module files, unused `pub` items and re-exports, unused and
misplaced dependencies, and `cfg` gates that can never hold. It's in the spirit
of knip and Fallow, for Rust. It needs no build and no nightly, and it analyzes
the entire `rustc` workspace in about 3.5s.

**Status:** v1.0.0. The v1 check set is complete and the roadmap is empty.

## What it finds

| Check | What it finds | Why `rustc` doesn't |
| --- | --- | --- |
| **Dead files** | `.rs` files no target root reaches via `mod` or `include!` | Files outside the module tree are never compiled, so no lint sees them |
| **Unused pub items** | `pub` items nothing live in the workspace reaches | `dead_code` assumes `pub` items have external consumers |
| **Unused re-exports** | `pub use` that nothing goes through and outside code can't reach | `unused_imports` only sees imports the crate itself doesn't use |
| **Unused dependencies** | `Cargo.toml` entries the package's code never mentions | Cargo has no reason to look |
| **Misplaced dependencies** | Entries in a table the code naming them can't see, such as a `[dependencies]` entry only tests use | Cargo builds the entry wherever you put it, and when that fails, the error doesn't name the entry |
| **Unsatisfiable `cfg` gates** | `#[cfg(...)]` that holds in no build, such as a feature the manifest doesn't declare | The code is never compiled, and the gate reads as deliberate |
| **Test-only public items** (off by default) | `pub` items reached only from test code | `dead_code` says nothing about items in test, bench, or example targets |

[`docs/ANALYSIS.md`](https://github.com/rlorenzo/deadwood/blob/main/docs/ANALYSIS.md)
has the exact rule behind each check.

## Install

The package is `deadwood-rs` because `deadwood` on crates.io is an unrelated
project. The binary it installs is `deadwood`.

```console
$ cargo install deadwood-rs
```

Or build from the repository:

```console
$ cargo install --locked --git https://github.com/rlorenzo/deadwood
```

Each [GitHub release](https://github.com/rlorenzo/deadwood/releases) has prebuilt
binaries with SHA-256 checksums for Linux (x86_64, aarch64), macOS (Intel, Apple
silicon), and Windows.

## Usage

```console
$ deadwood check path/to/workspace
Dead files:
  src/orphan.rs: not reachable from any target of package `simple` via `mod` declarations

Unused public items:
  src/lib.rs:7: pub fn `dead_fn` is never referenced by any resolved path in this workspace
  src/lib.rs:9: pub fn `dead_helper` is referenced only from items that nothing reaches

Misplaced dependencies:
  Cargo.toml: dependency `assert_cmd` is referenced only by the test, example and bench code of package `demo`, so it belongs in `[dev-dependencies]` rather than `[dependencies]`

4 finding(s) in workspace `/path/to/workspace`.
```

| Option | Effect |
| --- | --- |
| `[PATH]` | Package or workspace to analyze (default `.`) |
| `--json` | Machine-readable findings and warnings |
| `--config PATH` | Use this file instead of searching for `deadwood.toml` |
| `--write-baseline` | Record current findings so later runs fail only on new ones |
| `--prune-baseline` | Drop baseline entries that no longer occur |

Exit codes: `0` clean, `1` denied findings, `2` error. Every kind is `deny` by
default except `test_only_item`, which is `off`.

Deadwood needs `cargo` on `PATH`. It runs `cargo metadata --no-deps`, which works
offline.

## Why Deadwood

No other Rust tool covers more than one row of the table above. Dependency
checkers check dependencies, and `dead_code` stops at `pub`, assuming someone outside the crate
calls it. That's false for binary crates, internal workspace members, and items
made `pub` out of habit, and those are where dead code piles up.

Three design choices matter before you run it:

- **Reachability, not reference counting.** An item survives only if something
  live names it. A dead subsystem comes out in one run rather than one layer per
  run, and a mutually recursive pair nothing calls gets reported at all.
- **False negatives over false positives.** Anything Deadwood can't resolve
  (macro input, attribute strings, ambiguous paths) counts as a use. Every claim
  class was measured against ten major workspaces, including rust-lang/rust,
  tokio, serde, zed, deno, and ripgrep. A misfire there became a filed issue, a
  fix, and a regression fixture, each recorded in
  [`docs/HISTORY.md`](https://github.com/rlorenzo/deadwood/blob/main/docs/HISTORY.md).
- **Reporting only.** Deadwood never rewrites your code, at least until its
  precision has been proven in the field.

| | Deadwood | cargo-machete | cargo-udeps | rustc `dead_code` | knip <sub>(JS/TS)</sub> | jscpd <sub>(many langs)</sub> |
| --- | --- | --- | --- | --- | --- | --- |
| Dead source files | ✓ | — | — | — | ✓ | — |
| Unused `pub` items / re-exports | ✓ | — | — | private items only | ✓ (exports) | — |
| Unused dependencies | ✓ | ✓ | ✓ | — | ✓ | — |
| Misplaced dependencies (dev ↔ normal ↔ build) | ✓ | — | — | — | — | — |
| Unsatisfiable `cfg` gates | ✓ | — | — | — | — | — |
| Test-only `pub` items | ✓ *(opt-in)* | — | — | — | — | — |
| Copy-paste duplication | — | — | — | — | — | ✓ |
| Needs nightly / a full build | no / no | no / no | yes / yes | comes with the build | no | no |
| Baseline for existing codebases | ✓ | — | — | — | — | — |

Some jobs belong to other tools. Duplication is [jscpd](https://jscpd.dev)'s and
stays [out of scope](https://github.com/rlorenzo/deadwood/blob/main/docs/SCOPE.md)
until the dead-code core has earned its precision. Security advisories belong to
[`cargo-audit`](https://crates.io/crates/cargo-audit), and license and ban policy
to [`cargo-deny`](https://crates.io/crates/cargo-deny). If unused dependencies are
your only question, [`cargo-machete`](https://github.com/bnjbvr/cargo-machete)
answers it in milliseconds.

## Adopt it on an existing codebase

Record today's findings once, commit the file, and only new findings fail CI:

```console
$ deadwood check --write-baseline
No issues found.
Wrote 34 finding(s) to baseline `deadwood-baseline.json`.

$ deadwood check
No issues found.
34 finding(s) suppressed by baseline `deadwood-baseline.json`.
```

Matching survives line drift and, for item findings, moved files. Entries for
code you've since deleted are reported as stale, never as failures. See
[`docs/BASELINE.md`](https://github.com/rlorenzo/deadwood/blob/main/docs/BASELINE.md).

## GitHub Action

```yaml
- uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4.4.0
- uses: rlorenzo/deadwood@<release-commit-sha> # v1.0.0
  with:
    path: .       # workspace to analyze (default `.`)
    args: ""      # extra arguments for `deadwood check`, e.g. `--json`
```

Pin the full commit SHA of a release tag. By default the action builds Deadwood
from that commit. Set
`version: 1.0.0` to install that release from crates.io instead. The runner
needs stable Rust on `PATH`, which every GitHub-hosted runner has.

## Configuration

Deadwood needs no configuration. A `deadwood.toml` expresses what the analysis
can't infer:

```toml
ignore = ["crates/*/src/generated/**", "vendor"]  # not yours to fix

[severity]                     # deny, warn, or off, per finding kind
unused_pub_item = "warn"
test_only_item = "warn"

[public-api]                   # consumers Deadwood can't see
crates = ["my-library"]

[dependencies]                 # load bearing without being named in code
allow = ["getrandom"]

[cfg]                          # narrow the builds analyzed
features = ["default", "serde"]
```

Mistakes, including unknown keys, are exit 2 rather than silently ignored. For
libraries with outside consumers, treat unused-pub findings as advisory until the
crate is listed under `[public-api]`. Every setting is in
[`docs/CONFIGURATION.md`](https://github.com/rlorenzo/deadwood/blob/main/docs/CONFIGURATION.md).

## Limitations

Resolution is syntactic: Deadwood doesn't expand macros or resolve method calls
and trait dispatch, so it misses some dead code by design. It can't see consumers
outside the workspace, so a `pub` item only they use looks dead. A dependency
that's named by no code, such as one declared to turn on a transitive feature, is
reported until you list it under `[dependencies]`. The full list is in
[`docs/LIMITATIONS.md`](https://github.com/rlorenzo/deadwood/blob/main/docs/LIMITATIONS.md).

## Development

```console
$ cargo test             # unit + integration tests
$ scripts/check.sh       # full gate: fmt, clippy -D warnings, doc links, tests, cargo deny
$ scripts/check.sh --fix # apply rustfmt + clippy fixes
```

CI runs the same gate on every push and PR. See
[CONTRIBUTING.md](https://github.com/rlorenzo/deadwood/blob/main/CONTRIBUTING.md)
for how changes are shaped, and
[`docs/SCOPE.md`](https://github.com/rlorenzo/deadwood/blob/main/docs/SCOPE.md) for
what's in and out of scope.

## Getting help

Questions, bugs, and feature requests go through
[GitHub issues](https://github.com/rlorenzo/deadwood/issues). For a bug, a small
fixture package that reproduces it is ideal. The
[code of conduct](https://github.com/rlorenzo/deadwood/blob/main/CODE_OF_CONDUCT.md)
covers the project's spaces.

## License

MIT. See [LICENSE](https://github.com/rlorenzo/deadwood/blob/main/LICENSE).
