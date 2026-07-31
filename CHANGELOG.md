# Changelog

Notable changes per release. The full phase-by-phase record — every decision,
its rejected alternatives, the corpus measurements and mutation runs behind it
— lives in [`docs/HISTORY.md`](docs/HISTORY.md); this file is the short
version, newest first.

## 1.0.0-beta.1 — 2026-07-30

First public pre-release. The v1 check set is complete; what stands between
this and 1.0 is soak time on codebases that are not this one.

### Checks

- **Dead files** — `.rs` files under `src/` unreachable from any target root
  via `mod` declarations or a readable `include!`.
- **Unused pub items** — fully-`pub` items nothing live in the workspace
  reaches, decided by path resolution and a reachability walk, not by
  counting identifiers: dead subsystems and dead cycles come out in one run.
- **Unused re-exports** — `pub use` re-exports nothing goes through, inside
  the workspace or out.
- **Unused dependencies** — manifest entries (normal, dev, build) whose crate
  the declaring package's code never names.
- **Misplaced dependencies** — entries declared in a table the code naming
  them cannot see (e.g. a `[dependencies]` entry only tests use).
- **Unsatisfiable `cfg` gates** — `#[cfg(...)]` gates that can hold in no
  build of the package.
- **Test-only public items** *(off by default)* — `pub` items the workspace
  reaches only through test code.

### Operation

- `deadwood check [PATH]` with text or `--json` output; exit codes `0`
  clean / `1` denied findings / `2` error, suitable for CI gates.
- `deadwood.toml` configuration: `ignore` globs, per-kind severity,
  `public-api` and dependency allowlists. Every default is the unconfigured
  behavior.
- Baseline adoption: `--write-baseline` records today's findings so later
  runs fail only on what is new; `--prune-baseline` drops entries that no
  longer occur. Entries survive file moves and renames of the paths around
  them.
- Workspace discovery via `cargo metadata --no-deps`; works offline; no
  `unsafe` anywhere in the crate.
