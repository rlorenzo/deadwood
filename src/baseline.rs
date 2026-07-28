//! The baseline file: today's findings, recorded so tomorrow's are the only
//! ones that fail.
//!
//! A codebase that has never been analyzed has findings on day one, and a tool
//! that fails the build for all of them on day one gets uninstalled. The
//! baseline is how a project draws a line: record what is there now, commit the
//! file, and from then on only *new* findings fail the run. The debt stays
//! visible — it is a committed file anyone can read — and it can only shrink,
//! because entries that stop occurring are reported as stale.
//!
//! # The file is the report's finding shape
//!
//! A baseline is a JSON object with one key, `findings`, holding exactly the
//! objects the `--json` report puts in its own `findings` array
//! ([`crate::report`]). There is no second format to learn and no second
//! serializer to keep in step: a written entry is byte-for-byte what the report
//! would have printed for that finding. What is *not* in the file is as
//! deliberate: the report's `workspace_root` is an absolute path from whichever
//! machine ran the analysis, and its `warnings` are not findings, so neither
//! belongs in a file that is committed and read on someone else's checkout.
//!
//! Reading is looser than writing on purpose. Only `kind` and `file` are
//! required, so a hand-written or hand-edited entry is a two-line object;
//! `line`, `name`, `module`, `severity` and `message` are accepted, written
//! back, and shown to whoever reads the file, but see below for which of them
//! the matching actually uses. Unknown keys are rejected, for the reason every
//! other file Deadwood reads rejects them: a key that is parsed and ignored is
//! a setting the user believes is working.
//!
//! # The match key, and what is deliberately not in it
//!
//! An entry matches a finding when **kind, file, name and module** are equal,
//! with one exception for the module that the next section is entirely about.
//! That is the whole key, and each exclusion is a decision:
//!
//! - **Not the line.** Code moves. A baseline keyed on line numbers would
//!   un-suppress a whole file the first time someone adds an import at the top,
//!   which is a page of false alarms caused by nothing at all. The line is
//!   still recorded, because a human reading the file wants somewhere to jump
//!   to; it is simply not compared.
//! - **Not the severity.** Severity is a *config* decision, not a property of
//!   the finding: the same dead file is `deny` or `warn` depending on a table
//!   in `deadwood.toml`. Putting it in the key would mean that flipping one
//!   kind from `deny` to `warn` — an act of turning the volume *down* — un-
//!   baselines every entry of that kind and turns them all into new findings.
//!   It is recorded for the reader and ignored by the matcher.
//! - **Not the message.** It is prose, it names packages and tables, and it is
//!   reworded whenever a finding is made clearer. Nothing about identity lives
//!   in it that `kind`, `file` and `name` do not already carry.
//! - **The kind, though, is load bearing.** `unused_dependency` and
//!   `misplaced_dependency` both point at the same `Cargo.toml` with the same
//!   entry name and both carry no line at all; the kind is the *only* thing
//!   that separates them. Baselining "`serde` is unused" must not suppress
//!   "`serde` is in the wrong table", because those are different claims and
//!   the second one is news.
//!
//! - **The module, which arrived last and is the only field the key acquired
//!   after shipping.** Two `pub fn twin` in two inline modules of one file
//!   differ in nothing else the key looks at, so one entry used to suppress
//!   both — and a *third* `twin` added later was suppressed before it existed
//!   ([#16](https://github.com/rlorenzo/deadwood/issues/16)). The module path
//!   is `crate`-rooted and crate-relative (`crate`, `crate::alpha`), it is what
//!   [`crate::Finding::module`] carries, and — the property that makes it
//!   admissible at all — it does not move when code above it does. That is the
//!   whole test the line failed.
//!
//! `name` is an `Option`, and `None` is a value like any other: a dead file has
//! no name, and matches only a baseline entry that has none either. `module` is
//! an `Option` too and behaves entirely differently — see below.
//!
//! The file path is what makes the key specific enough to be safe — without it
//! one `unused_pub_item foo` entry would cover every `foo` in the workspace,
//! and two `dead_file` findings would be indistinguishable. It is compared
//! first and it is never dropped; what it no longer has to be, for the kinds
//! that name a module, is the *whole* of where an item lives. That is the next
//! section.
//!
//! # A moved file: the second pass, and the four things that hold it back
//!
//! `git mv src/legacy.rs src/legacy/mod.rs` changes no code and no item, and
//! under the key above it turns every finding in the file into a new one and
//! every entry recording them into a stale one. A pure rename failing the run
//! is the defect ([#17](https://github.com/rlorenzo/deadwood/issues/17)).
//!
//! The fix is not rename detection, and nothing here computes a similarity
//! signal. It is the observation that for an *item* the file is a second name
//! for a place the key already records: `crate::legacy::gone` in package
//! `alpha` names one definition, and the file it is written in is where you go
//! to read it, not what it is. **The file is to the module path what the line
//! is to the file** — recorded, printed, and matched only until something
//! better is available. So matching runs in two passes:
//!
//! 1. The key above, exactly as before. Everything it matches is matched by it.
//! 2. Whatever is *left over on both sides* — entries no finding matched, and
//!    findings no entry covered — paired on [`Relocation`], the identity a move
//!    preserves: kind, package, module path and name.
//!
//! Four things keep the second pass from becoming the guess the issue warned
//! about, and each is a place a mutation is caught:
//!
//! - **It cannot reach a finding with no module.** [`Relocation`] is built from
//!   [`Key::relocation`], which answers `None` unless *both* the module and the
//!   name are recorded. `dead_file` has neither, so the 39 dead files in the
//!   corpus are structurally out of reach rather than excluded by a kind list
//!   someone could extend. Two unrelated dead files are indistinguishable
//!   without a content signal, and this pass does not have one — so it does not
//!   pretend to. The two dependency kinds and `unsatisfiable_cfg` are out for
//!   the same reason, and a manifest path moves only when a whole package does,
//!   which is a rarer event with no signal here either.
//! - **It runs second, so it can never overrule the file.** A finding the exact
//!   key matched is not left over, and neither is the entry that matched it. Two
//!   items sharing a kind, a name *and* a module in two different files —
//!   `clap`'s `CLAP_STYLING`, defined at `crate` in two example targets — are
//!   still two findings, and baselining one still leaves the other reported:
//!   both occur, so the exact key claims both sides and there is nothing left
//!   over to pair.
//! - **It refuses when the pairing is ambiguous.** A relocation is accepted only
//!   when exactly one leftover entry and exactly one leftover finding carry it.
//!   Two candidates for one entry is an inference we cannot make — a move is
//!   one-to-one and the observation is not — so the pass declines and the run
//!   falls back to reporting the findings and naming the entries stale. That is
//!   the direction this project takes when the evidence runs out: noise, not
//!   silence. It is deliberately *not* the set semantics the exact key uses,
//!   where one entry covers every finding under its key; there the question is
//!   which occurrence is new (unanswerable, so cover them all) and here it is
//!   whether a move happened at all (unanswerable, so assume not).
//! - **It is scoped to the package**, because the module path is not enough on
//!   its own. `module` is crate-*relative* and `crate`-rooted, so two workspace
//!   members each with a `crate::legacy::gone` differ in nothing else the pass
//!   looks at. The file supplies what the module path cannot: which package the
//!   entry was recorded in, resolved by containment against the workspace's
//!   manifest directories ([`Packages`]). So the path stays in the key and is
//!   read for a second, narrower thing — and an entry whose path lies in no
//!   package of this workspace (a package directory that itself moved) resolves
//!   to nothing and stays exactly as stale as it is today.
//!
//! What that leaves, stated rather than glossed. Within one package the module
//! path is shared by every target's root, so two binaries or examples each
//! defining `pub const X` at `crate` are one relocation identity; if one
//! disappears and the other appears in the same run, the second pass reads it as
//! a move. Measured over the corpus every phase uses, that shape occurs once in
//! 2659 reportable `pub` definitions — `clap`'s `CLAP_STYLING` — and it produces
//! no finding today. Distinguishing it needs the *target*, which no finding
//! carries and which one file can belong to several of.
//!
//! An entry matched this way keeps the path it was written with: like the line
//! beside it, the path is recorded for whoever opens the file and is not
//! rewritten by `--prune-baseline`, which drops entries and edits none.
//! `--write-baseline` re-records everything from the current run and is how a
//! baseline picks the new paths up.
//!
//! # An absent module is not a module: the fallback, in both directions
//!
//! Only three of the seven kinds have a module to name. A dead file is not an
//! item at all, the two dependency kinds name an entry in a manifest, and an
//! unsatisfiable gate names the site the gate is written at rather than a
//! definition — so the key gained a field most kinds can never fill, and every
//! baseline written before the field existed fills it for none of them.
//!
//! So `None` here means *nothing was said about the module*, never *the crate
//! root* — which is why the root is spelled `crate` and not the empty string.
//! Matching compares modules **only when both sides name one** ([`Modules`]).
//! An entry with no module covers every finding under its shared key, exactly as
//! it did before this field existed; a finding with no module is covered by an
//! entry that names one. The forgiving direction is not a convenience: the
//! alternative un-baselines every entry of every baseline in every project that
//! upgrades without touching its file, which is a run failing over code nobody
//! changed. This project prefers noise to silence, but only when the noise is
//! about the user's own code.
//!
//! The same relation answers both questions — a finding is suppressed when some
//! entry covers it, and an entry is stale when no finding covers it — from one
//! function, because two copies could drift into an entry that suppresses a
//! finding *and* reads as no longer occurring.
//!
//! # Two findings that still share a key
//!
//! The module narrows the key; it does not make it unique, and one entry still
//! suppresses **every** finding matching its key. Set semantics, not counting:
//! recording a multiplicity and reporting the (n+1)th occurrence as new stays
//! rejected for the reason phase 6 rejected it — with lines unmatched we cannot
//! say which occurrence is the new one, so the report would point at a line that
//! is very likely baselined. That is a wrong finding where this is a missed one.
//!
//! Two shapes survive, both measured on real code and both cases where two
//! definitions genuinely share a file, a name *and* a module:
//!
//! - **Two `cfg`-alternative definitions of one item.** `#[cfg(feature =
//!   "utf8")] pub type DefaultCharAccumulator = Utf8Parser;` beside its
//!   `not(utf8)` twin. Deadwood's matrix is the union of every build, so both
//!   halves are analyzed and both could be reported. One entry covering both is
//!   *right* here — it is one item, and a reader fixing it fixes both.
//! - **A type and a value sharing a name.** `pub struct Group` beside
//!   `#[allow(non_snake_case)] pub fn Group(..)`, syn's constructor-shim idiom.
//!   These are two different items, and the module path cannot separate them
//!   because Rust separates them by *namespace*, which nothing in the key
//!   models. This is the residual of #16, and it is left open rather than
//!   guessed at.
//!
//! # Suppressed findings do not appear in the report
//!
//! A baselined finding is removed from [`crate::Analysis::findings`] rather
//! than carried along with a flag. Two reasons, one of them about honesty and
//! one about compatibility:
//!
//! - The report answers "what should I act on". A suppressed finding is by
//!   definition not that, and printing it anyway reproduces exactly the
//!   day-one noise the baseline was adopted to remove.
//! - `findings` is the JSON contract every consumer already parses, and
//!   [`crate::Analysis::has_denied`] is the exit code. Leaving suppressed
//!   findings in the array would silently break every count, every filter, and
//!   the exit code itself unless each of them learned about a new field first.
//!
//! Nothing is hidden by this: the suppressed count and the file it came from
//! are printed on every run, and the file itself is in the repository.
//!
//! # Stale entries are reported and never fail the run
//!
//! An entry no finding matches has done its job — the code got fixed — and
//! leaving it in the file is how a baseline rots into a permanent excuse. Every
//! run names the stale entries and points at `--prune-baseline`.
//!
//! It does not fail the run. The exit code follows severity and nothing else
//! (see [`crate::config::Severity`]), a stale entry has no severity to follow,
//! and failing a build because a developer *fixed* something is the kind of
//! rule that gets a tool uninstalled. Note the two ways an entry can go stale
//! without the code changing: an `ignore` pattern or a `severity = "off"` that
//! now covers it means the finding no longer exists, so the entry that recorded
//! it no longer matches. Pruning is the right answer in both cases.
//!
//! One more way an entry can appear or disappear without the code changing:
//! every detector skips a package whose module resolution was incomplete, so a
//! baseline written while the analysis was warning records less than a clean
//! one would, and fixing the warning turns the difference into new findings.
//! That is the honest answer — those findings really were never recorded — and
//! the warnings are printed on every run that has them.
//!
//! # Missing and malformed files are hard errors
//!
//! Reading a baseline that is not there, or one that does not parse, exits 2
//! with a message naming the file. Silently treating a typo'd path as "nothing
//! is baselined" would be bad enough; the reverse — treating an unreadable file
//! as "everything is baselined" — would turn a broken file into a green CI run
//! forever. This is the same reasoning that makes `--config` require its file
//! while discovery may find none, and the split here is exactly parallel: a
//! path written down in `deadwood.toml` must exist, while the default location
//! simply may or may not have a file in it yet.
//!
//! # Reading a newer baseline is a hard error too, and that door is one-way
//!
//! [`Entry`] rejects unknown fields, so the compatibility of the `module` field
//! runs one way only. A newer Deadwood reads every older baseline (the fallback
//! above); an **older** Deadwood handed a baseline carrying `module` exits 2 with
//! ``unknown field `module` ``, on a file it read yesterday. Downgrading after
//! rewriting the baseline needs the field deleted by hand, or the file
//! regenerated by the older binary.
//!
//! Relaxing the strictness would not fix that — the strict reader is the one
//! already released — and it would cost the protection outright. "A setting that
//! silently does nothing is worse than no setting" is a rule about config, and
//! the objection to applying it to a *data* file is fair as far as it goes: an
//! ignored decoration on an entry still leaves the entry matching, and the
//! failure is noise rather than silence. But `module` is not a decoration. It is
//! part of the key, so an entry whose `module` was silently dropped as a typo
//! falls back to the broad shared key and suppresses the neighbour this field
//! exists to stop suppressing — silence, in the exact place the phase was about.
//! The strictness stays, and the door is documented rather than papered over.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::Severity;
use crate::{Finding, FindingKind};

