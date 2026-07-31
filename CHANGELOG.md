# Changelog

Notable changes per release. The full phase-by-phase record — every decision,
its rejected alternatives, the corpus measurements and mutation runs behind it
— lives in
[`docs/HISTORY.md`](https://github.com/rlorenzo/deadwood/blob/main/docs/HISTORY.md);
this file is the short version, newest first.

## Unreleased

### Fixed

Two `#[path]` resolution bugs, both found by running the analyzer over
[bun](https://github.com/oven-sh/bun)'s Rust tree — a 108-crate, ~1M-line
workspace that spells `#[path]` about eight hundred times, far past anything
in the ten-workspace corpus. Each rule below was checked by handing the
alternative spelling to rustc and watching it refuse to compile.

- A `#[path]` written inside an inline `mod` block resolved from the declaring
  file's directory rather than from the block's. `#[path = "Sub.rs"] mod sub;`
  inside `mod outer { ... }` in `src/lib.rs` names `src/outer/Sub.rs`, not
  `src/Sub.rs`. A `#[path]` on the inline block itself renames that directory,
  and resolves one level out, from the declaring file's own directory.
- A `#[path]` target only owned its parent directory when it was literally
  named `mod.rs`. Every `#[path]` target owns it — rustc treats them all as
  `mod.rs` files — so `mod child;` written in a `#[path = "body.rs"]` target
  names a *sibling*, `src/child.rs`, and not `src/body/child.rs`.

Both failures were loud rather than silent: unresolved modules raised warnings
and the affected checks skipped themselves, which is the conservative
direction but cost real coverage. On bun the first bug alone left 160 files
unresolved and took the workspace-wide unused-pub check down with them; with
both fixed, module resolution over bun is clean and the run reports 821
findings instead of 472.

Excluding a `cfg`-ruled-out module keeps the narrower of the two rules on
purpose: what leaves the build with a file is only the directory the file
*owns*, not the one it merely resolves children from. Sweeping the shared
directory would exclude live neighbours — for `#[path = "body.rs"] mod body;`
in `src/lib.rs`, the whole crate — and an excluded file is withheld from the
dependency check as well as the dead-file one, so over-reaching there would
invent unused-dependency claims rather than merely lose findings.

## 1.0.0-beta.1 — 2026-07-30

First public pre-release. The v1 check set is complete; what stands between
this and 1.0 is soak time on codebases that are not this one. Published as
the `deadwood-rs` package, since the plain crates.io name belongs to an
unrelated project; it installs a binary named `deadwood`.

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
