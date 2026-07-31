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

## Shipped phases

Each phase is one slice, and each shipped with its decisions, rejected
alternatives, corpus measurements and mutation runs written down in full in
[`HISTORY.md`](HISTORY.md). This index is the short version.

1. **Path-aware usage resolution** — the name census replaced by per-crate
   symbol-table resolution (`src/resolve.rs`): `use` trees and renames,
   `crate::`/`self::`/`super::`, cross-crate paths between workspace members.
   Unresolvable paths still count as uses of everything they could name.
2. **Unused dependency detection** — `unused_dependency`: a manifest entry
   whose crate name the declaring package's code never mentions, through any
   channel (`src/deps.rs`).
3. **Configuration file** — `deadwood.toml` (`src/config.rs`): `ignore` globs,
   per-kind severity, `public-api` and dependency allowlists; every default is
   the unconfigured behavior. Closes #4, #9.
4. **`cfg` awareness** — gates evaluated against a build matrix instead of
   always followed (`src/cfg.rs`); new `unsatisfiable_cfg` finding. Closes #5.
5. **Misplaced dependency kinds** — `misplaced_dependency`: a `[dependencies]`
   entry only dev targets reference, a `[build-dependencies]` entry the build
   script never touches. Closes #10.
6. **Baseline file** — `--write-baseline` records today's findings
   (`src/baseline.rs`); later runs subtract them and fail only on what is new.
   Closes #6.
7. **A `mod` declaration's gate reaches the file it names** —
   `#[cfg(test)] mod tests;` makes the out-of-line file test code, as the
   inline form always was. Closes #14.
8. **Lexical scope tracking** — a local binding no longer marks a same-named
   module item used; namespace-aware (values and types shadow apart).
   Closes #8.
9. **Reachability, not reference counting** — an item is alive only when a
   walk from the root set reaches it, so dead subsystems and dead cycles come
   out in one run. Closes #21.
10. **A `test_only_item` finding kind** — an item only test code reaches;
    ships `off` because every `#[cfg(test)]` helper is a candidate.
    Closes #23.
11. **A `pub use` glob is public surface** — the glob closure phase 10 built
    becomes the surface rule itself. Closes #25.
12. **The baseline key gains the module path** — two same-named items in one
    file stop sharing an entry. Closes #16.
13. **A baseline entry survives a moved file** — a second matching pass over
    the identity a move preserves (kind, package, module, name). Closes #17.
14. **The two spellings of a `#[cfg(test)] mod` agree** — inline and
    out-of-line confinement give the entry-point split one answer. Closes #27.
15. **A named `pub use` of a module is public surface** — the third route onto
    the surface, followed to a fixed point. Closes #28.
16. **The baseline key gains the namespace** — a `struct` and a `fn` sharing a
    file, a name and a module stop sharing an entry. Closes #30.
17. **The identity a moved dead file does not have** — #32 measured and closed
    as working as intended; the content-hash design is written down for
    whoever picks it up. Found and filed #39 while measuring.
18. **The module tree an `include!` splices in** — a spliced file and the
    `mod` chains under it are not dead files; 246 false positives in
    `windows-sys` gone. Closes #39.
19. **What a `use` alias actually binds** — an alias's namespace resolved from
    its target instead of recorded as `both`. Closes #37.
20. **A `#[test]` function is test code** — bare `#[test]`/`#[bench]` confine
    the function they sit on, so what it names is dev code with no
    `#[cfg(test)]` anywhere near it. Closes #44.
