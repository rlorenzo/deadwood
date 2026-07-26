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
//! `line`, `name`, `severity` and `message` are accepted, written back, and
//! shown to whoever reads the file, but see below for which of them the
//! matching actually uses. Unknown keys are rejected, for the reason every
//! other file Deadwood reads rejects them: a key that is parsed and ignored is
//! a setting the user believes is working.
//!
//! # The match key, and what is deliberately not in it
//!
//! An entry matches a finding when **kind, file and name** are equal. That is
//! the whole key, and each exclusion is a decision:
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
//! `name` is an `Option`, and `None` is a value like any other: a dead file has
//! no name, and matches only a baseline entry that has none either.
//!
//! The file path is what makes the key specific enough to be safe — without it
//! one `unused_pub_item foo` entry would cover every `foo` in the workspace,
//! and two `dead_file` findings would be indistinguishable. The price is that
//! moving a file un-baselines everything in it
//! ([#17](https://github.com/rlorenzo/deadwood/issues/17)); the failure mode is
//! noise on a deliberate, reviewed act, which is the right side to be on.
//!
//! # Two findings that share a key
//!
//! The key is not unique. Two `pub fn twin` in two inline modules of one file
//! produce two `unused_pub_item` findings with the same kind, file and name,
//! differing only in the line the key deliberately ignores.
//!
//! One baseline entry suppresses **every** finding matching its key — set
//! semantics, not counting. The alternative, recording a multiplicity and
//! reporting the (n+1)th occurrence as new, was rejected: since lines are not
//! matched, we could not say *which* occurrence is the new one, so the report
//! would point at a line that is very likely baselined. That is a wrong
//! finding. Suppressing the extra one is a missed finding. The tenet is
//! explicit about which of those to prefer, and
//! [#16](https://github.com/rlorenzo/deadwood/issues/16) tracks the gap.
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

use std::collections::HashSet;
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

/// The identity of a finding, for baseline matching: kind, file, and name.
///
/// Deliberately not the line (code moves), the severity (a config decision) or
/// the message (prose); the module docs have the reasoning for each.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct Key {
    pub kind: FindingKind,
    pub file: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Key {
    fn of(finding: &Finding) -> Key {
        Key {
            kind: finding.kind,
            file: finding.file.clone(),
            name: finding.name.clone(),
        }
    }

    /// `src/lib.rs: unused_pub_item `dead``, for the text report.
    pub fn describe(&self) -> String {
        let kind = self.kind.label();
        match &self.name {
            Some(name) => format!("{}: {kind} `{name}`", self.file.display()),
            None => format!("{}: {kind}", self.file.display()),
        }
    }
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
            message: Some(finding.message.clone()),
        }
    }

    fn key(&self) -> Key {
        Key {
            kind: self.kind,
            file: self.file.clone(),
            name: self.name.clone(),
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
#[derive(Debug, Default)]
pub struct Baseline {
    entries: Vec<Entry>,
}

impl Baseline {
    /// Read and validate the baseline at `path`.
    ///
    /// A missing file is an error, not an empty baseline: a run that cannot
    /// find the file it was told to use is a run whose result means nothing.
    pub fn load(path: &Path) -> Result<Baseline> {
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
    pub fn write(path: &Path, findings: &[Finding]) -> Result<()> {
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
    /// `path` is only carried into the report; nothing is read or written.
    pub fn apply(&self, findings: Vec<Finding>, path: PathBuf) -> (Vec<Finding>, Report) {
        let occurring: HashSet<Key> = findings.iter().map(Key::of).collect();
        let baselined: HashSet<Key> = self.entries.iter().map(Entry::key).collect();

        let total = findings.len();
        let kept: Vec<Finding> = findings
            .into_iter()
            .filter(|finding| !baselined.contains(&Key::of(finding)))
            .collect();
        let suppressed = total - kept.len();

        (
            kept,
            Report {
                path,
                action: Action::Applied,
                suppressed,
                stale: self.stale(&occurring),
            },
        )
    }

    /// The keys this baseline records that nothing in `occurring` matches, in
    /// file order and without repeats.
    fn stale(&self, occurring: &HashSet<Key>) -> Vec<Key> {
        let mut seen = HashSet::new();
        self.entries
            .iter()
            .map(|entry| entry.key())
            .filter(|key| !occurring.contains(key))
            .filter(|key| seen.insert(key.clone()))
            .collect()
    }

    /// Drop the entries no finding matches, keeping every surviving entry's
    /// fields and the order they were written in.
    ///
    /// Not the file's bytes: pruning re-serializes, so a hand-formatted or
    /// hand-abbreviated file comes back in the canonical form
    /// [`Baseline::save`] emits. Only the entries are preserved, which is the
    /// property that matters — pruning must never quietly accept a new finding
    /// or drop a field off a surviving one.
    fn without_stale(&self, occurring: &HashSet<Key>) -> Baseline {
        Baseline {
            entries: self
                .entries
                .iter()
                .filter(|entry| occurring.contains(&entry.key()))
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
            let occurring: HashSet<Key> = findings.iter().map(Key::of).collect();
            baseline.without_stale(&occurring).save(path)?;
            // `apply`'s stale set is exactly the entries just removed, which is
            // what the report should name under this action.
            let (kept, mut report) = baseline.apply(findings, display(path));
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
                let (kept, report) = baseline.apply(findings, display(path));
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
            message: "why".to_string(),
        }
    }

    fn baseline(entries: &[Finding]) -> Baseline {
        Baseline {
            entries: entries.iter().map(Entry::of).collect(),
        }
    }

    fn applied(baseline: &Baseline, findings: Vec<Finding>) -> (Vec<Finding>, Report) {
        baseline.apply(findings, PathBuf::from(FILE_NAME))
    }

    /// The property the whole file format rests on: an entry is the report's
    /// finding object, field for field. If these drift, a baseline stops being
    /// something you can produce from `--json` output by hand.
    #[test]
    fn entry_matches_a_report_finding_field_for_field() {
        for sample in [
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

    /// Set semantics, pinned: one entry covers every finding sharing its key,
    /// because we cannot say which of them is the new one.
    #[test]
    fn one_entry_suppresses_every_finding_that_shares_its_key() {
        let first = finding(
            FindingKind::UnusedPubItem,
            "src/lib.rs",
            Some(2),
            Some("twin"),
        );
        let second = finding(
            FindingKind::UnusedPubItem,
            "src/lib.rs",
            Some(9),
            Some("twin"),
        );

        let (kept, report) = applied(
            &baseline(std::slice::from_ref(&first)),
            vec![first.clone(), second],
        );
        assert!(kept.is_empty(), "{kept:?}");
        assert_eq!(report.suppressed, 2);
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
        let full = baseline(&[gone, here.clone()]);

        let occurring: HashSet<Key> = [Key::of(&here)].into_iter().collect();
        let pruned = full.without_stale(&occurring);
        assert_eq!(pruned.entries.len(), 1);
        assert_eq!(pruned.entries[0].key(), Key::of(&here));
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
