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
  targets are scanned like any other. The wrong-table question got its own
  check and its own finding kind in phase 5 below, and this one still answers
  only whether the code names the entry at all.
- **What cannot be judged is skipped out loud**: optional and
  `[target.'cfg(...)'.dependencies]` entries (both gated by a `cfg` we did not
  evaluate then — closed by phase 4 below), packages whose module tree did not
  resolve, and packages including code from a file that cannot be read.

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

## Phase 4 — `cfg` awareness (shipped)

`cfg` gates are evaluated instead of always followed (`src/cfg.rs`). This is
the first phase that can report *more* rather than less, and the first that
could manufacture a false positive rather than merely miss a finding, so its
shape was chosen for that risk rather than for coverage.

- **A matrix, not a configuration.** Deadwood analyzes a *set* of builds and
  follows a gate that holds in at least one of them, so a predicate answers
  one of three things: holds always, holds never, or holds sometimes. The
  default matrix is the union of every possibility — every feature on and off,
  every target, tests included — which makes "follow it" the answer for every
  gate that could hold anywhere. That is byte for byte the pre-`cfg` behavior,
  and it is why an absent `[cfg]` section is a no-op.
- **Two questions, two matrices.** *Is this code in the analyzed build?* is
  judged against the configured matrix; code it rules out is not read, not
  resolved, and — the part that matters — not reported dead either, since
  "nothing reaches it" and "this build does not contain it" are different
  facts. *Can this gate hold at all?* is judged against every build there
  could be, and a gate that cannot is the new `unsatisfiable_cfg` finding.
  Keeping them apart is what makes the phase safe: an impossible gate is
  reported and its code is still analyzed, so adding a finding kind never
  moves what the other detectors see.
- **Unevaluable means follow.** `cfg(accessible(..))`, a `cfg` a build script
  sets, anything behind `cfg_attr`, and every predicate the matrix has no axis
  for (`target_arch`, `debug_assertions`) answer "sometimes", which lands on
  today's behavior at every decision point. Correlation between atoms is not
  tracked either, so `all(feature = "a", not(feature = "a"))` reads as
  satisfiable — a lost finding, never an invented one, and the alternative is
  a SAT solver nobody asked for.
- **The `cfg(test)` decision, which needed one.** Test code counts as a use by
  default, exactly as before, and `[cfg] test = false` is how a project asks
  the other question. The two rejected alternatives are worth recording.
  Leaving it always-on with no lever forecloses a question a shipping project
  legitimately has. Giving test-only items their own finding kind sounds
  better than it is: proving "only tests reach this" needs the reachability
  analysis of item 2 below, without which the claim is really "no *non-test*
  path resolves here", which is a different and weaker statement to put in a
  finding message; and any default that reports it is not the quiet default
  the tenet asks for, since every `#[cfg(test)]` helper in every codebase
  would fire. Making it a matrix axis adds no second mechanism, and the
  answers come back as ordinary `unused_pub_item` findings, which is what they
  are. Revisit once reachability lands.
- **Pruning, not plumbing.** Items the matrix rules out are removed from the
  AST in `src/modtree.rs` right after parsing, so `src/resolve.rs` and
  `src/deps.rs` never learn what a `cfg` is. The one thing that could not be
  handled that way is a file-backed `mod`: its path and the directory its
  children would live in are returned separately, so the dead-file check can
  subtract them.
- **Phase 2's skips are closed.** Optional and
  `[target.'cfg(...)'.dependencies]` entries are judged like any other,
  because the default matrix analyzes the code that uses them. Two cases
  remain unjudgeable and say so: an entry no feature in a *narrowed* matrix
  can turn on, and `[target.'cfg(any())'.dependencies]`, the idiom for an
  entry that pins a version and is deliberately compiled by nothing. Closing
  this needed one related fix — Cargo synthesizes a `foo = ["dep:foo"]`
  feature per optional dependency, and counting it as "named by the features
  table" would have left every optional entry alive by its own existence.
- **What it found.** Across the fixtures and 34 crates in the local registry,
  the only new finding was in `heck 0.5.0`: `#[cfg(feature = "unicode")]` in
  `src/train.rs`, left behind when the feature was removed. Not one existing
  finding changed, and not one newly-judged dependency entry turned into a
  false positive.

