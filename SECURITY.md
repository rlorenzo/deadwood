# Security policy

## Reporting a vulnerability

Report vulnerabilities privately through GitHub's advisory flow:
[Report a vulnerability](https://github.com/rlorenzo/deadwood-rs/security/advisories/new).
Don't open a public issue for anything exploitable; the report stays private
until a fix ships.

You should hear back within a week. Once a fix is out, the advisory is
published and credits you unless you'd rather it didn't.

## What counts

Deadwood reads a workspace and writes a report. It gets manifests from
`cargo metadata`, run with `--no-deps` and then `--frozen`, so it never touches
the network or the lockfile. It parses source with `syn`: the workspace's own
files, the files its `include!`s name (wherever they are, as rustc would read
them), and the cached registry source of the proc macros it uses. It never
builds or runs the code it analyzes, and the crate forbids `unsafe`. Report
anything that breaks that model: an analyzed workspace that makes `deadwood`
execute code, write outside its stated outputs, or read files rustc wouldn't.

Wrong findings on strange-but-honest code are bugs, not vulnerabilities. File
them as ordinary [issues](https://github.com/rlorenzo/deadwood-rs/issues).

## Supported versions

Fixes land on `main` and ship in the next release; there are no backports.
