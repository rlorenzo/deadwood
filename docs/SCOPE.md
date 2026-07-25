# Deadwood — scope and sequencing

## Product direction

A Fallow-style codebase health tool for Rust: point it at a workspace, get a
prioritized, low-noise list of dead and unmaintained code. CLI first; output
formats stable enough (JSON) that IDE/UI layers can be added later without
reworking the core.

## v0.1 — shipped in this baseline

- Workspace discovery via `cargo metadata --no-deps` (all targets: lib, bins,
  tests, examples, benches, build scripts).
- **Dead file detection**: `src/**.rs` unreachable from any target root
  through `mod` resolution (inline mods, `mod.rs`/`name.rs` layouts,
  `#[path]`).
- **Unused pub item detection**: name-census heuristic over the whole
  workspace; conservative (false negatives over false positives); attribute
  and `fn main` escape hatches. (The census was replaced in phase 1 below;
  the conservative bias and escape hatches carried over.)
- Text + JSON reporting, CI-friendly exit codes (0 clean / 1 findings /
  2 error).
- Quality gate: fmt + clippy `-D warnings` + tests, locally
  (`scripts/check.sh`) and in CI.

## Phase 1 — path-aware usage resolution (shipped)

The name census is gone. Usage now comes from resolving paths against a
per-crate symbol table (`src/resolve.rs`): `use` declarations including
renames, nested trees and `pub use`; `crate::`/`self::`/`super::`; and
cross-crate paths between workspace members. Globs into the workspace are
expanded.

This removed three documented false-negative classes — items sharing a name
with a used item, types whose only mention is their own `impl` block, and
unused `pub use` re-exports (now their own finding kind — reported only where
outside code cannot reach them either, since a re-export on a library's
public surface is doing its job with no workspace-internal user). The
conservatism
tenet is unchanged and load-bearing: unresolvable paths (macro input,
attribute arguments, globs leading outside the workspace, ambiguity of any
kind) count as uses of every item with that name. Resolution stays syntactic
— no rustc or rust-analyzer.

## Next (sequenced, one slice at a time)

1. **Unused dependency detection** — compare `Cargo.toml` dependencies
   against `use`/path references per crate.
2. **Config file** (`deadwood.toml`): ignore globs, per-check severity,
   public-API allowlist for library crates.
3. **`cfg` awareness** — evaluate simple `cfg(feature = ...)` / platform
   gates instead of always following them.
4. **Baseline/suppress file** for adopting Deadwood on brownfield codebases.
5. **Reachability over reference counting** — an item referenced only by
   other dead items is still dead; today each item is judged on whether
   anything names it, not on whether that something is alive.
6. **Lexical scope tracking** — a local, parameter, or generic parameter
   sharing a name with a module item currently resolves to that item and
   keeps it alive. Costs findings only; the fix must be namespace-aware, as
   a value binding must not silence a type of the same name.

## Explicitly out of scope for now

- **Duplicate/similar-logic detection** — needs token/AST fingerprinting and
  careful noise control; deferred until the dead-code core is trustworthy.
- **Architecture analysis** (layering, cycles, module coupling metrics).
- **IDE integration, LSP, or any UI/visual reporting** — the JSON output is
  the seam where these will attach later.
- **Plugin system** — no third detector exists yet; premature.
- **Semantic (type-level) analysis** via rustc internals or rust-analyzer —
  revisit once the syntactic approach hits its accuracy ceiling (tracked in
  `docs/ENVIRONMENT.md`).
- **Auto-fix / code removal** — reporting only until precision is proven.

## Design tenets

- Prefer false negatives to false positives; a noisy dead-code tool gets
  uninstalled.
- Every limitation is documented where it lives (module docs) and in the
  README.
- New dependencies only for confirmed problems; std + `syn` + `serde` +
  `clap` + `anyhow` is the current ceiling. (Path resolution needed no new
  crate, only `syn`'s `visit` feature.)