/// The file name Deadwood looks for when no `baseline` key names one.
pub const FILE_NAME: &str = "deadwood-baseline.json";

/// What a run does with the baseline file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Subtract the recorded findings and report only what is new. The
    /// default, and the only mode that never touches the file.
    #[default]
    Apply,
    /// Replace the file with the current finding set (`--write-baseline`).
    Write,
    /// Drop the entries that no longer occur and rewrite the file
    /// (`--prune-baseline`). Never records anything new.
    Prune,
}

/// What a run did with the baseline, for the report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// The file was read and its entries subtracted.
    Applied,
    /// The file was rewritten from this run's findings.
    Written,
    /// Stale entries were dropped and the file rewritten.
    Pruned,
}

/// What the baseline did to this run, rendered alongside the findings.
///
/// Absent entirely when no baseline was in play, which is what keeps a run
/// without one byte-identical to a Deadwood that has never heard of baselines.
#[derive(Debug, Serialize)]
pub struct Report {
    /// The file, relative to the workspace root where possible.
    pub path: PathBuf,
    pub action: Action,
    /// How many findings the baseline removed from this run's report.
    ///
    /// Under [`Action::Written`] that is every finding the run produced: they
    /// were just recorded, so the file covers all of them and the report is
    /// empty. It is the same number `--write-baseline` says it wrote, and the
    /// same number the *next* run will report suppressed.
    pub suppressed: usize,
    /// Entries that matched no finding: fixed code, or a finding now silenced
    /// by config. Under [`Action::Pruned`] these are the entries just removed.
    pub stale: Vec<Key>,
}

