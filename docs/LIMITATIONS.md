# Known limitations

Tracked here rather than hidden. Almost all of them cost findings rather than invent them.

- Resolution is syntactic, not semantic: method calls (`x.foo()`), trait
  dispatch, and associated items are not resolved. Only free-standing item
  definitions are ever reported, so this costs findings, never precision.
- Macro input is not expanded, so an identifier inside a macro invocation
  counts as a use of every workspace item with that name. The same goes for
  attribute arguments, including paths hidden in strings
  (`#[serde(with = "crate::codec")]` keeps everything in `codec` alive).
  A `mod` declaration inside a macro token stream *is* read — as a claim, not
  a resolution: the file it names is spared from the dead-file check
  (tokio's `cfg_fs! { pub mod fs; }`, serde's `crate_root!`, and the
  invocation idents of a macro whose rules say `mod $x`, like
  `rustc_target`'s `supported_targets!`, all count), but the items *in* such
  a file are still invisible to resolution: the module path the macro gives
  them is unknowable without expansion, so they are neither reported nor able
  to keep anything else alive.
- Module-declaring macros are followed across crates and through each other:
  `bun_lint::rules! { .. }` defined in one crate and invoked from another
  expands to `pub mod rules { $( $crate::rule_module! { .. } )* }`, and the
  `pub mod $module;` in `rule_module!` is probed under `rules/` at the
  invocation site. Which macro an invocation means is decided by its last path
  segment, so two workspace macros sharing a name both contribute their
  claims, which only ever spares files.
- An `include!` path is followed when it is a literal or a `concat!` of
  literals and `env!`s whose values the tree or the environment gives:
  `CARGO_MANIFEST_DIR` is the including package's directory, `OUT_DIR` is the
  newest build-script output directory cargo made for the package (under
  `CARGO_TARGET_DIR` or the nearest `target/`), and any other variable is
  whatever Deadwood is run with. So generated code is read like any other
  file once a build has made it: bun's thunks are under
  `env!("BUN_CODEGEN_DIR")`, and running Deadwood with that variable set reads
  them. Results then depend on the environment, as the build's do.
- Code that still cannot be read keeps alive what it could name. A package
  that `include!`s a file Deadwood cannot open has every item kept alive, and
  so does the public surface of every workspace crate it depends on, directly
  or not. On a bun checkout without its codegen output, `bun_runtime` depends
  on nearly the whole workspace, so the unused-pub check reports almost
  nothing there. A warning names each such package.
- Lexical scopes are tracked syntactically, so a binding a macro expands to
  shadows nothing — though an identifier in macro input already counts as a
  use of every item with that name, so the two errors point the same way. A
  bare name in pattern position is read as a *use* whenever it could name a
  unit struct, a variant or a `const`, which costs a finding for the braced
  struct and type alias it could equally be binding over.
- In a module with a `#[cfg]` on a glob import, two globs defining one name are
  read as alternatives, so a use of the name counts for both. Elsewhere the first
  glob to answer wins, so an alternative whose gate sits on its `mod` rather than
  its `use` can still be reported dead.
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
  under [Configuration](CONFIGURATION.md) is why both exist.
- The test-only claim is narrower than it sounds, in two directions that both
  cost findings rather than invent them. **Anything a consumer could name is
  out**: a library's public surface, whatever a surface item reaches, and
  anything `[public-api]` declares. That still covers everything a `pub use
  inner::*;` glob or a `pub use inner::sub;` re-export carries out, but by the
  ordinary route rather than a rule of its own — either form *is* public
  surface, so it is a root in both walks like the rest of it. And **an opaque mention keeps an item out entirely** —
  a name in macro input is a root, and `assert_eq!(thing(), 1)` is how most
  tests name what they test, so one assertion is enough.
- What a `#[cfg(test)] mod` confines is test code whichever way the module is
  written: an entry point inside an inline `#[cfg(test)] mod tests { ... }`
  that is not itself `#[test]`/`#[bench]` — a `#[no_mangle]`, an
  `#[allow(dead_code)]`, an `#[allow(unused)] use` — is a test root exactly as
  the same code in a `#[cfg(test)] mod tests;` file is. The gate is read in
  full, so `all(test, feature = "x")` confines a module, `any(test, unix)` does
  not, and a module nested inside a confined one is confined whatever its own
  gate says.
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
- An `include!` is followed when its path is a string literal and the chain of
  them is no more than eight deep: the file it names is not dead, and neither is
  anything a `mod` chain from that file reaches. Two forms are not followed, and
  both cost noise rather than silence — a path only the build knows
  (`include!(concat!(env!("OUT_DIR"), ...))`), and one more than eight `include!`s
  from a crate root. Files they would reach are still reported dead, because a
  file Deadwood could not read is not evidence that anything beside it is alive.
- A file reached **only** through an `include!` has its *items* take no part in
  resolution: nothing in one is reported as an unused public item, and nothing
  in one keeps another item alive. The file is evidence that it was reached and
  evidence of nothing else. A file a `mod` chain also reaches is analyzed
  exactly as it always was — the exemption is for files the module tree would
  otherwise never have seen, not for a file that happens to be spliced as well.
  Admitting the items would be a finding population of its own — measured at
  132,414 unused public items in `windows-sys-0.61.2` alone, against the 10 it
  reports today.
- A `pub` item with consumers outside the workspace looks identical to a dead
  one; for library crates, these findings are advisory until the crate or its
  item paths are listed under `[public-api]`. Re-exports on a library's public
  surface are skipped for the same reason, which also means a genuinely dead
  one there is missed.
- A `pub use inner::*;` glob and a named `pub use inner::sub;` both put the
  module's items — and the `pub` modules under it — on a library's public
  surface, and the root set follows both. What neither reaches is a re-export
  that leads out of the workspace, or one resolution cannot follow: it puts
  nothing on the surface at all, which costs findings rather than inventing
  them. A re-export written in a module that is not itself on the surface roots
  nothing either, since outside code cannot name it to go through it.
- Anything that resolves ambiguously (a name behind two modules, an alias
  chain we cannot follow) is treated as used.
- A dependency whose name is a common word (`log`, `time`, `bytes`) is kept
  alive by any mention of that word anywhere in the package, including in
  macro input and doc comments. Findings are lost, never invented.
- A dependency whose lib target name differs from its package name (`md-5`
  builds a lib named `md5`, `rustls-webpki` one named `webpki`) is matched by
  the *lib* name wherever a current lockfile and a cached, offline-resolvable
  dependency graph exist — any checkout that has built once — and by a workspace member's
  targets always. A non-default optional dependency needs to be cached too, which
  takes a build with that feature on. On a cold cache the package-name heuristic runs as before,
  which can report such an entry against code that uses it; building the
  project once is the fix.
- A dependency named only by the code a derive expands to is kept alive by
  its companion: a mentioned `X_derive`, `X_macros` or `X_impl` declared
  beside `X` counts as evidence for `X`, because the expansion names the base
  crate (`#[derive(Serialize)]` emits `extern crate serde as _serde;`). The
  price is the mirror image: a genuinely stale `X` beside a live companion is
  missed, never invented.
- A dependency named only by what a proc macro expands to is kept alive by
  reading the macro: every identifier its `quote!`-family invocations write as
  the head of a path (`::x::Y`, `extern crate x`) and every word of its
  whitespace-free string literals (`"bevy_extract"`, `"{}::Key"`) counts as a
  mention by a package that uses the macro. What counts is per macro: the
  function behind the derive, attribute or function-like macro the package
  names, the functions it calls by path (`helper(..)`, `utils::get_path(..)`;
  method calls are not followed, since `.parse()` would join every derive to
  every other), and code outside any function. The macro's helper libraries
  are read with it — the packages it depends on, however indirectly, that
  depend on `quote` themselves (`bevy_macro_utils`, `tauri-codegen`,
  `tauri-utils`). A package is a user when it names one of the macro's entry
  points and its dependencies reach the proc-macro package, however
  indirectly, which covers a derive re-exported by a library (bevy's
  `ExtractComponent` through `bevy_render`, naming `bevy_extract`).

  A proc macro from *outside* the workspace is read from the source a full,
  frozen `cargo metadata` points at, which is the registry cache any checkout
  that has built once holds. On a cold cache it is not read, and
  `tauri::generate_context!`'s `::serde_json::Value` leaves an app's
  `serde_json` entry looking unused, or misplaced when only tests name it;
  building once is the fix, and `[dependencies.allow-in]` in `deadwood.toml`
  the alternative. Calls are matched by name, so a helper sharing a name with
  another macro's is followed too, which costs findings rather than inventing
  them.
- A path in a `macro_rules!` body resolves in the crate that invokes the macro,
  so the path heads in a workspace macro's body count as mentions by every
  package that invokes it (ruff's `warn_user_once!` in `ruff_linter` says `use
  colored::Colorize;`). They count by name, with the bodies of the macros a
  body invokes folded in, and with nowhere to place them, so such a dependency
  is never reported misplaced either. A bare identifier in a body is not read
  this way: it is far more often a local (`tokio::join!` binds `futures`).
- A crate renamed by `extern crate real as alias;` or `use real as alias;` is
  followed: every `alias::` in the crate that declares it counts for `real`, so
  a package that renames one dependency to the name of another — `serde_json`
  does exactly this — is not judged against the wrong entry. The rename is
  scoped to the target that writes it, since a test target is a separate crate
  that links the dev-dependencies directly.

  The two spellings bind differently and are followed differently. Only an
  `extern crate real as alias;` written at the **crate root** enters the extern
  prelude, so only that one holds for every module of the crate. A `use real as
  alias;`, or an `extern crate` at the top of a module file like `src/foo.rs`,
  is an ordinary item binding in its own module, so it is followed within the
  file that writes it and no further. A rename inside a nested `mod` or a function
  body is not followed at all. Getting this wrong invents findings rather than
  losing them: a crate that renames one dependency to a name another module
  uses for real would have that second crate's mentions folded away, and the
  entry reported unused.
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
  than the unused one. Across the 35 crates in a local registry it reports
  nothing at all — including after `#[test]` and `#[bench]` started counting as
  confinements, which moved mentions in two of those crates without moving a
  finding in any of them.
- A runtime item an attribute macro owns is opaque, as of phase 23: the item
  is the macro's input, and what survives expansion — the item verbatim, the
  item confined to a test build the way `#[tokio::test]` confines it, or
  nothing — is unknowable before expansion. That closed the one shape in which
  the check invented a finding (a `[dev-dependencies]` entry named only from a
  `#[tokio::test] fn` was reported as belonging in `[dependencies]`, against a
  manifest that compiles), and it costs what opacity always costs: a
  `[dependencies]` entry named only from such a function is not reported as
  belonging in `[dev-dependencies]`, even when it does.

  The cost is **half the size it looks**, and the half that is missing is
  impossible rather than merely unobserved. An attribute macro has to resolve
  in the build the item is compiled into, so `#[rstest]` in library code with
  `rstest` under `[dev-dependencies]` is not a case Deadwood mishandles — it is
  a case that does not compile (`error[E0433]`, verified). That rules out any
  test-macro crate declared *only* as a dev-dependency, which is where
  `rstest`, `test-case`, `serial_test` and `proptest` are conventionally put.
  What remains opaque is an attribute macro from a crate the manifest resolves
  anyway, `#[tokio::test]` being the common spelling rather than the whole of
  it. The boundary has one corner read the other way: an unknown
  *single-segment* attribute sharing an item with a `#[derive(..)]` is
  indistinguishable from that derive's helper (`#[serde(..)]` beside
  `#[derive(Serialize)]`), is read as the inert helper it almost always is,
  and leaves the item attributed as written — the multi-segment spelling stays
  a macro, derive or no derive. A test-confining macro in the helper position
  would still be read as library code; no such spelling has been seen (the
  known test-confining macros sit on bare `fn`s).
- A file that `#[path]` mounts as two modules of one crate has its items, and
  those of the files under it, take no part in resolution, the same as a file
  reached only through an `include!`. Its paths are read from every mount and
  count as uses, so nothing in it is reported and it keeps what it names alive.
- A file that both a `#[cfg(test)]` `mod` declaration and one no gate confines
  to a test build reach is attributed to the second, so what it names is judged
  as library code. One file gets one answer, and this is the direction that
  misses findings rather than inventing them. An inline `mod` two declarations
  in one file reach — `cfg`-alternatives of each other — is answered the same
  way, for the same reason. The declaration that decides it need not be
  ungated: `#[cfg(all(not(test), unix))]` carries a gate and is still compiled
  by a build with no tests in it.
- A `[target.'...'.dependencies]` table keyed by a bare target triple rather
  than a `cfg(...)` expression is not modelled, so narrowing `target-os` does
  not reach its entries; they are judged as if always built.
- A baseline entry suppresses *every* finding that shares its key. The item's
  module and namespace are both part of that key, so two same-named items in one
  file are two entries and so are a type and a value of one name in one module.
  What is left sharing a key is two `#[cfg]`-alternative definitions of one item,
  where covering both with one entry is the right answer — and nothing else, because
  Rust does not compile two definitions of one name in one module unless the
  namespaces separate them. Since the key deliberately ignores the line, there is
  no way to say which occurrence is the new one, and pointing at a baselined line
  would be a wrong finding rather than a missed one.
- One exception to "and nothing else", and it costs a finding rather than
  inventing one: a `pub use` whose target Deadwood cannot resolve records `both`,
  which overlaps everything. The kind is in the key too, so an unused re-export
  and an unused item of one name are already two entries whatever the namespace
  says; where this bites is `test_only_item`, the one kind reported about both,
  and a second `pub use` of that name in that module. It takes a target outside
  the workspace, one behind a glob that leads outside it, or a chain of more
  than eight aliases to get there — and then a third item of that name added to
  the module later before anything is actually missed. Resolvable targets, which
  is nearly all of them, record what they bind.
- Moving a file un-baselines the findings in it for the four kinds that name no
  module: a `dead_file`, an `unsatisfiable_cfg` gate site, and a `Cargo.toml`
  entry of either dependency kind whose whole package moved. The item kinds
  survive a move ([BASELINE.md](BASELINE.md)); these do not, because there is nothing about them that
  a move preserves and guessing would suppress a genuinely new finding.
  `--prune-baseline` then `--write-baseline` is the workaround, at the cost of
  re-accepting anything else that regressed in between. So is editing the two or
  three paths by hand, which the format is meant to allow. This is a settled
  decision rather than a pending one: the alternative was measured in phase 17
  and costs a new field on every entry of the one baseline shape that is
  portable across releases, to convert a noisy failure into a silent one.
- A **whole package directory** moving defeats the second pass for the *item*
  kinds too, not just the four above: an entry's package is resolved by
  containment against the workspace's manifest directories, and a path in no
  package resolves to nothing. It takes a workspace with a member outside the
  root to happen at all — a single-package workspace's package directory *is*
  the workspace root — and none of the 35 registry crates the corpus measures on
  is one.
- The second pass that survives a move is scoped to the package but not to the
  *target*, because no finding carries one and one file can belong to several.
  Within a package every target's crate root is spelled `crate`, so two binaries
  or examples each defining `pub const X` at the root are one identity to it.
