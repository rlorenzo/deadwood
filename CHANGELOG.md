# Changelog

Notable changes per release, newest first. The full phase-by-phase record, with
every decision, its rejected alternatives, and the corpus measurements and
mutation runs behind it, is in
[`docs/HISTORY.md`](https://github.com/rlorenzo/deadwood-rs/blob/main/docs/HISTORY.md).

## 1.0.0 — 2026-11-08

First public release. The v1 check set is complete. It is published as
the `deadwood-rs` package, since the plain crates.io name belongs to an
unrelated project, and it installs a binary named `deadwood`.

### Checks

- **Dead files**: `.rs` files under `src/` unreachable from any target root
  via `mod` declarations or a readable `include!`.
- **Unused pub items**: fully-`pub` items nothing live in the workspace
  reaches, decided by path resolution and a reachability walk rather than by
  counting identifiers. Dead subsystems and dead cycles come out in one run.
- **Unused re-exports**: `pub use` re-exports nothing goes through, inside
  the workspace or out.
- **Unused dependencies**: manifest entries (normal, dev, build) whose crate
  the declaring package's code never names.
- **Misplaced dependencies**: entries declared in a table the code naming
  them cannot see, such as a `[dependencies]` entry only tests use.
- **Test-only public items** (off by default): `pub` items the workspace
  reaches only through test code.

### Operation

- `deadwood check [PATH]` prints text or, with `--json`, JSON. Exit codes are
  `0` clean, `1` denied findings, `2` error, so it works as a CI gate.
- `deadwood.toml` sets `ignore` globs, per-kind severity, and `public-api` and
  dependency allowlists. Every default is the unconfigured behavior.
- `--write-baseline` records today's findings so later runs fail only on what
  is new. `--prune-baseline` drops entries that no longer occur. Entries
  survive file moves and renames of the paths around them.
- Workspace discovery uses `cargo metadata --no-deps`, so it works offline.
  The crate has no `unsafe`.

### Changed

- Moved to `syn` 3, so the tree holds one copy of it. Output is unchanged across
  427 crates and workspaces compared against the `syn` 2 build, apart from the
  wording of one parser error.

### Fixed before release

Bugs found while testing against bevy, ruff, helix, bun, serde, tokio, ripgrep
and 1265 registry crates. Details are in `docs/HISTORY.md`.

- `--prune-baseline` and `--write-baseline` leave entries for skipped checks
  alone. Both exit 2 and leave the file untouched when the run raised a warning.
- `ignore` patterns still match when the workspace path is a symlink.
- Dependencies are no longer wrongly reported unused or misplaced when named by
  `[features]`, an optional dependency's custom lib name, a `macro_rules!` body in
  another workspace crate, or a proc macro's expansion. Deadwood reads macros
  outside the workspace from the registry cache.
- `use real as alias;` inside a function body no longer hides uses of a
  dependency named `alias` elsewhere in the file.
- Module resolution handles `mod r#struct;`, `#[path]` inside inline `mod`
  blocks, `#[path]` targets that own their directory, and
  `#[cfg_attr(cond, path = "..")]`. The condition is not evaluated: the first
  arm naming a real file is the module and the others are spared.
- Files mounted twice, or reached under two spellings through a symlinked
  directory, no longer produce false dead-file or unused-item reports.
- `macro_rules!` that declare modules are followed across crates and from inside
  other macros.
- The dead-file check walks the directories the lib and bin roots sit in, so a
  crate with `[lib] path = "lib.rs"` is no longer skipped. It stops at nested
  packages and at `tests/`, `examples/` and `benches/`.
- Excluding a `cfg`-ruled-out module withholds only the directory the file
  owns, which keeps live neighbours and the dependency check intact.
- Glob imports reach every `cfg` alternative and see a sibling's private `use`
  only from inside that module, so `pub use a::*; pub use b::*;` no longer
  reports `b`'s items as unused. `use bun_core::{self, Output};` no longer cuts
  later `bun_core::` paths short.
- Paths and imports in files reached only through `include!` or a macro token
  stream count as references. Doctests count as uses of the documented item,
  but an item named only by its own doctest is still reported.
- Proc-macro entry points are not reported, since the compiler invokes them.
  Items under an attribute macro Deadwood cannot expand are not reported either,
  since the macro may export them. The latter hides 37 findings over 11 registry
  crates, 18 of them `#[async_trait]` items.
- A package that `include!`s an unreadable file, such as a build-generated one,
  keeps its `pub` items alive along with those of the workspace crates it
  depends on, and a warning says so. `include!` paths built from `concat!` and
  `env!` are followed where the tree or environment answers them.

On bun, resolution now completes with 8 warnings instead of 183 and reports 818
findings instead of 472. serde's `serde_derive_internals` now resolves and
surfaces five real items. tokio and ripgrep are unchanged.
