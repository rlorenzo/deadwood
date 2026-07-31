# Deadwood — phase history

The full record of every shipped phase: what it changed, the decisions and
their rejected alternatives, what it found when measured against the fixture
and registry corpus, and the mutation runs that checked recall. This is the
project's memory; [`SCOPE.md`](SCOPE.md) carries the index, the roadmap, and
the tenets.

Each phase was written down as it shipped, so prose that points at "the Next
list", "item N below" or "the list below" describes the roadmap as it stood at
the time. The live roadmap is in [`SCOPE.md`](SCOPE.md).

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

## Phase 16 — a baseline entry that names one of two same-named neighbours (shipped)

Phase 12 made the match key `(kind, file, name, module)` and filed what it could
not separate. `pub struct Group { .. }` and the `#[allow(non_snake_case)] pub fn
Group(..)` beside it agree in all four, so one entry covered both and a third
same-named item added to that module was suppressed before it existed. The key
now carries the **namespace** the definition binds its name in, which is what
Rust separates those two by.

- **The decision was whether to build it, and it is the third zero in a row.**
  #27 and #28 were measured at zero and built anyway, each on an argument that
  was not the count — two spellings of one construct disagreeing. #30 has no
  such argument: nothing here is inconsistent with itself, the key is simply
  coarser than the language. So the frame is what the measurement can and cannot
  see. Like #27 and unlike #28 the failure is a *missed* finding, and its trigger
  — a third same-named item added to that module later — appears in no output, so
  "zero findings today" is not evidence of harmlessness the way an output
  measurement of a false-positive gap is. What decided it is the second
  measurement below: the field turns out to make the key exactly as fine as
  rustc's own rule for one module, so the defect is closed rather than narrowed,
  and the cost is bounded by where the field is written. Closing #30 on the
  population would have been legitimate; this is the stronger outcome for the
  same one-way door.
- **The population, re-measured against `SymbolTable`.** The corpus resolves to
  **35** registry crates today rather than the 34 phases 12–15 measured on, plus
  the fixtures and Deadwood itself. Across it there are **3254** reportable `pub`
  definitions and **26** groups sharing a file and a name; the module path
  separates **16** and cannot separate **10**. (Phase 15 counted 27 and 17 on a
  corpus one crate older; the residual **10** is identical, item for item, and it
  is the only half this phase turns on.) Those ten are the six the issue lists —
  `serde_json`'s `Limb`, `POW5_LIMB` and `POW10_LIMB`, `anstream`'s `RawStream`,
  `anstyle-parse`'s `DefaultCharAccumulator`, `clap_builder`'s two `pub use ...
  as DefaultFormatter` — and four type-and-value pairs, `syn` 2.0.119 and 3.0.3's
  `token::Group` and `tests/debug::Lite`. **The namespace separates four of four
  and leaves six of six joined.** None of the ten produces a finding today.
