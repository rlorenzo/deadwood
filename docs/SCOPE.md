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
  are. Reachability landed in phase 9 below, which made the claim provable;
  phase 10 built the kind, kept the matrix axis, and wrote down why both
  exist.
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
  explanation. The known one was
  [#14](https://github.com/rlorenzo/deadwood/issues/14) — a `#[cfg(test)] mod
  tests;` whose gate lives in the parent file — closed in phase 7 below.
  Reporting only where the evidence is positive is what kept that gap from
  becoming a false positive while it was open.
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

`#[cfg(test)] mod tests { ... }` and `#[cfg(test)] mod tests;` are the same
code written two ways, and phase 5 could only see the first. The gate is
written in the parent file, so the out-of-line body arrived at the detectors
looking like ordinary library code, and a `[dependencies]` entry only the tests
named went unreported. Module resolution now carries the answer with the file
(`ParsedFile::test_only`).

- **Confinement accumulates downward and never lifts.** A module inside
  `#[cfg(test)] mod tests` is test code whatever its own gate says, so a
  declaration only ever adds to what it inherited.
- **Any ungated declaration clears it, however late it arrives.** A file two
  declarations of one crate root disagree about — `#[path]` naming one file
  from two modules — is loaded once, by whichever the queue reaches first, so
  without a rule the answer would be decided by queue order. (Two *targets*
  never collide this way: each is walked from its own root, so a file both
  compile is resolved once per target and answers per target, which is what
  attribution wants anyway.) A file wrongly marked
  test-only would move the crates it names into the dev context and could have
  a `[dependencies]` entry the library genuinely uses reported as belonging in
  `[dev-dependencies]`: a false positive, where the other direction is a missed
  finding. Clearing it late means redoing that file's `mod` walk so its
  children are cleared too, which each file can need at most once.
- **The flag says which code a file *is*, never whether it is live.** Nothing
  about reachability or dead files changed; the gate on a declaration was
  already what decided whether the file was followed at all.
- **What it found.** One new finding, in the fixture written for it. Output is
  byte-identical to phase 6 across every other fixture and the 34 crates in the
  local registry, and on Deadwood itself.

Closes [#14](https://github.com/rlorenzo/deadwood/issues/14).

## Phase 8 — lexical scope tracking (shipped)

`let helper = 5;` names a local, and until now it also marked the module's
`pub fn helper` used. `src/resolve.rs` now tracks bindings as it walks and
resolves nothing for a path one of them covers. This is the second phase that
can report *more* rather than less, and the first where a mistake in the new
code manufactures a false positive directly — a suppressed path is a use that
vanishes — so, as in phase 4, the shape was chosen for that risk.

- **Namespaces are the whole reason this was its own slice.** Rust resolves
  values and types apart, so a `let` binding shadows only *expression* paths
  and a generic parameter only *type* paths. A binding set applied by name
  alone would let `let Foo = 1;` silence the `: Foo` on the next line and
  report a live type as dead.
- **The namespace is recorded on the walker, not resolved at each parent.**
  `visit_path` sees a bare `syn::Path` and cannot tell expression position from
  type position, so the position has to arrive from the parent node. The two
  parents that establish one — `visit_expr_path` and `visit_type_path`, plus
  `visit_trait_bound`, which holds a bare `Path` — set a field that
  `visit_path` reads back. The rejected alternative was resolving the path in
  those parents instead: a dozen syn node kinds own a `syn::Path`, and each
  would then need its own copy of the `impl_self` self-reference check and the
  descent into generic arguments. Everything else keeps the position it always
  had — "neither namespace", which is never shadowed and so resolves exactly
  as before.
- **A pattern is not automatically a binding, and this was the sharpest
  edge.** `let Foo(x) = y;` and `Foo { field: x }` name a struct or a variant;
  a *bare* name is a unit-struct, unit-variant or `const` pattern when one is
  in scope and a fresh binding otherwise, and no syntax separates the two. The
  symbol table decides: only those three kinds can appear as a bare path
  pattern, so a name resolving to a `fn`, `mod`, `trait`, `enum` or `static`
  binds, and anything less certain — including a name in a module an
  unfollowable glob made opaque — is marked used and binds nothing. Rust
  agrees where it can: `let Cfg = ..;` beside `pub struct Cfg;` is rejected
  outright (E0530), so the conservative reading is the only reading of that
  program.
- **Order and scope exit are ordinary correctness, and both are pinned.** A
  `let` initializer is resolved before its pattern binds (`let x = x();` still
  names the item), a `let ... else` block before that too, and `match` arms,
  `if let`, `for` patterns and closure parameters bind inside their own body
  and nowhere else. An item nested in a function body starts from an empty
  scope: Rust rejects reaching an enclosing local or generic from there
  (E0434, E0401), so no compiling program depends on the answer, and starting
  empty is the direction that keeps the module's item alive.
- **Only a bare name is ever shadowed.** `helper::thing` names a module
  however `helper` is bound, and so does `::helper`.
- **What it found.** Not one finding changed, anywhere: byte-identical `--json`
  across all 14 existing fixtures, the 34 crates in the local registry, and
  Deadwood itself. Instrumenting the suppression showed why that is a result
  and not a no-op — 171,371 paths in the registry crates are now resolved
  against a binding instead of against the symbol table, and none of them was
  the last reference to an unreferenced `pub` item. Recall was checked the
  other way instead, as in phase 5: shadowing `strsim`'s `generic_levenshtein`
  with a local of that name reports it, and removing the shadow makes it quiet
  again. The new `scopes` fixture carries the rest, in code that compiles.

One nuance in the issue's example does *not* land here. `pub struct Cfg;` in
`pub fn entry(_c: Cfg)` is genuinely referenced by `entry`'s signature; it is
dead only because `entry` is, which is reachability — item 1 below — and not
lexical scope. The `helper` half of that example is reported.

Closes [#8](https://github.com/rlorenzo/deadwood/issues/8).

## Phase 9 — reachability, not reference counting (shipped)

A use is now recorded against the definition the naming path is written
*inside*, and an item is alive only when a walk from the root set reaches it
(`src/resolve.rs`). A dead subsystem comes out in one run instead of one layer
per run, and a dead cycle — which no number of reruns ever found, because both
halves are permanently referenced — comes out at all.

This is the first check that reports items which *are* resolved and referenced,
on the strength of a claim about the referrer, so its failure mode is a false
positive by construction rather than by bug. Every decision below is shaped by
that.

- **The report is two conditions, and dropping either was rejected.** An item
  survives when something names it *and* that something is alive. Keeping the
  first condition is what preserves every finding Deadwood made before this
  phase: a root is not exempt from it, so a library's `pub fn` that nothing in
  the workspace calls is reported exactly as it always was. Keeping the second
  is the phase. The two read apart in the message — "is never referenced by any
  resolved path" against "is referenced only from items that nothing reaches" —
  because a message saying nothing names an item that visibly has callers reads
  as a bug in the tool. Both stay one `unused_pub_item`, since they are one
  claim with two kinds of evidence, and the baseline key (kind, file, name) is
  unchanged by either.
- **The root set is the whole of the risk**, since every omission is a live
  item reported dead. It is: everything opaque; every entry point; a library's
  public surface; and whatever `[public-api]` declares.
- **Opaque means *root*, not merely reachable.** Macro input, attribute
  arguments including strings, an unfollowable glob, an alias we cannot pin
  down — all were already uses of every item with that name, and all now count
  on their own rather than on their referrer's behalf. A mention we have
  admitted we cannot read must never become evidence that something is dead.
  The same goes for a use with no definition to attribute it to: at module
  level, in an `impl` for a foreign or generic self type, inside an item nested
  in a function body.
- **"Public surface" means externally reachable, which is the line phase 1
  already drew for re-exports**: `pub`, under `pub` modules, all the way to the
  crate root of a crate something outside the workspace can name. That is what
  keeps a library's API from cascading into a page of noise, and — because
  roots are still reported when nothing names them — it costs no finding. The
  rejected reading was the narrower "only what `[public-api]` declares": on a
  library that would report the entire API and everything under it, which is
  the failure the issue named. `[public-api]` still matters, for the surface
  this rule cannot infer — an item behind a private module, or in a binary.
- **A definition that is not `pub` is an ordinary node.** Rooting private items
  would stop every cascade at the first private helper, and
  `pub fn orphan()` → `fn glue()` → `pub fn helper()` is exactly the chain
  rustc's `dead_code` cannot see and this check exists for. Where rustc *can*
  see one, it already reports it.
- **A dead cycle reports every member, not the group once.** Each member is
  separately deletable and separately located; a group finding would need a
  name, the baseline keys on names, and a group's name moves whenever a member
  joins or leaves it ([#16](https://github.com/rlorenzo/deadwood/issues/16) is
  already open on that key being weaker than it looks). Falling out of the
  per-definition rule is also what makes the answer identical run to run.
- **An `impl` block hangs off its self type and, where we can resolve it, its
  trait.** A block has no definition of its own, and nothing can call a method
  on a type nothing can name; the trait is in because dispatch through a `dyn`
  or a bound never spells the implementing type. Everything else — a foreign
  self type, a blanket `impl<T>`, a tuple, a reference — is a root.
- **Containment is not reference.** An item inside a module nothing names is
  judged on the paths that name *it*: a module can be reached through a glob,
  a `pub use`, or generated code without ever being named.
- **A `use` names its target on the bound name's behalf**, so an import nothing
  goes through no longer keeps what it imports alive. This is also why a dead
  `pub use` and the definition under it are now two findings rather than one —
  two deletions in two places, and reporting only the first is the layer-per-run
  behavior the phase exists to remove.
- **What it found.** Six new findings across the 34 crates in the local
  registry (123 → 129), not one existing finding changed, and every new one
  opened and confirmed dead by hand: four in `proc-macro2 1.0.107`, where the
  vendored `rustc_literal_escaper` module's `check_for_errors` was already
  reported and its three `check_raw_*` callees and the `Mode` enum only it
  names now come out with it; and two in `zmij 1.0.23`, where the already-
  reported `_mm_set1_epi32`/`_mm_set1_epi16` are the only callers of
  `_mm_set_epi32`/`_mm_set_epi16`. Dogfooding stays clean, before and after.
  Instrumenting the walk shows the shape of the graph rather than just its
  result: across those crates 132,017 uses are now attributed to an enclosing
  definition, against 13,087 definitions the walk had to root because the
  mention was opaque, 1,985 rooted as entry points and 1,521 as a library's
  public surface. Of the 17,705 definitions something names, reachability
  removed six.
- Recall was checked the other way as well, by mutation: fifteen inversions of
  the rules above — dropping each root clause, each half of the report, the
  `impl` owner, the `use` attribution, the opaque-is-a-root rule, the
  second-referrer edge — and all fifteen were caught by a named test.

Closes [#21](https://github.com/rlorenzo/deadwood/issues/21).

## Phase 10 — a "test-only item" finding kind (shipped)

Phase 4 wanted this kind and refused to build it, because without reachability
"only tests reach this" is really "no *non-test* path resolves here". Phase 9
built the analysis, so the honest version is available and it is one extra
traversal over an edge set that already exists: walk from the full root set,
walk again from the root set with the test entry points removed, and an item in
the first and not the second is reached only by test code.

The phase turned on a measurement rather than on a mechanism — the mechanism is
fifty lines — so the numbers are first.

- **What it found.** Five findings across the 34 crates in the local registry,
  in three of them: `clap 4.6.4`'s `pub enum Value` in `examples/find.rs`, and
  `for_each_rust_file`/`rayon_init` in `tests/repo/mod.rs` in each of
  `syn 2.0.119` and `syn 3.0.3`. Every one was opened and confirmed by hand,
  and every one is the same shape: `pub` on an item in a target nothing outside
  a test binary can name. Run on itself, Deadwood reports none. Default output is
  byte-identical everywhere — the kind ships `off`, so there is no finding to
  print — across every fixture, the registry crates and Deadwood itself, exit
  codes included. One line moved, and it is not a finding: the warning that
  names the checks an incomplete parse skips now names this kind too, because
  the same resolution pass produces both and a reader who turned the kind on
  and saw nothing would otherwise not know the check had been skipped. It
  appears in exactly one place in the corpus, the `broken` fixture, which
  exists to produce it.
- **The first run found a false positive, and that shaped the kind.**
  `winnow 0.7.15`'s `combinator::iterator` came out test-only: documented,
  doc-tested public API, reached from `pub use self::core::*;` in a `pub mod`
  over a *private* `mod core`. The surface rule follows `pub mod` chains, and a
  glob binds no name so it records no edge — so the item looked unreachable
  from anything but the crate's own tests. Modules reached through an exported
  glob are now excluded from the claim — and the `pub` modules under them,
  since a glob re-exports modules as well as functions, so `facade::nested` is
  as nameable as `facade::from_glob` and stopping at the glob's own module
  would leave the same false positive one level down. That exclusion is
  deliberately *not*
  folded into the root set: rooting those items would change what
  `unused_pub_item` says about the code naming them, which is a behavior change
  to a shipped kind and wants its own measurement. It is filed as
  [#25](https://github.com/rlorenzo/deadwood/issues/25), with the
  false-positive it already causes.
- **The overlap with rustc is the honest limit of the kind's value, and it is
  large.** For an item in a package's own `src/`, `dead_code` reports the same
  thing — as "never used" — in any build that leaves the tests out, which
  includes `cargo clippy --all-targets`, since that compiles the crate both
  ways. The `testonly` fixture is reported in full by clippy, `mentioned`
  included, which Deadwood misses. What rustc cannot report is a `pub` item in
  a test, bench or example target, because the only build that compiles one
  also uses it — and that is where all five registry findings landed. So the
  kind earns its place by being `off`: it costs nothing to a project that does
  not ask, it puts the answer in the report with everything else when asked,
  and README says plainly that a compiler is telling most projects most of this
  already.

The four decisions the issue left open.

- **Severity, and it is a first.** `off`, not `warn`. Both are quiet about the
  exit code, but `warn` *prints*, and every `#[cfg(test)]` helper in every
  codebase is a candidate — a project that installed Deadwood for dead files
  would get a page of visibility advice on its first run, which is the
  quiet-default tenet exactly. Shipping `off` also keeps the issue's own
  acceptance criterion true as written: default output is byte-identical, not
  merely equal in exit code. The cost is the honest one: `[severity]` no longer
  defaults uniformly. The default moved onto the kind
  (`FindingKind::default_severity`, an exhaustive match) rather than onto the
  `Severity` type, so phase 3's guarantee survives intact — `FindingKind::ALL`,
  `[severity]`, `ignore` and the baseline all cover the new kind with no
  plumbing of its own — and a kind added later has to state its default rather
  than inherit one. The config documentation says all of this now; claiming
  uniformity would have been the second-worst outcome after shipping `deny`.
- **What the claim is.** "Only tests reach this" is not "this is dead", and the
  message says the actionable thing: *make it `pub(crate)`, or move it behind
  `#[cfg(test)]`*. A test-only helper is frequently exactly what the author
  wants; a finding that said "delete this" about a function with visible
  callers would read as a bug. It is a separate kind rather than a flag on
  `unused_pub_item` for the same reason, and the two can never describe one
  definition: the test-only claim is only made about items that pass *both* of
  the unused check's conditions — something names it, and something live does.
- **Two mechanisms, and both stay.** `[cfg] test = false` takes the tests out
  of the build, so it takes them out of the evidence too: a dev-dependency only
  the tests name becomes an unused-dependency finding, a `#[cfg(test)]`-only
  file becomes a dead one, and the answer arrives as `unused_pub_item`, whose
  message says the item is dead. It has the better recall — it does not care
  that an `assert_eq!` names the item — and a blast radius across every
  detector. The kind keeps the tests in the build, changes no other check's
  answer, and says what to do instead. Neither subsumes the other, and README
  now carries that paragraph rather than leaving a reader to guess.
- **Opaque stays opaque.** A mention we have admitted we cannot read must not
  become evidence, so an opaque mention is a root in *both* walks and an item
  one names is never test-only. `assert_eq!(thing(), 1)` is how most tests name
  what they test, so this is not a corner: it is most of the recall the kind
  gives up, it is documented as a limitation rather than worked around, and the
  fixture carries the two shapes side by side.

The mechanism, briefly. `Def::entry_point` was a `bool` conflating `fn main`,
`#[test]`, `#[bench]`, the linker and compiler exports and the `dead_code`
opt-outs, and it splits into `EntryPoint::{None, Test, NonTest}`. The split
covers whole *targets* as well as attributes — a test, bench or example target
is code `cargo test` builds and no consumer runs, so a `harness = false` test's
`fn main` is a test root exactly as a `#[test]` function is — and reuses
`deps::is_dev_target` rather than copying the target list, since the dependency
check already asks that question about those targets. Phase 7's
`ParsedFile::test_only` is the third input. `reachable` takes a `RootSet`
instead of gaining a second copy: the difference between the two answers *is*
the claim, so they must not be able to drift. Every other root — a library's
public surface, whatever it reaches, `[public-api]`, everything opaque — seeds
both walks unchanged. The second traversal is not measurable against parsing.

Recall was checked the other way, by mutation: fifteen inversions — each half
of the entry-point split, each root clause in the second walk, the
opaque-is-a-root rule, the glob-visibility rule and its `pub` half, each
condition on the claim, and both halves of the per-kind severity default — and
all fifteen were caught by a named test — sixteen with the `pub`-children
descent, added in review. Two of them were not on the first attempt: the surface and `[public-api]` clauses were each covered twice, by a
filter and by the root set, so neither inversion was visible. The redundant
filter is gone and the tests now pin what a surface item *reaches*, which is
the part only the root set can answer.

One asymmetry is left open and measured. `#[cfg(test)] mod tests;` in a file
makes that file test code (phase 7), and an inline `#[cfg(test)] mod tests {
... }` does not — so an entry point that is neither `#[test]` nor `#[bench]`
inside an inline one (`#[allow(dead_code)]`, `#[no_mangle]`) reads as a
non-test root, and what it reaches is not test-only. The honest predicate is
`cfg::Gates::test_only`, which is per-package and lives in a module
`src/resolve.rs` has never been given, and the cheap alternative is a second
copy of a rule `src/cfg.rs` already owns. Simulating the fix with a
deliberately over-broad predicate changed **not one finding** across the
fixtures, the registry crates and Deadwood itself, so it is filed with that
number in it rather than built on a hunch
([#27](https://github.com/rlorenzo/deadwood/issues/27)).

Closes [#23](https://github.com/rlorenzo/deadwood/issues/23), and files the two
gaps it leaves: [#25](https://github.com/rlorenzo/deadwood/issues/25), a glob
re-export the public-surface rule does not follow, and
[#27](https://github.com/rlorenzo/deadwood/issues/27), the inline `#[cfg(test)]
mod` above.

## Next (sequenced, one slice at a time)

1. **Follow a `pub use` glob into the public surface**
   ([#25](https://github.com/rlorenzo/deadwood/issues/25)) — `mod inner; pub
   use inner::*;` makes `inner`'s items public API, and the reachability root
   set does not follow it, so an item whose only referrer is dead is reported
   though a consumer can name it. Phase 10 built the rule that does follow
   those globs and consults it only where it can remove a finding; moving it
   into the root set changes what `unused_pub_item` reports, so it needs a
   measurement of its own. First because it is a live false positive rather
   than a missed finding.
2. **Give a baseline entry an identity a same-named neighbour cannot share**
   ([#16](https://github.com/rlorenzo/deadwood/issues/16)) — one entry
   suppresses every finding with its key, so a second `twin` in the same file
   is accepted before it exists. The item's module path is the candidate, and
   it is a format change: older baselines carry no such field and must keep
   matching.
3. **Survive a moved file**
   ([#17](https://github.com/rlorenzo/deadwood/issues/17)) — the path is in
   the key, so `git mv` un-baselines everything in the file. Rename detection
   needs a similarity signal we do not compute, and the honest answer may be
   to document the `--prune-baseline` + `--write-baseline` workaround instead
   of guessing; weigh that before building anything.
4. **Make an inline `#[cfg(test)] mod` test code for the entry-point split**
   ([#27](https://github.com/rlorenzo/deadwood/issues/27)) — the out-of-line
   spelling is handled and the inline one is not, so the two disagree about an
   entry point that is neither `#[test]` nor `#[bench]`. Last of the filed
   entries because it was measured before it was filed: simulating the fix
   changed no finding anywhere. It wants `cfg::Gates` reachable from
   `src/resolve.rs`, which phase 4 deliberately kept out of it.
5. **Report a `[dev-dependencies]` entry the library itself names.** The
   check has never made that claim, because the likeliest explanation used to
   be a mis-attribution of ours rather than a manifest that cannot compile.
   The largest of those, an out-of-line `#[cfg(test)] mod tests;`, is closed
   ([#14](https://github.com/rlorenzo/deadwood/issues/14)), so the direction
   is now blocked on evidence of its own rather than on that gap.

Everything above except the last is filed; the roadmap and the issue list say
the same thing, so neither can quietly rot.

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
