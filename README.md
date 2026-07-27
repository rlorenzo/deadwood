# Deadwood

A codebase health analyzer for Rust workspaces. Deadwood finds maintainability
issues that `rustc` and `clippy` stay quiet about — starting with dead module
files, unused `pub` items, unused re-exports, unused and misplaced
dependencies, and `cfg` gates that can never hold, in the spirit of
Fallow/knip-style analyzers for other ecosystems.

**Status:** v0.1 — early, narrow, and honest about it. Tunable through a
`deadwood.toml`, adoptable on an existing codebase through a baseline file, and
correct without either. See
[`docs/SCOPE.md`](docs/SCOPE.md) for what is in and out of scope, and
[`docs/ENVIRONMENT.md`](docs/ENVIRONMENT.md) for the environment assessment
this project was bootstrapped from.

## What it detects today

| Check | What it finds | Why rustc doesn't |
| --- | --- | --- |
| **Dead files** | `.rs` files under `src/` not reachable from any target root via `mod` declarations | Files outside the module tree are never compiled, so no lint ever sees them |
| **Unused pub items** | Fully-`pub` fns, structs, enums, traits, type aliases, consts, statics, and unions that nothing live in the workspace reaches — either no path resolves to them, or every path that does is written inside something itself unreachable | `dead_code` assumes `pub` items have external consumers |
| **Unused re-exports** | `pub use` re-exports nothing live in the workspace goes through, where outside code cannot reach them either | `unused_imports` only sees imports the crate itself does not use, not ones re-exported for nobody |
| **Unused dependencies** | `Cargo.toml` entries — normal, dev, or build — whose crate name the declaring package's code never mentions | Cargo has no reason to look, and an unused entry still costs build time and supply-chain surface |
| **Misplaced dependencies** | `Cargo.toml` entries declared in a table the code naming them cannot see: a `[dependencies]` entry only tests, examples and benches use, or a `[build-dependencies]` entry the build script never touches | Cargo builds the entry wherever you put it; a normal dependency only your tests need is compiled by everyone who depends on you |
| **Unsatisfiable `cfg` gates** | `#[cfg(...)]` gates that can hold in no build of the package, e.g. a `mod` behind a feature the manifest does not declare | The code is never compiled, so no lint ever sees it — and the gate reads as deliberate |
| **Test-only public items** *(off by default)* | `pub` items the workspace reaches only through its test code — reached, so not dead, but `pub` for nobody | `dead_code` says nothing about an item in a test, bench or example target, since the only build that compiles one also uses it. Where rustc *can* see the item it usually does report it, in a build with the tests left out — see [Known limitations](#known-limitations-tracked-not-hidden) |

What each check reports can be tuned by a `deadwood.toml` — see
[Configuration](#configuration).

Usage is decided by *resolving paths*, not by counting identifiers: `use`
declarations (renames, nested trees, `pub use`), qualified paths (`crate::`,
`self::`, `super::`), and cross-crate paths between workspace members are
resolved against a per-crate symbol table. So two items sharing a name no
longer hide each other, and a type mentioned only inside its own `impl` block
is still reported.

Lexical scopes are part of that resolution: a local, a parameter, or a generic
parameter that shares a name with a module item shadows it exactly as it does
in Rust, so `let helper = 5;` no longer keeps a dead `pub fn helper` alive.
Shadowing is per namespace — a `let` binding hides only expressions and a
generic parameter only types, so `let Foo = 1;` cannot silence a `: Foo` beside
it — and it stops at the end of the block, arm, or body that opened it.

And being referenced is not enough — the referrer has to be alive too. Each
use is recorded against the definition the naming path is written *inside*,
and an item survives only when something names it *and* that something is
itself reached:

```rust
// in `main.rs`, or anywhere a consumer outside the crate cannot reach:
pub fn orphan() { helper(); }   // reported: nothing names it
pub fn helper() {}              // reported too: only `orphan` names it
```

The crate kind matters, and it is the root rule below at work: in a *library*
both of these sit on the public surface, so `helper` is reached and only
`orphan` is reported.

So a dead subsystem comes out in one run rather than one layer per run, and a
pair of mutually recursive functions nothing reaches — permanently referenced,
and so invisible to any reference count — comes out at all. Both members of a
dead cycle are reported: each is separately deletable, and a group finding
would need a name that moved whenever a member joined or left it. The two
kinds of evidence read apart in the message, since saying "never referenced"
about an item with visible callers would read as a bug:

```console
src/api.rs:3: pub fn `orphan` is never referenced by any resolved path in this workspace
src/api.rs:7: pub fn `helper` is referenced only from items that nothing reaches
```

The walk starts from a set of **roots**, and every omission from it would be a
live item reported dead, so the set is deliberately generous: `fn main` and the
build script, `#[test]` and `#[bench]` functions, the linker and compiler
exports (`#[no_mangle]`, `#[export_name]`, `#[proc_macro*]`, `#[panic_handler]`
and the rest, including the `#[unsafe(...)]` spelling), the `dead_code`
opt-outs, everything `[public-api]` declares, **a library's public surface** —
every `pub` item under `pub` modules from the crate root, and everything a
`pub use inner::*;` glob re-exports from the crate root or from one of those
modules — `inner` itself need not be `pub`, which is the whole point — since
consumers Deadwood cannot see call it — and **everything opaque**. A root is still
reported when nothing in the workspace names it, which is why rooting the
public surface costs no finding: what it changes is that an item the surface
*calls* is not dragged down with it.

That walk runs twice, and the difference between the two answers is a
finding of its own. Once from the whole root set — the build Deadwood analyzes
— and once from the root set with the test entry points taken out: `#[test]`
and `#[bench]` functions, and every entry point written in a test, bench or
example target, which are test code in their entirety rather than only where
the attribute is. An item in the first and not the second is reached only by
test code, which is not the same claim as "dead", so it is not the same
finding:

```console
src/parser.rs:14: pub fn `scan_all` is reached only from test code: make it `pub(crate)`, or move it behind `#[cfg(test)]`
```

The kind is `off` by default — every `#[cfg(test)]` helper in every codebase
is a candidate, so it would fire on the first run of every project — and a
project asks for it with `test_only_item = "warn"` under
[`[severity]`](#configuration). Nothing on a library's public surface is ever
reported this way, whatever its tests do: a consumer Deadwood cannot see
reaches it in a build with no tests in it at all.

The bias is still toward staying quiet rather than raising noise. Anything
that cannot be resolved counts as a use of *every* item with that name:
identifiers inside macro invocations and attribute arguments, and names in a
module holding a glob import that leads outside the workspace. Under
reachability those count as *roots* rather than as ordinary references — a
mention Deadwood has admitted it cannot read must never become evidence that
something is dead. So does a use written where there is no definition to
attribute it to: in an `impl` block for a type outside the workspace or for a
generic parameter, or inside an item nested in a function body. Items marked
`#[no_mangle]`, `#[used]`, `#[export_name]`, or
`#[allow(dead_code)]`/`#[expect(dead_code)]` are skipped, as is `fn main`. For
library crates with external consumers, treat unused-pub findings as advisory
— Deadwood cannot see your dependents.

The dependency check leans on the same bias, harder. A `Cargo.toml` entry is
reported only when *nothing* in the package mentions its crate name — not a
path, not an `extern crate`, not an identifier in macro input or an attribute
(strings included), not a word in a doc comment (doc examples are compiled,
and often use a dependency that appears nowhere else), and not the
`[features]` table, where `test = ["helper/all-features"]` is a use with no
code behind it. Reachability is not required either: files no `mod`
declaration names are read too, because `automod::dir!` and friends expand
into declarations Deadwood never sees. A package pulling in code from a file
that cannot be read (`include!(concat!(env!("OUT_DIR"), ...))`) is skipped with
a warning instead of guessed at.

Optional and `[target.'cfg(...)'.dependencies]` entries are judged like any
other, because the default analysis covers every feature combination and every
target: the code that uses them *is* read, so a reference to one is found
wherever it exists. Two cases are still skipped, out loud — an entry that no
feature in a narrowed `cfg` matrix can turn on, and a
`[target.'cfg(any())'.dependencies]` entry, which is how a crate pins the
version of something it deliberately never compiles.

Whether an entry sits in the *right* table is a separate check, because it is a
separate question: the unused check asks whether anything names the crate, and
this one asks whether the code that does can see the table it is declared in.
It needs stronger evidence, so it accepts less. Every mention is attributed to
the code it was written in — runtime targets, test/example/bench targets, the
build script — and only two claims are ever made: a `[dependencies]` entry
every mention of which is test code belongs in `[dev-dependencies]`, and a
`[build-dependencies]` entry the build script never names belongs wherever the
code that does name it lives.

Everything else stays quiet, by design:

- **`#[cfg(test)]` code counts as test code wherever it sits**, so the unit
  tests inside a library do not make every dev-dependency they use look
  misplaced. That is the single largest false positive the check could make.
- **A mention in a doc comment places nothing.** Doc examples are compiled as
  doctests, which link the normal *and* the dev dependencies, so a crate named
  in one is correctly declared under either table.
- **A mention through a macro, an attribute, or a file no `mod` declaration
  names places nothing either.** These keep an entry alive for the unused check
  precisely because we cannot see through them; a reference that cannot be
  attributed to a target cannot prove misplacement. This is most of what the
  check gives up.
- **A dev-dependency is never reported.** The only claim available — "the
  library names it" — describes a manifest that does not compile, so a
  mis-attribution on our side is the likelier explanation.

`cfg` gates are evaluated rather than always followed, but the *default* set of
builds analyzed is the union of every possibility — every feature on and off,
every target, tests included — so a gate is followed whenever it could hold
anywhere. That is exactly the old always-follow behavior;
[Configuration](#configuration) is where a project narrows it. What the
evaluation adds is a finding: a gate that can hold in *no* build, which in
practice means one naming a feature the manifest does not declare. Such a gate
is reported and the code behind it is still analyzed, so the new finding never
moves the others. Gates Deadwood cannot read at all —
`cfg(accessible(..))`, a `cfg` a build script sets, a `cfg_attr` indirection —
are followed as before.

Re-exports get one extra filter, because a `pub use` exists *only* to expose a
name outward: one that is reachable from a library's crate root (`pub use
inner::Thing;` in `lib.rs`, in any `pub mod` under it, or in a module a
`pub use inner::*;` glob re-exports) is doing its job
even when nothing inside the workspace uses it, so it is never reported. A
re-export that outside code cannot reach — because some module on the way is
private — has no such excuse, and is reported. A `use` names what it imports
on the bound name's behalf, so such a re-export stops keeping its target
alive: the definition under it is reported alongside it, because deleting one
does not delete the other.

## Usage

```console
$ cargo run -- check path/to/workspace
Dead files:
  src/orphan.rs: not reachable from any target of package `simple` via `mod` declarations

Unsatisfiable cfg gates:
  src/lib.rs:14: `#[cfg(feature = "legacy")]` can never hold: package `demo` declares no feature `legacy`

Unused public items:
  src/lib.rs:3: pub fn `entry` is never referenced by any resolved path in this workspace
  src/lib.rs:7: pub fn `dead_fn` is never referenced by any resolved path in this workspace
  src/lib.rs:9: pub fn `dead_helper` is referenced only from items that nothing reaches

Unused re-exports:
  src/lib.rs:11: `pub use` re-export of `Stale` is never referenced through this module

Unused dependencies:
  Cargo.toml: dev-dependency `tempfile` is never referenced by any target of package `demo`

Misplaced dependencies:
  Cargo.toml: dependency `assert_cmd` is referenced only by the test, example and bench code of package `demo`, so it belongs in `[dev-dependencies]` rather than `[dependencies]`

8 finding(s) in workspace `/path/to/workspace`.
```

- `deadwood check [PATH]` — analyze the package/workspace at `PATH` (default `.`)
- `--json` — machine-readable output (findings + warnings)
- `--config PATH` — use this configuration file instead of searching for
  `deadwood.toml`
- `--write-baseline` — record the current findings so later runs fail only on
  new ones; see [Adopting on an existing codebase](#adopting-on-an-existing-codebase)
- `--prune-baseline` — drop baseline entries that no longer occur
- Exit codes: `0` clean, `1` findings that are configured `deny` (the default
  for every kind except `test_only_item`, which is `off`), `2` error —
  suitable for CI gates.

Requires `cargo` on `PATH` (workspace discovery shells out to
`cargo metadata --no-deps`, which works offline).

## Adopting on an existing codebase

A codebase that has never been analyzed has findings on day one, and a tool
that fails the build for all of them on day one gets uninstalled. Record them
once and only new ones fail:

```console
$ deadwood check --write-baseline
No issues found.
Wrote 34 finding(s) to baseline `deadwood-baseline.json`.

$ deadwood check                     # commit the file; CI is green from here
No issues found.
34 finding(s) suppressed by baseline `deadwood-baseline.json`.
```

The file is `deadwood-baseline.json` in the workspace root unless a
`deadwood.toml` says otherwise, and it holds exactly the objects `--json` puts
in its `findings` array — no second format, and readable in a diff. That is a
constraint rather than a coincidence: everything the matching keys on has to be
producible from a report, so a baseline stays something you can write by hand.
It is meant to be committed: the debt stays visible, and it can only shrink.

**A baselined finding is subtracted, not marked.** It is absent from the text
report, from the JSON `findings` array, and from the count; only the summary
line says how many there were. The report is for what you have to act on, and
reprinting the accepted list would reproduce exactly the noise the baseline was
adopted to remove.

**Matching survives line drift.** An entry is matched on kind, file, item name
and the module the item is written in — not the line, which moves with every
edit above it, and not the severity, which is a `deadwood.toml` decision:
putting it in the key would mean that turning a check down from `deny` to
`warn` un-baselines every finding of that kind at once. The module path is in
the key for the same reason the line is not: it tells `alpha::twin` from
`beta::twin` in one file, and it does not move when code above it does.

```json
{ "kind": "unused_pub_item", "severity": "deny", "file": "src/lib.rs",
  "line": 11, "name": "twin", "module": "crate::alpha",
  "message": "pub fn `twin` is never referenced by any resolved path in this workspace" }
```

Only the three item kinds have a module: a dead file is not an item, the two
dependency kinds name an entry in a manifest, and an unsatisfiable gate names
the site the gate is written at. **An entry with no `module` is not an entry in
the crate root** — it is an entry that says nothing about modules, and modules
are compared only when both sides name one. That is what keeps a baseline
written by an older Deadwood matching exactly what it always matched, with no
edit: the crate root is spelled `crate`, never omitted, so the two cases can
never be confused.

The compatibility runs one way. A baseline this version writes makes an older
Deadwood exit 2 with ``unknown field `module` ``, on a file it read yesterday —
the same strictness that turns a typo'd key into an error rather than a silently
ignored one, and the reason a field that decides matching may not be quietly
dropped. Downgrading after rewriting the baseline means deleting the field by
hand, or regenerating the file with the older binary.

**Fixed findings are reported, not forgotten.** An entry nothing matches any
more is stale, and every run says so; `--prune-baseline` rewrites the file
without them. Stale entries never fail the run — the exit code follows severity
and nothing else, and a developer who deletes dead code should not be punished
for it.

```console
$ deadwood check
Unused public items:
  src/api.rs:12: pub fn `fresh` is never referenced by any resolved path in this workspace

1 finding(s) in workspace `/path/to/workspace`.

Stale baseline entries in `deadwood-baseline.json` (no longer occur; rerun with --prune-baseline to drop them):
  src/old.rs: unused_pub_item `finally_deleted`
33 finding(s) suppressed by baseline `deadwood-baseline.json`.
```

Two rules keep the file from lying:

- **Writing is explicit.** No run without `--write-baseline` or
  `--prune-baseline` creates or modifies the file, so a CI job can never
  quietly accept what it found.
- **A missing or unparsable baseline is exit 2**, never "nothing is
  baselined" and never "everything is". A typo'd path would otherwise disarm a
  CI gate silently. The one non-error case is the default location with no file
  in it — that is a project that has not adopted a baseline, and it behaves
  exactly like a Deadwood without the feature.

One entry covers every finding that shares its key, so two items that share a
file, a name *and* a module are still suppressed together — a `pub struct Group`
beside a `pub fn Group(..)`, or two `#[cfg]`-alternative definitions of one item
([#30](https://github.com/rlorenzo/deadwood/issues/30)). Moving a file
un-baselines the findings in it
([#17](https://github.com/rlorenzo/deadwood/issues/17)).

## Configuration

Deadwood needs no configuration, and with no `deadwood.toml` present it
behaves exactly as described above. The file exists to express what the
analysis cannot infer: which files are not yours to fix, which checks you are
ready to enforce, which crates have consumers Deadwood cannot see, which
manifest entries are load bearing without being named in code, and which
builds — features, targets, tests — you actually care about.

It is looked for by walking up from the analyzed path to the workspace root,
and the nearest one wins; `--config PATH` overrides the search and fails if
that file is missing. Relative patterns are resolved against the directory
holding the file.

```toml
# deadwood.toml — every setting, with its default behavior noted. The values
# shown are an illustrative policy, not the defaults: each block states its own
# default in the comment above it, and an omitted block leaves it in force.

# Files no finding may be reported about. Patterns are `/`-separated globs
# where `*` stays inside one segment, `**` spans any number of them, and `?` is
# one character; a pattern matching a directory covers everything under it.
# Default: nothing is ignored.
ignore = ["crates/*/src/generated/**", "vendor"]

# What each finding kind costs. `deny` reports it and fails the run (exit 1),
# `warn` reports it and exits 0, `off` never reports it at all. The keys are
# the finding kinds as they appear in `--json`.
# Default: `deny` for every kind that reports something to delete — which is
# the pre-config behavior — and `off` for `test_only_item`, which reports a
# visibility to narrow and would otherwise fire on the first run of every
# project that has a `#[cfg(test)]` helper. There is no other exception, and a
# new kind has to state its own default rather than inherit one.
[severity]
dead_file = "deny"
unused_pub_item = "warn"
unused_reexport = "warn"
unused_dependency = "deny"
misplaced_dependency = "deny"
unsatisfiable_cfg = "deny"
test_only_item = "warn"     # `off` unless you ask; nothing else defaults to `off`
# (`unused_pub_item` and `unused_reexport` above are `deny` unless a line like
# these turns them down — the two spellings are what the setting is for.)

# Crates and items whose `pub` surface is API rather than leftovers. Deadwood
# only sees consumers inside the workspace, so for a published library this is
# the difference between a usable report and a page of noise.
# Default: nothing is treated as declared API.
[public-api]
# Every `pub` item in these crates. Dashes and underscores are interchangeable.
crates = ["my-library"]
# ...or `crate::module::Item` paths, as globs, for finer control.
items = ["my-app::prelude::*", "my-app::error::**"]

# Manifest entries the dependency checks must never judge — neither whether
# anything names them nor which table they belong in: the ones that are load
# bearing without any code naming them. Matched on the manifest key exactly as
# written, so a renamed entry is listed by its alias.
# Default: every entry is judged.
[dependencies]
# Exempt in every package of the workspace.
allow = ["getrandom", "openssl"]

# Exempt only in the package named.
[dependencies.allow-in]
my-app = ["vendored-native"]

# Which builds to analyze. Every key here *narrows* the analysis, and omitting
# one means "not narrowed" rather than "empty": the default is the union of
# every possibility, which is what makes an absent `[cfg]` section a no-op.
[cfg]
# Feature names to treat as enabled, closed over the features they enable, in
# every package. `#[cfg(feature = "...")]` code behind anything else is not
# analyzed at all.
# Default (key omitted): every feature may be on or off, so every gate holds
# somewhere. `features = []` is different — it is the build with none of them.
features = ["default", "serde"]

# `target_os` values to analyze, which also decide `cfg(unix)`, `cfg(windows)`
# and `target_family`. Other target predicates (`target_arch`, `target_env`,
# ...) are not modelled and are never narrowed.
# Default: every target is possible.
target-os = ["linux", "macos"]

# Whether `#[cfg(test)]` code is part of the build being analyzed. With it on,
# a test is a use, so an item only tests reach is not reported.
# Default: true.
test = true

# Where the baseline file lives, relative to this config file. Omitting the key
# does not mean "no baseline": it means the default location,
# `deadwood-baseline.json` in the workspace root, which may or may not have a
# file in it yet. A path written here and not on disk is an error.
# Default: the default location.
baseline = ".deadwood/baseline.json"
```

Five things are worth knowing about how these behave.

**`ignore` suppresses findings, not evidence.** An ignored file is still read,
and the paths in it still count as uses. Generated code that calls your
`pub fn` is still calling it, and dropping that would make every `ignore` entry
a source of false positives in the code beside it. The one thing `ignore` does
reach into is module resolution: a `mod` declaration pointing at a *missing*
file the patterns cover is skipped silently instead of warned about, so
ignoring a generated module does not stop Deadwood checking the rest of its
package.

**Severity is per kind, and only `deny` fails the run.** A `warn` finding is
printed with its group marked `(warn)` and carries `"severity": "warn"` in the
JSON, so a project can adopt a check as advisory before enforcing it. An `off`
finding does not exist: it is absent from the output, the JSON, and the count —
which is exactly what `test_only_item` is until a `[severity]` entry asks for
it, and why adding that kind changed no output anywhere.

**`public-api` covers unused-pub items and unused re-exports alike**, since a
`pub use` is surface too. An allowlisted dependency entry that *is* referenced
is not an error — the list means "do not judge this", not "assert this is
unused".

**`cfg` narrows what is analyzed, not what is reported.** Code the matrix
leaves out is not read, so it neither defines nor uses anything — and it is not
a dead file either, because nothing reaches it only in the sense that this
build does not contain it. That is the lever's real cost and its real value:
`test = false` turns a test-only helper into an unused-pub finding, which is
either the question you wanted answered or a page of noise, depending on the
project. That overlaps `test_only_item` and does not replace it, in either
direction. The matrix takes the tests out of the *build*, so it also takes
them out of the evidence — a dev-dependency only the tests use becomes an
unused-dependency finding, and a `#[cfg(test)]`-only file becomes a dead one —
and what it reports is `unused_pub_item`, whose message says the item is dead.
`test_only_item` keeps the tests in the build, changes no other check's
answer, and says what to do instead. The matrix has the better recall (it does
not care that an `assert_eq!` names the item); the kind has the narrower blast
radius and the truer message. The `unsatisfiable_cfg` finding is the one thing the matrix does *not*
affect — a gate is judged impossible against every build there could be, so
narrowing the matrix never invents one and never silences one.

**`baseline` names a file, it does not switch a feature on.** A run reads the
baseline whether or not the key is present — with it, from where it points;
without it, from the default location if a file is there. What the key changes
is the error contract: a path you wrote down must exist, while the default
location may simply be empty. Note also that `ignore` and `severity = "off"`
outrank the baseline, since a finding they remove never exists to be
suppressed — which makes any baseline entry for it stale, and prunable.

Configuration mistakes are hard failures (exit 2), including unknown keys. A
`deadwood.toml` that quietly does nothing because of a typo is worse than none
at all, so a misspelled key names itself, its file, and the keys that do exist:

```console
$ deadwood check
error: invalid config file `deadwood.toml`: TOML parse error at line 1, column 1
  |
1 | ignor = ["vendor"]
  | ^^^^^
unknown field `ignor`, expected one of `ignore`, `severity`, `public-api`, `dependencies`, `cfg`, `baseline`
```

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
   they name; everything reached is parsed with `syn`, and each file records
   the module path its items live in (`src/modtree.rs`). A `mod` behind a
   `cfg` the configured build matrix rules out is not followed, and neither it
   nor the files under it can be reported dead.
3. **`cfg` evaluation** — each gate is answered against two matrices
   (`src/cfg.rs`): the configured one, which decides whether the code is part
   of the build being analyzed, and every build there could be, which decides
   whether the gate can hold at all. Items the first rules out are pruned from
   the AST, so the detectors below simply never see them; gates the second
   rules out are the `unsatisfiable_cfg` findings.
4. **Usage resolution** — every target is a crate. For each one, a symbol
   table maps its modules to the items they define, the `use` aliases they
   bind, and the globs they import; then every path in every file is resolved
   from the module it is written in, marking what it names and recording which
   definition it was written inside (`src/resolve.rs`).
5. **Reachability** — those recorded edges are walked from the root set
   (entry points, the linker and compiler exports, a library's public surface,
   `[public-api]`, and everything opaque), so an item is alive only when
   something live names it (`src/resolve.rs`). The same walk runs a second
   time over the same edges with the test entry points removed, and what only
   the first reaches is the `test_only_item` finding.
6. **Detectors** — dead files are `src/**.rs` minus the reachable set and the
   `cfg`-excluded set; unused pub items and re-exports are the definitions
   nothing live reached, and test-only items the ones only the first walk
   reached (`src/unused.rs`); unused dependencies are the
   manifest entries whose crate name appears nowhere in the package, reachable
   or not, and misplaced ones are the entries every mention of which lands in
   code their table does not serve (`src/deps.rs`).
7. **Configuration** — `deadwood.toml` is applied in one pass over the
   findings, so `ignore` and `[severity]` cover every detector identically
   (`src/config.rs`); `public-api`, the dependency allowlist, and the `cfg`
   matrix are consulted by the detectors they belong to.
8. **Baseline** — last of all, and after the configuration: recorded findings
   are subtracted by kind, file and name, and recorded entries that matched
   nothing are reported stale (`src/baseline.rs`).
9. **Reporting** — grouped text or JSON (`src/report.rs`).

## Known limitations (tracked, not hidden)

- Resolution is syntactic, not semantic: method calls (`x.foo()`), trait
  dispatch, and associated items are not resolved. Only free-standing item
  definitions are ever reported, so this costs findings, never precision.
- Macro input is not expanded, so an identifier inside a macro invocation
  counts as a use of every workspace item with that name. The same goes for
  attribute arguments, including paths hidden in strings
  (`#[serde(with = "crate::codec")]` keeps everything in `codec` alive).
  Macro-*generated* `mod` declarations and items are invisible to the parser
  entirely.
- Lexical scopes are tracked syntactically, so a binding a macro expands to
  shadows nothing — though an identifier in macro input already counts as a
  use of every item with that name, so the two errors point the same way. A
  bare name in pattern position is read as a *use* whenever it could name a
  unit struct, a variant or a `const`, which costs a finding for the braced
  struct and type alias it could equally be binding over.
- A glob import that leads outside the workspace makes its module opaque:
  names not otherwise in scope there count as uses of every item with that
  name. Globs within the workspace are expanded and hide nothing.
- `cfg` evaluation covers `feature`, `test`, `target_os`, `target_family`,
  `unix` and `windows`, and `not`/`all`/`any` over those. Every other
  predicate — `target_arch`, `debug_assertions`, a `cfg` a build script sets,
  `cfg(accessible(..))`, anything reached through `cfg_attr` — reads as
  "could go either way", so the code behind it is analyzed exactly as before.
- Gate evaluation does not track correlation between atoms:
  `all(feature = "a", not(feature = "a"))` reads as satisfiable even though it
  provably is not. The finding is lost, never invented.
- Under the default matrix `#[cfg(test)]` code counts as a use and `#[test]`
  functions are roots, so an item only tests reach is not an `unused_pub_item`
  finding. It may be a `test_only_item` one, which is `off` by default; `[cfg]
  test = false` asks the same question a blunter way, and the paragraph on it
  under [Configuration](#configuration) is why both exist.
- The test-only claim is narrower than it sounds, in three directions that all
  cost findings rather than invent them. **Anything a consumer could name is
  out**: a library's public surface, whatever a surface item reaches, and
  anything `[public-api]` declares. That still covers everything a `pub use
  inner::*;` re-exports, but by the ordinary route rather than a rule of its
  own — a glob re-export *is* public surface, so it is a root in both walks
  like the rest of it. **An
  opaque mention keeps an item out entirely** — a name in macro input is a
  root, and `assert_eq!(thing(), 1)` is how most tests name what they test, so
  one assertion is enough. And **an entry point inside an inline `#[cfg(test)]
  mod`** that is not itself `#[test]`/`#[bench]` — a `#[no_mangle]`, an
  `#[allow(dead_code)]` — reads as a non-test root, so what it reaches is not
  test-only either. Out-of-line `#[cfg(test)] mod tests;` files do not have
  that gap ([#27](https://github.com/rlorenzo/deadwood/issues/27); simulating
  the fix changed no finding on any fixture, on the 34 crates in a local
  registry, or on Deadwood itself).
- Much of what `test_only_item` reports about a package's own `src/` is also
  reported by rustc, as `dead_code`, in any build that leaves the tests out —
  `cargo build`, and `cargo clippy --all-targets`, which compiles the crate
  both ways. What rustc cannot report is a `pub` item in a test, bench or
  example target, because the only build that compiles one also uses it. The
  kind is worth the `[severity]` line where you want that answer in the report
  with everything else, in JSON, and baselineable; it is not worth turning on
  expecting to be told something your compiler is not already telling you.
- Reachability follows references, not containment: an item inside a module
  nothing names is judged on the paths that name *it*. A module can be reached
  through a glob, a `pub use`, or generated code without ever being named, so
  reading "unnamed module" as "everything in it is dead" would be a claim about
  code Deadwood has not seen.
- An `impl` block hangs off its self type, and off the trait too where that
  resolves inside the workspace. For anything else — a foreign self type
  (`impl Trait for Vec<T>`), a blanket `impl<T>`, a tuple, a reference — there
  is no definition to hang it off, so what its body names counts
  unconditionally. That is most of the recall reachability gives up on generic
  code, deliberately.
- A definition that is not `pub` takes part in the walk like any other, so a
  private helper only dead code calls stops keeping what *it* calls alive.
  Rooting private items would end every cascade at the first one; rustc's
  `dead_code` lint already reaches them where it can see them.
- An unsatisfiable gate is reported where it is written, and only for the
  outermost gate — an inner `#![cfg(...)]` gates the whole file it is in, and
  nothing below a dead gate is walked. Enum variants and struct fields are not
  walked either, nor are items inside function bodies.
- A module the matrix excludes takes its whole conventional subtree with it,
  unread. An orphan file inside that subtree is therefore not reported as
  dead: Deadwood did not resolve that module tree, and claiming a file is
  unreachable from a tree it never read is the failure mode it refuses.
- `include!()`-ed files are not tracked and may be reported as dead.
- A `pub` item with consumers outside the workspace looks identical to a dead
  one; for library crates, these findings are advisory until the crate or its
  item paths are listed under `[public-api]`. Re-exports on a library's public
  surface are skipped for the same reason, which also means a genuinely dead
  one there is missed.
- A `pub use inner::*;` glob puts `inner`'s items — and the `pub` modules under
  it — on a library's public surface, and the root set follows it. A *named*
  `pub use` of a **module** (`pub use inner::sub;`) reaches the same place by a
  route the rule does not take, so an item under one whose only referrer is
  dead is still reported though a consumer can name it
  ([#28](https://github.com/rlorenzo/deadwood/issues/28); simulating the fix
  changed no finding on any fixture, on the 34 crates in a local registry, or
  on Deadwood itself).
- Anything that resolves ambiguously (a name behind two modules, an alias
  chain we cannot follow) is treated as used.
- A dependency whose name is a common word (`log`, `time`, `bytes`) is kept
  alive by any mention of that word anywhere in the package, including in
  macro input and doc comments. Findings are lost, never invented.
- A dependency declared to turn on a feature of a *transitive* dependency
  (`getrandom = { features = ["js"] }`), to select a vendored native library,
  or to force feature unification is named by no code and no `[features]`
  entry, and is reported. Nothing syntactic separates it from a stale entry,
  so the answer is intent: list it under `[dependencies]` in `deadwood.toml`.
- The dependency check judges the source tree in front of it. A crate
  unpacked from a published `.crate` archive usually has `tests/` and
  `benches/` stripped, so the dev-dependencies they used are reported —
  correctly for that tree, not for the repository it came from.
- One mention through a macro, an attribute, or a doc comment is enough to
  make an entry unplaceable, so the misplaced-dependency check is much quieter
  than the unused one. Across the 34 crates in a local registry it reports
  nothing at all.
- A file that both a `#[cfg(test)]` `mod` declaration and an ungated one
  reach is attributed to the ungated one, so what it names is judged as
  library code. One file gets one answer, and this is the direction that
  misses findings rather than inventing them.
- A `[target.'...'.dependencies]` table keyed by a bare target triple rather
  than a `cfg(...)` expression is not modelled, so narrowing `target-os` does
  not reach its entries; they are judged as if always built.
- A baseline entry suppresses *every* finding that shares its key. The item's
  module is part of that key, so two same-named items in one file are two
  entries — but two definitions sharing a file, a name *and* a module are not
  ([#30](https://github.com/rlorenzo/deadwood/issues/30)). That is `pub struct
  Group` beside `#[allow(non_snake_case)] pub fn Group(..)`, which Rust
  separates by namespace and the key does not model, and two
  `#[cfg]`-alternative definitions of one item, where covering both with one
  entry is the right answer. Since the key deliberately ignores the line, there
  is no way to say which occurrence is the new one, and pointing at a baselined
  line would be a wrong finding rather than a missed one.
- The baseline key includes the file path, so moving a file un-baselines every
  finding in it and makes every entry that recorded them stale
  ([#17](https://github.com/rlorenzo/deadwood/issues/17)). `--prune-baseline`
  then `--write-baseline` is the workaround, at the cost of re-accepting
  anything else that regressed in between.

## License

MIT — see [LICENSE](LICENSE).