- **The type-and-value count is four, not six, and the two extra are not
  reachable.** `syn`'s `pub mod parse;` at `src/lib.rs:474` and its `pub fn
  parse` at `:909` are a genuine namespace collision, and both versions have it —
  but a `mod` declaration is not *reportable* in Deadwood (`Def::reportable` is
  `false` for every one of them, deliberately, since a `mod` is a leaf in the
  edge graph), so it produces no finding, and a key needs two findings to be
  shared. `a_pub_mod_is_not_reported_so_it_shares_no_key_with_a_same_named_fn`
  puts that in the fixtures rather than in prose.
- **The third value, and why it does not make the fix partial.** A `namespace`
  cannot be one value per struct: of the **448** `pub` structs the corpus's
  module trees reach, **315** are braced and in the type namespace alone, while
  **76** tuple and **57** unit structs — **133**, 29.7% — bind a constructor
  *value* of the same name too. So the field has three values, `Both` is one of
  them, and matching is set **overlap** rather than equality: `Both` covers
  either half, which is the forgiving direction and the one that does not
  un-baseline a `pub struct Foo;` the day it gains a field. The obvious cost is
  that a `Both` definition and a `Value` one in a module still share a key — and
  that cost is zero, because rustc will not compile them together: `pub struct
  Foo;` beside `pub fn Foo()` is E0428, "`Foo` must be defined only once in the
  value namespace of this module", verified with `rustc` rather than asserted.
  Every pair that *can* be compiled together is a pair the field separates, and
  every pair it does not separate cannot be, which makes it two `cfg`-alternative
  spellings of one item — the case one entry is right for. The measurement agrees
  with the argument: not one of the four type-and-value pairs involves a tuple or
  unit struct, and all six of the joined pairs are `cfg` alternatives.
- **The half that must not move, verified rather than assumed.** All six
  `cfg`-alternative pairs have both halves in one namespace — four type aliases
  or traits (`Type`), two constants (`Value`), and `clap_builder`'s two `pub use`
  aliases (`Both` on each side, since a `use` binds whatever its target does).
  So none of them is split, and the fixture carries the same claim twice: two
  `pub type Limb`, and a unit struct opposite a function of its name, which is
  the `Both`-versus-`Value` shape written the only way that compiles.
- **The one-way door, which is the real cost, and the answer phase 12
  deferred.** `deny_unknown_fields` **stays**. The objection to it for a data
  file is fair for a decoration, and this is not one: an entry whose `namespace`
  a typo turned into an unknown field would fall back to the broad shared key and
  suppress the neighbour the field exists to stop suppressing — silence, in the
  exact place the phase is about, which is the same reasoning that kept `module`
  strict. A door is per *reader version*, so this is genuinely a second door: a
  Deadwood from between phase 12 and this one reads `module` and exits 2 on
  ``unknown field `namespace` ``. What it is not is a second *class of file*.
  Both fields are written on exactly the three kinds that name an item, so every
  baseline carrying a `namespace` carries a `module` and was already unreadable
  by anything older than phase 12, while a baseline recording only dead files,
  dependency entries and unsatisfiable gates carries neither and round-trips
  through all three. Measured, not read off the serde attribute: a `namespace`
  baseline written by this binary makes the pre-`module` binary (built from
  `de25aa2~1`) fail on ``unknown field `module` `` and `main` fail on ``unknown
  field `namespace` ``, and a `misplaced_dependency` baseline written by this
  binary is read and applied by all three.
  `namespace_is_recorded_on_exactly_the_entries_module_is` keeps the property
  that bounds it.
- **Phase 13's relocation pass: the field is not in the identity, and it reaches
  the pass twice anyway.** The relocation identity is still `(kind, package, module,
  name)`. Putting the namespace in it would answer `None` for every entry in
  every baseline already committed — a moved file would un-baseline for all of
  them — and a forgiving comparison is not available there, because a relocation
  is a hash key and "these might be the same definition" is not an equivalence.
  What the field does reach is (1) the **ambiguity guard**, which now counts
  distinct *keys* rather than distinct *files*: those were the same measurement
  until this phase, since two keys sharing a relocation and a file agreed in
  kind, name and module and so *were* one key, and counting files would have let
  one entry for the struct pair with both findings the moment the file moved —
  pass two quietly undoing what pass one was changed to do; and (2) a **check on
  the one pairing** the identity proposes, so a struct that went away and a
  function of its name that appeared are read as two events rather than one move.
  An entry that records no namespace passes both, which is what keeps every
  committed baseline relocating exactly as it did.
- **Conservatism, in the direction the phase can only lose in.** Adding to the
  key can only make entries match less, so the failure to avoid is a project that
  upgrades, touches nothing, and finds its baseline quieter than yesterday. Every
  fallback here is the forgiving one — an absent namespace is *nothing said*,
  never a namespace of its own; `Both` overlaps both halves; the relocation guard
  waves through an entry that records nothing — and
  `tests/fixtures/namespace/legacy-baseline.json` is checked in as the previous
  release wrote it: four entries naming modules and no namespaces, covering all
  seven of the fixture's findings with no edit. The module and the namespace are
  compared as a *pair* rather than as two independent sets, because an entry is
  one claim and a finding that matched one entry's module and another entry's
  namespace has been matched by neither.
- **What it found.** Default output is byte-identical across all 55 targets — the
  35 registry crates, the 19 pre-existing fixtures and Deadwood itself — text
  report, stderr and exit codes alike, against a binary built from `main` in a
  detached worktree. `--json` gained **131** lines across the same 55 and lost
  none, every one of them a `namespace` key: one per finding that already carried
  a `module`. The only behaviour that moved is in the fixture written to produce
  it, where `token::Group`'s value half is reported once its type half is
  recorded. One thing a reader sees change without changing their code: a stale
  entry that records a namespace now names it, because two stale entries under
  one name in one module were otherwise the same line printed twice.
- **What it leaves.** A `use` alias claims `Both` because nothing here resolves
  which namespaces its target occupies, so a reportable `pub use` of a braced
  struct beside a `pub fn` of that name in one module — which compiles — still
  shares a key. Zero occurrences in the corpus, and filed rather than guessed at
  ([#37](https://github.com/rlorenzo/deadwood/issues/37)).

Recall was checked by mutation: twenty-two inversions — the namespace dropped
from the finding's key, from the entry's key and from what is written; the field
recorded and never compared; an absent namespace made a value of its own and an
absent module likewise; overlap replaced by equality and by "always true"; a
struct classified by neither of its shapes, and a tuple struct made a value
alone; a `use` alias made a type; a `mod` made a value; a `fn` made a type; the
module and the namespace matched independently, and an entry made to match every
recorded qualifier rather than any; the relocation guard reverted to counting
files; the relocation pairing accepted without the namespace check; the namespace
put *into* the relocation identity; the namespace left off the stale description;
the absent namespace serialized as `null`; and the unknown-field strictness
relaxed. **All twenty-two were caught by a named test.** Two are honestly not
this phase's catch and are reported as such: the absent module is phase 12's rule
and is caught by nine tests including phase 12's own, and the unknown-field
strictness is phase 6's and is caught by three. One more is worth naming from the
other side: "a `mod` declaration binds a value" is invisible in every output,
because a `mod` is never reportable, and the only thing that catches it is the
kind-to-namespace table test written for exactly that reason.

Closes [#30](https://github.com/rlorenzo/deadwood/issues/30), and files the
residual it leaves: [#37](https://github.com/rlorenzo/deadwood/issues/37), a
`use` alias claiming both namespaces because resolution does not say which.

## Phase 17 — the identity a moved dead file does not have (measured; #32 closed, behaviour unchanged)

Phase 13 made a baseline entry survive `git mv` for the three kinds that name an
item. Four kinds have no identity a move preserves — `dead_file`, the two
dependency kinds, `unsatisfiable_cfg` — and a **package directory** that moves
defeats even the item kinds, because an entry's package is resolved by
containment and a path in no package resolves to nothing.
[#32](https://github.com/rlorenzo/deadwood/issues/32) asked for both. This phase
measured both and closed it as working as intended. No behaviour changed; the
boundary moved from prose into fixtures, and the design the next phase would
start from is written down below rather than dismissed in a line.

Every phase since 10 asked whether an unreachable defect was worth fixing. This
one is the first where the defect is real, reachable, and reproducible in three
commands, and the answer is still no. That is a different argument and it is
made on different evidence.

- **The corpus, re-measured, and which number this argues on.** Against `main`
  at `2627c75` over the 35 registry crates in `~/.cargo/registry/src/*/*/`, the
  20 fixtures and Deadwood itself — 56 targets — there are **464** findings, of
  which **326 (70%)** name no module. That number is one crate's.
  `windows-sys-0.61.2` is new to the lockfile and contributes **246 dead files
  by itself**; excluding it leaves **208** findings and **80 (38%)** naming no
  module — 40 `dead_file`, 31 `unused_dependency`, 5 `misplaced_dependency`, 4
  `unsatisfiable_cfg` — which is the shape every phase since 13 has quoted. The
  phase argues on **38%**, and the reason is not that 70% is inconvenient: all
  246 of `windows-sys`'s dead files are **false**. Its `src/lib.rs` reaches its
  whole module tree through `include!("Windows/mod.rs")`, which the module tree
  does not follow, so every file under it is reported unreachable when it is
  compiled. A population of invented findings cannot be evidence for how often a
  *real* finding is moved, and quoting 70% without saying so would be the
  misleading kind of measurement. Filed as its own defect
  ([#39](https://github.com/rlorenzo/deadwood/issues/39)) — it is the largest
  single source of false positives the corpus has ever shown, and it was found
  by measuring for something else. The roadmap's "88 of 219" predates the
  lockfile change and is replaced by the 80 of 208 above. **Phase 18 closed
  it**, and with it the qualifier: the corpus this bullet had to describe twice
  — once whole, once with `windows-sys` set aside — now has one number, 42 dead
  files of 220 findings, with nothing set aside. The 246 are not excluded, they
  are gone. The 286/40 split above is a measurement of `2627c75` and stays as
  one; it is no longer the shape of the tool.
- **Whether the package name is needed at all, which #32 assumed rather than
  checked.** It is. A manifest-kind entry already records the kind, `file:
  alpha/Cargo.toml` and `name: serde`; what is missing is which package declared
  it, and every route to that from what is recorded is a guess rather than an
  identity. Reading `[package] name` out of the recorded manifest is out by
  construction — the file is gone, which is the premise. The directory basename
  is a convention, not an identity, and it is exactly what a move may change
  (`git mv alpha crates/core`). The `message` does name the package —
  "…of package `alpha`" — and it is the one place the name is already written
  down, but it is prose, it is optional on a hand-written entry, and phase 6
  kept it out of the key precisely so that rewording a finding could not
  un-baseline it; matching on it would undo that. One thing is free, and worth
  recording for whoever picks this up: `unsatisfiable_cfg` already carries its
  package internally — `GateSites::impossible` maps a site to `(package name,
  gate)` and drops the name when it builds the `Finding` — so if a package name
  were ever worth recording, one of the four kinds needs no new plumbing for it.
  Four findings' worth, in this corpus.
- **How often a package directory moves, versus a file inside one — the two
  halves #32 treats as one feature.** A package directory only moves *within* a
  workspace, and a single-package workspace has exactly one package whose
  directory is the workspace root: the empty path, which contains every path, so
  nothing can fall outside it. **Zero of the 36 real workspaces in the corpus —
  the 35 registry crates and Deadwood itself — has a package outside the root.**
  Only 7 of the 20 fixtures do, and every one was written to ask a multi-member
  question. So the package half's population is not small, it is empty outside
  synthetic input, and the item kinds' failure under it is a failure of a case
  that does not occur here.
  `every_path_is_inside_the_root_package_of_a_single_package_workspace` puts
  that in the code rather than in this paragraph.
- **The price of the field, measured rather than reasoned about.** Phase 16
  settled `deny_unknown_fields` and it stays; what phase 16 also established is
  that a door is per *reader version* and the thing that actually matters is
  which *files* it reaches. `module` and `namespace` are written on exactly the
  three item kinds, so a baseline recording only dead files, dependency entries
  and gate sites carries neither and is portable across every Deadwood released.
  A package name — or a content signal — belongs on the kinds that have neither,
  so it is the first proposed field to reach that class. Measured against three
  binaries: a `misplaced_dependency`-only baseline written by `main` is read and
  applied by `main` and by the pre-`module` binary (`de25aa2~1`) alike, exit 0
  both times; the same file with one extra key on each entry makes **both** exit
  2 with ``unknown field `package` ``. That is the bill, and it is paid by every
  project whose baseline records a dead file — which is most of them — to buy
  back an event that produces noise.
- **The direction of failure, which is the reason this is not a close-and-move-on.**
  Every baseline phase so far moved from silence toward noise or held noise
  steady. This one would move noise toward silence: a match that fires wrongly
  suppresses a genuinely new finding. So the invariant on the table was
  `tests/fixtures/moved/unmoved.toml`'s refusal, and **it is not traded**. What
  the phase adds instead is the evidence for keeping it, in the fixture set:
  `tests/fixtures/deadfiles/unrelated.toml` records a dead file that was deleted
  beside one that still occurs, and reports a dead file that is new. That is one
  leftover entry and one leftover finding — the exact one-to-one shape the
  relocation pass accepts for an *item* — and for a dead file it is the whole of
  the evidence there is: no name, no module, two paths that share nothing. A
  content-free rule pairs them and silences the new finding. The fixture is what
  being wrong looks like, and it sits beside `pair.toml`, where both files exist
  and the exact key answers, so the difference between the two configs is only
  whether the recorded path is still there.
- **The `dead_file` half, with the design written down rather than dismissed.**
  #32 says a content signal means "adding a hashing or similarity crate", and
  that is not right: `std::collections::hash_map::DefaultHasher` needs no
  dependency and exact content equality needs no hashing at all. The hard part
  is elsewhere — **the old file is gone**, so the signal cannot be computed at
  match time and has to have been recorded when the entry was written. That is a
  field, on every `dead_file` entry, named for what it holds (`content_hash`: a
  hash of the file's bytes, not a similarity measure and not the content). The
  design, and each thing it has to survive:
  - *An entry that predates it records nothing*, and nothing said cannot pair —
    pairing on an absent signal is the guess the pass exists not to make. So the
    field is **inert on every baseline already committed** and starts working
    after the next `--write-baseline` — which is the command that re-records the
    moved paths and fixes the problem by itself. `--prune-baseline` does not
    backfill it: pruning drops entries and rewrites none.
  - *Two identical dead files hash identically* — two empty `mod.rs` stubs, two
    generated placeholders — and the pass must decline there, which the existing
    one-to-one ambiguity guard already does once the signal is part of the
    identity. Measured: **0 of the corpus's 286 dead files share content with
    another dead file in the same workspace**, so the guard would be quiet, and
    that cuts both ways — the rule would mostly work, and mostly is not the
    standard this pass is held to.
  - *A file that moved **and** was edited* has a different hash and falls back to
    noise, which is correct and is also the common shape of a refactor.
  Nothing in that is unsound. What it is not is worth a one-way door on the one
  portable class of baseline, for a failure that is noise, when the activation
  condition is the workaround. If a later phase has an argument the population
  cannot supply — the way phase 16 had one — it starts here rather than from a
  one-line dismissal.
- **The workaround, and its real cost stated rather than waved at.**
  `--prune-baseline` then `--write-baseline` re-accepts anything else that
  regressed in between, which is a genuine safety cost and the strongest thing
  #32 has going for it. The alternative is editing two or three paths by hand in
  a format built to be hand-edited, and the run prints exactly which entries went
  stale and which findings are new, so the edit is mechanical and reviewable in a
  diff. That is what `README.md` documents.
- **What it found.** Default output — no baseline file — is byte-identical
  across all 56 pre-existing targets, text report, `--json`, stderr and exit
  codes alike, against a binary built from `main` in a detached worktree: 112
  runs, 0 differing. Nothing changed but tests, fixtures and prose, and the
  measurement is here to prove that rather than to assert it.

Recall was checked by mutation: fourteen inversions — a path in no package given
a package anyway, and given one by falling back to the first package and by
matching a package directory's basename against the recorded path (the two
shapes an actual fix for #32 would take); the module dropped from the relocation
identity, and the module and the name both dropped, which is what hands a dead
file an identity; the root package stopped from holding anything; the package
directories sorted shallowest first; the ambiguity guard loosened to take the
first candidate; the entry side fed the whole baseline instead of the leftovers;
the kind dropped from the identity; the namespace check on the proposed pairing
removed; the second pass removed outright; and suppression and staleness each
left un-relocated. **All fourteen were caught by a named test.** The honest
attribution: **none is caught only by a test this phase added**, because phase 13
already defended every one of these rules at the unit level — what this phase
adds is the end-to-end pin, and the two mutations shaped like the fix #32
actually proposes, which had one unit test between them before and now have a
fixture as well. Reporting that the other way round would be claiming credit for
phase 13's coverage.

Closes [#32](https://github.com/rlorenzo/deadwood/issues/32) as working as
intended, and files what measuring for it turned up:
[#39](https://github.com/rlorenzo/deadwood/issues/39), an `include!`-ed module
tree reported dead, 246 findings in one crate.

## Phase 18 — the module tree an `include!` splices in (shipped)

`include!("Windows/mod.rs")` is not a `mod` declaration, so `src/modtree.rs`
never followed one, and every file a crate splices into its build that way was
counted unreachable and reported dead. In `windows-sys-0.61.2` that is **246
files in a crate where none is dead** — the largest single source of false
positives this corpus has ever shown, and the first entry on the roadmap that
was findings *invented* rather than missed. The module tree now follows a
literal-path `include!`, and the file it names plus everything a `mod` chain
from that file reaches is not a dead file.

**This phase went before [#37](https://github.com/rlorenzo/deadwood/issues/37),
and the roadmap said the opposite.** #39 was item 2 on the Next list and #37 was
item 1, which contradicts the rule phase 15 stated — *"#27 lost findings; this
one invents them, which is the direction this project cares about most, so a
small population is worth more here than a large one was there."* By that
rule #39 outranks #37 on both axes at once: it invents 246 findings in one
crate, while #37 misses one, in a shape that occurs **zero** times in the
corpus and needs a third same-named item in a module before it costs
anything. #37 was item 1 because it inherited the baseline sequence's
position, not because anything weighed it against #39. The list below is
resequenced, and this paragraph is why — the roadmap's ordering is a claim
like any other and it was wrong.

**Two of #39's claims were wrong, and one of its acceptance criteria rested on
one of them.** Both were checked before anything was built:

- *"A nested `include!` chain, since the real case is two deep."* It is not.
  `grep -rn 'include!' src/` in `windows-sys-0.61.2` returns exactly one line,
  `src/lib.rs:17`. The other 245 files hang off `src/Windows/mod.rs` by
  ordinary `#[cfg(feature = "Wdk")] pub mod Wdk;` declarations. The shape the
  corpus needs handled is **one `include!`, then a normal module tree under
  it** — and a fix that only marked the named file reached would have left 245
  of the 246 still reported. The nested-chain fixture is still here, because
  `src/deps.rs` caps depth and this reader has to cap it identically, but it is
  justified as a guard and not as the corpus's shape.
