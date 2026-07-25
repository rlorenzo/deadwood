# Deadwood

A codebase health analyzer for Rust workspaces. Deadwood finds maintainability
issues that `rustc` and `clippy` stay quiet about — starting with dead module
files, unused `pub` items, unused re-exports, unused dependencies, and `cfg`
gates that can never hold, in the spirit of Fallow/knip-style analyzers for
other ecosystems.

**Status:** v0.1 — early, narrow, and honest about it. Tunable through a
`deadwood.toml`, and correct without one. See
[`docs/SCOPE.md`](docs/SCOPE.md) for what is in and out of scope, and
[`docs/ENVIRONMENT.md`](docs/ENVIRONMENT.md) for the environment assessment
this project was bootstrapped from.

## What it detects today

| Check | What it finds | Why rustc doesn't |
| --- | --- | --- |
| **Dead files** | `.rs` files under `src/` not reachable from any target root via `mod` declarations | Files outside the module tree are never compiled, so no lint ever sees them |
| **Unused pub items** | Fully-`pub` fns, structs, enums, traits, type aliases, consts, statics, and unions that no path in the workspace resolves to | `dead_code` assumes `pub` items have external consumers |
| **Unused re-exports** | `pub use` re-exports nothing in the workspace goes through, where outside code cannot reach them either | `unused_imports` only sees imports the crate itself does not use, not ones re-exported for nobody |
| **Unused dependencies** | `Cargo.toml` entries — normal, dev, or build — whose crate name the declaring package's code never mentions | Cargo has no reason to look, and an unused entry still costs build time and supply-chain surface |
| **Unsatisfiable `cfg` gates** | `#[cfg(...)]` gates that can hold in no build of the package, e.g. a `mod` behind a feature the manifest does not declare | The code is never compiled, so no lint ever sees it — and the gate reads as deliberate |

What each check reports can be tuned by a `deadwood.toml` — see
[Configuration](#configuration).

Usage is decided by *resolving paths*, not by counting identifiers: `use`
declarations (renames, nested trees, `pub use`), qualified paths (`crate::`,
`self::`, `super::`), and cross-crate paths between workspace members are
resolved against a per-crate symbol table. So two items sharing a name no
longer hide each other, and a type mentioned only inside its own `impl` block
is still reported.

The bias is still toward staying quiet rather than raising noise. Anything
that cannot be resolved counts as a use of *every* item with that name:
identifiers inside macro invocations and attribute arguments, and names in a
module holding a glob import that leads outside the workspace. Items marked
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
inner::Thing;` in `lib.rs`, or in any `pub mod` under it) is doing its job
even when nothing inside the workspace uses it, so it is never reported. A
re-export that outside code cannot reach — because some module on the way is
private — has no such excuse, and is reported.

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

Unused re-exports:
  src/lib.rs:11: `pub use` re-export of `Stale` is never referenced through this module

Unused dependencies:
  Cargo.toml: dev-dependency `tempfile` is never referenced by any target of package `demo`

6 finding(s) in workspace `/path/to/workspace`.
```

- `deadwood check [PATH]` — analyze the package/workspace at `PATH` (default `.`)
- `--json` — machine-readable output (findings + warnings)
- `--config PATH` — use this configuration file instead of searching for
  `deadwood.toml`
- Exit codes: `0` clean, `1` findings that are configured `deny` (the default
  for every kind), `2` error — suitable for CI gates.

Requires `cargo` on `PATH` (workspace discovery shells out to
`cargo metadata --no-deps`, which works offline).

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
# deadwood.toml — every setting, with its default behavior noted.

# Files no finding may be reported about. Patterns are `/`-separated globs
# where `*` stays inside one segment, `**` spans any number of them, and `?` is
# one character; a pattern matching a directory covers everything under it.
# Default: nothing is ignored.
ignore = ["crates/*/src/generated/**", "vendor"]

# What each finding kind costs. `deny` reports it and fails the run (exit 1),
# `warn` reports it and exits 0, `off` never reports it at all. The keys are
# the finding kinds as they appear in `--json`.
# Default: `deny` for every kind, which is the pre-config behavior.
[severity]
dead_file = "deny"
unused_pub_item = "warn"
unused_reexport = "warn"
unused_dependency = "deny"
unsatisfiable_cfg = "deny"

# Crates and items whose `pub` surface is API rather than leftovers. Deadwood
# only sees consumers inside the workspace, so for a published library this is
# the difference between a usable report and a page of noise.
# Default: nothing is treated as declared API.
[public-api]
# Every `pub` item in these crates. Dashes and underscores are interchangeable.
crates = ["my-library"]
# ...or `crate::module::Item` paths, as globs, for finer control.
items = ["my-app::prelude::*", "my-app::error::**"]

# Manifest entries the unused-dependency check must never judge: the ones that
# are load bearing without any code naming them. Matched on the manifest key
# exactly as written, so a renamed entry is listed by its alias.
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
```

Four things are worth knowing about how these behave.

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
finding does not exist: it is absent from the output, the JSON, and the count.

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
project. The `unsatisfiable_cfg` finding is the one thing the matrix does *not*
affect — a gate is judged impossible against every build there could be, so
narrowing the matrix never invents one and never silences one.

Configuration mistakes are hard failures (exit 2), including unknown keys. A
`deadwood.toml` that quietly does nothing because of a typo is worse than none
at all, so a misspelled key names itself, its file, and the keys that do exist:

```console
$ deadwood check
error: invalid config file `deadwood.toml`: TOML parse error at line 1, column 1
  |
1 | ignor = ["vendor"]
  | ^^^^^
unknown field `ignor`, expected one of `ignore`, `severity`, `public-api`, `dependencies`, `cfg`
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
   from the module it is written in, marking what it names (`src/resolve.rs`).
5. **Detectors** — dead files are `src/**.rs` minus the reachable set and the
   `cfg`-excluded set; unused pub items and re-exports are the definitions no
   resolved path reached (`src/unused.rs`); unused dependencies are the
   manifest entries whose crate name appears nowhere in the package, reachable
   or not (`src/deps.rs`).
6. **Configuration** — `deadwood.toml` is applied in one pass over the
   findings, so `ignore` and `[severity]` cover every detector identically
   (`src/config.rs`); `public-api`, the dependency allowlist, and the `cfg`
   matrix are consulted by the detectors they belong to.
7. **Reporting** — grouped text or JSON (`src/report.rs`).

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
- Under the default matrix `#[cfg(test)]` code counts as a use, so an item
  only tests reach is not reported. `[cfg] test = false` asks the other
  question, and the answers are `unused_pub_item` findings rather than a kind
  that says "test-only" — proving that would need reachability analysis
  Deadwood does not have yet (`docs/SCOPE.md` has the reasoning).
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
- A `[target.'...'.dependencies]` table keyed by a bare target triple rather
  than a `cfg(...)` expression is not modelled, so narrowing `target-os` does
  not reach its entries; they are judged as if always built.

## License

MIT — see [LICENSE](LICENSE).
