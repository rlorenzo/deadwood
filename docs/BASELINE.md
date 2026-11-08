# Adopting on an existing codebase

A codebase that has never been analyzed has findings on day one, and a tool
that fails the build for all of them gets uninstalled. Record them once and
only new ones fail:

```console
$ deadwood check --write-baseline
No issues found.
Wrote 34 finding(s) to baseline `deadwood-baseline.json`.

$ deadwood check                     # commit the file; CI is green from here
No issues found.
34 finding(s) suppressed by baseline `deadwood-baseline.json`.
```

The file is `deadwood-baseline.json` in the workspace root unless a
`deadwood.toml` says otherwise. It holds exactly the objects `--json` puts in its
`findings` array, with no second format, so it reads well in a diff and you can
write one by hand. Commit it: the debt stays visible, and it can only shrink.

**A baselined finding is subtracted, not marked.** It is absent from the text
report, the JSON `findings` array and the count; only the summary line says how
many there were. The report is for what you have to act on.

**Matching survives line drift.** An entry is matched on kind, file, item name,
the module the item is written in and the namespace it binds that name in. The
line is out of the key because it moves with every edit above it. Severity is
out because it is a `deadwood.toml` decision: keying on it would un-baseline
every finding of a kind when you turn that check down from `deny` to `warn`. The
module path is in because it tells `alpha::twin` from `beta::twin` in one file
and does not move when code above it does.

**It tells a type from a value of the same name.** `pub struct Group { .. }` and
the `#[allow(non_snake_case)] pub fn Group(..)` beside it share a kind, a file,
a name *and* a module. Rust tells them apart by namespace, and so does the key.
The namespace is `type`, `value`, or `both`. A unit or tuple struct binds a
constructor value of its own name, and a `use` alias binds whatever the path it
names binds, resolved through the symbol table like any other path. Two entries
match when their namespaces overlap, so `both` covers either half. An alias
whose target cannot be resolved keeps `both`: a re-export leading outside the
workspace, or through a glob that does, says nothing about namespaces, so
Deadwood does not guess one.

Two definitions share a key in exactly one case, and one entry is right there.
Rust will not compile two definitions of one name in one module unless they are
in different namespaces and neither is in both, so whatever the key still joins
is two `#[cfg]`-alternative spellings of one item: one place to open, one fix.

**It survives a moved file, for the kinds that name an item.** `git mv
src/legacy.rs src/legacy/mod.rs` changes no code and no item, so the findings in
it stay suppressed. Matching runs in two passes: the key above, then, over what
that pass left unmatched *on both sides*, the identity a move preserves, which
is the kind, the package, the module path and the name. The file is to the
module path what the line is to the file: where you go to read the item, not
what the item is.

The second pass declines far more often than it fires, and every refusal is
deliberate:

- **A finding with no module is out of its reach.** A dead file has no name and
  no module, so nothing about it survives a move to compare. Two unrelated dead
  files are indistinguishable without a content signal Deadwood does not
  compute, and a rule that appeared to handle them would be guessing. The two
  dependency kinds are out for the same reason; a manifest path moves only when
  a whole package does. That boundary was re-examined and kept, with the
  measurement in [`docs/HISTORY.md`](HISTORY.md) phase 17. The fix needs a
  signal *recorded* in the baseline, and that field would land on the one class
  of baseline portable across every Deadwood released: a file recording only dead
  files and dependency entries carries no `module` and no
  `namespace`, so every version reads it today. Adding the field makes an older
  Deadwood exit 2 on a file it read yesterday, to buy back a failure that is
  noise.
- **It never overrules the file.** Two items with the same kind, name and module
  in two different files are both matched by the key, so baselining one still
  leaves the other reported.
- **It matches only a one-to-one pairing**: exactly one leftover entry and
  exactly one leftover finding under an identity, the only shape a move can
  have. With two candidates for one entry, or two entries for one candidate, it
  refuses: the run reports the finding and names the entry stale.
- **It will not cross a package.** `module` is `crate`-relative and says nothing
  about which crate, so two members can each have a `crate::legacy::gone`; the
  recorded file supplies the package. An entry whose path is in no package of
  the workspace (a package directory that itself moved) matches nothing new.

A relocated entry keeps the path it was written with, as a matched entry keeps
its drifted line. `--prune-baseline` drops entries and rewrites none;
`--write-baseline` re-records the new paths.

```json
{ "kind": "unused_pub_item", "severity": "deny", "file": "src/lib.rs",
  "line": 11, "name": "twin", "module": "crate::alpha", "namespace": "value",
  "message": "pub fn `twin` is never referenced by any resolved path in this workspace" }
```

Only the three item kinds have a module and a namespace: a dead file is not an
item, and the two dependency kinds name an entry in a manifest. **An entry with
no `module` is not an entry in the crate root**, and one with no `namespace` is
not an entry in some default namespace. Each says nothing about that field, and
each field is compared only when both sides record it. That keeps a baseline
written by an older Deadwood matching exactly what it always matched, with no
edit. The crate root is spelled `crate`, never omitted, so the two cases cannot
be confused.

Compatibility runs one way. Both fields are written only for the kinds that have
them (`unused_pub_item`, `unused_reexport`, `test_only_item`), so a baseline
recording none of those carries neither and every Deadwood that ever shipped
still reads it. Record one item finding and a Deadwood older than the field
exits 2 with ``unknown field `module` `` or ``unknown field `namespace` ``, on a
file it read yesterday. That is the same strictness that turns a typo'd key into
an error, and a field that decides matching may not be quietly dropped. To
downgrade after rewriting such a baseline, delete the fields by hand or
regenerate the file with the older binary.

**Fixed findings are reported.** An entry nothing matches any more is stale, and
every run says so; `--prune-baseline` rewrites the file without them. Stale
entries never fail the run: the exit code follows severity and nothing else, and
a developer who deletes dead code should not be punished for it.

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
recorded them, since two entries that differ only in those are two different
items.

Three rules keep the file from lying:

- **Writing is explicit.** No run without `--write-baseline` or
  `--prune-baseline` creates or modifies the file, so a CI job can never quietly
  accept what it found.
- **An incomplete run never rewrites it.** A skipped check's findings are
  missing, not fixed, so `--prune-baseline` exits 2 if the run raised any
  warning, and so does `--write-baseline` when a file is already there. The file
  is unchanged.
- **A missing or unparsable baseline is exit 2**, never "nothing is baselined"
  and never "everything is". A typo'd path would otherwise disarm a CI gate
  silently. The one non-error case is the default location with no file in it:
  that project has not adopted a baseline, and it behaves exactly like a
  Deadwood without the feature.

One entry covers every finding that shares its key, so two `#[cfg]`-alternative
definitions of one item are still suppressed together, which is right since they
are one item with one fix. Within one package the module path is shared by every
target's crate root, so two binaries or examples each defining `pub const X` at
`crate` are one relocation identity as well. If one disappears and the other
appears in the same run, the second pass reads it as a move. That shape occurs
once in the reportable `pub` definitions of the corpus every phase measures on,
and produces no finding today.
