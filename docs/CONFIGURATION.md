# Configuration

Deadwood needs no configuration. With no `deadwood.toml` it behaves as the
[README](../README.md) describes. The file expresses what the analysis cannot
infer: which files are not yours to fix, which checks you are ready to enforce,
which crates have consumers Deadwood cannot see, which manifest entries are load
bearing without being named in code, and which builds (features, targets, tests)
you care about.

Deadwood walks up from the analyzed path to the workspace root and uses the
nearest `deadwood.toml`. `--config PATH` overrides the search and fails if that
file is missing. Relative patterns resolve against the directory holding the
file.

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

Five things are worth knowing.

**`ignore` suppresses findings, not evidence.** An ignored file is still read
and its paths still count as uses: generated code that calls your `pub fn` is
still calling it. It does affect module resolution. A `mod` declaration pointing
at a *missing* file the patterns cover is skipped silently instead of warned
about, so ignoring a generated module does not stop Deadwood checking the rest
of its package.

**Severity is per kind, and only `deny` fails the run.** A `warn` finding is printed with its group marked
`(warn)` and carries `"severity": "warn"` in the JSON, so a project can adopt a
check as advisory before enforcing it. An `off` finding is absent from the
output, the JSON and the count. `test_only_item` is `off` until a `[severity]`
entry asks for it, which is why adding that kind changed no output anywhere.

**`public-api` covers unused-pub items and unused re-exports**, since a
`pub use` is surface too. An allowlisted dependency entry that *is* referenced
is not an error; the list means "do not judge this".

**`cfg` narrows what is analyzed, not what is reported.** Code the matrix leaves
out is not read, so it neither defines nor uses anything. It is not a dead file
either, because nothing reaches it only in the sense that this build does not
contain it. So `test = false` turns a test-only helper into an unused-pub
finding, which is either the question you wanted answered or a page of noise,
depending on the project.

This overlaps `test_only_item` without replacing it. The matrix takes the tests
out of the *build*, so it also takes them out of the evidence: a dev-dependency
only the tests use becomes an unused-dependency finding, a `#[cfg(test)]`-only
file becomes a dead one, and the item is reported as `unused_pub_item`, whose
message says it is dead. `test_only_item` keeps the tests in the build, changes
no other check's answer, and says what to do instead. The matrix has the better
recall (it does not care that an `assert_eq!` names the item); the kind has the
narrower blast radius and the truer message.

**`baseline` names a file.** A run reads the baseline whether or not the key is
present, from where the key points or else from the default location if a file
is there. The key changes the error contract: a path you wrote down must exist,
while the default location may be empty. `ignore` and `severity = "off"`
outrank the baseline, since a finding they remove never exists to be suppressed,
which makes any baseline entry for it stale and prunable.

Configuration mistakes, including unknown keys, are hard failures (exit 2). A
misspelled key names itself, its file, and the keys that do exist, so a typo
cannot leave a `deadwood.toml` quietly doing nothing:

```console
$ deadwood check
error: invalid config file `deadwood.toml`: TOML parse error at line 1, column 1
  |
1 | ignor = ["vendor"]
  | ^^^^^
unknown field `ignor`, expected one of `ignore`, `severity`, `public-api`, `dependencies`, `cfg`, `baseline`
```
