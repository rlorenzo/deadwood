//! Rendering of analysis results for humans and machines.
//!
//! Severity is per finding *kind*, so it is rendered on the group header
//! rather than repeated on every line: `Unused public items (warn)` says
//! exactly once that nothing below it will fail the run. The summary line
//! grows a breakdown only when some kind is not `deny`, which keeps the
//! default output — the one every existing consumer already parses —
//! unchanged.
//!
//! The baseline gets the same treatment for the same reason. A run without one
//! renders exactly as it always did, down to the `No issues found.` line; a run
//! with one adds a block naming the file, what it did, and any entry that no
//! longer matches anything. Baselined findings themselves are not printed —
//! they are not what the reader has to act on, and [`crate::baseline`] has the
//! argument.

use anyhow::Result;

use crate::baseline::{Action, Report};
use crate::config::Severity;
use crate::{Analysis, FindingKind};

/// Human-readable report, grouped by finding kind.
pub fn render_text(analysis: &Analysis) -> String {
    if analysis.findings.is_empty() {
        let mut out = "No issues found.\n".to_string();
        if let Some(baseline) = &analysis.baseline {
            out.push_str(&render_baseline(baseline));
        }
        return out;
    }

    let mut out = String::new();
    for (kind, header) in [
        (FindingKind::DeadFile, "Dead files"),
        (FindingKind::UnsatisfiableCfg, "Unsatisfiable cfg gates"),
        (FindingKind::UnusedPubItem, "Unused public items"),
        (FindingKind::UnusedReexport, "Unused re-exports"),
        (FindingKind::UnusedDependency, "Unused dependencies"),
        (FindingKind::MisplacedDependency, "Misplaced dependencies"),
        (FindingKind::TestOnlyItem, "Test-only public items"),
    ] {
        let group: Vec<_> = analysis
            .findings
            .iter()
            .filter(|f| f.kind == kind)
            .collect();
        let Some(first) = group.first() else {
            continue;
        };
        out.push_str(header);
        if first.severity != Severity::Deny {
            out.push_str(&format!(" ({})", first.severity.label()));
        }
        out.push_str(":\n");
        for finding in group {
            match finding.line {
                Some(line) => {
                    out.push_str(&format!("  {}:{line}: ", finding.file.display()));
                }
                None => out.push_str(&format!("  {}: ", finding.file.display())),
            }
            out.push_str(&finding.message);
            out.push('\n');
        }
        out.push('\n');
    }
    let denied = analysis
        .findings
        .iter()
        .filter(|f| f.severity == Severity::Deny)
        .count();
    out.push_str(&format!(
        "{} finding(s) in workspace `{}`",
        analysis.findings.len(),
        analysis.workspace_root.display()
    ));
    if denied < analysis.findings.len() {
        out.push_str(&format!(
            ": {denied} deny, {} warn (warn findings do not fail the run)",
            analysis.findings.len() - denied
        ));
    }
    out.push_str(".\n");
    if let Some(baseline) = &analysis.baseline {
        out.push_str(&render_baseline(baseline));
    }
    out
}

/// The baseline block: what the file did, and what in it has gone stale.
///
/// It follows the findings rather than leading them, because the findings are
/// the run's answer and this is a footnote about the run's *scope*. The stale
/// list names its own fix, since an unactionable report is how a baseline rots.
fn render_baseline(baseline: &Report) -> String {
    let path = baseline.path.display();
    let mut out = String::new();
    if !baseline.stale.is_empty() {
        // Spelled out rather than defaulted: a new `Action` would otherwise
        // inherit the "rerun with --prune-baseline" advice by accident, and
        // the compiler is the only reader guaranteed to notice.
        let heading = match baseline.action {
            Action::Pruned => format!("Pruned from baseline `{path}` (no longer occur)"),
            Action::Applied | Action::Written => format!(
                "Stale baseline entries in `{path}` (no longer occur; rerun with \
                 --prune-baseline to drop them)"
            ),
        };
        out.push_str(&format!("\n{heading}:\n"));
        for key in &baseline.stale {
            out.push_str(&format!("  {}\n", key.describe()));
        }
    }
    let summary = match baseline.action {
        Action::Applied => format!(
            "{} finding(s) suppressed by baseline `{path}`.\n",
            baseline.suppressed
        ),
        Action::Written => format!(
            "Wrote {} finding(s) to baseline `{path}`.\n",
            baseline.suppressed
        ),
        Action::Pruned => format!(
            "{} finding(s) suppressed by baseline `{path}`, {} stale entry(ies) removed.\n",
            baseline.suppressed,
            baseline.stale.len()
        ),
    };
    out.push_str(&summary);
    out
}