- *"Mark the file it names reached, with the module path of the item the
  `include!` was written in."* The second half is right and the first is not
  enough, because an included file is also a **directory-ownership root**.
  Verified against rustc, not assumed: with `include!("a/mod.rs")` in
  `src/lib.rs` and `pub mod b;` inside it, a child at `src/a/b.rs` compiles and
  a child at `src/b.rs` is `error[E0583]: file not found for module 'b'`. The
  same holds for `include!("a/gen.rs")`: the child is `src/a/b.rs`, *not*
  `src/a/gen/b.rs`, so this is a third rule and not the `mod.rs` rule wearing
  another hat.

A third number in #39 is stale rather than wrong. Its table is `2627c75`'s —
464 findings, 286 dead files — and `main` at `3eb6cfb` is **466 and 288**,
because phase 17 added the `deadfiles` fixture after taking the measurement it
quotes. Everything below is measured against `3eb6cfb`.

### The four decisions

- **What "reached" means for a spliced file, and where the two answers part.**
  An included file has two answers and they differ. *Was it reached* — yes, and
  that is what the dead-file check asks. *What module are its items in* — the
  **including** module's, because the tokens land there, so
  `include!("Windows/mod.rs")` at a crate root gives `crate::Wdk` and not
  `crate::Windows::Wdk` (rustc resolves the first and rejects the second). The
  answers part again for the file's *children*, which resolve beside the
  included file whatever it is named — so `child_base` treats every `include!`
  target as owning its directory, while `Pending::module` carries the
  includer's path. `src/modtree.rs`'s module docs say which caller wants which.
- **How far to take it, and the safe stopping point — measured before choosing.**
  Reachability only, and only for a file nothing else reaches. A file reached
  **only** through an `include!` has its items take no part in resolution:
  nothing in one is reported as an unused public item, and nothing in one keeps
  another item alive; a file a `mod` chain also reaches is analyzed exactly as
  it always was. The alternative was measured rather than reasoned about,
  by building it and running it: admitting the spliced items turns
  `windows-sys`'s **10 findings into 132,414**, all `unused_pub_item`, over a
  generated Windows API surface. That is a finding population this phase would
  have created while removing another, and it is not this phase's to create.
  The boundary is one filter in `analyze_with` rather than a missing answer —
  spliced files are parsed, gated and given their module paths exactly like the
  rest — and it is named by a test,
  `an_included_files_items_take_no_part_in_resolution`, whose control is
  `reached_both_ways`: a file both an `include!` and a `mod` chain reach *is*
  analyzed, so the test cannot pass by resolution simply not running.
- **The unreadable form stays unreadable, under one policy and not two.**
  `include!(concat!(env!("OUT_DIR"), ...))` cannot be resolved without
  building. `src/deps.rs` already answers it — skip the package with a warning
  (`INCLUDE_REASON`) rather than guess — and the module tree adds nothing: no
  warning of its own, and no exemption either. Both callers now go through one
  reader, `deps::included_file`, which returns `Included::At` or
  `Included::Unreadable` and leaves the *policy* to each; a second copy of
  "which forms are readable" is exactly the drift phases 11 and 15 had to clean
  up. A warning from the module tree would have done worse than nothing: an
  incomplete module tree skips the whole package's dead-file check, so it would
  have turned an unreadable `include!` into silence. `serde` and `serde_core`
  are written this way and contribute 36 of the corpus's dead files, every one
  of which still reports.
- **This phase removes findings, which is the opposite of the usual risk.**
  Every dead-file change so far could only add noise; this one takes 246 away,
  so the failure to avoid is exonerating a file that really is dead. A file is
  spared only when an `include!` Deadwood actually read names it, or a `mod`
  chain from such a file does. Everything the reader stops at keeps reporting:
  past the depth cap, behind a `cfg` the matrix rules out, or named by a path
  only a build knows. `tests/fixtures/included/src/attic.rs` is a plain dead
  file sitting beside a spliced tree, and it is reported.

### What it found

- **The corpus, re-measured.** Against `3eb6cfb` over the 57 targets phase 17's
  corpus has grown to — 35 registry crates in `~/.cargo/registry/src/*/*/`, 21
  fixtures and Deadwood itself — findings fall from **466 to 220** and
  `dead_file` from **288 to 42**. `windows-sys-0.61.2` reports **no dead
  file**: 256 findings become 10, and those 10 are the `unused_pub_item`
  findings it already had, unchanged.
- **And nothing else moved.** 342 output artefacts were compared against a
  binary built from `main` in a detached worktree — 57 targets × {text,
  `--json`} × {stdout, stderr, exit code}. **Two differ, and both are
  `windows-sys`'s stdout.** Every other target is byte-identical, stderr and
  exit codes included. The dead-file message itself was deliberately left
  alone: "not reachable … via `mod` declarations" is still true of every file
  that gets it, and rewording it would have changed 42 findings' text to prove
  nothing.
- **The three fixtures this phase adds** bring the corpus to 60 targets, 226
  findings and 46 dead files. Four of those dead files are boundaries being
  pinned: the wrong directory-ownership layout, a plain dead file beside a
  spliced tree, the file one past the depth cap, and the orphan beside an
  unreadable `include!`.

Recall was checked by mutation: thirteen inversions — the dead-file check
ignoring the spliced set; only the named file reached and its `mod` children
not, which is the naive fix #39 describes; an `include!` target nesting its
children in a stem-named directory, and resolving its own path from the
declaring module's base instead of the file's directory; the depth cap removed;
the module tree warning about an `include!` it cannot read, which is the second
policy the third decision exists to prevent; the spliced items admitted to
resolution; `include!` targets followed as found instead of after the `mod`
walk drains; a `cfg`-excluded `include!` followed anyway; the spliced items
placed under a module named after their file; spliced files counted as already
read by the dependency check; `include_str!` read as an `include!` by the
shared reader; and the test-build confinement of an `include!` site dropped.
**Twelve of the thirteen were caught by a named test**, all but one of them a
test this phase added — the exception is `include_str!`, which phase 2's
`unused_dependencies_are_reported_and_every_reference_channel_counts` catches
as well, because the `deps` fixture's README is spliced in that way.

The thirteenth is honest and worth naming: **dropping the test-build
confinement of an `include!` site is invisible to every output there is.** A
spliced file's `ParsedFile::test_only` is consumed by nothing, because
reachability is all a spliced file is used for and the dead-file check does not
read it. It is computed correctly anyway, so that the answer is already right
on the day the second decision's boundary moves.

**Review found a case where it was not**, which is the argument for pinning a
value no output can see. When a file is reached twice — once by a declaration
that confines it to a test build and once by one that does not — the second
reach lifts the confinement and re-walks the file to lift it from the file's
children too. That re-walk discarded its `include!` targets, on the reasoning
that the first walk had already queued them; true, but it had queued them
*confined*, and dropping the second queueing left a spliced file holding a
`test_only` its includer no longer had. The `mod` children were already
re-queued for exactly this reason — the two queues are drained through the
same repeat-reach check, and only one of them was being used. Both are now.
`lifting_a_files_test_confinement_lifts_it_from_what_the_file_includes` pins
it against `depkinds`, the fixture phase 7 built for the three-declaration
case, whose `shared_view.rs` now splices a file in beside the one it declares.
Output is unchanged — 360 artefacts over the 60 targets, byte-identical — and
that is the point: this is the one value in the phase that a test has to
defend, because nothing else does.

