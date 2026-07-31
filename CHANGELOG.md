# Changelog

Notable changes per release. The full phase-by-phase record — every decision,
its rejected alternatives, the corpus measurements and mutation runs behind it
— lives in
[`docs/HISTORY.md`](https://github.com/rlorenzo/deadwood/blob/main/docs/HISTORY.md);
this file is the short version, newest first.

## Unreleased

### Fixed

Nine bugs, all found by running the analyzer over
[bun](https://github.com/oven-sh/bun)'s Rust tree — a 108-crate, ~1M-line
workspace, and the first in the corpus laid out flat, declared through macros,
and entered from C++. Every rule below was settled by handing rustc the
alternative spelling and watching it refuse to compile, not by reading the
reference and hoping.

**`#[path]` resolution.**

- A `#[path]` written inside an inline `mod` block resolved from the declaring
  file's directory rather than from the block's. `#[path = "Sub.rs"] mod sub;`
  inside `mod outer { ... }` in `src/lib.rs` names `src/outer/Sub.rs`, not
  `src/Sub.rs`. A `#[path]` on the inline block itself renames that directory,
  and resolves one level out, from the declaring file's own directory.
- A `#[path]` target only owned its parent directory when it was literally
  named `mod.rs`. Every `#[path]` target owns it — rustc treats them all as
  `mod.rs` files — so `mod child;` written in a `#[path = "body.rs"]` target
  names a *sibling*, `src/child.rs`, and not `src/body/child.rs`.
- `#[cfg_attr(cond, path = "..")]` was not read at all, though the macro token
  scanner had always read it. serde declares the only module of
  `serde_derive_internals` with a pair of them, one arm per build; the
  condition is still not evaluated, the first arm naming a real file is the
  module, and the others are spared rather than reported dead.

**False positives — claims invented out of code that was read.**

- Paths written in a file reached only through an `include!` or a macro token
  stream were thrown away, so every item such a file was the sole caller of was
  reported unused. Their *definitions* stay out of the symbol table, which is
  the original rule and still right: a definition admitted at a guessed module
  path invents claims of its own. A reference is the opposite trade — resolving
  one can only mark definitions reached — so the two halves now separate.
- A `macro_rules!` whose body says `#[path = $f] mod $m;` takes its file names
  from the invocation's string literals, and its module names from idents that
  say nothing about them: bun pairs `"toBeArrayOfSize.rs"` with
  `to_be_array_of_size`, and no rule turns one into the other. Probing only the
  idents missed all forty-eight matcher files. Such a macro defined *and*
  invoked inside another macro's tokens — neither half an item the parser sees
  — now settles itself from the one stream holding both.
- One file reached under two spellings, because a symlinked directory gives it
  two, was reported dead under the name its own package never claimed. serde
  symlinks `serde/src/core` at `serde_core/src`.

**Dead-file coverage.**

- The check walked `<package>/src`, which is a convention rather than a rule:
  the manifest says where a crate root lives, and `[lib] path = "lib.rs"` puts
  it in the package directory. All 101 of bun's crates are laid out that way,
  so the check found no directory to walk and reported nothing across a million
  lines — silence, not a wrong answer, which is why it went unnoticed. It now
  walks the directories the package's own lib and bin roots sit in, stopping at
  a nested package and at the auto-discovered `tests/`, `examples/` and
  `benches/` roots.
- Whether a file is compiled is a question about the workspace, not one
  package: `bun_runtime` mounts a file living in `bun_jsc`'s directory with
  `#[path = "../jsc/generated_classes_list.rs"]`, and asking only `bun_jsc` —
  whose module tree quite correctly never reaches it — reported a file the
  build compiles.

Every one of these was loud or silent rather than wrong in the first instance:
unresolved modules raised warnings and skipped the affected checks, which is
the conservative direction, but the coverage they cost was real. On bun,
resolution now completes with 8 warnings instead of 183 — the remainder are
`include!`s of build-generated files, unreadable without a build — and the run
reports 818 findings instead of 472. On serde, resolving `serde_derive_internals`
un-skipped the workspace-wide unused-pub check and surfaced five real items.
tokio and ripgrep are unchanged, finding for finding.

Excluding a `cfg`-ruled-out module keeps the narrower of the two `#[path]`
rules on purpose: what leaves the build with a file is only the directory the
file *owns*, not the one it merely resolves children from. Sweeping the shared
directory would exclude live neighbours — for `#[path = "body.rs"] mod body;`
in `src/lib.rs`, the whole crate — and an excluded file is withheld from the
dependency check as well as the dead-file one, so over-reaching there would
invent unused-dependency claims rather than merely lose findings.

**Proc-macro entry points**, the eighth find from the same run. A
`#[proc_macro]`, `#[proc_macro_derive]` or `#[proc_macro_attribute]` function
is invoked by the compiler, never by a written path: code spells the derive,
attribute, or macro the function registers, so no workspace will ever name
the function itself, and deleting it — the advice the finding gives — breaks
every crate using the macro. The three attributes already rooted what an
entry point reaches; the entry point itself is now off the report beside
`#[no_mangle]`, whose case this is — an export whose caller the analyzer
cannot see. All 12 on bun's tree were false positives. Across the 1265
unpacked registry crates on this machine (three `windows-0.x`
generated-bindings crates skipped for size, none of them a proc-macro
target), the change removes 244 findings over 82 proc-macro crates — every
one an `unused_pub_item` on an entry-point function, nothing added, no other
finding kind moved — and 62 of those crates, `clap_derive` and
`serde_derive` among them, now run entirely clean.

**Items an attribute macro may have exported**, the ninth. An attribute
macro Deadwood cannot expand receives its item as tokens and may emit
anything beside it — bun's `#[bun_jsc::host_fn]` writes an `extern "C"`
shim around 800 times, handing the item to a caller outside Rust entirely —
so an item under one is no longer reported. The analyzer already refuses to
read through such a macro everywhere else (the item's mentions go to the
opaque context, and its body already keeps alive what it names); the same
refusal now covers the claim made about the item itself, so suppression is
the whole change and nothing below the item moves. The cost is deliberate
and measured: across the same registry sweep, 37 findings over 11 crates
disappear — 18 of them `#[async_trait]` items, which that macro certainly
does not export and the rule cannot know it — while `temporal_capi`, a
diplomat-generated C-FFI crate of exactly bun's shape, and tokio now run
entirely clean, with nothing added and no other finding kind moved. A
`staticlib`/`cdylib` crate-type rule was considered and rejected: a mangled
`pub fn` is not callable over the C ABI, and bun's `*_sys` crates carry
genuinely-unreached blanket `pub` — findings the audit confirmed correct —
that a crate-type rule would have silenced wholesale.

## 1.0.0-beta.1 — 2026-07-30

First public pre-release. The v1 check set is complete; what stands between
this and 1.0 is soak time on codebases that are not this one. Published as
the `deadwood-rs` package, since the plain crates.io name belongs to an
unrelated project; it installs a binary named `deadwood`.

### Checks

- **Dead files** — `.rs` files under `src/` unreachable from any target root
  via `mod` declarations or a readable `include!`.
- **Unused pub items** — fully-`pub` items nothing live in the workspace
  reaches, decided by path resolution and a reachability walk, not by
  counting identifiers: dead subsystems and dead cycles come out in one run.
- **Unused re-exports** — `pub use` re-exports nothing goes through, inside
  the workspace or out.
- **Unused dependencies** — manifest entries (normal, dev, build) whose crate
  the declaring package's code never names.
- **Misplaced dependencies** — entries declared in a table the code naming
  them cannot see (e.g. a `[dependencies]` entry only tests use).
- **Unsatisfiable `cfg` gates** — `#[cfg(...)]` gates that can hold in no
  build of the package.
- **Test-only public items** *(off by default)* — `pub` items the workspace
  reaches only through test code.

### Operation

- `deadwood check [PATH]` with text or `--json` output; exit codes `0`
  clean / `1` denied findings / `2` error, suitable for CI gates.
- `deadwood.toml` configuration: `ignore` globs, per-kind severity,
  `public-api` and dependency allowlists. Every default is the unconfigured
  behavior.
- Baseline adoption: `--write-baseline` records today's findings so later
  runs fail only on what is new; `--prune-baseline` drops entries that no
  longer occur. Entries survive file moves and renames of the paths around
  them.
- Workspace discovery via `cargo metadata --no-deps`; works offline; no
  `unsafe` anywhere in the crate.
