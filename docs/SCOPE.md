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

## Phase 2 — unused dependency detection (shipped)

A `Cargo.toml` entry whose crate name the declaring package's code never
mentions is reported (`src/deps.rs`), as `unused_dependency`. The semantics
that were actually implemented, since several were judgement calls:

- **Any mention counts, through any channel** — path heads, `extern crate`,
  identifiers in macro input, identifiers in attributes including words
  inside their strings, words in doc comments (doc examples are compiled as
  doctests), and dependency names in the manifest's own `[features]` table
  (`dep:foo`, `foo/bar`), which is a use with no code behind it.
- **Reachability is not required.** Unlike the other detectors, this one also
  reads `.rs` files no `mod` declaration names, because macros expand into
  `mod` declarations Deadwood never sees (`automod::dir!`). `include!` and
  `#![doc = include_str!(..)]` are followed for the same reason.
- **Kinds are reported, not scoped.** An entry is judged against every target
  of its package rather than only the ones that can legitimately see it.
  Scoping per kind would turn "declared in the wrong table" into an
  unused-dependency finding, which reads as a false positive; the message
  still names the table to edit. A dev-dependency used only in `tests/` and a
  build-dependency used only in `build.rs` are both seen, because those
  targets are scanned like any other.
- **What cannot be judged is skipped out loud**: optional and
  `[target.'cfg(...)'.dependencies]` entries (both gated by a `cfg` we do not
  evaluate — item 2 below), packages whose module tree did not resolve, and
  packages including code from a file that cannot be read.

Known gap, closed by phase 3: a dependency declared only to enable a feature
of a transitive dependency (`getrandom = { features = ["js"] }`) is named by
nothing and was reported. It is now allowlistable in the config file.

## Phase 3 — configuration file (shipped)

A `deadwood.toml`, discovered by walking up from the analyzed path to the
workspace root (or named with `--config`), carrying four settings
(`src/config.rs`): `ignore` globs, a severity per finding kind, a `public-api`
allowlist, and a dependency allowlist. The decisions that shaped it:

- **The default value of every setting is today's behavior.** No config file
  means the pre-config semantics byte for byte, which the `config` fixture
  pins by asserting the unconfigured baseline that every other case is
  measured against.
- **`ignore` suppresses findings, not evidence.** An ignored file is still
  read and its paths still count as uses; only findings *about* it are
  dropped. The alternative — excluding it from analysis outright — would make
  ignoring generated code invent unused-pub findings for everything that code
  calls, which is the exact failure the conservatism tenet exists to prevent.
  The single exception is in `src/modtree.rs`: a `mod` declaration pointing at
  a *missing* ignored file is skipped silently, since warning about it would
  skip every check for that package.
- **Severity is keyed by `FindingKind`'s own serde tags**, so a new finding
  kind is configurable the day it is added and the two spellings cannot drift.
  Only `deny` findings set the exit code; `warn` prints and exits 0; `off`
  never produces a finding at all.
- **Unknown keys are hard errors** (`#[serde(deny_unknown_fields)]`, exit 2).
  A setting that silently does nothing is worse than no setting, because the
  user believes it worked.
- **`public-api` is the noise lever the phase was for.** Running against
  `anyhow`, `clap_builder`, and `memchr` each surfaces exactly one advisory
  finding class — `pub` items with consumers outside the workspace — and a
  one-line `crates` listing silences all of it.
- Parsing needed a TOML crate (`toml`, parse-only features); nothing else was
  added. The glob matcher is ~60 lines in `src/glob.rs` rather than a
  dependency, because the four wildcards the settings need are the whole
  requirement.

Closes [#4](https://github.com/rlorenzo/deadwood/issues/4) and
[#9](https://github.com/rlorenzo/deadwood/issues/9).

## Next (sequenced, one slice at a time)

1. **`cfg` awareness** — evaluate simple `cfg(feature = ...)` / platform
   gates instead of always following them. Unblocks the optional and
   platform-gated dependency entries phase 2 skips.
2. **Baseline/suppress file** for adopting Deadwood on brownfield codebases.
   Its path is a config key waiting to be added; `src/config.rs` marks the
   spot deliberately left empty, since a key parsed and ignored is the silent
   misconfiguration phase 3 was built to prevent.
3. **Reachability over reference counting** — an item referenced only by
   other dead items is still dead; today each item is judged on whether
   anything names it, not on whether that something is alive.
4. **Lexical scope tracking** — a local, parameter, or generic parameter
   sharing a name with a module item currently resolves to that item and
   keeps it alive. Costs findings only; the fix must be namespace-aware, as
   a value binding must not silence a type of the same name.
5. **Misplaced dependency kinds** — a `[dependencies]` entry used only by
   tests belongs in `[dev-dependencies]`. Deliberately not folded into the
   unused-dependency check, whose question is whether an entry is named at
   all; this is a separate finding with its own noise profile, tracked in
   [#10](https://github.com/rlorenzo/deadwood/issues/10).

## Explicitly out of scope for now

- **Duplicate/similar-logic detection** — needs token/AST fingerprinting and
  careful noise control; deferred until the dead-code core is trustworthy.
- **Architecture analysis** (layering, cycles, module coupling metrics).
- **IDE integration, LSP, or any UI/visual reporting** — the JSON output is
  the seam where these will attach later.
- **Plugin system** — three detectors share one analysis pass and one report;
  nothing yet wants to be pluggable.
- **Semantic (type-level) analysis** via rustc internals or rust-analyzer —
  revisit once the syntactic approach hits its accuracy ceiling (tracked in
  `docs/ENVIRONMENT.md`).
- **Auto-fix / code removal** — reporting only until precision is proven.

## Design tenets

- Prefer false negatives to false positives; a noisy dead-code tool gets
  uninstalled.
- Every limitation is documented where it lives (module docs) and in the
  README.
- New dependencies only for confirmed problems; std + `syn` + `proc-macro2` +
  `serde` + `serde_json` + `clap` + `anyhow` + `toml` is the current ceiling.
  (Path resolution needed no new crate, only `syn`'s `visit` feature; unused
  dependency detection needed none at all — `cargo metadata` already reports
  the manifest. `toml` arrived with the config file in phase 3, parse-only, and
  was the only addition: glob matching stayed in-tree.)