/// The identity of a finding, for baseline matching: kind, file, name, and the
/// module the name is written in.
///
/// Deliberately not the line (code moves), the severity (a config decision) or
/// the message (prose); the module docs have the reasoning for each.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct Key {
    pub kind: FindingKind,
    pub file: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `crate::alpha`, for the kinds that have one. `None` is not a wildcard in
    /// the *key* — two keys with different modules are different keys, and both
    /// are reported and deduplicated on their own — but it is one in
    /// [`Modules::covers`], which is where matching happens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
}

/// The part of a key that every key has, whatever it says about the module.
///
/// Matching falls back to this whenever one side records no module, which is
/// what makes a baseline written before the field existed keep working. The
/// module is not in it: it is carried alongside, as a [`Modules`].
type Shared = (FindingKind, PathBuf, Option<String>);

impl Key {
    fn of(finding: &Finding) -> Key {
        Key {
            kind: finding.kind,
            file: finding.file.clone(),
            name: finding.name.clone(),
            module: finding.module.clone(),
        }
    }

    fn shared(&self) -> Shared {
        (self.kind, self.file.clone(), self.name.clone())
    }

    /// What of this key a move leaves alone, or `None` when there is not
    /// enough of it to say.
    ///
    /// Both the module and the name are required, which is the whole of what
    /// keeps the second pass off the kinds that have no item to identify: a
    /// dead file records neither, a manifest entry and a gate site record no
    /// module. The package comes from the file, and a file belonging to no
    /// package of this workspace answers `None` as well.
    ///
    /// The name is the one condition here no test can catch on its own, and
    /// that is worth stating rather than leaving as an apparent gap in the
    /// coverage: every finding Deadwood produces that carries a module also
    /// carries a name, so an entry that lost the name requirement still has
    /// nothing to pair with. It stays because an identity without a name is not
    /// one, not because it is load bearing today.
    fn relocation(&self, packages: &Packages) -> Option<Relocation> {
        Some(Relocation {
            kind: self.kind,
            package: packages.holding(&self.file)?.to_string(),
            module: self.module.clone()?,
            name: self.name.clone()?,
        })
    }

    /// `src/lib.rs: unused_pub_item `dead` in `crate::alpha``, for the text
    /// report.
    ///
    /// The module is appended only when there is one, so an entry written
    /// before the field existed is described exactly as it always was.
    pub fn describe(&self) -> String {
        let kind = self.kind.label();
        let mut out = match &self.name {
            Some(name) => format!("{}: {kind} `{name}`", self.file.display()),
            None => format!("{}: {kind}", self.file.display()),
        };
        if let Some(module) = &self.module {
            out.push_str(&format!(" in `{module}`"));
        }
        out
    }
}

/// What one side of the match records about the module, under one [`Shared`]
/// key.
///
/// This is the whole of the format's compatibility story, and it is
/// deliberately the *forgiving* reading in both directions. A side that records
/// no module has not said the modules differ — it has said nothing about
/// modules at all — so there is nothing to compare and the shared key is the
/// whole key, which is exactly today's behaviour. Only when both sides name a
/// module does a mismatch separate them.
///
/// The alternative, treating an absent module as a module of its own, would
/// un-baseline every entry in every baseline ever written the moment a project
/// upgraded — a file the user committed, breaking on a run that changed no code
/// of theirs. That is the loudest possible noise, and this project prefers noise
/// to silence only when the noise is about the user's own code.
///
/// Never empty: it exists only where something was added to it.
#[derive(Debug, Default)]
struct Modules {
    /// Something under this key records no module.
    unqualified: bool,
    /// The modules that are named.
    qualified: HashSet<String>,
}

impl Modules {
    fn add(&mut self, module: Option<&str>) {
        match module {
            None => self.unqualified = true,
            Some(module) => {
                self.qualified.insert(module.to_string());
            }
        }
    }

    /// Whether this side covers something on the other side that records
    /// `module`.
    ///
    /// Symmetric on purpose: `apply` asks it of the entries and `stale` asks it
    /// of the findings, and an entry that suppresses a finding must be the same
    /// relation as a finding that keeps an entry fresh. Two copies of it, drifting,
    /// would produce an entry that both suppresses a finding and reads as stale.
    fn covers(&self, module: Option<&str>) -> bool {
        match module {
            // The other side records no module either: nothing to compare on,
            // so the shared key is the whole key.
            None => true,
            Some(module) => self.unqualified || self.qualified.contains(module),
        }
    }
}

/// The identity of an *item* finding that a move preserves: which package it
/// is in, where it sits in that package's module tree, and what it is called.
///
/// Everything a file rename changes is absent from it, and everything it names
/// is something the item would have to actually change for the finding to be a
/// different claim. That is why the second pass is not a similarity heuristic —
/// it compares identities, and declines whenever two of them collide.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Relocation {
    kind: FindingKind,
    /// The package the recorded file belongs to. The module path is
    /// `crate`-relative and so says nothing about which crate; without this,
    /// two members each with a `crate::legacy::gone` would be one identity.
    package: String,
    module: String,
    name: String,
}

/// The workspace's packages, by the directory each one owns.
///
/// Built from `cargo metadata`, so nothing here is inferred from a path's
/// shape. The lookup is containment against the manifest directories, which is
/// what lets a *recorded* path still name its package after the file it points
/// at is gone — the case the whole second pass exists for.
#[derive(Debug, Default)]
pub(crate) struct Packages {
    /// `(directory, package name)`, deepest directory first, so a member
    /// nested inside another package's directory answers for its own files.
    dirs: Vec<(PathBuf, String)>,
}

impl Packages {
    /// `dirs` are package directories relative to the workspace root, as
    /// [`Finding::file`] and a baseline entry's `file` both are. The root
    /// package's directory is the empty path, which contains everything and so
    /// sorts last.
    pub(crate) fn new(dirs: impl IntoIterator<Item = (PathBuf, String)>) -> Packages {
        let mut dirs: Vec<(PathBuf, String)> = dirs.into_iter().collect();
        dirs.sort_by_key(|(dir, _)| std::cmp::Reverse(dir.components().count()));
        Packages { dirs }
    }

    fn holding(&self, file: &Path) -> Option<&str> {
        self.dirs
            .iter()
            .find(|(dir, _)| file.starts_with(dir))
            .map(|(_, name)| name.as_str())
    }
}

/// The relocations both sides agree on, from what the exact key left over.
///
/// `entries` and `findings` are the leftovers — deduplicated keys, so a
/// baseline that records one entry twice is one candidate rather than two — and
/// a relocation survives only when exactly one of each carries it. Anything
/// else is two readings of the same evidence, and the pass has no way to choose
/// between them; declining leaves the run reporting the findings and naming the
/// entries stale, which is what it does today.
///
/// One set answers both questions, which is the same reason [`Modules::covers`]
/// is one function: an entry kept fresh by a move and a finding suppressed by
/// one are the same claim, and two copies of it could drift into an entry that
/// suppresses a finding *and* reads as no longer occurring.
fn relocations(entries: &[Key], findings: &[Key], packages: &Packages) -> HashSet<Relocation> {
    fn candidates(keys: &[Key], packages: &Packages) -> HashMap<Relocation, HashSet<PathBuf>> {
        let mut out: HashMap<Relocation, HashSet<PathBuf>> = HashMap::new();
        for key in keys {
            if let Some(relocation) = key.relocation(packages) {
                out.entry(relocation).or_default().insert(key.file.clone());
            }
        }
        out
    }

    let from = candidates(entries, packages);
    let to = candidates(findings, packages);
    from.into_iter()
        .filter(|(_, files)| files.len() == 1)
        .filter(|(relocation, _)| to.get(relocation).is_some_and(|files| files.len() == 1))
        .map(|(relocation, _)| relocation)
        .collect()
}

/// Index one side of the match by its shared key.
fn index(keys: impl IntoIterator<Item = Key>) -> HashMap<Shared, Modules> {
    let mut out: HashMap<Shared, Modules> = HashMap::new();
    for key in keys {
        out.entry(key.shared())
            .or_default()
            .add(key.module.as_deref());
    }
    out
}

/// Whether `side` — one side of the match, indexed by [`index`] — covers `key`.
fn covered(side: &HashMap<Shared, Modules>, key: &Key) -> bool {
    side.get(&key.shared())
        .is_some_and(|modules| modules.covers(key.module.as_deref()))
}

