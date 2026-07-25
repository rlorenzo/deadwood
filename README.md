# Deadwood

A codebase health analyzer for Rust workspaces. Deadwood finds maintainability
issues that `rustc` and `clippy` stay quiet about — starting with dead module
files, unused `pub` items, unused re-exports, and unused dependencies, in the
spirit of Fallow/knip-style analyzers for other ecosystems.

**Status:** v0.1 — early, narrow, and honest about it. See
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
into declarations Deadwood never sees. Entries it cannot judge — optional
ones, and `[target.'cfg(...)'.dependencies]`, both gated by a `cfg` Deadwood
does not evaluate — are skipped with a warning instead of guessed at, as is
any package pulling in code from a file that cannot be read
(`include!(concat!(env!("OUT_DIR"), ...))`).

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

Unused public items:
  src/lib.rs:3: pub fn `entry` is never referenced by any resolved path in this workspace
  src/lib.rs:7: pub fn `dead_fn` is never referenced by any resolved path in this workspace

Unused re-exports:
  src/lib.rs:11: `pub use` re-export of `Stale` is never referenced through this module

Unused dependencies:
  Cargo.toml: dev-dependency `tempfile` is never referenced by any target of package `demo`

5 finding(s) in workspace `/path/to/workspace`.
```

- `deadwood check [PATH]` — analyze the package/workspace at `PATH` (default `.`)
- `--json` — machine-readable output (findings + warnings)
- Exit codes: `0` clean, `1` findings, `2` error — suitable for CI gates.

Requires `cargo` on `PATH` (workspace discovery shells out to
`cargo metadata --no-deps`, which works offline).

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
   the module path its items live in (`src/modtree.rs`).
3. **Usage resolution** — every target is a crate. For each one, a symbol
   table maps its modules to the items they define, the `use` aliases they
   bind, and the globs they import; then every path in every file is resolved
   from the module it is written in, marking what it names (`src/resolve.rs`).
4. **Detectors** — dead files are `src/**.rs` minus the reachable set; unused
   pub items and re-exports are the definitions no resolved path reached
   (`src/unused.rs`); unused dependencies are the manifest entries whose crate
   name appears nowhere in the package, reachable or not (`src/deps.rs`).
5. **Reporting** — grouped text or JSON (`src/report.rs`).

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
- `cfg` is not evaluated. `cfg`-gated `mod`s are always followed, so
  platform-specific files are never reported dead, and `#[cfg(test)]` code
  counts as a use — an item used only by tests is not reported.
- `include!()`-ed files are not tracked and may be reported as dead.
- A `pub` item with consumers outside the workspace looks identical to a dead
  one; for library crates, these findings are advisory. Re-exports on a
  library's public surface are skipped for that reason, which also means a
  genuinely dead one there is missed.
- Anything that resolves ambiguously (a name behind two modules, an alias
  chain we cannot follow) is treated as used.
- A dependency whose name is a common word (`log`, `time`, `bytes`) is kept
  alive by any mention of that word anywhere in the package, including in
  macro input and doc comments. Findings are lost, never invented.
- A dependency declared to turn on a feature of a *transitive* dependency
  (`getrandom = { features = ["js"] }`) is named by no code and no
  `[features]` entry, and is reported. An ignore list in the planned config
  file is the intended answer ([#4]).
- The dependency check judges the source tree in front of it. A crate
  unpacked from a published `.crate` archive usually has `tests/` and
  `benches/` stripped, so the dev-dependencies they used are reported —
  correctly for that tree, not for the repository it came from.

[#4]: https://github.com/rlorenzo/deadwood/issues/4

## License

MIT — see [LICENSE](LICENSE).
