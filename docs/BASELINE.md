# Adopting on an existing codebase

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

**Matching survives line drift.** An entry is matched on kind, file, item name,
the module the item is written in and the namespace it binds that name in — not
the line, which moves with every edit above it, and not the severity, which is a
`deadwood.toml` decision: putting it in the key would mean that turning a check
down from `deny` to `warn` un-baselines every finding of that kind at once. The
module path is in the key for the same reason the line is not: it tells
`alpha::twin` from `beta::twin` in one file, and it does not move when code
above it does.

**And it tells a type from a value of the same name.** `pub struct Group { .. }`
and the `#[allow(non_snake_case)] pub fn Group(..)` beside it share a kind, a
file, a name *and* a module; Rust tells them apart by namespace, and so does the
key. The namespace is `type`, `value`, or `both` — a unit or tuple struct binds
a constructor value of its own name, and a `use` alias binds whatever the path
it names binds, resolved through the symbol table like any other path — and two
entries match when the namespaces they stand for overlap, so `both` covers
either half. An alias whose target cannot be resolved keeps `both`: a re-export
leading outside the workspace, or through a glob that does, says nothing about
namespaces rather than guessing at one.

That leaves two definitions sharing a key in exactly one case, and it is the
case where one entry is right: Rust will not compile two definitions of one name
in one module unless they are in different namespaces and neither is in both, so
whatever the key still joins is two `#[cfg]`-alternative spellings of one item —
one item, one place to open, one fix.

**And it survives a moved file, for the kinds that name an item.** `git mv
src/legacy.rs src/legacy/mod.rs` changes no code and no item, so the findings
in it stay suppressed. Matching runs in two passes: the key above first, and
then — over what that pass left unmatched *on both sides* — the identity a move
preserves, which is the kind, the package, the module path and the name. The
file is to the module path what the line is to the file: where you go to read
the item, not what the item is.

That second pass declines far more often than it fires, and every refusal is
deliberate:

- **A finding with no module is out of its reach entirely.** A dead file has no
  name and no module, so nothing about it survives a move for anything to
  compare — two unrelated dead files are indistinguishable without a content
  signal Deadwood does not compute, and a rule that appeared to handle them
  would be guessing. The two dependency kinds and `unsatisfiable_cfg` are out
  for the same reason; a manifest path moves only when a whole package does.
  That boundary was re-examined and kept, with the measurement in
  [`docs/HISTORY.md`](HISTORY.md)
  phase 17: the fix needs a signal *recorded*
  in the baseline, and that field would land on the one class of baseline that
  is portable across every Deadwood released — a file recording only dead files,
  dependency entries and gate sites carries no `module` and no `namespace`, so
  every version reads it today. Adding the field ends that: it makes an older
  Deadwood exit 2 on a file it read yesterday, to buy back a failure that is
  noise.
- **It never overrules the file.** Two items with the same kind, name and module
  in two different files are both matched by the key, so baselining one still
  leaves the other reported.
- **It matches only a one-to-one pairing** — exactly one leftover entry and
  exactly one leftover finding under an identity, which is the only shape a move
  can have. Two candidates for one entry, or two entries for one candidate, and
  it refuses: the run goes back to reporting the finding and naming the entry
  stale, which is where a move stops being distinguishable from a coincidence.
- **It will not cross a package.** `module` is `crate`-relative and says nothing
  about which crate, so two members can each have a `crate::legacy::gone`; the
  recorded file supplies the package. An entry whose path is in no package of
  the workspace — a package directory that itself moved — matches nothing new.

A relocated entry keeps the path it was written with, exactly as a matched entry
keeps its drifted line. `--prune-baseline` drops entries and rewrites none;
`--write-baseline` is what re-records the new paths.

```json
{ "kind": "unused_pub_item", "severity": "deny", "file": "src/lib.rs",
  "line": 11, "name": "twin", "module": "crate::alpha", "namespace": "value",
  "message": "pub fn `twin` is never referenced by any resolved path in this workspace" }
```

Only the three item kinds have a module and a namespace: a dead file is not an
item, the two dependency kinds name an entry in a manifest, and an unsatisfiable
gate names the site the gate is written at. **An entry with no `module` is not
an entry in the crate root**, and one with no `namespace` is not an entry in
some default namespace — each is an entry that says nothing about that field,
and each field is compared only when both sides record it. That is what keeps a
baseline written by an older Deadwood matching exactly what it always matched,
with no edit: the crate root is spelled `crate`, never omitted, so the two cases
can never be confused.

The compatibility runs one way, and only as far as those fields reach. Both are
written only for the kinds that have them — `unused_pub_item`,
`unused_reexport`, `test_only_item` — so a baseline recording none of those
carries neither and every Deadwood that ever shipped still reads it. Record one
item finding and it does not: that file makes a Deadwood older than the field it
carries exit 2 with ``unknown field `module` `` or ``unknown field `namespace`
``, on a file it read yesterday. That is the same strictness that turns a typo'd
key into an error rather than a silently ignored one, and the reason a field
that decides matching may not be quietly dropped. Downgrading after rewriting
such a baseline means deleting the fields by hand, or regenerating the file with
the older binary.

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
  src/old.rs: unused_pub_item `finally_deleted` in `crate::old` (value namespace)
33 finding(s) suppressed by baseline `deadwood-baseline.json`.
```

A stale line names the module and the namespace the entry recorded, when it
recorded them, because two entries that differ only in those are two different
items and the reader has to know which one to go and find.

Three rules keep the file from lying:

- **Writing is explicit.** No run without `--write-baseline` or
  `--prune-baseline` creates or modifies the file, so a CI job can never
  quietly accept what it found.
- **An incomplete run never rewrites it.** A skipped check's findings are missing,
  not fixed, so `--prune-baseline` exits 2 if the run raised any warning, and so
  does `--write-baseline` when a file is already there. The file is unchanged.
- **A missing or unparsable baseline is exit 2**, never "nothing is
  baselined" and never "everything is". A typo'd path would otherwise disarm a
  CI gate silently. The one non-error case is the default location with no file
  in it — that is a project that has not adopted a baseline, and it behaves
  exactly like a Deadwood without the feature.

One entry covers every finding that shares its key, so two `#[cfg]`-alternative
definitions of one item are still suppressed together — which is what should
happen, since they are one item with one fix. Within one package the module path
is shared by every target's crate root, so two binaries or examples each defining
`pub const X` at `crate` are one relocation identity as well: if one disappears
and the other appears in the same run, the second pass reads it as a move. That
shape occurs once in the reportable `pub` definitions of the corpus every phase
measures on, and produces no finding today.