/// A recorded finding, as it sits in the file.
///
/// The serialized form is the report's finding object exactly. Only the two
/// fields the key is built from are required to read one back, so a
/// hand-written entry can be two lines; the rest is context for whoever opens
/// the file, and `entry_matches_a_report_finding_field_for_field` pins that the
/// two shapes have not drifted apart.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Entry {
    kind: FindingKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    severity: Option<Severity>,
    file: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    line: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    /// Absent in every baseline written before this field existed, and in every
    /// entry for a kind that has no module to name. Absent means *unqualified*,
    /// never *the crate root*: see [`Modules`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    module: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

impl Entry {
    fn of(finding: &Finding) -> Entry {
        Entry {
            kind: finding.kind,
            severity: Some(finding.severity),
            file: finding.file.clone(),
            line: finding.line,
            name: finding.name.clone(),
            module: finding.module.clone(),
            message: Some(finding.message.clone()),
        }
    }

    fn key(&self) -> Key {
        Key {
            kind: self.kind,
            file: self.file.clone(),
            name: self.name.clone(),
            module: self.module.clone(),
        }
    }
}

/// The file's top level: the report's `findings` array, and nothing else.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    findings: Vec<Entry>,
}

/// A parsed baseline file, in the order its entries are written.
///
/// Crate-private, unlike the module around it. What a consumer of the library
/// needs is what a run *did* — [`Report`], the [`Key`]s in it, [`Mode`] to ask
/// for a mode, [`FILE_NAME`] to find the default location — and
/// [`crate::analyze_with`] is the entry point that does it. The file reader
/// itself is machinery: [`Baseline::apply`] needs a [`Packages`] index built
/// from `cargo metadata`, so publishing it would publish that too, and an
/// index of manifest directories is not something to promise anybody. Phase 13
/// narrowed it for that reason; before then it was `pub` with no caller
/// outside this crate.
#[derive(Debug, Default)]
pub(crate) struct Baseline {
    entries: Vec<Entry>,
}

impl Baseline {
    /// Read and validate the baseline at `path`.
    ///
    /// A missing file is an error, not an empty baseline: a run that cannot
    /// find the file it was told to use is a run whose result means nothing.
    pub(crate) fn load(path: &Path) -> Result<Baseline> {
        let text = std::fs::read_to_string(path).with_context(|| {
            format!(
                "could not read baseline file `{}` (run `deadwood check --write-baseline` to \
                 create it)",
                path.display()
            )
        })?;
        let file: File = serde_json::from_str(&text)
            .with_context(|| format!("invalid baseline file `{}`", path.display()))?;
        Ok(Baseline {
            entries: file.findings,
        })
    }

    /// Write `findings` to `path` as the whole baseline, replacing whatever
    /// was there.
    pub(crate) fn write(path: &Path, findings: &[Finding]) -> Result<()> {
        Baseline {
            entries: findings.iter().map(Entry::of).collect(),
        }
        .save(path)
    }

    fn save(&self, path: &Path) -> Result<()> {
        let file = File {
            findings: self.entries.clone(),
        };
        let mut text = serde_json::to_string_pretty(&file)
            .with_context(|| format!("could not encode baseline file `{}`", path.display()))?;
        text.push('\n');
        // The directory the configured path names is created along with the
        // file. `baseline = ".deadwood/baseline.json"` is a perfectly ordinary
        // thing to write, and failing it with exit 2 until the user runs
        // `mkdir` is a papercut, not a safeguard: the path came from the user
        // and the write came from an explicit flag, so there is nothing here
        // to protect them from.
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).with_context(|| {
                format!(
                    "could not create directory `{}` for baseline file `{}`",
                    parent.display(),
                    path.display()
                )
            })?;
        }
        // Written beside the destination and renamed over it, so the file a
        // reader sees is always one whole baseline. A direct write that is
        // interrupted — a cancelled CI job, a kill, a full disk — leaves a
        // truncated file behind, and this module treats a malformed baseline
        // as a hard error, so the interruption would turn a committed
        // artifact into a broken one that only a hand edit recovers. The
        // temporary sits in the destination's own directory, which is what
        // makes the rename atomic rather than a copy across filesystems.
        let mut temporary_name = path.file_name().unwrap_or_default().to_os_string();
        temporary_name.push(".tmp");
        let temporary = path.with_file_name(temporary_name);
        std::fs::write(&temporary, text).with_context(|| {
            format!(
                "could not write baseline file `{}` (via `{}`)",
                path.display(),
                temporary.display()
            )
        })?;
        std::fs::rename(&temporary, path).map_err(|err| {
            // The half-written temporary is this function's litter, not the
            // user's: leaving it would be a second surprise on top of the
            // failure being reported.
            let _ = std::fs::remove_file(&temporary);
            anyhow::Error::new(err).context(format!(
                "could not write baseline file `{}`",
                path.display()
            ))
        })
    }

    /// Split `findings` into the ones this baseline does not cover and a
    /// record of what it did.
    ///
    /// Both passes live here, and they have to: the second one is defined on
    /// what the first left over, so neither the suppressed set nor the stale
    /// set can be computed without the other side's leftovers in hand.
    ///
    /// `path` is only carried into the report; nothing is read or written.
    pub(crate) fn apply(
        &self,
        findings: Vec<Finding>,
        path: PathBuf,
        packages: &Packages,
    ) -> (Vec<Finding>, Report) {
        let occurring = index(findings.iter().map(Key::of));
        let baselined = self.index_entries();
        let total = findings.len();

        // Pass one: the exact key, unchanged. What it matches is matched, and
        // the second pass never sees either side of it.
        let unmatched: Vec<Finding> = findings
            .into_iter()
            .filter(|finding| !covered(&baselined, &Key::of(finding)))
            .collect();
        let orphaned = self.orphaned(&occurring);

        // Pass two: the identity a move preserves, over what is left.
        let moved = relocations(
            &orphaned,
            &unmatched.iter().map(Key::of).collect::<Vec<_>>(),
            packages,
        );
        let relocated = |key: &Key| {
            key.relocation(packages)
                .is_some_and(|relocation| moved.contains(&relocation))
        };

        let kept: Vec<Finding> = unmatched
            .into_iter()
            .filter(|finding| !relocated(&Key::of(finding)))
            .collect();
        let stale: Vec<Key> = orphaned.into_iter().filter(|key| !relocated(key)).collect();
        let suppressed = total - kept.len();

        (
            kept,
            Report {
                path,
                action: Action::Applied,
                suppressed,
                stale,
            },
        )
    }

    fn index_entries(&self) -> HashMap<Shared, Modules> {
        index(self.entries.iter().map(Entry::key))
    }

    /// The keys this baseline records that no occurring finding matches *by the
    /// exact key*, in file order and without repeats.
    ///
    /// `occurring` is the finding side indexed by [`index`], so the relation
    /// asked here is [`Modules::covers`] read the other way round: an entry
    /// naming `crate::alpha` is fresh when some finding is in `crate::alpha` —
    /// or when a finding under the same shared key names no module at all,
    /// which is the same forgiving fallback that suppresses it.
    ///
    /// Not the stale set: these are the candidates the second pass then draws
    /// from, and what survives it is what the report calls stale.
    fn orphaned(&self, occurring: &HashMap<Shared, Modules>) -> Vec<Key> {
        let mut seen = HashSet::new();
        self.entries
            .iter()
            .map(|entry| entry.key())
            .filter(|key| !covered(occurring, key))
            .filter(|key| seen.insert(key.clone()))
            .collect()
    }

    /// Drop the entries whose keys are in `stale`, keeping every surviving
    /// entry's fields and the order they were written in.
    ///
    /// It takes the report's own stale set rather than recomputing the match,
    /// so `--prune-baseline` removes exactly the entries the run just named and
    /// the two can never disagree.
    ///
    /// Not the file's bytes: pruning re-serializes, so a hand-formatted or
    /// hand-abbreviated file comes back in the canonical form
    /// [`Baseline::save`] emits. Only the entries are preserved, which is the
    /// property that matters — pruning must never quietly accept a new finding,
    /// drop a field off a surviving one, or *rewrite* one. An entry the second
    /// pass matched keeps the path it was written with, exactly as a matched
    /// entry keeps its drifted line; `--write-baseline` is what re-records
    /// either.
    fn without(&self, stale: &[Key]) -> Baseline {
        let stale: HashSet<&Key> = stale.iter().collect();
        Baseline {
            entries: self
                .entries
                .iter()
                .filter(|entry| !stale.contains(&entry.key()))
                .cloned()
                .collect(),
        }
    }
}

