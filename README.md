# Deadwood

A codebase health analyzer for Rust workspaces. Deadwood finds maintainability
issues that `rustc` and `clippy` stay quiet about — starting with dead module
files and unused `pub` items, in the spirit of Fallow/knip-style analyzers for
other ecosystems.

**Status:** v0.1 — early, narrow, and honest about it. See
[`docs/SCOPE.md`](docs/SCOPE.md) for what is in and out of scope, and
[`docs/ENVIRONMENT.md`](docs/ENVIRONMENT.md) for the environment assessment
this project was bootstrapped from.

## What it detects today

| Check | What it finds | Why rustc doesn't |
| --- | --- | --- |
| **Dead files** | `.rs` files under `src/` not reachable from any target root via `mod` declarations | Files outside the module tree are never compiled, so no lint ever sees them |
| **Unused pub items** | Fully-`pub` fns, structs, enums, traits, type aliases, consts, statics, and unions whose name is never mentioned anywhere else in the workspace | `dead_code` assumes `pub` items have external consumers |

The unused-pub check is a *name-based heuristic*, deliberately biased toward
staying quiet rather than raising noise: any mention of the name anywhere in
the workspace (including inside macro invocations) counts as a use. Items
marked `#[no_mangle]`, `#[used]`, `#[export_name]`, or
`#[allow(dead_code)]`/`#[expect(dead_code)]` are skipped. For library crates
with external consumers, treat unused-pub findings as advisory.

## Usage

```console
$ cargo run -- check path/to/workspace
Dead files:
  src/orphan.rs: not reachable from any target of package `simple` via `mod` declarations

Unused public items:
  src/lib.rs:3: pub fn `entry` is never referenced by name anywhere in this workspace
  src/lib.rs:7: pub fn `dead_fn` is never referenced by name anywhere in this workspace

3 finding(s) in workspace `/path/to/workspace`.
```

- `deadwood check [PATH]` — analyze the package/workspace at `PATH` (default `.`)
- `--json` — machine-readable output (findings + warnings)
- Exit codes: `0` clean, `1` findings, `2` error — suitable for CI gates.

Requires `cargo` on `PATH` (workspace discovery shells out to
`cargo metadata --no-deps`, which works offline).

## Development

From a fresh checkout:

```console
$ cargo build            # build
$ cargo test             # unit + integration tests
$ scripts/check.sh       # full gate: fmt --check, clippy -D warnings, tests
$ scripts/check.sh --fix # apply rustfmt + clippy fixes
```

CI (`.github/workflows/ci.yml`) runs the same gate on every push and PR. The
toolchain is pinned to `stable` with `clippy` and `rustfmt` via
`rust-toolchain.toml`; `Cargo.lock` is committed for reproducible builds.

## How it works

1. **Workspace discovery** — `cargo metadata --no-deps` provides workspace
   members, target roots (lib/bin/test/example/bench/build), and the
   workspace root (`src/metadata.rs`).
2. **Module-tree resolution** — from each target root, `mod` declarations
   (including nested inline modules and `#[path]`) are followed to the files
   they name; everything reached is parsed once with `syn`
   (`src/modtree.rs`).
3. **Detectors** — dead files are `src/**.rs` minus the reachable set;
   unused pub items come from comparing declared `pub` items against a
   workspace-wide identifier census (`src/unused.rs`).
4. **Reporting** — grouped text or JSON (`src/report.rs`).

## Known limitations (tracked, not hidden)

- `cfg`-gated `mod`s are always followed → platform-specific files are never
  reported dead (conservative by design).
- `include!()`-ed files are not tracked and may be reported as dead.
- Name collisions and `impl` blocks hide unused items (false negatives, never
  false positives).
- Macro-*generated* `mod` declarations and items are invisible to the parser.

## License

MIT — see [LICENSE](LICENSE).
