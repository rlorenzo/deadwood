# How Deadwood decides

The rules behind each finding, moved here from the README. The tenets they follow are in [`SCOPE.md`](SCOPE.md).

## What each check reports

| Check | What it finds | Why rustc doesn't |
| --- | --- | --- |
| **Dead files** | `.rs` files in the directories a package's crate roots sit in — `src/`, or wherever the manifest points — not reachable from any target root in the workspace via `mod` declarations — including ones a macro token stream declares, which are followed without expanding it — or through an `include!` naming a file we can read | Files outside the module tree are never compiled, so no lint ever sees them |
| **Unused pub items** | Fully-`pub` fns, structs, enums, traits, type aliases, consts, statics, and unions that nothing live in the workspace reaches — either no path resolves to them, or every path that does is written inside something itself unreachable | `dead_code` assumes `pub` items have external consumers |
| **Unused re-exports** | `pub use` re-exports nothing live in the workspace goes through, where outside code cannot reach them either | `unused_imports` only sees imports the crate itself does not use, not ones re-exported for nobody |
| **Unused dependencies** | `Cargo.toml` entries — normal, dev, or build — whose crate name the declaring package's code never mentions | Cargo has no reason to look, and an unused entry still costs build time and supply-chain surface |
| **Misplaced dependencies** | `Cargo.toml` entries declared in a table the code naming them cannot see: a `[dependencies]` entry only tests, examples and benches use, a `[dev-dependencies]` entry the library itself names — a manifest that does not compile — or a `[build-dependencies]` entry the build script never touches | Cargo builds the entry wherever you put it for the two directions that compile, and a normal dependency only your tests need is compiled by everyone who depends on you. The third does not compile — but the error lands on the first line that used the crate, naming neither the entry nor the table it should have been in |
| **Unsatisfiable `cfg` gates** | `#[cfg(...)]` gates that can hold in no build of the package, e.g. a `mod` behind a feature the manifest does not declare | The code is never compiled, so no lint ever sees it — and the gate reads as deliberate |
| **Test-only public items** *(off by default)* | `pub` items the workspace reaches only through its test code — reached, so not dead, but `pub` for nobody | `dead_code` says nothing about an item in a test, bench or example target, since the only build that compiles one also uses it. Where rustc *can* see the item it usually does report it, in a build with the tests left out — see [Known limitations](LIMITATIONS.md) |

## Resolution and reachability

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
opt-outs, `#[cfg(doctest)]` items — rustdoc's doctest collection is their
consumer, and the `pub struct ReadmeDoctests;` idiom is deliberately
referenced by nothing — everything `[public-api]` declares, **a library's
public surface** —
every `pub` item under `pub` modules from the crate root, and everything a
`pub use` re-exports from the crate root or from one of those modules, whether
it is written as a glob (`pub use inner::*;`) or names a module outright
(`pub use inner::sub;`, with or without an `as`) — `inner` itself need not be
`pub`, which is the whole point — since
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
[`[severity]`](CONFIGURATION.md). Nothing on a library's public surface is ever
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

- **Test-confined code counts as test code wherever it sits**, so the unit
  tests inside a library do not make every dev-dependency they use look
  misplaced. That is the single largest false positive the check could make.
  `#[cfg(test)]` is one way to write it and `#[test]`/`#[bench]` is the other:
  rustc leaves the function those sit on out of every build that is not a test
  build, so a bare `#[test] fn` at module scope is test code with no
  `#[cfg(test)]` in sight. Only the built-in, single-segment attribute counts
  as confinement — an attribute macro Deadwood cannot expand
  (`#[tokio::test]`, and the `#[core::prelude::v1::test]` the built-in one is
  also spelled as) *owns* the item instead: the item is that macro's input,
  what survives expansion and in which build is unknowable, so a runtime
  item's mentions land in the opaque context — known used, unknown where —
  and place nothing in either direction. Built-in attributes, tool attributes
  (`#[rustfmt::skip]`) and derive helpers (`#[serde(..)]` beside its
  `#[derive]`) rewrite nothing and leave their item attributed as written.
- **A mention in a doc comment places nothing.** Doc examples are compiled as
  doctests, which link the normal *and* the dev dependencies, so a crate named
  in one is correctly declared under either table.
- **A mention through a macro, an attribute, or a file no `mod` declaration
  names places nothing either.** These keep an entry alive for the unused check
  precisely because we cannot see through them; a reference that cannot be
  attributed to a target cannot prove misplacement. This is most of what the
  check gives up.
- **A dev-dependency the library names is reported**, as of phase 21 — the
  claim phase 5 refused. "The library names it" describes a manifest that does
  not compile, and what kept it unmade was never the reasoning but the risk that
  a mis-attribution of ours would invent the finding. Both known sources are
  closed, and with them closed the claim has no candidates in the 35-crate
  registry corpus nor in Deadwood itself, where it had two before.
- **The two directions need different evidence, and that is deliberate.** An
  entry moves *down* only when every mention is dev code, since one library
  mention justifies it where it is. It moves *up* on a single runtime mention,
  since the library cannot link it at all — test code naming it too changes
  nothing about the build that fails.
- **A crate declared in both `[dependencies]` and `[dev-dependencies]`** —
  usually so the tests get extra features, the way `zerocopy-derive` declares
  `syn` twice — is judged per entry: the library mentions justify the normal
  copy and are never held against the dev copy, whose own evidence is the dev
  mentions. A doubled dev copy dev code names is correctly placed; one nothing
  dev names is reported as the stale duplicate it is.

`cfg` gates are evaluated rather than always followed, but the *default* set of
builds analyzed is the union of every possibility — every feature on and off,
every target, tests included — so a gate is followed whenever it could hold
anywhere. That is exactly the old always-follow behavior;
[Configuration](CONFIGURATION.md) is where a project narrows it. What the
evaluation adds is a finding: a gate that can hold in *no* build, which in
practice means one naming a feature the manifest does not declare. Such a gate
is reported and the code behind it is still analyzed, so the new finding never
moves the others. Gates Deadwood cannot read at all —
`cfg(accessible(..))`, a `cfg` a build script sets, a `cfg_attr` indirection —
are followed as before.

Re-exports get one extra filter, because a `pub use` exists *only* to expose a
name outward: one that is reachable from a library's crate root (`pub use
inner::Thing;` in `lib.rs`, in any `pub mod` under it, or in a module a
`pub use inner::*;` glob or a `pub use inner::sub;` re-export carries there) is doing its job
even when nothing inside the workspace uses it, so it is never reported. A
re-export that outside code cannot reach — because some module on the way is
private — has no such excuse, and is reported. A `use` names what it imports
on the bound name's behalf, so such a re-export stops keeping its target
alive: the definition under it is reported alongside it, because deleting one
does not delete the other.

## Pipeline

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
6. **Detectors** — dead files are `src/**.rs` minus the reachable set, the
   spliced-in set and the `cfg`-excluded set; unused pub items and re-exports are the definitions
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
   are subtracted by kind, file, name, module and namespace, and recorded
   entries that matched nothing are reported stale (`src/baseline.rs`).
9. **Reporting** — grouped text or JSON (`src/report.rs`).