/// Run `mode` against `findings`, returning what the run should report.
///
/// This is the one place that both reads and writes the file, so the rules
/// about when it may be created live together: [`Mode::Write`] never reads,
/// which is what makes it the bootstrap; [`Mode::Apply`] and [`Mode::Prune`]
/// never create.
pub(crate) fn run(
    mode: Mode,
    location: &Location,
    findings: Vec<Finding>,
    workspace_root: &Path,
    packages: &Packages,
) -> Result<(Vec<Finding>, Option<Report>)> {
    let display = |path: &Path| crate::relative_to(path, workspace_root);
    match mode {
        // Recording the current set means the file now covers all of it, so
        // the run reports nothing and exits clean — which is exactly what the
        // next run would do, and the reason writing is a mode rather than a
        // separate command.
        Mode::Write => {
            let path = location.path();
            Baseline::write(path, &findings)?;
            Ok((
                Vec::new(),
                Some(Report {
                    path: display(path),
                    action: Action::Written,
                    suppressed: findings.len(),
                    stale: Vec::new(),
                }),
            ))
        }
        Mode::Prune => {
            let path = location.path();
            let baseline = Baseline::load(path)?;
            // `apply`'s stale set is exactly the entries to remove, so the
            // report names precisely what was pruned rather than what a second
            // computation of the same match happened to agree on.
            let (kept, mut report) = baseline.apply(findings, display(path), packages);
            baseline.without(&report.stale).save(path)?;
            report.action = Action::Pruned;
            Ok((kept, Some(report)))
        }
        Mode::Apply => match location {
            // A path nobody wrote down, with nothing at it: this project has
            // not adopted a baseline, and that has to stay indistinguishable
            // from a Deadwood without the feature.
            //
            // *Nothing at it* is the whole exemption, and it is narrower than
            // "no file at it". Anything else there — a directory, an entry we
            // are not allowed to stat — is a misconfiguration, and treating it
            // as "nothing is baselined" is the same silent green run this
            // module refuses everywhere else. `is_file()` cannot make that
            // distinction: it answers false for a directory and for a
            // permission error alike, and never says which.
            Location::Default(path) if !exists_as_file(path)? => Ok((findings, None)),
            _ => {
                let path = location.path();
                let baseline = Baseline::load(path)?;
                let (kept, report) = baseline.apply(findings, display(path), packages);
                Ok((kept, Some(report)))
            }
        },
    }
}

/// Whether a readable file sits at `path`, erroring on anything that is
/// neither a file nor an absence.
///
/// The three answers are deliberately kept apart. `Ok(true)` is a baseline to
/// read, `Ok(false)` is a project that has not adopted one, and an error is a
/// path that means something we cannot honor — a directory, or an entry the
/// process may not stat. Only the middle one is allowed to pass silently.
fn exists_as_file(path: &Path) -> Result<bool> {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(true),
        Ok(_) => Err(anyhow::anyhow!(
            "baseline path `{}` is not a file (expected `{FILE_NAME}` or nothing at all)",
            path.display()
        )),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(anyhow::Error::new(err)
            .context(format!("could not read baseline file `{}`", path.display()))),
    }
}

/// Where the baseline file is, and whether the user said so.
///
/// The distinction is the whole error contract: a configured path is a promise
/// that must be kept, while the default location is a place to look.
pub(crate) enum Location {
    /// Named by the `baseline` key of a `deadwood.toml`.
    Configured(PathBuf),
    /// `deadwood-baseline.json` in the workspace root.
    Default(PathBuf),
}