/// Machine-readable JSON report (findings and warnings included).
pub fn render_json(analysis: &Analysis) -> Result<String> {
    Ok(serde_json::to_string_pretty(analysis)?)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::Finding;

    fn sample() -> Analysis {
        Analysis {
            workspace_root: PathBuf::from("/ws"),
            findings: vec![
                Finding {
                    kind: FindingKind::UnusedPubItem,
                    severity: Severity::Deny,
                    file: PathBuf::from("src/lib.rs"),
                    line: Some(3),
                    name: Some("dead".into()),
                    module: Some("crate".into()),
                    namespace: Some(crate::Namespace::Value),
                    message: "pub fn `dead` is never referenced by any resolved path in this \
                              workspace"
                        .into(),
                },
                Finding {
                    kind: FindingKind::UnusedReexport,
                    severity: Severity::Deny,
                    file: PathBuf::from("src/lib.rs"),
                    line: Some(9),
                    name: Some("Stale".into()),
                    module: Some("crate".into()),
                    namespace: Some(crate::Namespace::Both),
                    message: "`pub use` re-export of `Stale` is never referenced through this \
                              module"
                        .into(),
                },
                Finding {
                    kind: FindingKind::UnusedDependency,
                    severity: Severity::Deny,
                    file: PathBuf::from("Cargo.toml"),
                    line: None,
                    name: Some("regex".into()),
                    module: None,
                    namespace: None,
                    message: "dependency `regex` is never referenced by any target of package \
                              `demo`"
                        .into(),
                },
            ],
            warnings: vec![],
            baseline: None,
        }
    }

    #[test]
    fn text_report_includes_location_and_summary() {
        let text = render_text(&sample());
        assert!(text.contains("src/lib.rs:3:"));
        assert!(text.contains("3 finding(s)"));
    }

    #[test]
    fn text_report_groups_re_exports_separately() {
        let text = render_text(&sample());
        let items = text.find("Unused public items:").expect("items group");
        let reexports = text.find("Unused re-exports:").expect("re-exports group");
        let dependencies = text
            .find("Unused dependencies:")
            .expect("dependencies group");
        assert!(items < reexports, "groups render in kind order:\n{text}");
        assert!(
            reexports < dependencies,
            "groups render in kind order:\n{text}"
        );
    }

    /// A dependency finding points at the manifest, which has no line number
    /// to give: the location line must still render.
    #[test]
    fn a_finding_without_a_line_renders_its_file_alone() {
        let text = render_text(&sample());
        assert!(
            text.contains("  Cargo.toml: dependency `regex`"),
            "manifest findings render without a line:\n{text}"
        );
    }

    /// Default severities must render exactly as they did before severity
    /// existed: no marker on the headers, no breakdown on the summary.
    #[test]
    fn an_all_deny_report_carries_no_severity_decoration() {
        let text = render_text(&sample());
        assert!(text.contains("Unused public items:\n"), "{text}");
        assert!(
            text.ends_with("3 finding(s) in workspace `/ws`.\n"),
            "{text}"
        );
    }

    /// A `warn` finding has to be visibly different from a `deny` one, or the
    /// exit code and the output tell different stories.
    #[test]
    fn warn_findings_are_marked_on_their_group_and_counted_separately() {
        let mut analysis = sample();
        analysis.findings[2].severity = Severity::Warn;

        let text = render_text(&analysis);
        assert!(text.contains("Unused dependencies (warn):\n"), "{text}");
        assert!(text.contains("Unused public items:\n"), "{text}");
        assert!(
            text.contains("3 finding(s) in workspace `/ws`: 2 deny, 1 warn"),
            "the summary must separate what fails the run from what does not:\n{text}"
        );
    }

    /// A new finding kind needs a group of its own, or its findings would be
    /// dropped from the text report while still counting toward the summary.
    #[test]
    fn every_finding_kind_has_a_group_of_its_own() {
        let analysis = Analysis {
            workspace_root: PathBuf::from("/ws"),
            findings: vec![Finding {
                kind: FindingKind::UnsatisfiableCfg,
                severity: Severity::Deny,
                file: PathBuf::from("src/lib.rs"),
                line: Some(4),
                name: Some("win".into()),
                module: None,
                namespace: None,
                message: "`#[cfg(feature = \"gone\")]` can never hold: package `demo` declares no \
                          feature `gone`"
                    .into(),
            }],
            warnings: vec![],
            baseline: None,
        };

        let text = render_text(&analysis);
        assert!(text.contains("Unsatisfiable cfg gates:\n"), "{text}");
        assert!(text.contains("  src/lib.rs:4: `#[cfg("), "{text}");
        assert!(text.contains("1 finding(s)"), "{text}");

        let value: serde_json::Value =
            serde_json::from_str(&render_json(&analysis).unwrap()).unwrap();
        assert_eq!(value["findings"][0]["kind"], "unsatisfiable_cfg");
        assert_eq!(value["findings"][0]["name"], "win");

        // And not just this one: every kind there is must land in some group,
        // or its findings vanish from the text while still being counted.
        for kind in FindingKind::ALL {
            let mut one = sample();
            one.findings.truncate(1);
            one.findings[0].kind = kind;
            let text = render_text(&one);
            assert!(
                text.lines().any(|line| line.starts_with("  src/lib.rs:3:")),
                "`{}` has no group of its own:\n{text}",
                kind.label()
            );
        }
    }

    #[test]
    fn empty_analysis_reports_clean() {
        let clean = Analysis {
            workspace_root: PathBuf::from("/ws"),
            findings: vec![],
            warnings: vec![],
            baseline: None,
        };
        assert_eq!(render_text(&clean), "No issues found.\n");
    }

    fn with_baseline(
        action: Action,
        suppressed: usize,
        stale: Vec<crate::baseline::Key>,
    ) -> Report {
        Report {
            path: PathBuf::from("deadwood-baseline.json"),
            action,
            suppressed,
            stale,
        }
    }

    fn stale_key() -> crate::baseline::Key {
        crate::baseline::Key {
            kind: FindingKind::UnusedPubItem,
            file: PathBuf::from("src/old.rs"),
            name: Some("gone".into()),
            module: None,
            namespace: None,
        }
    }

    /// A clean run under a baseline is still clean, and still says so — plus
    /// how much of the quiet it owes to the file.
    #[test]
    fn a_baseline_is_summarized_even_when_nothing_is_left_to_report() {
        let mut clean = sample();
        clean.findings.clear();
        clean.baseline = Some(with_baseline(Action::Applied, 3, Vec::new()));

        assert_eq!(
            render_text(&clean),
            "No issues found.\n3 finding(s) suppressed by baseline `deadwood-baseline.json`.\n"
        );
    }

    /// A stale entry is the baseline shrinking rather than rotting, so it is
    /// named and so is the flag that removes it.
    #[test]
    fn stale_entries_are_listed_with_the_flag_that_prunes_them() {
        let mut analysis = sample();
        analysis.findings.clear();
        analysis.baseline = Some(with_baseline(Action::Applied, 1, vec![stale_key()]));

        let text = render_text(&analysis);
        assert!(text.contains("Stale baseline entries"), "{text}");
        assert!(text.contains("--prune-baseline"), "{text}");
        assert!(
            text.contains("  src/old.rs: unused_pub_item `gone`\n"),
            "{text}"
        );
    }

    /// Writing and pruning say what they did, because both changed a committed
    /// file and the user has to see it in the diff they are about to make.
    #[test]
    fn writing_and_pruning_each_report_what_they_changed() {
        let mut written = sample();
        written.findings.clear();
        written.baseline = Some(with_baseline(Action::Written, 3, Vec::new()));
        assert!(
            render_text(&written).contains("Wrote 3 finding(s) to baseline"),
            "{}",
            render_text(&written)
        );

        let mut pruned = sample();
        pruned.findings.clear();
        pruned.baseline = Some(with_baseline(Action::Pruned, 2, vec![stale_key()]));
        let text = render_text(&pruned);
        assert!(text.contains("Pruned from baseline"), "{text}");
        assert!(
            !text.contains("--prune-baseline"),
            "already pruned:\n{text}"
        );
        assert!(
            text.contains(
                "2 finding(s) suppressed by baseline `deadwood-baseline.json`, 1 stale \
                           entry(ies) removed."
            ),
            "{text}"
        );
    }

    /// The JSON grows a `baseline` object only when there was one, so every
    /// consumer that never adopts a baseline parses byte-identical output.
    #[test]
    fn the_json_carries_the_baseline_only_when_one_was_used() {
        let value: serde_json::Value =
            serde_json::from_str(&render_json(&sample()).unwrap()).unwrap();
        assert!(value.get("baseline").is_none(), "{value}");

        let mut analysis = sample();
        analysis.baseline = Some(with_baseline(Action::Applied, 4, vec![stale_key()]));
        let value: serde_json::Value =
            serde_json::from_str(&render_json(&analysis).unwrap()).unwrap();
        assert_eq!(value["baseline"]["action"], "applied");
        assert_eq!(value["baseline"]["suppressed"], 4);
        assert_eq!(value["baseline"]["stale"][0]["kind"], "unused_pub_item");
        assert_eq!(value["baseline"]["stale"][0]["name"], "gone");
    }

    #[test]
    fn json_report_is_valid_and_typed() {
        let json = render_json(&sample()).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["findings"][0]["kind"], "unused_pub_item");
        assert_eq!(value["findings"][0]["line"], 3);
        assert_eq!(value["findings"][1]["kind"], "unused_reexport");
        assert_eq!(value["findings"][1]["name"], "Stale");
        assert_eq!(value["findings"][2]["kind"], "unused_dependency");
        assert_eq!(value["findings"][2]["name"], "regex");
        assert!(
            value["findings"][2].get("line").is_none(),
            "a manifest finding carries no line number"
        );
    }

    /// Machine consumers gate on the JSON, so severity has to be there too —
    /// a CI job that only fails on `deny` needs to see which is which.
    #[test]
    fn json_report_carries_the_severity_of_every_finding() {
        let mut analysis = sample();
        analysis.findings[1].severity = Severity::Warn;
        let value: serde_json::Value =
            serde_json::from_str(&render_json(&analysis).unwrap()).unwrap();
        assert_eq!(value["findings"][0]["severity"], "deny");
        assert_eq!(value["findings"][1]["severity"], "warn");
    }
}