Closes [#39](https://github.com/rlorenzo/deadwood/issues/39).

## Phase 19 — what a `use` alias actually binds (shipped)

Phase 16 put the namespace a definition binds its name in into the baseline
match key and left one value it could not answer. An alias's namespace is not in
its own syntax — it is whatever the path it names binds — and `describe` decides
a namespace from the `syn::Item` alone while the table is still being filled. So
`DefKind::Import` and `DefKind::Reexport` recorded `Both`, which overlaps
everything. A second pass over alias definitions, after the table exists, now
walks each target with the same walker the marking pass uses and records what
the target binds. Everything it cannot be certain of keeps `Both`.

**Three of [#37](https://github.com/rlorenzo/deadwood/issues/37)'s claims were
wrong and one was stale. All four were checked before anything was built, and
the third is the reason this phase's fixture is not the one the issue asks
for.**

- *"`SymbolTable::walk_path` already answers this — an alias could take the
  target's `Def`."* It answers it, but not where the issue puts the fix. The
  namespace is decided by the free function `describe()`, from the `syn::Item`
  alone, at *index* time — before the symbol table exists, and nothing at that
  point can resolve a path. So this is a second pass over alias definitions
  after the table is built, structurally the move phase 13 made for
  relocations, and not an edit to `describe()`.
- *"A `use` group ... genuinely is `both`, and would keep recording it."* No.
  `flatten_use` splits `use inner::{Braced, plain};` into a definition per leaf
  before anything is resolved, so a group is not one question — it is one
  question per leaf, and each leaf records its own namespace.
  `every_leaf_of_a_use_group_is_resolved_on_its_own` puts that in the tests. A
  *glob* is not a question at all: it binds no name, so it is no definition and
  carries no namespace.
- **The headline example does not share a baseline key today, and the *kind* is
  why.** `pub use inner::Bar;` beside `#[allow(non_snake_case)] pub fn Bar()` is
  an `unused_reexport` finding and an `unused_pub_item` one, and the kind is the
  first field of the match key. Verified rather than reasoned about: an entry
  recording the re-export leaves the function reported on `main` at `a9dd93a`,
  unchanged. The collision is real, but it is on **`test_only_item`** — the one
  kind under which both a re-export and a definition of its name are reported —
  where an alias claiming `both` overlaps the function's `value` and one entry
  covers them both. Three commands reproduce it, and the fixture makes the claim
  there rather than where the issue points.
- *"26 groups of reportable `pub` items share a file and a name; the module
  separates 16."* One phase stale. Re-derived against `a9dd93a`: **3263**
  reportable `pub` definitions, **29** groups, the module separates **16** and
  cannot separate **13**. The three new ones are phase 16's own `namespace`
  fixture, which it added after taking the measurement it quotes; the registry
  crates' ten are identical, item for item, to phase 16's ten. Of the thirteen,
  one is a pair of `pub use ... as DefaultFormatter` aliases in `clap_builder` —
  the only alias pair in the corpus, and this phase resolves **both** of them to
  `Both`, because both targets are unit structs. It stays one entry, which is
  right: they are `cfg` alternatives.

### The four decisions

- **Whether to build it, for the fifth time the answer was not obvious, and the
  argument is not the count.** Measured, this phase narrows **one** finding's
  namespace in the whole corpus and changes no finding's text anywhere. #27, #28
  and #30 were each measured at zero and built anyway; #32 was measured and
  closed as working as intended in phase 17. Closing #37 the same way was a
  legitimate outcome and the population would have supported it. What decided it
  is that phase 17's two reasons for closing #32 both fail here, and one reason
  for building is new. #32 cost a field on the one class of baseline that is
  portable across every release, and converted a noisy failure into a silent
  one. This changes **which value** an existing field holds — the first slice in
  the sequence that adds no field and opens no door — and it moves silence
  *toward* noise, which is the direction every baseline phase before #32 moved
  in. And the fallback is the value an alias has today, so unlike #32 there is
  no new failure mode to weigh against it: the pass can only refuse back to
  where it started. Phase 16 shipped on the argument that the key should be
  exactly as fine as Rust's own rule for one module, and wrote down the one
  place where that was not yet true. This is that place, and nothing else was
  left in it.
- **What the second pass may conclude, and what it must refuse.** The direction
  is not symmetric and that is the whole design: narrowing an alias that really
  binds both halves un-baselines a finding a user has already accepted, while
  staying `Both` is exactly the behaviour they have today. So every uncertainty
  refuses, and each refusal has a test named for it — a target outside the
  workspace (`Outcome::Foreign`), one behind a glob that leads outside it
  (`Outcome::Opaque`), a chain past `MAX_ALIAS_DEPTH`, and a final segment
  naming nothing indexed (`use crate as alias;`). The depth cap is also what
  makes a cycle of mutual re-exports terminate on the conservative answer rather
  than recur, and `a_cycle_of_mutual_re_exports_stays_both` is what catches its
  removal — by aborting the test binary on a stack overflow.
- **The union, which is the trap #37's own design walks into.**
  `terminal_def` ends with `self.terminal_def_of(*reached.last()?.last()?,
  depth)`: `reached.last()` is every definition the final segment names and
  `.last()` takes one. That is right for a question any of them answers, and
  wrong here — a name binding a struct **and** a fn is the collision this phase
  exists for, and an alias to it binds both halves. Taking either end records a
  narrow namespace for an alias that is genuinely broad, which is the
  un-baselining the whole design avoids. `owner_defs` already had the right
  shape; the pass maps the whole group and combines, and
  `a_use_alias_of_a_name_binding_a_type_and_a_value_binds_both` fails on either
  pick.
- **A re-exported module is the general case, not a special one.** `pub use
  tucked::inner;` ends at `Outcome::Module`, where there is no item definition to
  read a namespace off — but the `mod` declaration the walk went *through* is
  itself a `Def`, with `DefKind::Mod` and `Namespace::Type`, so the ordinary rule
  answers it and no second route is written. Two answers for one construct is
  the drift phases 11, 15 and 18 each had to clean up, and the alternative here
  is worse than redundant: a rule reading `Outcome::Module` also fires for `use
  crate as alias;`, which names a crate root — no `mod` declaration, nothing
  indexed, and so a refusal. The test asserts against `DefKind::Mod::namespace`
  rather than against `Type`, so the two cannot be changed apart.

### What it found

- **The corpus, re-measured against `a9dd93a`.** 35 registry crates in
  `~/.cargo/registry/src/*/*/`, 24 fixtures and Deadwood itself — **60 targets,
  226 findings, 46 dead files**. 140 of those are item findings carrying a module
  and a namespace: **90** `value`, **32** `both`, **18** `type`. All five
  `unused_reexport` findings record `both`, and resolving their targets by hand
  says four of them are already right — `config`'s `Buried`, `globs`'s `Stale`
  and `paths`'s `Ignored` and `Alias` all name **unit** structs, which bind a
  constructor value of their own name. The fifth, `globs`'s `pub use
  tucked::inner;`, names a **module**. So the population this phase narrows is
  **one**, and it is the module case — not the braced struct the issue is named
  for, which occurs nowhere in the corpus.
- **And nothing else moved.** 360 output artefacts were compared against a
  binary built from `main` in a detached worktree — the 60 pre-existing targets ×
  {text, `--json`} × {stdout, stderr, exit code}. **One differs**, and it is one
  line of one `--json`: `globs`'s `inner`, `both` → `type`. Every text report is
  byte-identical, stderr and exit codes included. One finding's namespace
  changed; no finding's text did.
- **The upgrade path is absorbed by phase 16's overlap rule, and it is pinned
  from a file the previous release wrote.** A recorded `both` overlaps a reported
  `type`, so an entry a user accepted before this phase still matches the
  narrower finding — and so does the relocation pass's namespace check, which
  compares the same way. `tests/fixtures/aliases/legacy-baseline.json` is
  `a9dd93a`'s `--write-baseline` output, checked in unedited: twelve of its
  entries record `both` for an alias and **seven** of those are aliases this
  phase narrows. All 23 findings stay suppressed, nothing goes stale, and the
  file is read by the binary that did not write it.
- **The fixture the phase adds** brings the corpus to 61 targets, 243 findings
  and 46 dead files. `hidden` is one dead re-export per answer the pass can give
  — braced, unit, fn, module, a name binding a type *and* a value, a group, and
  the two refusals — so the table is in the report rather than only in a unit
  test. `shared` is the collision, on `test_only_item`, and `collision.toml`
  records the **value** half deliberately: an entry naming the type half is
  covered by an alias claiming `both` just as well, so recorded that way the file
  would answer identically on `a9dd93a` and prove nothing. Recorded this way
  `a9dd93a` reports nothing at all and this binary reports the re-export.

Recall was checked by mutation: seventeen inversions — the pass removed
outright, and computed but never written; a foreign target and an opaque one
each narrowed instead of refused; the alias-depth cap removed; the union
replaced by the last definition the final segment names and by the first (the
two shapes #37's own design takes), and two namespaces behind one name combined
to the first instead of to both; a link read off the namespace it currently
records instead of re-resolved; only a `pub use` narrowed and a plain `use` left
alone; a re-exported module refused, and answered by a route of its own; the
edition-2015 `use` fallback dropped from the walk; a refusal recorded as a narrow
namespace rather than left at `both`; the pass rewriting a concrete definition's
namespace too; and the overlap rule replaced by equality. **All seventeen were
caught by a named test.**

Two are worth naming from the other side. The depth cap's removal is caught by
`a_cycle_of_mutual_re_exports_stays_both` *aborting* — a stack overflow rather
than a failed assertion — as well as by the cap's own test failing. And the
overlap rule is honestly not this phase's catch: it is phase 16's, defended by
two of phase 16's unit tests, and this phase adds two integration tests to the
five that fail with it inverted. Everything else is caught by a test written
here.

One value in the phase is invisible in every output and is pinned anyway: a
plain `use` is narrowed exactly as a `pub use` is, and an `Import` is never
reportable, so no finding carries its namespace. It is computed because one rule
for both alias kinds leaves no second rule to drift, and
`a_plain_use_import_is_narrowed_like_a_pub_use` is the only thing that can
defend it. Phase 18's lesson is the precedent: a value no output could see was
wrong there and review found it.

Closes [#37](https://github.com/rlorenzo/deadwood/issues/37).

## Phase 20 — a `#[test]` function is test code (shipped)

`cfg::Gates::test_only` asks "is this item confined to a test build?" and
answered it by evaluating `cfg` attributes. `#[test]` is not a `cfg` — it carries
no predicate and names no configuration axis — so the answer was `false` for it,
and every mention inside a bare `#[test] fn` was read as ordinary library code.
`src/deps.rs`'s module docs stated the rule as *"`#[cfg(test)]` code is dev code,
wherever it sits"*, which was true and incomplete. This phase closes the gap:
`#[test]` and `#[bench]` confine the function they sit on, so what such a
function names is dev code with no `#[cfg(test)]` anywhere near it.

**This phase is [#44](https://github.com/rlorenzo/deadwood/issues/44), which it
filed, and it does not close
[#42](https://github.com/rlorenzo/deadwood/issues/42), which was item 1 on the
Next list.** #42 asks for a measurement before anything is built and names the
answer it expects — zero `[dev-dependencies]` entries the library appears to
name, which would be the argument for closing it. The measurement was taken and
came back *blocking*. Over the 61 targets at `bc6625f`, with #42's branch added
to `misplacement`, there are **two candidates and both are false positives**:

| package | entry | `Contexts` | why it fires |
| --- | --- | --- | --- |
| `clap_builder-4.6.2` | `static_assertions` | `RUNTIME｜DEV` | 4 mentions: 3 in bare `#[test] fn`s at module scope (`src/parser/error.rs:57`, `src/error/mod.rs:943`, `src/builder/command.rs:5292`), 1 in a `#[cfg(test)] mod tests` |
| `winnow-1.0.4` | `term-transcript` | `RUNTIME` | 1 mention, `src/combinator/debug/mod.rs:75` — a bare `#[test] fn` carrying `#[cfg(feature = …)]`/`#[cfg(unix)]` and no `#[cfg(test)]` |

So zero survive, and both fired because the class of mis-attribution #42's "What
has changed" section calls gone was not gone. #14 was one instance; this was
another, and nothing had filed it. Verified against rustc rather than reasoned
about: a bare `#[test] fn` naming a crate that does not exist compiles under
`rustc --crate-type=lib` and fails under `rustc --test` with `E0433`.

### The four decisions

- **Which issue this is for, and it is one phase rather than two.** The
  prerequisite ships alone and #42 was re-measured afterwards, because a
  measurement over a known mis-attribution is worth nothing in either direction —
  building #42 on top of `bc6625f` invents a finding in **2 of 35** registry
  crates, and closing it on that count repeats the mistake its own text warns
  against. Re-measured with #44 in, #42 has **zero candidates over all 61
  targets**. It still is not closed, and the reason is in a comment on the issue
  rather than in a silent edit: the zero is taken over a mis-attribution that is
  *known* again, one decision 3 introduces deliberately. An attribute macro that
  confines a function — `#[tokio::test]`, and the `#[core::prelude::v1::test]`
  spelling rustc honours — is not matched, so such a function is still read as
  library code. That is #42's own failure mode in a shape this corpus cannot
  measure, because it contains no instance of it. #42's entry on the Next list
  says so.
- **Where the gate lives, and where it must not.** It lives in
  `Gates::test_only`, because that is the question being asked and it has three
  callers; a second predicate beside it is the drift phases 11, 15 and 18 each had
  to clean up. It does **not** live in the `cfg` evaluator, and that was measured
  rather than argued (see below): teaching `eval` that a test attribute means
  `cfg(test)` makes `prune` delete `#[test] fn`s, which takes the only mention of
  a dev-dependency and the only reference to an item out of every detector's view.
  What is new beside the predicate is `cfg::Site`, because rustc honours a test
  attribute on a free function and nowhere else — `#[test] mod tests { .. }` and
  `#[test]` on an associated function are errors, and on a macro invocation it is
  a warning with the spliced code compiled anyway. So every caller says which kind
  of item it is asking about, and `src/modtree.rs`'s two callers — `mod`
  declarations and `include!` sites — are `Site::Other` by construction rather
  than by luck.
- **What the rule matches, and the boundary is narrower than `resolve`'s.** The
  bare, single-segment `test` and `bench`, with no arguments, on a `fn`.
  `#[bench]` is in because `cargo bench` is no more a consumer of the crate than
  `cargo test` is and rustc strips it identically; `#[should_panic]` is out
  because on its own it confines nothing (rustc resolves the body of a
  `#[should_panic] fn` under `--crate-type=lib`); a multi-segment path is out
  because nothing before expansion tells `#[tokio::test]`, which does confine,
  from an attribute macro merely named `test`, which need not. `crate::resolve`
  matches the same two names on the *last* path segment, and that is not drift:
  there an over-eager match keeps an item alive, here it moves a mention out of
  the library and invents a finding. Two questions, two rules, and both module
  docs now say why.
- **The two directions were measured separately, because they move opposite
  ways.** For the dependency check the change moves silence toward noise: fewer
  entries look correctly placed, so more get reported. For `unused_pub_item` the
  question was whether a reference from a `#[test] fn` stops counting — and the
  answer is that it cannot, structurally: `crate::resolve` has its own rule for
  `#[test]`/`#[bench]` (`TEST_ENTRY_POINT_ATTRS`, matched generously for the
  reason above) and reads `ParsedFile::test_only` for nothing else, so this phase
  changes no value it consumes. Counted rather than asserted: **zero**
  `unused_pub_item` findings move, on any of the 61 targets.

### What it found

- **The two named cases stop being candidates, and nothing replaces them.** With
  #44 in and #42's branch added, the corpus reports **zero** `[dev-dependencies]`
  entries the library appears to name — down from two, both false positives.
- **`misplaced_dependency` grows by three, all of them in the fixture.** Over the
  61 targets the corpus goes from **243 findings to 246**, `misplaced_dependency`
  from **5 to 8**, and every new one is a `depkinds` entry this phase added to
  pin the behaviour. **Nothing moves in the 35 registry crates, and nothing in
  Deadwood itself.** The registry crates' two affected entries are both
  dev-dependencies, so their mentions moved from `RUNTIME` to `DEV` without
  changing a verdict — which is the whole point of the phase and the reason its
  output diff is so small.
- **And nothing else moved at all.** 366 output artefacts were compared against a
  binary built from `main` at `bc6625f` in a detached worktree — 61 targets ×
  {text, `--json`} × {stdout, stderr, exit code}. **Two differ, and both are
  `depkinds`'s own stdout.** Every other artefact is byte-identical, stderr and
  exit codes included.
- **The rejected route was built and run, which is how it was rejected.**
  Teaching `eval_attrs` to read a test attribute as `cfg(test)` — so `compiled`,
  `prune` and `gate_sites` all obey it — changes **nothing** under the default
  matrix, because `cfg(test)` is `Either` there and nothing is pruned. Under
  `[cfg] test = false` it moves **13 of the 61 targets**: **+19
  `unused_dependency`**, **+18 `unused_pub_item`**, **−4
  `misplaced_dependency`**, across ten registry crates (`anyhow`, `clap_builder`,
  `proc-macro2`, `quote`, `serde_json`, `strsim`, `syn` twice, `winnow`, `zmij`)
  and three fixtures — because a pruned `#[test] fn` was the only thing naming a
  dev-dependency, or the only thing referencing an item. It is *consistent* with
  what `test = false` already does to `#[cfg(test)]` code, and that is precisely
  why it is a separate change with its own measurement rather than a detail of
  this one.
- **The generous boundary was measured too.** Matching the last path segment, as
  `resolve` does, moves exactly **one** finding across all 61 targets, and it is
  `depkinds`'s own `proc_macro_test_crate` — the boundary case this phase added.
  Not one library file in the 35 registry crates names a dependency only from a
  function carrying a multi-segment test attribute, so the narrow rule costs no
  recall the corpus can see.
- **The fixture is `depkinds`, extended rather than a new package.** Six entries:
  a `[dependencies]` entry named only from a bare `#[test] fn` and one named only
  from a `#[bench] fn` (both reported), a `[dev-dependencies]` entry named the
  same way (silent — the `clap_builder`/`winnow` shape), a `[dependencies]` entry
  named only from a `#[should_panic]` function and one named only from a
  `#[harness::test]` function (both silent), and a `[build-dependencies]` entry
  named only from a `#[test] fn` inside `build.rs` (silent: a build script has no
  test harness, so its test functions are build-script code like the rest of the
  file). A seventh entry, named from a bare `#[test] fn` *inside* the
  `#[cfg(test)] mod tests`, is the regression guard for the nested case: the
  module already moved its subtree, so it is reported exactly as it was before
  `#[test]` counted for anything.

Recall was checked by mutation: thirteen inversions — the test attribute not
counted at all; `#[bench]` dropped from the list and `#[should_panic]` added to
it; the match moved to the last path segment, and the bare-attribute requirement
dropped so `#[test(flavor = "…")]` counts; the site ignored, so any item honours
a test attribute; a `fn` asked about as if it were anything else, and each of the
three callers that must answer `Site::Other` — an associated function, a `mod`
declaration, an `include!` site — flipped to `Site::FreeFn`; the
dead-by-construction guard dropped, so `#[test] #[cfg(feature = "nope")] fn`
counts as test code; a test attribute read as a `cfg` predicate, which is the
rejected route; and the runtime-only restriction on the context shift dropped.
**All thirteen were caught by a named test**, twelve of them by a test this phase
added.

The thirteenth is the one worth naming, because it was invisible until a test was
written for it. Dropping the restriction that only *runtime* code shifts is
phase 5's rule rather than this phase's, and no test in the repository could see
it: the only context it can wrongly move is the build script's, and nothing
named a build-dependency from inside test-confined code. `#[test]` is a second
spelling of exactly that shape, so
`a_test_function_in_the_build_script_stays_build_script_code` now pins it from
both sides — the unit test naming the claim, and a `build.rs` entry in `depkinds`
that a mutated build reports as belonging in `[dev-dependencies]`. Phase 18's
lesson stands: an invisible mutation is a reason to write the test.

Closes [#44](https://github.com/rlorenzo/deadwood/issues/44).

## Phase 21 — the claim phase 5 refused (shipped)

`misplaced_dependency` made two claims and refused a third: a
`[dev-dependencies]` entry the *library* names. That manifest does not compile —
a library build links no dev-dependency — so it is a defect rather than a
preference, and Deadwood was silent about it from phase 5 to phase 20.

The refusal was never about the reasoning. It was that a mis-attribution of
ours was the likelier explanation, so the claim would have invented findings
against manifests that compile — the direction this project cares about most.
Both known sources are now closed:
[#14](https://github.com/rlorenzo/deadwood/issues/14) (phase 7) and
[#44](https://github.com/rlorenzo/deadwood/issues/44) (phase 20).
This phase takes the measurement #42 asked for with both closed, and makes the
claim.

### The idea this phase started with, and why it died

Phase 21 was drafted as something else: infer confinement from the manifest, so
that `#[tokio::test]` would count as test code when `tokio` is declared only
under `[dev-dependencies]`. That would have been "a way to read such an
attribute that is not syntax", which is what #42's entry said the direction
needed.

**The first experiment killed it.** An attribute macro must resolve in the build
the item is compiled into, so a dev-only macro in library code is not a case
Deadwood mishandles — it is a case that does not compile:

```console
$ cargo build   # error[E0433]: unresolved module or unlinked crate `devmac`
$ cargo test    # the same error: the plain lib target is built first
```

The inference could only ever fire on code no checked-in package contains. Built
as drafted it would have gained exactly nothing.

**It produced something better than the idea it killed.** The residual is half
the size it was written down as, and the missing half is impossible rather than
unobserved. A test-macro crate declared *only* under `[dev-dependencies]` —
which is where `rstest`, `test-case`, `serial_test` and `proptest`
conventionally go — cannot confine a function sitting in library code, because
the attribute would not resolve there. Cargo takes the kind from the table
rather than from the crate, so that is a claim about how these crates are
declared and not about what they are: the same crate listed under
`[dependencies]` puts the case back in the reachable half. What remains is a
test-confining attribute macro from a crate the library links anyway,
`#[tokio::test]` being the common spelling rather than the whole of it.
`README.md`'s
limitation now says so, and that narrowing is what made this phase's claim
defensible: the shape that can still invent a finding is one specific and rare
one, not the open-ended class the docs described.

### The decisions

**The two directions are not mirror images.** Moving an entry *down* needs every
mention to be dev code, because one library mention justifies it where it is.
Moving one *up* needs a single runtime mention, because the library cannot link
the entry at all — one such mention is a build that fails, however much test
code names it too. Flattening that into one rule is the way to make this check
noisy, and
`one_runtime_mention_places_a_dev_dependency_however_much_test_code_names_it`
is what defends it.

**Build-script evidence places nothing.** A dev-dependency only `build.rs` names
is in the wrong table too, but build-script evidence does not say which of the
other two tables it belongs in, and a placement claim that cannot name a table
is not a claim. Silent, deliberately.

**No new kind, no new field.** A third message on `misplaced_dependency`, which
already carries a `file` and a `name` and no `module`, so no baseline field and
no new configuration surface.

### What it found

- **Zero movement on real code.** 366 artefacts over the 61 targets ×
  {text, `--json`} × {stdout, stderr, exit code} against a binary built from
  `main` at `199b97e`: the only differences are `depkinds`'s own stdout and
  JSON, from the fixture entries added here. **Nothing moves in any of the 35
  registry crates, nor in Deadwood itself.** That is the measurement #42 asked
  for, taken with the mis-attributions closed.
- **Recall, checked the way phase 5 checked it.** Four of Deadwood's own
  `[dependencies]` demoted into `[dev-dependencies]`: **two reported, two not**,
  and the two misses are exactly the opaque channel doing its job — `syn` has
  eight doc-comment mentions and `serde_json` two, and one opaque mention
  anywhere stops an entry being judged. `anyhow` and `proc-macro2` have none and
  are reported. The same two-of-four shape phase 5 got, for the same reason.
- **That guard costs a finding**, and it is now pinned rather than incidental:
  `doc_and_library_dev_crate` is named by library code *and* by a doc comment,
  and is not reported even though the library mention alone would place it. It
  survived the first mutation run, which is how it got a fixture entry.

**Recall by mutation: eight inversions, all eight caught by a named test.** Two
survived the first run — the opaque guard above, and a message that dropped its
direction and read as the *other* claim while still passing a test that only
checked the table names. Both have tests now.

Closes [#42](https://github.com/rlorenzo/deadwood/issues/42).

## Phase 22 — a crate is not always spelled like its entry (shipped)

The dependency check matches a mention against a manifest entry by *spelling*.
`extern crate real as alias;` breaks that: the rename binds `alias` for the
crate that declares it, so every later `alias::` means `real` — and every one
of them was being charged to whatever entry happened to be spelled `alias`.

Until phase 21 that cost nothing visible. With #42's claim shipped it invents a
`misplaced_dependency` against a manifest that compiles, which is the direction
this project ranks above all others.

### It was found by measuring, not by reading

The phase began as a survey of what to do next, and the candidate on the list
was relaxing the opaque guard
([#50](https://github.com/rlorenzo/deadwood/issues/50)):
it costs real findings, two of four in phase 21's own recall check. Measured
corpus-wide, the guard blocks exactly **two** placement claims — one a
`depkinds` fixture entry, and one `serde_json`'s `serde`.

That second one turned out not to be a missed finding at all:

```console
$ grep -n "serde_core as serde" src/lib.rs
382:extern crate serde_core as serde;
```

`serde_json` carries `serde_core` in `[dependencies]` and `serde` in
`[dev-dependencies]`, and every `serde::` in its library is `serde_core`. The
opaque guard was the only thing stopping phase 21 reporting it — unrelated doc
comments mentioning `serde_bytes` and `serde_stacker`. Delete one of those doc
comments and the false positive appears.

So the candidate was disqualified by its own measurement — its entire real
population was a bug it was accidentally masking — and the bug it was masking
became this phase.

### The decision that took two attempts

**The fold is per target, not per package**, and getting that wrong is worse
than not fixing anything. The first cut folded a package's aliases across all
of its mentions, which is wrong for the same reason the rename is interesting:
a test target is a *separate crate* that links the dev-dependencies directly,
so `src/lib.rs` renaming `serde_core` to `serde` says nothing about what
`tests/it.rs` means by `serde`. That version reported `serde_json`'s `serde` as
an **unused dev-dependency** — a worse false positive than the one being fixed,
on the same crate.

It was caught by running the corpus, not by thinking harder; `main` reports
nothing on `serde_json` and the first cut reported something, which was the
whole of the signal.

**Both spellings count.** `use real as alias;` binds a crate exactly as
`extern crate real as alias;` does, and is the edition-2018 way to write it. It
was very nearly left out — the `README.md` paragraph for this phase originally
claimed the `use` form "costs a finding rather than inventing one", which a
two-minute check disproved: it invents the same one. Written down before it was
run, that sentence would have shipped as documentation of behaviour the code
did not have.

**Where it stops, which took a review comment to get right.** `use
crate_name::Item as Alias;` renames an item, not a crate: the head of the path
is still the crate. The scoping was the harder half. The first version folded a
rename across the whole target for both spellings, and the write-up claimed
that over-reach "loses a finding rather than inventing one" — which was
untested and false. Two cases prove it: a `use` rename inside a nested `mod`,
and one at a crate root whose alias is named in a submodule file. Both fold
away mentions of a crate that is genuinely used, and both report that crate's
entry as **unused**.

So the rule follows what actually binds, and it took a second review comment to
get the last of it right. `extern crate real as alias;` **at a crate root**
enters the extern prelude and holds for the whole target — the `serde_json`
case, where the rename is in `lib.rs` and the mentions are in `value/ser.rs`.
The first version of this rule tested only that the item sat at the top of *a*
file, which is also true of `src/foo.rs`, where `extern crate` is an ordinary
item of module `foo` and binds no wider than a `use` does. That version folded
a module file's rename across the whole target and reported a crate the root
genuinely uses as unused: the same invented finding, one layer down.

The crate root is now the test — `ParsedFile::module` is empty for exactly that
file — and everything else is file-scoped: a `use` rename anywhere, and an
`extern crate` rename outside the root. Neither counts inside a nested `mod`.

### What it found

- **Zero movement on real code.** 366 artefacts over the 61 targets against a
  binary built from `main` at `8726ed7`. Two differ, both `depkinds`'s own
  stdout and JSON, and the difference *is* the fix: `main` reports the invented
  `aliased_crate` finding on the new fixture content and this does not. Nothing
  moves in any of the 35 registry crates, nor in Deadwood itself.
- **`serde_json` is byte-identical to `main`**, which is the case the per-target
  fold exists for.

**Recall by mutation: twelve inversions, all twelve caught by a named test.**
Four of those twelve exist because two review comments on the pull request
asked about alias scope, and building the cases they described showed the
over-reach invented findings rather than losing them. The fixture grew four
pairs for it — a `use` rename inside a `mod`, an `extern crate` rename inside
one, a crate-root rename whose alias is named in a submodule file, and an
`extern crate` rename at the top of a module file — and each pair is one crate
that would be reported unused without the scoping.

A ninth mutation was written and then deleted rather than defended: a guard
skipping `extern crate real as real;` turned out to be an equivalent mutant —
removing a key and re-inserting the same contexts under the same name leaves
the map as it was — so the branch went, and the reasoning stayed as a doc
comment. A branch no test can distinguish is not a branch worth keeping.

Closes [#48](https://github.com/rlorenzo/deadwood/issues/48).

## Phase 23 — an item an attribute macro owns is macro input (shipped)

Phase 20 matched the built-in `#[test]`/`#[bench]` exactly and refused to
guess about anything else, and phase 21 shipped its claim on the strength of
that refusal. Together they left one shape inverted: a `[dev-dependencies]`
entry named only from a `#[tokio::test] fn` in library code was reported as
belonging in `[dependencies]` — the macro expands to the built-in `#[test]`,
no build a consumer gets compiles the function, and the manifest the finding
indicts is correct. A finding *invented*, the direction this project ranks
above all others, which is why it went first on the Next list with no corpus
instance to its name.

### The decisions

- **Opacity, not the allowlist.** Issue #49 allowed two answers: treat an item
  under an unexpandable attribute macro as opaque, or leave the shape to the
  `[dependencies]` allowlist in `deadwood.toml`. The allowlist answer means
  the tool knowingly reports a manifest that compiles and asks the user to say
  so — tenable for a shape with no corpus instance, but the wrong default for
  the failure direction the project ranks first, and the opaque state needed
  no new machinery: "known used, unknown where" is exactly what the mention
  under such a macro is. The whole of the work was the boundary.
- **The boundary: what is *not* a macro.** An attribute macro receives its
  item as tokens and may emit anything, so the recognizer
  (`crate::cfg::unexpandable_macro`) asks the opposite question — which
  attributes provably rewrite nothing. Three exclusions: the built-in
  attributes (a fixed list from the Reference's index, with `unsafe` on it for
  the `#[unsafe(no_mangle)]` wrapper), the reserved tool namespaces
  (`rustfmt`, `clippy`, `diagnostic`), and an unknown single-segment attribute
  on an item that also carries `#[derive(..)]` — a derive helper is only legal
  beside its derive, is inert, and cannot be told from an attribute macro by
  spelling, so it is read as the helper it almost always is. Everything else
  can be nothing but an attribute macro on stable rustc: a multi-segment
  non-tool path (`#[tokio::test]`, `#[core::prelude::v1::test]`) or an
  underived unknown single segment (`#[rstest]` brought in by `use`). A
  built-in the list is missing degrades in the safe direction — its item goes
  opaque, which costs a claim and cannot invent one.
- **Runtime code only**, and not out of caution: expansion happens inside one
  crate, so whatever a macro leaves of an item in a dev target or a build
  script compiles into that same target, and the attribution written there
  holds whatever the macro does. Only in runtime code can expansion move an
  item out of every build a consumer gets. Blanket opacity would have silently
  un-judged a `[dependencies]` entry named only from a `#[tokio::test] fn` in
  `tests/` — a real finding the fixture now pins (`attr_macro_test_target_crate`).
- **The gate is judged before the macro.** `#[cfg(test)] #[tokio::test] fn` is
  test code by its gate, which is written in the syntax and holds whatever the
  macro emits. Opacity there would trade a known answer for an unknown one.
- **`cfg_attr` stays unfollowed**, and for this recognizer the refusal is also
  simply correct: in every build whose predicate does not hold, the item is
  compiled exactly as written, so its mentions are attributable to the code
  they sit in.

### What it found

- **Zero movement on real code.** 342 artefacts over the 57 targets present in
  this environment (31 of the 35 lockfile registry crates — the four absent
  are Windows-only dependencies never unpacked on this machine — the 25
  fixtures, and Deadwood itself) against a binary built from `main` at
  `5bf0e2d`. Two differ, both `depkinds`'s own stdout and JSON, and the
  difference *is* the fix: `main` reports the four invented findings on the
  new fixture entries — the `#[tokio::test]` shape, its single-segment and
  `core::prelude::v1` spellings, and the `impl`-member case — and this does
  not. The corpus claim behind phase 21 still holds: its `src/` trees carry
  `target_feature`, `deprecated` and `rustfmt::skip`, all on the inert side of
  the boundary, and no unexpandable attribute macro.
- **An extended sweep, beyond the canonical corpus:** this environment's
  registry holds 330 unpacked crates, not 35. Both binaries were run over
  every one of them — JSON output and exit code — and all 330 are
  byte-identical. The recognizer's inert side (built-ins, tool namespaces,
  derive helpers) covers everything real library code carried.

**Recall by mutation: eight inversions, all eight caught by a named test.**
Dropping the opaque shift, applying it in every context, judging the macro
before the gate, guessing `DEV` instead of opaque, giving macro-ownership a
site, treating every underived single segment as a macro, sweeping the tool
namespaces in, and dropping the derive-helper exemption. The runtime-only
guard turned out to be load-bearing for the existing `DEV` shift as well —
removing it fails eleven tests across every context the fixture pins. The
ordering mutation is caught by exactly one test
(`a_gate_beside_an_attribute_macro_still_confines`), written because working
through the shift's cases showed that swapping gate and macro produces the
same *findings* on every other pinned shape — `DEV` and opaque are equally
silent about a correctly-placed dev entry — and the battery confirmed it is
the only test that can see the difference.

Closes [#49](https://github.com/rlorenzo/deadwood/issues/49).

## Phase 24 — the opaque guard's population, counted (shipped)

[#50](https://github.com/rlorenzo/deadwood/issues/50) proposed relaxing the
guard that stops a placement claim when any mention of the entry is opaque: an
opaque mention would no longer suppress when the non-opaque part of the
context set is decisive on its own. The issue set its own bar — re-measure
after #48, and zero population is the argument for closing rather than
building. This phase is the measurement, and it closed the issue. No code
shipped.

### What was measured

`find_misplaced` was instrumented (scratch worktree, never committed) to
report every entry whose context set carries `OPAQUE` and whose stripped set
(`found & !OPAQUE`) would support a claim — exactly the population the
relaxation would newly judge. Run over the canonical corpus and, because this
environment's registry holds far more than the canonical 35, over all 330
unpacked registry crates.

- **Canonical corpus: zero real instances.** `serde_json`'s `serde` — the
  entire real population when phase 22 counted — is gone, which is #48 doing
  what it shipped to do. The one remaining entry is `depkinds`'
  `doc_and_library_dev_crate`, the fixture pin that exists to hold the
  guard's behaviour.
- **Extended sweep: 29 blocked claims, and the vetted ones are all noise the
  guard is correctly suppressing.** `rust-embed`'s `actix-web`, `axum`,
  `rocket` and `tokio` are optional `[dependencies]` entries wired to
  features — "belongs in `[dev-dependencies]`" would indict manifests that
  are correct. `zerocopy-derive`'s `syn`, `schemars_derive`'s `syn` and
  `bumpalo`'s `serde` are the same crate deliberately declared in both
  tables, with extra features for the tests.

### It was found by measuring, again

That second class is not a missed finding but a mis-attribution: the context
map is keyed by crate *name*, so the library mentions that justify the
`[dependencies]` copy are also held against the `[dev-dependencies]` copy of
the same crate. Reproduced against `main` at `9d3e2ab` with a clean two-file
package — no opaque mention anywhere — and the invented finding appears:
the dev copy is reported as belonging in `[dependencies]`, against a manifest
that compiles. Every registry instance is masked by an incidental doc-comment
or macro mention, which is precisely how #48 hid. Filed as
[#55](https://github.com/rlorenzo/deadwood/issues/55), now first on the Next
list; relaxing the guard before it is fixed would invent
`zerocopy-derive`-shaped findings rather than recover missed ones.

The recall #50 hoped to buy back — phase 21's check demoted four of
Deadwood's own dependencies and the guard hid two — remains a synthetic
experiment with no live-tree instance. If #55 ships, the population should be
counted again before anyone builds the relaxation; this entry is the
write-up for whoever does.

Closes [#50](https://github.com/rlorenzo/deadwood/issues/50).

## Phase 25 — a claim is judged on the entry's own evidence (shipped)

Cargo allows the same crate in `[dependencies]` and `[dev-dependencies]`,
usually because the tests want extra features — `zerocopy-derive` declares
`syn` twice this way. Deadwood's context set is keyed by crate *name*, so both
entries read one mention set, and the library mentions that justify the
`[dependencies]` copy were also held against the dev copy: reported as
belonging in `[dependencies]`, against a manifest that compiles. Phase 24
found it while counting the opaque guard's population (#55); every registry
instance the count surfaced was masked by an incidental opaque mention,
exactly as #48 hid behind the same guard.

### The decisions

- **The narrow rule, not per-entry attribution.** The claim the doubling
  breaks is exactly one: `Development` moved up on a runtime mention. So the
  fix is one condition in `misplacement` — a dev copy of a crate the
  `[dependencies]` table also declares, with dev mentions of its own, is
  placed where it is — rather than re-keying the context map per entry. The
  wider design stays available if a second claim ever needs it.
- **The dev mentions are the condition, not a nicety.** A doubled dev copy
  *nothing dev* names is a stale duplicate of the entry the library already
  justifies, and that claim rests on the absence of dev mentions — evidence
  that is the entry's own, whoever else declares the crate. It stays a
  finding, on the same footing as the stale `[build-dependencies]` copy
  (`stale_build_crate`) has always been.
- **The Build arm deliberately does not consult the doubling.** Its claim
  rests on the build script's silence — again the entry's own evidence — and
  `depkinds` has pinned the doubled spelling of it as a finding since
  phase 5.

### What it found

- **Two live invented findings, gone.** Over 356 targets (all 330 unpacked
  registry crates, the 25 fixtures and Deadwood itself) against `main` at
  `da37382`, three differ. One is `depkinds` growing its pins. The other two
  are real crates the fix silences: `phf_generator`'s `criterion` (doubled —
  `[dependencies]` for its hash-test binary, `[dev-dependencies]` for its
  benches) and `prettyplease`'s `proc-macro2` (doubled for test features).
  Both manifests compile; both findings were invented, live since phase 21,
  and unnoticed because neither crate is in the canonical 35 the phases
  measure — the first practical return on the extended sweep phase 23 added.
- **Nothing else moves.** The `zerocopy-derive`-class instances phase 24
  vetted stay silent for their original reason (the opaque guard); they would
  now stay silent without it.

**Recall by mutation: five inversions, all five caught by a named test.**
Dropping the carve-out, skipping on the doubling alone (loses the stale-copy
finding), skipping on dev mentions alone (loses phase 21's
one-runtime-mention rule), consulting the doubling in the Build arm (loses
`stale_build_crate`), and counting any table as the doubling entry (a dev
entry doubles itself, same loss as the third).

Closes [#55](https://github.com/rlorenzo/deadwood/issues/55).

## Phase 26 — one place, spelled two ways (shipped)

The report prints every path relative to the workspace root, and one line
broke that on macOS: the baseline note printed
`/private/var/folders/.../.deadwood/baseline.json` where every other line was
relative. Found running the quality gate on a macOS machine — the first this
project was developed on — where `writing_a_baseline_creates_the_directory_its_path_names`
fails against an unmodified `main` (#53).

### The mechanism, which contradicted a comment

`relative_to` was a plain `strip_prefix`. The baseline path reaches it
canonicalized — `Config::discover`'s ancestor walk needs canonical paths —
while `cargo metadata`'s `workspace_root` keeps the spelling it was invoked
with, and on macOS the standard temp directory reaches its files through a
symlink (`/var` is `/private/var`): two spellings of one place, a strip that
misses, an absolute path in the report. The comment in `Config::discover`
claimed `cargo metadata` reports a symlink-resolved root; that is true where
the project was developed (Linux, where the walk's canonicalize is a no-op)
and false on macOS, and the comment now says what actually holds.

The fix is in the display: when the plain strip misses, strip again with both
sides canonicalized; a path still outside the root after that genuinely lives
elsewhere and stays absolute, exactly as before.

### What it found

- **Zero movement**: 356 targets, byte-identical output and exit codes — no
  corpus target reaches its workspace through a symlink.
- The macOS gate is clean for the first time: 127 of 127 integration tests,
  including the one that fails on unmodified `main`.
- The failing case now exists on every platform CI runs, not only where the
  OS supplies a symlinked temp dir: the new unit test builds its own symlink
  (`a_path_is_relative_to_a_root_spelled_through_a_symlink`), and it catches
  both inversions — dropping the canonicalized retry, and canonicalizing only
  one side. Two mutations, both caught.

Closes [#53](https://github.com/rlorenzo/deadwood/issues/53).

## Phase 27 — a doubled dev copy is judged on what it enables (shipped)

Found running Deadwood across ten public workspaces to verify the beta: zed
reported 121 misplaced dependencies, and every sampled one was the same
manifest shape — a crate declared in `[dependencies]` *and* re-declared under
`[dev-dependencies]` solely to add its `test-support` feature for the tests
(`clock.workspace = true` beside
`clock = { workspace = true, features = ["test-support"] }` in
`crates/action_log/Cargo.toml`), with no dev code naming the crate directly.
Each was reported as "referenced by the library, …, which cannot link a
dev-dependency" — false on both counts, since the `[dependencies]` copy links
it and the manifest compiles ([#61]).

### The mechanism, which phase 25 half closed

Phase 25 ([#55]) stopped runtime mentions from being held against the dev
copy — when dev code also names the crate. What it left was the copy whose
justification is not a mention at all: the entry's own declaration. Cargo
unifies features per build, so a dev copy that turns on a feature the
`[dependencies]` copy does not — or default features that copy opted out
of — changes what dev builds get, whether or not any dev code spells the
crate's name. That is the same footing as a `[features]`-listed entry: load
bearing without being named.

`misplacement` now takes a three-state answer in place of the doubled bool:
not doubled (the move claim proceeds), load bearing (no claim — extra
feature, defaults the normal copy declined, or a normal copy that is
`optional` and so, with its feature off, is no declaration at all), or
redundant (enables nothing more). A redundant copy dev code names is phase 25's answer,
unchanged. A redundant copy nothing dev names keeps its finding, but the
claim is reworded to the one that is true of it: it *duplicates* the
`[dependencies]` entry and is stale — not "cannot link", which would tell a
compiling manifest it does not compile. `MisplacedDependency` carries the
distinction (`duplicate`), and the subset boundary is pinned: a dev copy
asking for a feature the normal copy already enables adds nothing and stays
a duplicate.

The rejected alternative was dropping the stale-duplicate claim entirely
once features entered the picture. Rejected because phase 25 placed that
claim deliberately — an identical doubled copy nothing dev names is exactly
as stale as the `[build-dependencies]` copy the build script never touches,
and `depkinds` pins both — and because the wording, not the claim, was what
was false.

### What it found

Measured against the ten-workspace sweep (the public corpus this beta was
verified on; the local registry of earlier phases was not part of this run):

- **zed: 121 → 79.** 42 findings gone — every load-bearing doubled copy: 37
  justified by their feature lists, 5 more sitting beside `optional` normal
  copies. 6 re-worded to the duplicate claim, which is defensible on its own
  evidence (identical copies nothing dev names). The 73 that remain are a
  different, un-doubled shape — dev-dependencies referenced from
  feature-gated library code — and are out of this slice's scope.
- **Micro-repros** (both spellings: `workspace = true` and plain versions):
  the wording moves from "cannot link" to "duplicates … and is stale".
- **Everything else unchanged**: ripgrep, regex, tokio, clap report identical
  counts before and after.
- **Mutation runs, 5/5 caught**: counting a feature subset as load bearing
  (`a_doubled_dev_copy_asking_for_no_more_is_still_a_stale_duplicate`),
  dropping the default-features prong
  (`a_doubled_dev_copy_turning_default_features_on_is_load_bearing`),
  treating load bearing as stale (that test and the extra-feature one),
  dropping the duplicate marker
  (`a_doubled_dev_copy_nothing_dev_names_is_a_stale_duplicate`, and the
  wording pin in `tests/analyze.rs`), and dropping the optional-normal-copy
  guard the review pass added
  (`a_doubled_dev_copy_beside_an_optional_normal_copy_is_load_bearing`) —
  an optional copy exists only when its feature is on, so the dev copy is
  what provides the crate to dev builds at all.

`Dependency` gained `features` and `uses_default_features` from `cargo
metadata` to make the comparison at all; `depkinds` gained the two
load-bearing spellings (`loadbearing_dev_copy_crate`, `defaults_off_crate`)
as permanent pins beside the stale one.

Closes [#61](https://github.com/rlorenzo/deadwood/issues/61).

[#55]: https://github.com/rlorenzo/deadwood/issues/55
[#61]: https://github.com/rlorenzo/deadwood/issues/61

## Phase 28 — a `mod` declaration inside a macro token stream is a claim (shipped)

The largest false-positive source ever measured against this tool: ~820
dead-file findings across the ten-workspace sweep, ~794 of them live subtrees
whose `mod` declarations sit where the parser cannot follow them — inside a
macro. tokio wraps modules in `cfg_fs! { pub mod fs; }`-style wrappers (381
findings, all false); serde writes its module tree inside a `crate_root!`
macro body, `#[path]` attributes and all (36); `rustc_target` builds
`mod $module;` from the 330 idents its `supported_targets!` invocation
passes, and `rustc_mir_transform` declares its passes through
`declare_passes!` (~403 between them); rustdesk had 6 more. The README had
documented "macro-generated `mod` declarations are invisible" as a
limitation — but the consequence lands in the direction the first tenet
forbids, at a scale that made the check worthless on exactly the codebases
most worth running it on ([#60]).

### The mechanism: claims, spending themselves only on sparing

Deadwood still expands nothing. What changed is that a macro token stream is
now *scanned* ([`scan_token_mods`]): `mod` is a keyword and can be nothing
else in any stream, so a `mod` in tokens is read as a claim that a module of
that name may exist. A claim can be wrong in one direction only — the macro
may discard it, rewrite it, or never run — so claims are spent only on
sparing files from the dead-file check: a claim that names a real file
queues it, a claim that names nothing is dropped silently (no
unresolved-module warning: those skip a package's checks, and a speculative
miss proves nothing).

Three shapes are read:

- **A literal `mod` in an invocation's arguments** (tokio), resolved at the
  invocation site — including `mod name : Tokens;` shapes where only the
  macro understands what follows the name (`declare_passes!`).
- **A literal `mod` in a `macro_rules!` body** (serde), resolved at the
  definition site and re-resolved at every invocation site, because
  expansion happens there: serde defines `crate_root!` in one file and
  invokes it from `serde/src/lib.rs` and `serde_core/src/lib.rs`, and the
  `#[cfg_attr(docsrs, path = "core/de/mod.rs")]` attributes on its `mod`s
  resolve against the invoking file. A `#[path]` read through `#[cfg_attr]`
  is a claim on *two* files — the attribute's target where the condition
  holds, the stem-named file everywhere else — and the condition is never
  evaluated, so both are spared.
- **The bare idents of an invocation whose macro's rules say `mod $x`**
  (`supported_targets!`), probed under the inline-module prefix the rules
  wrap the `mod $x` in — the 330 target idents resolve under `targets/`
  because the macro body says `mod targets { $(mod $module;)+ }`.

Definitions and invocations need not share a file, and the definition may be
parsed after the invocation, so invocations are held unsettled and
re-checked whenever the walk runs out of other work — the same drain point
`include!` targets wait at. Everything queued this way lands in
[`Resolved::spliced`] and inherits phase 18's boundary exactly: spared from
the dead-file check, not admitted to item resolution. The module path a
macro gives its items is unknowable without expansion, so admitting them
would trade the dead-file false positive for invented `unused_pub_item`
findings; keeping them out costs findings instead, which is the direction
the tenet buys.

The rejected alternative was requiring a macro to be *known* mod-emitting
before reading literal `mod`s from its invocation arguments. Rejected
because the claim stands on its own: `mod` cannot be an identifier, an
argument the macro discards costs a lost finding rather than an invented
one, and tokio's wrappers would otherwise need their definitions parsed
first for no change in the answer.

### What it found

Measured against the ten-workspace public corpus:

- **tokio: 381 → 0. rust-lang/rust: 417 → 10. serde: 36 → 1.
  rustdesk: 6 → 0.** The ten that remain in rust are compiletest's
  deliberately-undeclared test-auxiliary files and a handful of plausible
  orphans; serde's one survivor (`serde/src/core/lib.rs`) is referenced by
  nothing in the repository tree.
- **Every other finding kind is byte-identical** across all ten workspaces,
  which is the spliced boundary doing its job: no macro-reached file's items
  entered resolution.
- **Wall clock is unchanged** on the largest workspace (rust-lang/rust,
  ~3.5s before and after).
- **Mutation runs, 5/5 caught**: never queueing literal token-mods
  (`a_mod_declared_inside_a_macro_invocation_spares_its_file`, the
  trailing-tokens and cfg-attr tests), never matching emissions at
  invocations (`a_macro_definitions_literal_mods_resolve_at_its_invocation_sites`,
  the emitting-idents test), probing idents without the rules' prefix
  (`an_emitting_macros_invocation_idents_are_probed_under_its_prefixes`),
  reading every stream as mod-emitting
  (`a_quiet_macros_idents_are_not_probed` — the boundary that keeps the scan
  from gutting the check), and short-circuiting the stem fallback behind a
  `#[path]` attribute
  (`a_cfg_attr_path_mod_in_a_macro_body_spares_both_files`).

The `macromods` fixture pins the three shapes end to end — files a macro
declares are spared, `src/orphan.rs` beside them is still the finding it
always was, and the unreferenced `pub fn` in a macro-reached file produces
nothing.

Closes [#60](https://github.com/rlorenzo/deadwood/issues/60).

[#60]: https://github.com/rlorenzo/deadwood/issues/60

## Phase 29 — a reference that exists only for rustdoc claims nothing false (shipped)

Rustdoc compiles doctests in a build of its own: `cfg(doctest)` set,
dev-dependencies linked, and no consumer anywhere near it. Deadwood read that
build's code as ordinary library code, which invented findings in three
shapes across the ten-workspace sweep ([#63]) — regex's
`#[cfg(doctest)] doc_comment::doctest!("../README.md")` had its `doc-comment`
dev-dependency reported as "cannot link a dev-dependency" against a manifest
that compiles (twice, with regex-automata); clap's
`#[cfg(doctest)] pub struct ReadmeDoctests;` idiom was reported unused in six
crates, where following the advice deletes the README's doctest coverage; and
tokio's `futures-concurrency` dev-dependency, named only from the doctests in
`select!`'s documentation — doc comments that sit *inside* a
`doc! { macro_rules! ... }` wrapper — was reported never referenced.

### Three mechanisms, one build

- **`doctest` rides the `test` axis.** `eval` reads `cfg(doctest)` exactly as
  `cfg(test)`: the doctest build links `[dev-dependencies]` as `cargo test`
  does, and no build a consumer makes sets either cfg, so telling them apart
  buys nothing any check needs. `#[cfg(doctest)]` now confines to a test
  build — its mentions are dev evidence — and `#[cfg(not(doctest))]` is
  ordinary code. The placement check gains a true claim for free: a
  `[dependencies]` entry named *only* from `cfg(doctest)` code costs every
  consumer a build for nothing, and is now reported as belonging in
  `[dev-dependencies]` (`doctest_only_normal_crate` pins it).
- **An item gated to rustdoc's build has rustdoc as its consumer.**
  `requires_doctest` — a `cfg` naming `doctest` as a bare path outside any
  `not(...)` — joins `has_skip_attr` beside `#[no_mangle]` and friends, and
  roots the item in both walks. The boundary is pinned: `not(doctest)` is no
  match, and a *feature* named "doctest" is a string, not the cfg.
- **Doc text is documentation wherever it sits.** The token walk that reads
  macro input already declines to mine string literals — macro-body literals
  are usually data — but the text of a `#[doc = "..."]` token tree is what a
  `///` comment becomes, and its words now count as opaque mentions. Only
  `doc` gets the exemption: a `#[path = "..."]` string inside a macro body
  stays data (`a_non_doc_attribute_literal_inside_a_macro_body_is_still_data`
  pins the boundary).

### What it found

Measured against the ten-workspace public corpus, combined with phase 28:

- **regex: 3 → 1** (both `doc-comment` placement claims gone; the stale
  `quickcheck` dev-dependency, verified real, remains). **clap: 7 → 1** (six
  `ReadmeDoctests` gone; `clap_mangen::generate_to`, a true
  never-referenced-in-workspace claim, remains). **tokio: −1**
  (`futures-concurrency` alive through its doctests). **rust-lang/rust: −1**
  (the same shape, found by the sweep rather than sought).
- **Everything else byte-identical**, including zed's 424 — no doctest idiom
  there to misread.
- **Mutation runs, 6/6 caught**: `doctest` off the `test` axis (the cfg unit
  test and `depkinds`), `not()` no longer shielding
  (`a_cfg_gate_requires_doctest_only_as_a_bare_path_outside_not`), the
  skip-attr arm dropped (`detects_dead_file_and_unused_pub_item`, whose
  `simple` fixture now carries the `ReadmeDoctests` idiom), every attribute
  literal mined and the mining dropped entirely (the two deps boundary
  tests), and the plain-literal boundary.

`depkinds` gained the three spellings (`doctest_only_normal_crate`,
`cfg_doctest_dev_crate`, `doc_literal_dev_crate`); `simple` gained the clap
idiom beside its existing findings, pinning that the unused list does not
grow.

Closes [#63](https://github.com/rlorenzo/deadwood/issues/63).

[#63]: https://github.com/rlorenzo/deadwood/issues/63