impl Location {
    pub(crate) fn path(&self) -> &Path {
        match self {
            Location::Configured(path) | Location::Default(path) => path,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(kind: FindingKind, file: &str, line: Option<usize>, name: Option<&str>) -> Finding {
        Finding {
            kind,
            severity: Severity::Deny,
            file: PathBuf::from(file),
            line,
            name: name.map(str::to_string),
            module: None,
            message: "why".to_string(),
        }
    }

    /// The same finding, recorded as written in `module` — what the three item
    /// kinds carry and the other four never do.
    fn in_module(mut finding: Finding, module: &str) -> Finding {
        finding.module = Some(module.to_string());
        finding
    }

    fn baseline(entries: &[Finding]) -> Baseline {
        Baseline {
            entries: entries.iter().map(Entry::of).collect(),
        }
    }

    /// A single-package workspace rooted where the findings are, which is what
    /// every case below wants unless it is specifically about two members.
    fn one_package() -> Packages {
        Packages::new([(PathBuf::new(), "only".to_string())])
    }

    fn applied(baseline: &Baseline, findings: Vec<Finding>) -> (Vec<Finding>, Report) {
        baseline.apply(findings, PathBuf::from(FILE_NAME), &one_package())
    }

    fn applied_in(
        baseline: &Baseline,
        findings: Vec<Finding>,
        packages: &Packages,
    ) -> (Vec<Finding>, Report) {
        baseline.apply(findings, PathBuf::from(FILE_NAME), packages)
    }

    fn item(file: &str, name: &str, module: &str) -> Finding {
        in_module(
            finding(FindingKind::UnusedPubItem, file, Some(1), Some(name)),
            module,
        )
    }

    fn reported(findings: &[Finding]) -> Vec<String> {
        findings
            .iter()
            .map(|finding| format!("{}", finding.file.display()))
            .collect()
    }

    fn stale_of(report: &Report) -> Vec<String> {
        report.stale.iter().map(Key::describe).collect()
    }

    /// The property the whole file format rests on: an entry is the report's
    /// finding object, field for field. If these drift, a baseline stops being
    /// something you can produce from `--json` output by hand.
    #[test]
    fn entry_matches_a_report_finding_field_for_field() {
        for sample in [
            in_module(
                finding(FindingKind::UnusedPubItem, "src/lib.rs", Some(3), Some("a")),
                "crate::alpha",
            ),
            finding(FindingKind::UnusedPubItem, "src/lib.rs", Some(3), Some("a")),
            finding(FindingKind::DeadFile, "src/orphan.rs", None, None),
        ] {
            assert_eq!(
                serde_json::to_value(Entry::of(&sample)).unwrap(),
                serde_json::to_value(&sample).unwrap(),
            );
        }
    }

    /// Only the key is required to read an entry back, so a hand-edited
    /// baseline does not need a message anyone has to keep accurate.
    #[test]
    fn a_minimal_entry_carries_only_the_key() {
        let file: File =
            serde_json::from_str(r#"{"findings":[{"kind":"dead_file","file":"src/a.rs"}]}"#)
                .unwrap();
        assert_eq!(
            file.findings[0].key(),
            Key {
                kind: FindingKind::DeadFile,
                file: PathBuf::from("src/a.rs"),
                name: None,
                module: None,
            }
        );
    }

    /// An unknown key in a baseline is a typo in a file the user believes is
    /// suppressing something, which is the same failure `deadwood.toml`
    /// refuses to have.
    #[test]
    fn an_unknown_key_is_an_error() {
        let entry = serde_json::from_str::<File>(
            r#"{"findings":[{"kind":"dead_file","file":"a.rs","lines":3}]}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(entry.contains("lines"), "{entry}");

        let top = serde_json::from_str::<File>(r#"{"finding":[]}"#)
            .unwrap_err()
            .to_string();
        assert!(top.contains("finding"), "{top}");
    }

    /// The other direction of the format change, and the reason it is a one-way
    /// door. `module` was added to a struct that rejects unknown fields, so a
    /// baseline written by a newer Deadwood makes an *older* one exit 2 on a
    /// file it used to read — and there is no version of this code that can fix
    /// that, because the strict reader is the one already released.
    ///
    /// Relaxing the strictness here would buy nothing for `module` and cost the
    /// protection outright: a field that participates in *matching* and is
    /// silently dropped on a typo turns one entry back into the broad key this
    /// phase narrowed, which is the exact silence the phase exists to remove.
    /// So the door stays one-way and is documented as one. This test pins the
    /// mechanism by standing in for the next field with a name of its own.
    #[test]
    fn a_field_a_newer_deadwood_adds_is_rejected_rather_than_ignored() {
        let err = serde_json::from_str::<File>(
            r#"{"findings":[{"kind":"dead_file","file":"a.rs","occurrence":2}]}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("occurrence"), "{err}");
        assert!(
            err.contains("module"),
            "and the fields it does know are listed, which is how a reader \
             identifies the version mismatch: {err}"
        );
    }

    /// The line is recorded and not compared: code moves, and a baseline that
    /// expires whenever an import is added is worse than none.
    #[test]
    fn a_finding_that_moved_is_still_matched() {
        let recorded = finding(FindingKind::UnusedPubItem, "src/lib.rs", Some(3), Some("a"));
        let moved = finding(
            FindingKind::UnusedPubItem,
            "src/lib.rs",
            Some(97),
            Some("a"),
        );

        let (kept, report) = applied(&baseline(&[recorded]), vec![moved]);
        assert!(kept.is_empty());
        assert_eq!(report.suppressed, 1);
        assert!(report.stale.is_empty(), "{:?}", report.stale);
    }

    /// Severity is a `deadwood.toml` decision, so turning a kind down from
    /// `deny` to `warn` must not un-baseline every entry of that kind.
    #[test]
    fn severity_is_recorded_but_never_matched_on() {
        let recorded = finding(FindingKind::DeadFile, "src/orphan.rs", None, None);
        let mut downgraded = recorded.clone();
        downgraded.severity = Severity::Warn;

        let (kept, report) = applied(&baseline(&[recorded]), vec![downgraded]);
        assert!(kept.is_empty(), "{kept:?}");
        assert_eq!(report.suppressed, 1);
    }

    /// The two dependency kinds share a file, a name and a missing line. Only
    /// the kind separates them, so it has to be in the key.
    #[test]
    fn the_two_dependency_kinds_are_never_confused() {
        let unused = finding(
            FindingKind::UnusedDependency,
            "Cargo.toml",
            None,
            Some("serde"),
        );
        let misplaced = finding(
            FindingKind::MisplacedDependency,
            "Cargo.toml",
            None,
            Some("serde"),
        );

        let (kept, report) = applied(&baseline(std::slice::from_ref(&unused)), vec![misplaced]);
        assert_eq!(kept.len(), 1, "the other claim is news: {kept:?}");
        assert_eq!(kept[0].kind, FindingKind::MisplacedDependency);
        assert_eq!(
            report.stale,
            vec![Key::of(&unused)],
            "and the recorded claim no longer occurs"
        );
    }

    /// The two twins, for the module cases below: one kind, one file, one name,
    /// two modules.
    fn twins() -> (Finding, Finding) {
        let twin = |line, module| {
            in_module(
                finding(
                    FindingKind::UnusedPubItem,
                    "src/lib.rs",
                    Some(line),
                    Some("twin"),
                ),
                module,
            )
        };
        (twin(2, "crate::alpha"), twin(9, "crate::beta"))
    }

    /// An entry from a baseline written before the module was recorded says
    /// nothing about modules, so it still covers every finding sharing its
    /// shared key — set semantics, exactly as before. This is the upgrade path:
    /// the file suppresses what it always suppressed, without an edit.
    #[test]
    fn an_entry_with_no_module_still_covers_every_finding_that_shares_its_key() {
        let (first, second) = twins();
        let mut recorded = first.clone();
        recorded.module = None;

        let (kept, report) = applied(&baseline(&[recorded]), vec![first, second]);
        assert!(kept.is_empty(), "{kept:?}");
        assert_eq!(report.suppressed, 2);
        assert!(report.stale.is_empty(), "{:?}", report.stale);
    }

    /// The phase itself: with the module recorded, an entry for one twin covers
    /// that twin and leaves the other one reported at its own line.
    #[test]
    fn an_entry_naming_a_module_leaves_its_same_named_neighbour_reported() {
        let (first, second) = twins();

        let (kept, report) = applied(
            &baseline(std::slice::from_ref(&first)),
            vec![first.clone(), second.clone()],
        );
        assert_eq!(kept.len(), 1, "only the recorded twin is covered: {kept:?}");
        assert_eq!(kept[0].line, second.line, "and at the right line");
        assert_eq!(kept[0].module.as_deref(), Some("crate::beta"));
        assert_eq!(report.suppressed, 1);
        assert!(
            report.stale.is_empty(),
            "the recorded twin still occurs: {:?}",
            report.stale
        );
    }

    /// Nothing counts. One entry still covers *every* finding matching its key,
    /// module included, so two items that share a module as well as a name are
    /// suppressed together — the multiplicity phase 6 rejected stays rejected,
    /// and the residual case is documented rather than guessed at.
    #[test]
    fn one_entry_still_covers_every_finding_sharing_its_module_too() {
        let (first, _) = twins();
        let mut sibling = first.clone();
        sibling.line = Some(40);

        let (kept, report) = applied(
            &baseline(std::slice::from_ref(&first)),
            vec![first, sibling],
        );
        assert!(kept.is_empty(), "{kept:?}");
        assert_eq!(report.suppressed, 2);
    }

    /// A module in the key must not reintroduce what the line was kept out of
    /// the key to avoid: a module path does not change when code above it does.
    #[test]
    fn a_finding_that_moved_within_its_module_is_still_matched() {
        let (recorded, _) = twins();
        let mut moved = recorded.clone();
        moved.line = Some(970);

        let (kept, report) = applied(&baseline(&[recorded]), vec![moved]);
        assert!(kept.is_empty(), "{kept:?}");
        assert_eq!(report.suppressed, 1);
        assert!(report.stale.is_empty(), "{:?}", report.stale);
    }

    /// The teeth on the comparison: an entry naming a module nothing occurs in
    /// suppresses nothing and reads as stale, module and all.
    #[test]
    fn an_entry_naming_a_module_no_finding_is_in_is_stale() {
        let (recorded, occurring) = twins();

        let (kept, report) = applied(&baseline(std::slice::from_ref(&recorded)), vec![occurring]);
        assert_eq!(kept.len(), 1, "nothing was suppressed: {kept:?}");
        assert_eq!(report.suppressed, 0);
        assert_eq!(report.stale, vec![Key::of(&recorded)]);
        assert_eq!(
            report.stale[0].describe(),
            "src/lib.rs: unused_pub_item `twin` in `crate::alpha`",
            "and the report says which module went missing"
        );
    }

    /// The fallback runs both ways. A finding with no module — every kind but
    /// the three that name an item — is covered by an entry that names one,
    /// because the entry knowing more than the finding is not evidence that they
    /// are different findings. Un-baselining on that would punish a project for
    /// a change in what Deadwood records.
    #[test]
    fn a_finding_with_no_module_matches_an_entry_that_names_one() {
        let (recorded, _) = twins();
        let mut unqualified = recorded.clone();
        unqualified.module = None;

        let (kept, report) = applied(&baseline(&[recorded]), vec![unqualified]);
        assert!(kept.is_empty(), "{kept:?}");
        assert_eq!(report.suppressed, 1);
        assert!(
            report.stale.is_empty(),
            "and the entry is not stale either: {:?}",
            report.stale
        );
    }

    /// Suppressing a finding and keeping an entry fresh have to be one relation.
    /// Two copies of it, drifting, would give an entry that both silences a
    /// finding and is reported as no longer occurring.
    #[test]
    fn suppression_and_staleness_are_the_same_relation() {
        let (alpha, beta) = twins();
        let unqualified = |finding: &Finding| {
            let mut copy = finding.clone();
            copy.module = None;
            copy
        };

        for recorded in [alpha.clone(), unqualified(&alpha)] {
            for occurring in [alpha.clone(), beta.clone(), unqualified(&alpha)] {
                let (kept, report) = applied(
                    &baseline(std::slice::from_ref(&recorded)),
                    vec![occurring.clone()],
                );
                assert_eq!(
                    kept.is_empty(),
                    report.stale.is_empty(),
                    "entry {:?} against finding {:?}: suppressed={} stale={:?}",
                    recorded.module,
                    occurring.module,
                    report.suppressed,
                    report.stale,
                );
            }
        }
    }

    /// An entry with no matching finding is stale — reported so the file can
    /// shrink, and reported once however many times it is written.
    #[test]
    fn entries_that_no_longer_occur_are_reported_once_each() {
        let gone = finding(FindingKind::UnusedPubItem, "src/lib.rs", Some(3), Some("a"));
        let here = finding(FindingKind::DeadFile, "src/orphan.rs", None, None);

        let (kept, report) = applied(
            &baseline(&[gone.clone(), gone.clone(), here.clone()]),
            vec![here],
        );
        assert!(kept.is_empty());
        assert_eq!(report.stale, vec![Key::of(&gone)]);
        assert_eq!(
            report.stale[0].describe(),
            "src/lib.rs: unused_pub_item `a`"
        );
    }

    /// Pruning drops exactly the stale entries and keeps the rest as written.
    #[test]
    fn pruning_removes_the_stale_entries_and_nothing_else() {
        let gone = finding(FindingKind::UnusedPubItem, "src/lib.rs", Some(3), Some("a"));
        let here = finding(FindingKind::DeadFile, "src/orphan.rs", None, None);
        let full = baseline(&[gone.clone(), here.clone()]);

        let (_, report) = applied(&full, vec![here.clone()]);
        let pruned = full.without(&report.stale);
        assert_eq!(pruned.entries.len(), 1);
        assert_eq!(pruned.entries[0].key(), Key::of(&here));
        assert_eq!(report.stale, vec![Key::of(&gone)]);
    }

    // -- a moved file: the second pass ------------------------------------
    //
    // Every case below turns on one question — is the leftover evidence enough
    // to say an item moved — and the answer is "no" far more often than "yes".

    /// The headline case of #17: `git mv` changes no item, so the findings in
    /// the moved file stay suppressed and the entries recording them stay
    /// fresh.
    #[test]
    fn an_item_whose_file_moved_is_still_baselined() {
        let before = item("src/legacy.rs", "gone", "crate::legacy");
        let after = item("src/legacy/mod.rs", "gone", "crate::legacy");

        let (kept, report) = applied(&baseline(&[before]), vec![after]);
        assert!(kept.is_empty(), "{:?}", reported(&kept));
        assert!(report.stale.is_empty(), "{:?}", stale_of(&report));
        assert_eq!(report.suppressed, 1);
    }

    /// The exact key answers first and the second pass never sees what it
    /// claimed. Both twins occur, so baselining one leaves the other reported —
    /// this is the case a matcher that simply dropped the file would get wrong,
    /// and the file is what it gets wrong.
    #[test]
    fn two_items_sharing_a_name_and_a_module_in_two_files_are_still_two_findings() {
        let one = item("src/bin/one.rs", "shared", "crate");
        let two = item("src/bin/two.rs", "shared", "crate");

        let (kept, report) = applied(&baseline(std::slice::from_ref(&one)), vec![one, two]);
        assert_eq!(reported(&kept), ["src/bin/two.rs"]);
        assert!(report.stale.is_empty(), "{:?}", stale_of(&report));
    }

    /// One entry and two candidates is not a move, it is two readings of the
    /// same evidence. The pass declines, and the run says so out loud rather
    /// than picking one.
    #[test]
    fn an_entry_with_two_candidates_relocates_to_neither() {
        let recorded = item("src/bin/three.rs", "shared", "crate");
        let one = item("src/bin/one.rs", "shared", "crate");
        let two = item("src/bin/two.rs", "shared", "crate");

        let (kept, report) = applied(&baseline(&[recorded]), vec![one, two]);
        assert_eq!(reported(&kept), ["src/bin/one.rs", "src/bin/two.rs"]);
        assert_eq!(
            stale_of(&report),
            ["src/bin/three.rs: unused_pub_item `shared` in `crate`"]
        );
    }

    /// And the same refusal read the other way: two entries competing for one
    /// finding leaves the finding reported and both entries stale.
    #[test]
    fn two_entries_competing_for_one_finding_relocate_to_neither() {
        let old = item("src/legacy.rs", "gone", "crate::legacy");
        let older = item("src/attic/legacy.rs", "gone", "crate::legacy");
        let now = item("src/legacy/mod.rs", "gone", "crate::legacy");

        let (kept, report) = applied(&baseline(&[old, older]), vec![now]);
        assert_eq!(reported(&kept), ["src/legacy/mod.rs"]);
        assert_eq!(stale_of(&report).len(), 2, "{:?}", stale_of(&report));
    }

    /// `module` is `crate`-relative, so two members can each own a
    /// `crate::legacy::gone` and the module path cannot tell them apart. The
    /// file supplies the package, and the pass will not cross it.
    #[test]
    fn a_move_is_not_matched_across_two_packages() {
        let packages = Packages::new([
            (PathBuf::from("alpha"), "alpha".to_string()),
            (PathBuf::from("beta"), "beta".to_string()),
        ]);
        let recorded = item("alpha/src/legacy.rs", "gone", "crate::legacy");
        let elsewhere = item("beta/src/legacy/mod.rs", "gone", "crate::legacy");

        let (kept, report) = applied_in(&baseline(&[recorded]), vec![elsewhere], &packages);
        assert_eq!(reported(&kept), ["beta/src/legacy/mod.rs"]);
        assert_eq!(stale_of(&report).len(), 1, "{:?}", stale_of(&report));

        // ...and inside one package the same move is matched, so the package is
        // doing the refusing rather than the pass being off entirely.
        let within = item("alpha/src/legacy/mod.rs", "gone", "crate::legacy");
        let recorded = item("alpha/src/legacy.rs", "gone", "crate::legacy");
        let (kept, report) = applied_in(&baseline(&[recorded]), vec![within], &packages);
        assert!(kept.is_empty(), "{:?}", reported(&kept));
        assert!(report.stale.is_empty(), "{:?}", stale_of(&report));
    }

    /// A member nested inside another package's directory answers for its own
    /// files, so the deepest directory wins the lookup.
    #[test]
    fn the_deepest_package_directory_owns_a_nested_members_files() {
        let packages = Packages::new([
            (PathBuf::new(), "root".to_string()),
            (PathBuf::from("crates/inner"), "inner".to_string()),
        ]);
        let recorded = item("crates/inner/src/legacy.rs", "gone", "crate::legacy");
        let elsewhere = item("src/legacy/mod.rs", "gone", "crate::legacy");

        let (kept, report) = applied_in(&baseline(&[recorded]), vec![elsewhere], &packages);
        assert_eq!(reported(&kept), ["src/legacy/mod.rs"]);
        assert_eq!(stale_of(&report).len(), 1, "{:?}", stale_of(&report));
    }

    /// A dead file has no name and no module, so there is nothing for the pass
    /// to compare and nothing it could compare would be evidence. Moving one
    /// behaves exactly as it did before this pass existed.
    #[test]
    fn a_dead_file_that_moved_is_reported_and_its_entry_goes_stale() {
        let before = finding(FindingKind::DeadFile, "src/dropped.rs", None, None);
        let after = finding(FindingKind::DeadFile, "src/attic/dropped.rs", None, None);

        let (kept, report) = applied(&baseline(&[before]), vec![after]);
        assert_eq!(reported(&kept), ["src/attic/dropped.rs"]);
        assert_eq!(stale_of(&report), ["src/dropped.rs: dead_file"]);
    }

    /// The dependency kinds name a crate in a manifest and record no module, so
    /// they are out of the pass's reach by the same construction — a manifest
    /// path moves only when a whole package does, which is a different event.
    #[test]
    fn a_manifest_entry_recorded_at_another_manifest_is_not_relocated() {
        let packages = Packages::new([
            (PathBuf::from("alpha"), "alpha".to_string()),
            (PathBuf::from("beta"), "beta".to_string()),
        ]);
        for kind in [
            FindingKind::UnusedDependency,
            FindingKind::MisplacedDependency,
        ] {
            let before = finding(kind, "alpha/Cargo.toml", None, Some("serde"));
            let after = finding(kind, "beta/Cargo.toml", None, Some("serde"));

            let (kept, report) = applied_in(&baseline(&[before]), vec![after], &packages);
            assert_eq!(reported(&kept), ["beta/Cargo.toml"], "{kind:?}");
            assert_eq!(stale_of(&report).len(), 1, "{kind:?}");
        }
    }

    /// An unsatisfiable gate names a site rather than a definition: it has a
    /// name and no module, and half an identity is not one.
    #[test]
    fn a_gate_site_has_a_name_and_no_module_so_it_is_not_relocated() {
        let before = finding(
            FindingKind::UnsatisfiableCfg,
            "src/lib.rs",
            Some(3),
            Some("mod imp"),
        );
        let after = finding(
            FindingKind::UnsatisfiableCfg,
            "src/moved.rs",
            Some(3),
            Some("mod imp"),
        );

        let (kept, report) = applied(&baseline(&[before]), vec![after]);
        assert_eq!(reported(&kept), ["src/moved.rs"]);
        assert_eq!(stale_of(&report).len(), 1, "{:?}", stale_of(&report));
    }

    /// The kind is as load bearing in a relocation as it is in the key. An item
    /// and a re-export of it are different claims, and a file move is not a
    /// licence to swap one for the other.
    #[test]
    fn a_relocation_does_not_cross_two_kinds() {
        let before = item("src/legacy.rs", "gone", "crate::legacy");
        let after = in_module(
            finding(
                FindingKind::UnusedReexport,
                "src/legacy/mod.rs",
                Some(1),
                Some("gone"),
            ),
            "crate::legacy",
        );

        let (kept, report) = applied(&baseline(&[before]), vec![after]);
        assert_eq!(reported(&kept), ["src/legacy/mod.rs"]);
        assert_eq!(stale_of(&report).len(), 1, "{:?}", stale_of(&report));
    }

    /// And the module is what makes the identity an identity. An item that
    /// changed module changed its position in the crate's tree, which is a
    /// different item as far as anything here can tell.
    #[test]
    fn a_relocation_does_not_cross_two_modules() {
        let before = item("src/legacy.rs", "gone", "crate::legacy");
        let after = item("src/legacy/mod.rs", "gone", "crate::compat::legacy");

        let (kept, report) = applied(&baseline(&[before]), vec![after]);
        assert_eq!(reported(&kept), ["src/legacy/mod.rs"]);
        assert_eq!(stale_of(&report).len(), 1, "{:?}", stale_of(&report));
    }

    /// The exact key claims its matches first, and what it claims is out of the
    /// second pass entirely — on *both* sides. So a baselined `shared` that is
    /// still where it was does not make its neighbour's move ambiguous: the
    /// matched pair is gone from the leftovers before anything is counted.
    #[test]
    fn what_the_exact_key_matched_does_not_crowd_out_a_move_beside_it() {
        let staying = item("src/bin/one.rs", "shared", "crate");
        let recorded = item("src/bin/three.rs", "shared", "crate");
        let arrived = item("src/bin/two.rs", "shared", "crate");

        let (kept, report) = applied(
            &baseline(&[staying.clone(), recorded]),
            vec![staying, arrived],
        );
        assert!(kept.is_empty(), "{:?}", reported(&kept));
        assert!(report.stale.is_empty(), "{:?}", stale_of(&report));
        assert_eq!(report.suppressed, 2);
    }

    /// A hand-written entry that names a module and no name identifies no item,
    /// so it relocates onto nothing.
    #[test]
    fn an_entry_with_a_module_and_no_name_is_not_relocated() {
        let mut before = item("src/legacy.rs", "gone", "crate::legacy");
        before.name = None;
        let after = item("src/legacy/mod.rs", "gone", "crate::legacy");

        let (kept, report) = applied(&baseline(&[before]), vec![after]);
        assert_eq!(reported(&kept), ["src/legacy/mod.rs"]);
        assert_eq!(stale_of(&report).len(), 1, "{:?}", stale_of(&report));
    }

    /// A package whose whole directory moved leaves entries pointing outside
    /// every package this workspace has. There is no package to scope the pass
    /// to, so it declines — today's behaviour, and honestly a limitation.
    #[test]
    fn an_entry_outside_every_package_directory_is_not_relocated() {
        let packages = Packages::new([(PathBuf::from("vendor/alpha"), "alpha".to_string())]);
        let recorded = item("crates/alpha/src/legacy.rs", "gone", "crate::legacy");
        let now = item("vendor/alpha/src/legacy.rs", "gone", "crate::legacy");

        let (kept, report) = applied_in(
            &baseline(std::slice::from_ref(&recorded)),
            vec![now.clone()],
            &packages,
        );
        assert_eq!(reported(&kept), ["vendor/alpha/src/legacy.rs"]);
        assert_eq!(stale_of(&report).len(), 1, "{:?}", stale_of(&report));

        // "No package" is not a package the two sides can agree on: with
        // nothing resolving, nothing relocates either.
        let (kept, report) = applied_in(&baseline(&[recorded]), vec![now], &Packages::default());
        assert_eq!(reported(&kept), ["vendor/alpha/src/legacy.rs"]);
        assert_eq!(stale_of(&report).len(), 1, "{:?}", stale_of(&report));
    }

    /// A finding the exact key already matched is not a candidate for anyone's
    /// relocation, and neither is the entry that matched it. Both sides are
    /// leftovers or neither is — which is what lets a move be found at all in a
    /// file that also holds an ordinary baselined finding of the same identity.
    #[test]
    fn a_finding_the_exact_key_matched_is_not_a_relocation_candidate() {
        let recorded = item("src/legacy.rs", "gone", "crate::legacy");
        let also = item("src/other.rs", "gone", "crate::legacy");
        let moved = item("src/legacy/mod.rs", "gone", "crate::legacy");

        // `also` is claimed by its own entry, so the leftovers are exactly one
        // entry and exactly one finding and the move is found. Offer the whole
        // set to the second pass instead and `recorded` has two candidates, the
        // one-to-one rule refuses, and `moved` is reported.
        let (kept, report) = applied(&baseline(&[recorded, also.clone()]), vec![also, moved]);
        assert!(kept.is_empty(), "{:?}", reported(&kept));
        assert!(report.stale.is_empty(), "{:?}", stale_of(&report));
        assert_eq!(report.suppressed, 2);
    }

    /// Suppression and staleness come out of one relocation set, so an entry
    /// can never both suppress a finding and read as no longer occurring.
    #[test]
    fn relocation_answers_suppression_and_staleness_together() {
        let recorded = item("src/legacy.rs", "gone", "crate::legacy");
        for occurring in [
            item("src/legacy/mod.rs", "gone", "crate::legacy"),
            item("src/legacy/mod.rs", "gone", "crate::other"),
            item("src/legacy/mod.rs", "other", "crate::legacy"),
        ] {
            let (kept, report) = applied(
                &baseline(std::slice::from_ref(&recorded)),
                vec![occurring.clone()],
            );
            assert_eq!(
                kept.is_empty(),
                report.stale.is_empty(),
                "against {:?}/{:?}",
                occurring.file,
                occurring.module,
            );
        }
    }

    /// A baseline naming a file that is not there must never read as "nothing
    /// is baselined": the run would be green for the wrong reason.
    #[test]
    fn a_missing_file_is_an_error_naming_the_fix() {
        let err = format!(
            "{:#}",
            Baseline::load(Path::new("/nonexistent/deadwood-baseline.json")).unwrap_err()
        );
        assert!(err.contains("could not read baseline file"), "{err}");
        assert!(err.contains("--write-baseline"), "{err}");
    }
}
