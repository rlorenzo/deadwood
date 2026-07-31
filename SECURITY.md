# Security policy

## Reporting a vulnerability

Report vulnerabilities privately through GitHub's advisory flow:
[Report a vulnerability](https://github.com/rlorenzo/deadwood/security/advisories/new).
Please don't open a public issue for anything exploitable — the report stays
private until a fix ships.

You should hear back within a week. Once a fix is out, the advisory is
published and the report is credited to you unless you'd rather it weren't.

## What counts

Deadwood reads a workspace — manifests via `cargo metadata --no-deps`, source
files via `syn` — and writes a report. It never builds the code it analyzes,
and the crate forbids `unsafe`. The interesting reports are anything that
breaks that model: an analyzed workspace that makes `deadwood` execute code,
write outside its stated outputs, or read files a workspace shouldn't reach.

Wrong findings on strange-but-honest code are bugs, not vulnerabilities —
those are welcome as ordinary
[issues](https://github.com/rlorenzo/deadwood/issues).

## Supported versions

Fixes land on `main` and ship in the next release; there are no backports.