21. **A `[dev-dependencies]` entry the library names** — the claim phase 5
    refused, made once both known mis-attribution sources (#14, #44) were
    closed. Closes #42.
22. **A crate is not always spelled like its entry** — `extern crate real as
    alias;` and `use real as alias;` fold mentions onto the crate they bind,
    scoped to where the binding actually holds. Closes #48.
23. **An item an attribute macro owns is macro input** — a runtime item under
    an attribute Deadwood cannot expand has its mentions moved to the opaque
    context instead of read as library code, which closes the one shape in
    which the placement check invented a finding. Closes #49.
24. **The opaque guard's population, counted** — #50 measured and closed as
    working as intended: zero missed findings in the canonical corpus, and
    every blocked claim in a 330-crate sweep a suppression of noise. Found and
    filed #55 while measuring. Closes #50.
25. **A claim is judged on the entry's own evidence** — a crate declared in
    both `[dependencies]` and `[dev-dependencies]` no longer has the library
    mentions that justify the normal copy held against the dev copy; two live
    invented findings in the extended registry gone. Closes #55.
26. **One place, spelled two ways** — the baseline note prints its path as
    configured on every platform: when the workspace root and a config-derived
    path disagree about a symlink (macOS's `/var`), the display strips against
    the canonical spelling of both. The macOS gate runs clean. Closes #53.
27. **A doubled dev copy is judged on what it enables** — a dev copy that
    turns on a feature the `[dependencies]` copy does not, default features
    it opted out of, or that sits beside an `optional` normal copy, is load
    bearing however silent the tests are; only a copy with neither dev
    mentions nor anything extra to enable is a stale duplicate, and it is
    worded as one rather than as a move. 42 invented findings in zed gone, 6
    more re-worded to the claim that is actually true of them. Closes #61.

28. **A `mod` declaration inside a macro token stream is a claim** — read
    without expanding the macro and used only to spare files: literal `mod`s
    in invocation arguments (tokio's `cfg_fs!`) and in `macro_rules!` bodies
    resolved at every invocation site (serde's `crate_root!`), and the
    invocation idents of a macro whose rules say `mod $x`
    (`supported_targets!`), probed under the rules' inline-module prefix.
    Spared files take the `include!` boundary: not dead, not resolved. 794
    dead-file findings across five workspaces gone — tokio 381 → 0, rust
    417 → 10, serde 36 → 1, rustdesk 6 → 0 — with every other finding kind
    byte-identical. Closes #60.

29. **A reference that exists only for rustdoc claims nothing false** —
    `doctest` rides the `test` axis, so `#[cfg(doctest)]` confines to a test
    build the way `#[cfg(test)]` always has; an item gated to rustdoc's
    doctest build has rustdoc as its consumer and is exempt from item
    findings (`ReadmeDoctests`); and the text of a `#[doc = "..."]` attribute
    inside a macro body is documentation, mined for mentions like any doc
    comment. Ten invented findings gone across regex, clap, tokio and
    rust-lang/rust. Closes #63.

30. **A derive's expansion names its base crate** — a mentioned `X_derive`,
    `X_macros` or `X_impl` declared beside `X` counts as evidence for `X`,
    because `#[derive(Serialize)]` emits `extern crate serde as _serde;` and
    the manifest must satisfy it. The invented finding in tokio's `examples`
    package gone; the price is the mirror image, a genuinely stale base
    beside a live companion, missed rather than invented. Closes #64.

31. **A dependency is spelled by its lib target name** — code writes
    `md5::` for the package `md-5` and `webpki::` for `rustls-webpki`, and
    matching mentions against the package name reported both unused against
    code that uses them. Lib names now come from a full
    `cargo metadata --frozen` where a current lockfile and cached
    resolution exist — any
    checkout that has built once — from the `--no-deps` view for workspace
    members always, and from the package-name heuristic, unchanged, where
    neither can see. A `rename` still sets the extern name outright. Both
    verified deno false positives gone; deno's third `rustls-webpki`
    finding, in a package that names it nowhere, correctly remains.
    Closes #62.

## Next (sequenced, one slice at a time)

1. **Roots the reachability walk cannot see** — a workspace entered from
   outside Rust (bun: a `staticlib` linked into C++, entered through
   `#[no_mangle]` sites and ~800 macro-generated `extern "C"` shims) has no
   Rust-visible chain above whole subsystems, so live items read as dead.
   The direction wants deciding before the code: recognize an unexpandable
   attribute macro as a possible export, an entry-point allowlist in
   `deadwood.toml`, or a crate-type policy for what `pub` means in a
   `staticlib`/`cdylib` — and five of the audited false positives sit behind
   ordinary functions, not yet root-caused. #74, population measured there.

The roadmap and the issue list say the same thing, so neither can quietly
rot. What shipped is in the index above and in [`HISTORY.md`](HISTORY.md).

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
  README — and *filed*: an unfiled limitation is one nobody is counting, and
  one (phase 22's #48) turned out to be masking a live false positive.
- New dependencies only for confirmed problems; std + `syn` + `proc-macro2` +
  `serde` + `serde_json` + `clap` + `anyhow` + `toml` is the current ceiling.
  (Path resolution needed no new crate, only `syn`'s `visit` feature; unused
  dependency detection needed none at all — `cargo metadata` already reports
  the manifest. `toml` arrived with the config file in phase 3, parse-only, and
  was the only addition: glob matching stayed in-tree, and `cfg` evaluation in
  phase 4 was `syn` attribute walking.)