Closes [#5](https://github.com/rlorenzo/deadwood/issues/5).

## Phase 5 — misplaced dependency kinds (shipped)

A `[dependencies]` entry only tests, examples and benches reference belongs in
`[dev-dependencies]`, where it stays out of every consumer's build; a
`[build-dependencies]` entry the build script never touches is in a table
nothing reads. Both are `misplaced_dependency` findings, and both are a
different question from `unused_dependency` — which is why phase 2 refused to
answer them by scoping its own check per kind, and why nothing it reports has
moved.

The phase is entirely about noise, so the decisions are the deliverable.

- **A reference now carries where it was written.** `src/deps.rs` attributes
  every mention to runtime code (lib, bins, proc-macro), dev code (test,
  example, bench), the build script, or nowhere in particular, and a crate name
  accumulates the *set* of places it appeared in. The unused check reads the
  key set and is unchanged by construction; the placement check reads the
  values. Making the accumulation per-target was the whole mechanical change
  phase 4 left as a seam.
- **`#[cfg(test)]` is dev code wherever it sits**, which is the difference
  between a check that ships and one that fires on every crate with unit tests
  in its library. `cfg::Gates::test_only` asks whether a gate confines an item
  to a test build — against the *maximal* matrix, since where code can be
  compiled is a property of the code and not of what the user asked to analyze
  — and moves its whole subtree.
- **Doctests are why doc mentions place nothing.** A doc example links the
  normal and the dev dependencies alike, so a crate named in one is correctly
  declared under either table; and the mining is word-level anyway. Attributing
  doc words to the lib target would have reported correctly-placed
  dev-dependencies as misplaced, which is the check's second-worst failure
  mode. They land in the opaque context, which every table serves, so they can
  only silence a finding.
- **Opaque channels stay opaque, for the same reason they always were.** Macro
  input, attribute arguments, and files no `mod` declaration names keep an
  entry alive precisely because we cannot see through them; a reference we
  cannot attribute to a target cannot prove misplacement. This is most of the
  recall the check gives up, deliberately.
- **Only two claims are made.** An entry nothing names is the unused check's
  answer. A dev-dependency the library appears to name is never reported: that
  manifest does not compile, so a mis-attribution on our side is the likelier
  explanation, and the known one is
  [#14](https://github.com/rlorenzo/deadwood/issues/14) — a `#[cfg(test)] mod
  tests;` whose gate lives in the parent file (closed by phase 7 below, which
  did not make the claim reportable). Reporting only where the evidence is
  positive is what keeps that gap from becoming a false positive.
- **What it found.** Nothing, across the fixtures and the 34 crates in the
  local registry: not one finding of any kind changed, and not one new one
  appeared. Recall was checked the other way instead, by promoting four of
  `syn`'s dev-dependencies into `[dependencies]` — two were reported, and the
  two that were not are named only in a doc example and inside a
  `macro_rules!` body, exactly as designed.

Closes [#10](https://github.com/rlorenzo/deadwood/issues/10).

## Phase 6 — baseline file (shipped)

`deadwood check --write-baseline` records today's findings to a committed JSON
file; later runs subtract them and fail only on what is new
(`src/baseline.rs`). The `baseline` key finally fills the slot phase 3 left
deliberately empty in `RawConfig`.

The phase is one decision — what makes two findings "the same finding" — and
several corollaries.

- **The key is kind, file and name.** Not the line: code moves, and a baseline
  that expired whenever someone added an import above it would be worse than
  none. Not the message: it is prose that gets reworded. And **not the
  severity**, which is the one that needed arguing. Severity is a `deadwood.toml`
  decision rather than a property of the finding, so putting it in the key would
  mean that turning a check *down* from `deny` to `warn` un-baselines every
  entry of that kind and reports them all as new. The kind, in contrast, is load
  bearing: `unused_dependency` and `misplaced_dependency` name the same
  `Cargo.toml`, the same entry, and neither carries a line, so the kind is the
  only thing that separates two entirely different claims.
- **The file is the report's finding shape.** A baseline is `{"findings": [...]}`
  holding exactly the objects `--json` puts in its own array — no second format
  and no second serializer, pinned by a test comparing the two serializations
  field for field. `workspace_root` is left out because it is an absolute path
  from whichever machine ran the analysis, and `warnings` because they are not
  findings. Reading is looser than writing: only `kind` and `file` are required,
  so a hand-edited entry is a two-line object.
- **A suppressed finding leaves the report entirely** rather than appearing
  marked. The report answers "what should I act on", and reprinting the accepted
  list reproduces the day-one noise the baseline was adopted to remove — but the
  compatibility argument is the stronger one: `findings` is the JSON contract
  every consumer parses and `has_denied` is the exit code, so carrying
  suppressed entries in it would break every count and the exit code itself
  unless each consumer learned about a new field first. The suppressed count and
  the file are printed on every run, and the file is in the repository.
- **A key two findings share suppresses both.** Recording a multiplicity and
  reporting the (n+1)th as new was rejected: with the line out of the key we
  cannot say which occurrence is new, so the report would point at a line that
  is very likely baselined — a wrong finding where this is a missed one
  ([#16](https://github.com/rlorenzo/deadwood/issues/16)).
- **Stale entries are reported and never fail the run.** The exit code follows
  severity and nothing else, a fixed finding has no severity, and failing a
  build because a developer deleted dead code is how a tool gets uninstalled.
  `--prune-baseline` rewrites the file without them and records nothing new,
  which is what keeps it from being a second `--write-baseline`.
- **Missing and malformed are errors, and the split matches `--config`.** A
  path written in `deadwood.toml` must exist; the default location may simply be
  empty, which is a project that has not adopted a baseline and behaves exactly
  like a Deadwood without the feature. Reading an unreadable file as "everything
  is baselined" would turn a broken file into a permanently green run.
- **What it found.** Byte-identical output across the fixtures and the 34 crates
  in the local registry — the no-baseline path is unchanged, which is the whole
  compatibility claim. The round trip was verified on `heck 0.5.0`, whose
  genuine `unsatisfiable_cfg` was recorded, went quiet, survived being pushed 30
  lines down the file, went stale when the gate was deleted, and pruned away.

Closes [#6](https://github.com/rlorenzo/deadwood/issues/6), and files the two
gaps it leaves: [#16](https://github.com/rlorenzo/deadwood/issues/16), a key two
findings share, and [#17](https://github.com/rlorenzo/deadwood/issues/17), a
moved file un-baselining everything in it.

## Phase 7 — a `mod` declaration's gate reaches the file it names (shipped)

`#[cfg(test)] mod tests;` with the body in `src/tests.rs` was read as runtime
code: `src/modtree.rs` used the gate to decide whether to *follow* the
declaration and then forgot what it was, so the file arrived at the detectors
with no memory of how it was declared. Written inline the same module was always
attributed correctly, because `src/deps.rs` walks the item tree itself.
`ParsedFile::test_only` closes the gap, and the placement check starts such a
file's walk in the dev context — so a `[dependencies]` entry only that file
names is now reported as belonging in `[dev-dependencies]`.

Two decisions were the whole risk of the slice.

- **A file two declarations reach with different gates is decided after the
  walk, not during it.** `#[path]` aliasing (and the same file pulled into two
  targets) can name one file from a gated and an ungated declaration at once,
  and resolution reads each file once from a LIFO queue — so a flag inherited
  into the queue entry would have been whichever declaration popped first.
  Every declaration is recorded instead, and a file is test-only unless the
  crate root reaches it through a chain of declarations that are all ungated:
  plain reachability, computed once the walk is over. The rejected alternative
  was letting a repeat visit merge its flag into the already-loaded file, which
  is order-independent for that file and *not* for the children it already
  queued under the wrong flag.
- **Any ungated declaration wins.** Getting this backwards is the one direction
  that manufactures a false positive — a crate the shipping build genuinely
  links, moved into `[dev-dependencies]` — while getting it wrong the other way
  costs a finding, which is the trade every other check here makes.

The `[dev-dependencies]` → `[dependencies]` direction is deliberately still not
reported. This phase removes the largest known mis-attribution behind that
refusal without removing the others (a feature only tests turn on, a `cfg_attr`
indirection, an `include!` from another target), so it is a prerequisite for
that claim becoming honest rather than a licence to make it.

**What it found.** One new finding, in the fixture written for it: across the 14
existing fixtures and the 34 crates in the local registry the `--json` output is
byte-identical, since a test module in its own file names dev-dependencies that
are already declared as such. Recall was checked the other way, as in phase 5:
promoting `zmij`'s `num-bigint` — named only from the `src/tests.rs` behind
`#[cfg(test)] mod tests;` — into `[dependencies]` is reported, and nothing else
about that crate's findings moves.

Closes [#14](https://github.com/rlorenzo/deadwood/issues/14).

## Next (sequenced, one slice at a time)

1. **Reachability over reference counting** — an item referenced only by
   other dead items is still dead; today each item is judged on whether
   anything names it, not on whether that something is alive. Also what a
   "test-only item" finding kind would need before it could be honest; see
   the `cfg(test)` decision in phase 4.
2. **Lexical scope tracking** — a local, parameter, or generic parameter
   sharing a name with a module item currently resolves to that item and
   keeps it alive. Costs findings only; the fix must be namespace-aware, as
   a value binding must not silence a type of the same name.
3. **The `[dev-dependencies]` → `[dependencies]` direction of the placement
   check** — an entry the library itself names is in a table that does not
   compile, and phase 7 removed the largest reason to distrust such an
   attribution. What is left are the gates Deadwood cannot read; the slice is
   deciding which of them can be recognized and which have to stay a skip.

## Explicitly out of scope for now

- **Duplicate/similar-logic detection** — needs token/AST fingerprinting and
  careful noise control; deferred until the dead-code core is trustworthy.
- **Architecture analysis** (layering, cycles, module coupling metrics).
- **IDE integration, LSP, or any UI/visual reporting** — the JSON output is
  the seam where these will attach later.
- **Plugin system** — every detector shares one analysis pass and one report;
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
  was the only addition: glob matching stayed in-tree, and `cfg` evaluation in
  phase 4 was `syn` attribute walking.)
