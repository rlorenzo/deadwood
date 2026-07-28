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
- **Any declaration no gate confines to a test build clears it, however late it
  arrives.** A file two
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
  would leave the same false positive one level down. That exclusion was
  deliberately *not*
  folded into the root set here: rooting those items changes what
  `unused_pub_item` says about the code naming them, which is a behavior change
  to a shipped kind and wanted its own measurement. It was filed as
  [#25](https://github.com/rlorenzo/deadwood/issues/25), with the
  false-positive it already caused, and phase 11 below is where it moved — so
  the exclusion described here is gone, and this kind now reads the answer off
  the root set like every other.
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
([#27](https://github.com/rlorenzo/deadwood/issues/27)). Phase 14 closed it,
and on a different number: that one measures the *output*, which for a
missed-finding gap is exactly what cannot see the cost.

Closes [#23](https://github.com/rlorenzo/deadwood/issues/23), and files the two
gaps it leaves: [#25](https://github.com/rlorenzo/deadwood/issues/25), a glob
re-export the public-surface rule does not follow, and
[#27](https://github.com/rlorenzo/deadwood/issues/27), the inline `#[cfg(test)]
mod` above.

## Phase 11 — a `pub use` glob is public surface (shipped)

Phase 10 built the closure that follows a `pub use` glob onto a library's
surface and consulted it from one place, where it could only *remove* a
`test_only_item` finding. This phase makes it the surface rule itself. That
inverts the risk of every phase before it: 4, 8, 9 and 10 could report *more*
and their failure mode was a false positive, while this one reports *less* and
silences findings `unused_pub_item` makes today. So the thing to prove is not
that the new findings are right — there are none — but that **every finding
that disappears was wrong**.

- **What it found, and it is the whole safety argument.** Across the 34 crates
  in the local registry, the other 17 fixtures and Deadwood itself: **not one
  finding changed**, and no exit code changed. The registry stays at 129
  findings — the same 58 "never referenced", the same 6 "referenced only from
  items that nothing reaches" (`check_raw_str`, `check_raw_byte_str`,
  `check_raw_c_str` and `Mode` in `proc-macro2 1.0.107`; `_mm_set_epi32` and
  `_mm_set_epi16` in `zmij 1.0.23`), the same 36 dead files, 28
  dev-dependencies and 1 unsatisfiable gate. Zero `unused_reexport` findings
  exist in the registry corpus at all, before or after. The four findings that
  moved are all in the new `globs` fixture, which exists to produce them:
  three `unused_pub_item` and one `unused_reexport`, and each is an item a
  consumer writes as `facade::thing`, `facade::nested::deeper` or
  `facade::Carried`. Run with `test_only_item = "warn"` the sweep is identical
  too, which is what says the second copy of the rule was removed and not
  weakened.
- **The rule does fire on real code; it just changes no answer there.** Seven
  registry crates carry a `pub use ...::*` in `src/`. In five of them the glob
  leads outside the crate — `clap` re-exports `clap_builder`, `clap_builder`
  re-exports `anstyle`, `serde` and `serde_core` re-export `core`/`std`,
  `zmij` re-exports `core::arch::x86_64` — so it is unfollowable, which makes
  its module *opaque*, which was already a root: a no-op by construction.
  In the other two it resolves inside the crate and the surface grows:
  `anstyle 1.0.14` gains 4 modules (`color`, `effect`, `reset`, `style`) and
  `winnow 0.7.15` gains 7 (`parser` and six under `combinator`). Winnow is the
  crate the issue was written about, and the measurement there is the clearest
  statement of what this phase does and does not do: `combinator::iterator`
  stays quiet, and `backtrack_err`, `separated_foldl1`, `separated_foldr1` and
  `fill` — in the very modules just rooted — are still reported, because
  nothing names them and rooting has never exempted an item from that.

The four decisions the issue left open.

- **One rule, not two, and the wart phase 10 left is gone.** The alternative was
  a separate predicate for the glob closure, leaving `is_externally_reachable`
  answering the narrow question for `is_root` and `is_worth_reporting` while a
  second rule answered the wide one — which is exactly the drift #25's
  acceptance criteria ask to remove, so adding a third copy was never a real
  option. `SymbolTable::externally_reachable_modules` is computed once per
  report and consulted by `is_root` and by `is_worth_reporting`; what is left
  of the old per-module walk is `is_pub_to_the_crate_root`, one of the
  closure's two edges and its only caller. `test_only_definitions` consults it
  by *not* consulting it: its `!visible.contains(..)` filter is deleted, because
  a surface item is now a root in both walks and so cannot reach the
  test-only conditions. That deletion is the point — phase 10 reported two
  mutations it could not catch because a rule was covered twice, and this phase
  is about collapsing two mechanisms into one. Phase 10's
  `a_pub_module_under_a_glob_re_export_is_never_test_only` and
  `a_private_glob_import_does_not_make_its_source_externally_visible` both pass
  unchanged, and now assert the root set's rule rather than a copy of it.
- **`unused_reexport` moves with it, and it is measured on its own.** A `pub
  use` sitting in a module only a glob exports is reachable from outside for
  exactly the reason an item beside it is, so `is_worth_reporting` was
  answering the same question the same wrong way. One re-export finding changes
  anywhere in the corpus — the fixture's `Carried`, which a consumer names as
  `facade::Carried` — and the registry has none of this kind to change. The
  risk was taken seriously because a stale `pub use` is the cheapest thing in
  this tool to delete: the other half is pinned by name
  (`a_pub_use_no_glob_exports_is_still_reported_with_the_definition_under_it`),
  and the fixture reports `Stale` twice over, as the re-export and as the
  definition under it.
- **The half that does not move is condition 1, and it is most of the
  behaviour.** An item behind a glob that *nothing names* is still reported.
  That is what makes a library's whole surface reportable-but-rooted, and it is
  the difference between silencing four findings and silencing sixty-four:
  rooting changes what an item's referrers prove, never whether the item itself
  is reported. Pinned by
  `an_item_behind_a_pub_use_glob_that_nothing_names_is_still_reported`, and by
  `never_named` in the fixture — which rustc does *not* warn about, since to a
  compiler it is public API.
- **A cross-crate glob roots nothing new, so the reading costs nothing.**
  `pub use other_member::*;` is arguably right to follow — a consumer really
  can name those items through this crate — but it is a claim about a crate the
  glob's author does not own, so it was worth settling rather than assuming.
  It settles itself: the only modules of another workspace member a path can
  name are `pub` from that member's own crate root, which this rule already
  covered, so following the edge changes no answer. The fixture's `hub` member
  carries the shape and `facade`'s private modules keep their findings; the
  unit test runs the same workspace with and without the glob and asserts the
  two reports are equal.

Conservatism is unchanged, and that was a constraint rather than an outcome. A
glob Deadwood cannot follow still makes its module opaque, and opaque is still
a root in every walk — the phase only stops a *readable* mention from being
ignored, and never turns an unreadable one into evidence.

Recall was checked the other way, by mutation: nine inversions — the root set
and the re-export filter each reverted to the pub-chain rule, each of the
closure's two edges dropped, each edge's `pub` half dropped (`use` for `pub
use`, `mod` for `pub mod`), the library check, the seed set, and the `is_pub`
half of the surface root — and all nine were caught by a named test.

Closes [#25](https://github.com/rlorenzo/deadwood/issues/25).

## Phase 12 — a baseline entry a same-named neighbour cannot share (shipped)

The match key was `(kind, file, name)`, so two `pub fn twin` in two inline
modules of one file were one key: one entry suppressed both, and a third `twin`
added later was suppressed before it existed. The key now carries the item's
**module path**, and matching compares it only when both sides name one.

This is the first phase whose risk sits outside the analysis entirely. The
failure mode of the defect is *silence* — a finding suppressed before it exists
appears in no output, so counting today's output cannot see the cost — while the
failure mode of the fix lands on a format change, a migration path, and every
baseline file already committed to somebody's repository. So the measurement had
to be of the population at risk rather than of today's findings, and the
migration is most of what the phase is.

- **What is at risk, measured.** Across the 34 crates in the local registry, the
  18 fixtures and Deadwood itself there are 213 findings and **exactly one key
  shared by more than one finding** — the `twin` case in the `baseline` fixture,
  which exists to demonstrate the collision. On real code the defect occurs zero
  times, and that number is not the argument. The population at risk is every
  *reportable* `pub` item sharing a name inside one file, since two of those are
  a collision the day both become findings: **27 such groups**, 26 of them in
  registry crates and none in Deadwood itself. Not one of the 26 produces even a
  single finding today. So the defect is reachable — it is not a theoretical key
  weakness — and it is nowhere near being reached.
- **What the module path fixes, and what it cannot.** Of the 27 groups it fully
  separates **17** and cannot separate **10**, and the 10 are the more
  interesting half. Six are two `cfg`-alternative definitions of one item —
  `#[cfg(fast_arithmetic = "32")] pub type Limb = u32;` beside its `"64"` twin
  in `serde_json`, `anstream`'s `RawStream`, `anstyle-parse`'s
  `DefaultCharAccumulator`, `clap_builder`'s two `pub use ... as
  DefaultFormatter` — where Deadwood's matrix is the union of every build, so
  both halves are analyzed. One entry covering both is *right* there: it is one
  item and one fix. The other four are a type and a value sharing a name,
  `pub struct Group` beside `#[allow(non_snake_case)] pub fn Group(..)` in `syn`
  2 and 3, twice each. Those are two different items in one module, separated
  only by Rust's namespaces, which nothing in the key models. That residual is
  filed rather than guessed at
  ([#30](https://github.com/rlorenzo/deadwood/issues/30)).
- **Where the module lives, and this was the phase's real decision.** On
  `Finding`, which means `--json` grows a key. The alternative — the field on the
  baseline entry alone — breaks the invariant the whole format rests on, that an
  entry *is* the report's finding object (`entry_matches_a_report_finding_field_for_field`),
  and it would make the one field that decides matching the one field no `--json`
  output can produce, so a baseline would stop being something you can write by
  hand from a report. The cost, stated plainly: a consumer that ignores unknown
  fields sees **nothing** change — every field it reads is present, unchanged, in
  the same order, and the finding list, its order, the counts and the exit code
  are identical — while a consumer that validates against a closed schema, or
  builds a `Finding` with a struct literal, has to learn the field. Measured over
  the whole corpus, `--json` gained 127 lines and lost none, every one of them a
  `module` key: one per item finding, which is exactly the 123 `unused_pub_item`
  plus 4 `unused_reexport` findings the corpus has.
- **An absent module is not a module, and the fallback is the migration.** Only
  three of seven kinds have a module to name — a dead file is not an item, the
  two dependency kinds name a manifest entry, an unsatisfiable gate names a gate
  site — and no baseline written before this phase names one for any kind. So
  `None` means *nothing was said*, never *the crate root*, which is why the root
  is spelled `crate` and not the empty string, and modules are compared only when
  both sides name one. An entry with no module covers every finding under its
  shared key, exactly as before; a finding with no module is covered by an entry
  that names one. The forgiving direction is not a convenience: the alternative
  un-baselines every entry of every baseline in every project that upgrades
  without touching its file, which is a run failing over code nobody changed —
  the loudest possible noise, and about the tool rather than the user's code.
  `collision-baseline.json` and `all-baseline.json` are checked in as the
  previous release wrote them, and the tests reading them assert the files carry
  no `module` before asserting they suppress what they always did; a round trip
  through one version proves nothing about the version that wrote the file last
  week.
- **The reverse direction is a one-way door, deliberately — for the baselines
  the field reaches.** `Entry` rejects unknown fields, so a baseline written by
  this Deadwood makes an *older* one exit 2 on a file it read yesterday —
  ``unknown field `module`, expected one of `kind`, `severity`, `file`, `line`,
  `name`, `message` `` — verified against a binary built from `main`. It is not
  every baseline: `module` is `skip_serializing_if`, and only three kinds have
  one, so a file recording nothing but dead files, dependency entries or
  unsatisfiable gates is byte-compatible in both directions. Both halves were
  measured against that same binary rather than read off the serde attribute —
  a one-finding `dead_file` baseline round-trips through the old reader and
  suppresses, and adding a single `unused_pub_item` entry to it is what closes
  the door. Relaxing the strictness cannot fix that —
  the strict reader is the one already released — and it would cost the
  protection outright. "A setting that silently does nothing is worse than no
  setting" is a rule about config, and the objection to applying it to a *data*
  file is fair as far as it goes: an ignored decoration still leaves the entry
  matching, so the failure is noise. But `module` is not a decoration. It is part
  of the key, so an entry whose `module` a typo turned into an unknown field
  would fall back to the broad shared key and suppress the neighbour this phase
  exists to stop suppressing — silence, in the exact place the phase was about.
  The door stays one-way and is documented as one, in `README.md` and in the
  module docs.
- **The multiplicity phase 6 rejected stays rejected.** One entry still covers
  *every* finding matching its key, module included. Nothing counts occurrences,
  because with the line still unmatched we still cannot say which occurrence is
  the new one — the argument has not changed, only the number of findings a key
  gathers. The two shapes above are what that costs, and both are cases where the
  findings share a module as well as a name.
- **Conservatism, unchanged in both directions.** The module path does not move
  when code above it does, which is the test the line failed and the reason this
  field is admissible at all: every line in `modules-baseline.json` is
  deliberately wrong and every entry still matches. And nothing is un-baselined
  on a project that upgrades without touching its file.
- **What it found.** Default output is byte-identical across all 53 targets — the
  18 fixtures, the 34 registry crates and Deadwood itself — text report, stderr
  and exit codes alike, verified against a binary built from `main` in a detached
  worktree. `--json` differs only by the added key described above. The one
  behaviour that moved is the one the phase is for, and it is in the fixture
  written to produce it: `crate::beta`'s `twin` is now reported when only
  `crate::alpha`'s is recorded.

Recall was checked the other way, by mutation: fourteen inversions — each of the
three branches of the match relation, the module dropped from the entry, from the
entry's key and from the finding's key, the module left off the stale
description, `stale` and `without_stale` each reverted to exact equality, the
serialization of an absent module, the unknown-field strictness, the crate root
spelled as an empty path, the module path truncated so inline `mod`s stop
separating, and an unqualified entry made a different shared key from a qualified
one — and all fourteen were caught by a named test.

Closes [#16](https://github.com/rlorenzo/deadwood/issues/16), and files the
residual it leaves: [#30](https://github.com/rlorenzo/deadwood/issues/30), two
definitions that share a file, a name *and* a module.

## Phase 13 — a baseline entry that survives a moved file (shipped)

`git mv src/legacy.rs src/legacy/mod.rs` changes no code and no item, and the
match key included the file path, so it turned every finding in the file into a
new one and every entry recording them into a stale one. Matching now runs in
two passes: the key exactly as it was, and then — over what that pass left
unmatched **on both sides** — the identity a move preserves.

The issue said the fix needed "a similarity signal Deadwood does not currently
compute". That is true of `dead_file` and false of the item kinds, and the
number that separates them is what the phase turns on.

- **What has an identity, measured.** Across the 34 crates in the local
  registry, the 18 fixtures that predate this phase and Deadwood itself there
  are 213 findings. **127 of them — 60% — carry a module**: 123
  `unused_pub_item` and 4 `unused_reexport`. Zero are ambiguous by
  (kind, name, module), per workspace or corpus-wide — the `moved` fixture this
  phase adds is the only such group in the corpus now, and it exists to
  demonstrate the collision, exactly as phase 12's `twin` does for the key. So
  for most of the corpus an identity that survives a path change already existed
  and needed no similarity signal, no content hash and no format change. The
  other 86 split three ways: `dead_file` (39), which has no name and no module
  and genuinely needs a signal nothing computes; the manifest kinds (43), whose
  path moves only when a whole package does; and `unsatisfiable_cfg` (4), which
  names a gate site rather than an item and moves with the file it is written
  in.
- **The file is to the module path what the line is to the file, and that is the
  whole idea.** For an *item*, the file is a second name for a place the key
  already records: `crate::legacy::gone` in package `alpha` names one definition
  and the file is where you go to read it. So the second pass compares
  identities rather than similarity, and it is not rename detection — nothing
  here looks at content.
- **The file is still in the key, and dropping it was tested rather than
  assumed.** Phase 6 rejected dropping it for being too broad, phase 12's module
  path narrowed it, and whether it narrowed it *enough to stand alone* is a
  claim the corpus can answer. Over **2659 reportable `pub` definitions**,
  exactly **one** group shares a module path and a name across two files of one
  workspace: `clap`'s `pub const CLAP_STYLING`, defined at `crate` in two of its
  examples. One counterexample in 2659 is enough — the module path is
  `crate`-*relative*, so it identifies neither the package nor the target, and
  the "zero collisions among findings" number would have hidden that. The file
  stays, compared first, and is read a second time for the one thing the module
  path cannot say: which **package** the entry was recorded in, by containment
  against the workspace's manifest directories.
- **The failure direction runs the other way from every phase since 9, and that
  is what the refusals are for.** Today a move produces noise; a matcher that
  gets a move wrong produces silence. Four things hold the second pass back, and
  each is a mutation caught by a named test. It cannot reach a finding with no
  module, so `dead_file` is out of range *by construction* rather than by a kind
  list someone could extend. It runs second, so a finding the exact key matched
  is never available to it — which is why two items sharing a name and a module
  in two files are still two findings, and baselining one still leaves the other
  reported. It matches only a one-to-one pairing — exactly one leftover entry and
  exactly one leftover finding, which is the only shape a move can have — and
  refuses every other count. And it will not cross a package.
  Deliberately *not* the set semantics the exact key uses — there the unanswerable
  question is which occurrence is new, so all are covered; here it is whether a
  move happened at all, so none is.
- **One public type narrowed, which is worth saying out loud.** `Baseline` and
  its `load`/`write`/`apply` were `pub` in a `pub mod` and are now
  `pub(crate)`: `apply` needs the `Packages` index built from `cargo metadata`,
  and publishing the reader would have published that with it. Nothing outside
  the crate used them — `src/main.rs` names only `baseline::Mode` — and what a
  consumer of the library actually needs is untouched and still public:
  `Report`, the `Key`s in it, `Mode`, `FILE_NAME`. The module's surface is the
  answer a run produces, and now nothing else. Raised by Copilot in review,
  which was right that a public module holding a crate-private main type is
  incoherent; the coherent direction was down rather than up.
- **No format change, which is most of what made the phase cheap.** `module` was
  already on `Finding` and on the entry, `--json` gains nothing, and the package
  comes from `cargo metadata` rather than from the file. Phase 12 predicted this
  and it held: no second one-way door, and every baseline already committed to
  somebody's repository reads exactly as it did.
- **What it found.** Default output — no baseline file — is byte-identical across
  all 54 targets, the 19 fixtures included, text report, `--json`, stderr and
  exit codes alike, against a binary built from `main` in a detached worktree
  (`--json` too: the phase adds no key). Against that same binary, four
  of the `moved` fixture's six configs are byte-identical *with* their baselines
  too: the refusals are not new behaviour, they are the old behaviour left
  alone. Only the two configs about a move differ, and only by suppressing what
  the move preserved.
- **What is left, and it is a decision rather than an oversight.**
  `tests/fixtures/moved/unmoved.toml` pins that a moved `dead_file` and a
  dependency entry recorded against another manifest behave exactly as they did
  before this phase; `unsatisfiable_cfg` is out of range by the same rule and for
  the same reason — it names a gate site, not an item. The residual is filed
  ([#32](https://github.com/rlorenzo/deadwood/issues/32)), including the half of
  it that is cheap — a package that moves keeps its name, and only the entry not
  recording that name stops it being matchable — and the half that is not, which
  is `dead_file` and wants a content signal and its own argument. One more
  limitation is stated rather than glossed: the second pass is scoped to the
  package but not to the *target*, because no finding carries one and one file
  can belong to several, so `clap`'s two-example shape is one identity to it.

Recall was checked the other way, by mutation: eighteen inversions — the second
pass removed outright, the module and the name each dropped from the identity
requirement, the package dropped from the identity and from the lookup, a file in
no package given one anyway, the kind and the module each dropped from the
relocation, each half of the ambiguity guard loosened, the entry side and the
finding side each fed the whole set instead of the leftovers, suppression and
staleness each left un-relocated, the package directories sorted shallowest
first, keyed by manifest file rather than directory, and given one shared name,
pruning inverted to keep what it drops, and the file dropped from the exact key
so the module path stands alone. **Seventeen were caught by a named test.** The
eighteenth — the name requirement — is unreachable and is documented as such
rather than left looking like a coverage gap: every finding Deadwood produces
that carries a module also carries a name, so an entry that lost the requirement
still has nothing to pair with. That is the trap phase 10 hit, reported instead
of hidden.

Closes [#17](https://github.com/rlorenzo/deadwood/issues/17), and files the
residual it leaves: [#32](https://github.com/rlorenzo/deadwood/issues/32), the
kinds with no item identity, and a package directory that moves.

## Phase 14 — the two spellings of a `#[cfg(test)] mod` agree (shipped)

`#[cfg(test)] mod tests { ... }` and `#[cfg(test)] mod tests;` are one
construct written two ways, and the entry-point split could only see the
second. `collect_items` recursed into an inline module with the parent's
`test_context` and never read the `mod`'s own attributes, so an entry point in
one that is neither `#[test]` nor `#[bench]` was an `EntryPoint::NonTest` root
— a root in *both* walks — and what it reached could never be
`test_only_item`. Phase 7 closed exactly this asymmetry for the dependency
check.

- **The decision was whether to build it, and the number that decided it is not
  the one the issue quotes.** Phase 10 simulated the fix with an over-broad
  predicate and no finding changed anywhere. That is an *output* measurement,
  and for a missed-finding defect the output is the one thing that cannot see
  the cost. The measurement that can is the input population: how many
  non-`#[test]`, non-`#[bench]` entry points sit inside an inline
  `#[cfg(test)] mod` in code Deadwood would otherwise treat as non-test.
  Counted on the AST with `syn` and `cfg::Gates::test_only` over the 34 crates
  in the local registry, the fixtures and Deadwood itself — 1101 files — there
  are **113** inline `mod` blocks a gate confines to a test build, **103** of
  them in lib or bin source, and **8** such entry points inside them, of which
  **0** are in a non-dev target. All eight are one shape,
  `#[allow(dead_code)] type Error = ...` inside `mod test` in `winnow`'s
  `examples/`, and an example is a dev target, so `CrateUnit::test_code`
  already calls everything in it test code. A `grep` for the same thing returns
  27, most of them inside the string literals in `src/resolve.rs`'s own unit
  tests.
- **It was built anyway, and not on the finding count.** Zero in 103 blocks is
  a real answer and closing the issue with it would have been a legitimate
  outcome. What decided the other way is that two spellings of one construct
  gave two answers, which is a correctness defect whether or not a 34-crate
  registry happens to trip it; that phase 7 already paid this bill for `deps`
  and left the two checks disagreeing with each other; and — the part that
  actually changed since phase 10 — that the route below costs neither a
  lifetime nor a second copy of the predicate, so the cost that argued against
  building it in phase 10 is gone.
- **The flag comes from where it was already computed, which is the third route
  and the one the issue does not name.** `collect_mod_decls` evaluates
  `Gates::test_only` for every `mod` it walks, inline ones included,
  accumulating downward — and kept the answer only for the file-backed
  children. It now records the module paths it confined on
  `ParsedFile::test_only_mods`, beside the `test_only` flag phase 7 added for
  the out-of-line spelling. `src/resolve.rs` reads a list of paths: it still
  does not know what a `cfg` is, phase 4's "pruning, not plumbing" stands, and
  the predicate keeps one copy. Two routes were rejected. Threading a `Gates`
  onto `CrateUnit` is what `src/deps.rs` does for its own walk
  (`Origin::gates`), so it is not unprecedented — it costs a lifetime on
  `CrateUnit` and a `Gates` in every unit-test helper that builds one, for an
  answer something else has already worked out. Matching `#[cfg(test)]`
  syntactically inside `resolve.rs` is a second, weaker copy of a rule
  `src/cfg.rs` owns, and it gets the gate shapes wrong by construction.
- **What the gate shapes buy, since reusing the predicate made them free.**
  `all(test, feature = "x")` confines a module — it holds in no build without
  the tests. `any(test, unix)` does not — it holds in a build that has none.
  `not(test)` is the opposite gate and is left alone. And an ungated module
  inside a confined one is confined, because confinement accumulates downward
  and never lifts. All four are fixture cases; the last two are what a
  syntactic match gets wrong, and inverting the predicate to either rejected
  form fails a named test.
- **One expression supplies the flag, which is what keeps the two spellings
  from drifting one level down.** Four things read `test_context` —
  `entry_point_attr`, the `fn main` exemption, `add_use`, and the visitor that
  collects `use` declarations nested in item bodies — and all four are
  downstream of the single recursive call into an inline module's items, so a
  fix that reaches three of them is not expressible here. That claim is a test
  with four bodies rather than a paragraph, and each of the four is a mutation
  caught by it.
- **Two inline declarations can share a module path**, when disjoint `cfg`s
  make them alternatives, and the symbol table merges them into one module. Any
  one of them no gate confines to a test build clears the path — the same
  direction `test_only` takes for a file two declarations disagree about, and
  for the same reason: an entry point wrongly read as test code is a false
  positive, where the other direction is a missed finding. "Not confined" is
  wider than "ungated" and the fixture pins the difference: `#[cfg(all(not(
  test), unix))] mod alt` carries a gate of its own and clears the path
  anyway, because what it contributes to the merged module is production code.
- **What it found, which is the population number holding.** With
  `test_only_item` on, the change adds **no finding** on the 34 registry crates
  or on Deadwood itself; the only difference anywhere is the five new findings
  in the fixture written for this phase. Default output — the kind is `off` as
  it ships — is byte-identical across the 19 fixtures, the 34 registry crates
  and Deadwood itself, exit codes included, against a binary built from `main`
  in a detached worktree. That is the "only `test_only_item` can move" argument
  run as an experiment rather than asserted: `RootSet::Full` admits both kinds
  of entry point, so reclassifying one leaves the full walk bit-identical and
  can only shrink the `WithoutTests` one.

Recall was checked by mutation: fourteen inversions — the flag ignored
entirely, the flag withheld from each of the four readers of `test_context` in
turn, any test-only module in a file taken to confine every module in it,
confinement stopped from accumulating downward, a `mod`'s own gate dropped,
inline modules not recorded at all, the list added to rather than replaced when
a file is lifted, an unconfined alternative stopped from clearing a shared path,
every inline module recorded gated or not, and the honest predicate replaced by
each of the two rejected ones. **All fourteen were caught by a named test.**
One further mutation was written and discarded as an equivalent mutant rather
than reported as a catch: dropping the `test_only` filter from
`confined_inline_mods` while leaving the unconfined subtraction in place
removes exactly the entries it removes.

Closes [#27](https://github.com/rlorenzo/deadwood/issues/27).

## Phase 15 — a named `pub use` of a module is public surface (shipped)

Phase 11 made `SymbolTable::externally_reachable_modules` the surface rule and
followed two edges: a `pub use` **glob** to the module it names, and a surface
module to its own `pub` children. A third route reaches the same place and was
not followed — a named `pub use` whose target is a **module**. A named `pub
use` of an *item* needs no help, which is what made this easy to miss: it is a
definition of its own, a root when its module is on the surface, and reaching
it records an edge to the item it names. A module target has no item to record
an edge to, and what became nameable is everything *inside* it — a surface
question rather than an edge question.

- **The decision was whether to build it, and the direction is the opposite of
  phase 14's.** #27 lost findings; this one *invents* them, which is the
  direction this project cares about most, so a small population is worth more
  here than a large one was there. But the zero this issue quotes is a
  different animal from #27's: for a false-positive gap the **output**
  measurement is the right instrument, because the over-broad simulation
  removes at least everything the honest rule can, so "no finding changed" is
  an upper bound of zero rather than a proxy for one. That is a stronger zero
  than #27's, and closing #28 on it would have been legitimate. What decided
  the other way is the same argument phase 14 turned on: three spellings of one
  construct, two of them already handled, and the asymmetry is a correctness
  defect whether or not a 34-crate registry trips it.
- **The population, measured against `SymbolTable` rather than by a `syn`
  walk**, since the question is what a path *resolves* to. Across the 35
  library crates in the corpus — the 34 in `~/.cargo/registry/src/*/*/` plus
  Deadwood, which contributes none — there are **930** non-glob `pub use`
  leaves. 391 name an item, 458 lead outside the workspace, 20 are unresolvable,
  and **61** resolve to a module, every one of them in the same crate. **54 of
  those name a module already `pub` to its crate root**, where the closure's
  first rule covers it and the missing edge changed nothing. The at-risk
  population is the remaining **7**, in two crates: `syn` 2.0.119 and 3.0.3's
  `pub use crate::gen::{fold, visit, visit_mut};`, where `mod gen` is a private
  inline module (6 in total), and `clap_builder` 4.6.2's `pub use
  value_parser::impl_prelude;`, under a non-`pub` ancestor. (The issue's draft
  numbers were 889/60/53; the 7 agree exactly, and the 7 are what the decision
  turns on. The extra module target is `anyhow`'s `pub use anyhow as
  format_err;`, a crate naming itself, which resolves to its own crate root and
  is on the surface already.)
- **The intersection, which is the number that explains why the 7 do not
  convert.** `unused_definitions` reports when `!(used && reached)`. Surface
  membership feeds `is_root`, which feeds `reached`; it does not feed `used`.
  So the edge can only ever change a finding whose message is *"referenced only
  from items that nothing reaches"* — the second condition. Instrumenting the
  built rule, those 7 modules are exactly the 7 the closure newly reaches, and
  they hold **1138 reportable `pub` items**. Of those, **one** is reported
  today: `pub fn visit_span_mut` in `syn` 2.0.119's `src/gen/visit_mut.rs`,
  which is a *first*-condition finding ("never referenced by any resolved
  path") and which no surface rule can touch. Second-condition findings under
  the seven: **zero**. So the at-risk population for #28 is not "items under
  such a re-export" but the intersection of that with "whose only referrer is
  itself unreached", and on this corpus the intersection is empty.
- **Where the edge goes, and how wide.** A third edge in
  `externally_reachable_modules`, beside the other two: for every `pub use` in
  a module the closure already covers, resolve the target and follow it where
  `walk_path` answers `Outcome::Module`, which is the shape `resolve_globs`
  uses for the glob form. The `pub` children descent then applies underneath it
  unchanged. This is deliberately *narrower* than the simulation, which
  followed re-exports from anywhere: a `pub use` written in a module outside
  the surface roots nothing, because outside code cannot name the re-export to
  go through it. The closure reaches a **fixed point** rather than making one
  pass — the worklist follows the edge from every module it reaches, including
  ones the edge itself put there — and the two-hop fixture is what proves it:
  `lib.rs` re-exports `chain::first`, `first` re-exports `super::second`, and
  `second` is nameable only if both hops are taken.
- **The second consumer of the same set, which the issue does not mention.**
  `is_worth_reporting` reads `externally_reachable_modules` too: a `pub use` in
  a surface module is doing its job by existing and is not reported. Widening
  the set widens that as well — the same direction, reporting less, but a
  second behaviour change, and phase 11's whole point was that these two must
  not drift apart. It gets its own fixture case and its own named test, and
  two mutations pin the drift in both directions: widening the root set without
  the filter, and narrowing the root set while the filter stays wide.
- **Conservatism, and the halves that do not move.** A `pub(crate) use sub;`
  re-exports nothing outward and is not a `DefKind::Reexport` at all, so it
  roots nothing. A binary seeds the closure with no module, so none of this
  reaches one — and the `pub use` in its crate root is still reported in its
  own right. A named `pub use` of an *item* is unchanged: it is an edge, and
  reading it as a surface fact would root every item in the target's module,
  which is a fixture case and a mutation of its own. A re-export leading
  outside the workspace, or one resolution cannot follow, puts nothing
  anywhere.
- **The reproducer in #28 does not compile, and the fixtures say so.** `mod
  sub; pub use sub as api;` is E0365 — "`sub` is only public within the crate,
  and cannot be re-exported outside". The shape that compiles is a `pub` module
  under a *private ancestor*, which is what `syn` and `clap_builder` both
  carry; the two spellings are then the presence or absence of a rename, not of
  a path. Every fixture package compiles.
- **What it found.** Default output is byte-identical across the 34 registry
  crates, the 19 pre-existing fixtures and Deadwood itself, exit codes
  included, against a binary built from `main` in a detached worktree with both
  binaries pointed at the same trees. The only difference anywhere is inside
  the `reexport` package this phase added: five findings gone, three
  `unused_pub_item` (one per spelling, one two hops in) and two
  `unused_reexport` (the filter half). The over-broad simulation was re-run on
  the same corpus and is byte-identical too, on a `main` three phases newer
  than the one phase 11 measured — so its claim still holds.

Recall was checked by mutation: eleven inversions — the edge dropped entirely;
the edge followed from any module regardless of surface (the simulation); the
re-export's visibility ignored, so a `pub(crate) use` roots; a named `pub use`
of an *item* rooting the module holding it; one pass instead of a fixed point;
the target resolved from the crate root rather than from the module the
re-export is written in; the closure seeded from a binary too; the re-export
filter reverted to the narrow `pub`-chain question; the root set widened
without the filter and the filter widened without the root set; and the surface
exempted from "nothing names it" as well as from the cascade. **All eleven were
caught by a named test.** Two of them are honestly not this phase's catch and
are reported as such: seeding the closure from a binary is phase 11's shared
rule and is caught by ten tests including phase 11's own, and exempting the
surface from condition 1 is caught by thirty-three — those are rules several
mechanisms already cover, which is the trap phases 10 and 14 both hit. The
mutation that isolates this phase's half of the second one is "widen the root
set but not the re-export filter", and that is caught by this phase's tests
alone.

Closes [#28](https://github.com/rlorenzo/deadwood/issues/28).

## Next (sequenced, one slice at a time)

1. **Separate two definitions that share a file, a name and a module**
   ([#30](https://github.com/rlorenzo/deadwood/issues/30)) — phase 12's
   residual. `pub struct Group` beside `#[allow(non_snake_case)] pub fn
   Group(..)` differ by Rust's namespaces, which the key does not model; two
   `cfg`-alternative definitions of one item differ by nothing at all, and
   suppressing those together is correct. Ten groups in the corpus are at risk
   this way and none of them produces a finding today. Phase 13 did not move it:
   the second pass compares a *relocation* built from the same fields, so a key
   two definitions share is a relocation they share, and the argument there is
   unchanged. Phase 15 re-measured the population against `SymbolTable`, since
   phase 14 changed what counts as test code and the entry is owed a number
   that is current: **27** groups of `pub` items share a file and a name, the
   module path separates **17**, and the same **10** it cannot are the six
   `cfg`-alternative pairs and the four type-and-value pairs the issue lists.
   None produces a finding. Phase 14 could not have moved any of it —
   `reportable` and `is_pub` do not read the test context — and the measurement
   says so rather than the reasoning alone.
2. **Match a moved dead file, and a moved package's manifest entries**
   ([#32](https://github.com/rlorenzo/deadwood/issues/32)) — phase 13's
   residual, and last of the filed entries because its failure mode is noise
   and it has a workaround. 88 of the corpus's 219 findings name no module and
   so have no identity a move preserves — `dead_file` (40), the manifest kinds
   (44) and `unsatisfiable_cfg` (4); the issue's 86 of 213 was counted before
   phases 14 and 15 added fixtures, and the fraction is unchanged at 40%. Two
   halves with different prices: a
   package that moves keeps its *name*, so recording that name on the entry
   would close the manifest kinds and extend the item kinds across a package
   move — at the cost of a second additive field and a second one-way door,
   which is the same bill #30 is weighing. `dead_file` and `unsatisfiable_cfg`
   have nothing but a path and want a content signal, which is a different phase
   with its own argument.
3. **Report a `[dev-dependencies]` entry the library itself names.** The
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
