//! Rendering of analysis results for humans and machines.
//!
//! Severity is per finding *kind*, so it is rendered on the group header
//! rather than repeated on every line: `Unused public items (warn)` says
//! exactly once that nothing below it will fail the run. The summary line
//! grows a breakdown only when some kind is not `deny`, which keeps the
//! default output — the one every existing consumer already parses —
//! unchanged.

use anyhow::Result;

use crate::config::Severity;
use crate::{Analysis, FindingKind};

/// Human-readable report, grouped by finding kind.
pub fn render_text(analysis: &Analysis) -> String {
    if analysis.findings.is_empty() {
        return "No issues found.\n".to_string();
    }

    let mut out = String::new();
    for (kind, header) in [
        (FindingKind::DeadFile, "Dead files"),
        (FindingKind::UnsatisfiableCfg, "Unsatisfiable cfg gates"),
        (FindingKind::UnusedPubItem, "Unused public items"),
        (FindingKind::UnusedReexport, "Unused re-exports"),
        (FindingKind::UnusedDependency, "Unused dependencies"),
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
                    message: "dependency `regex` is never referenced by any target of package \
                              `demo`"
                        .into(),
                },
            ],
            warnings: vec![],
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
                message: "`#[cfg(feature = \"gone\")]` can never hold: package `demo` declares no \
                          feature `gone`"
                    .into(),
            }],
            warnings: vec![],
        };

        let text = render_text(&analysis);
        assert!(text.contains("Unsatisfiable cfg gates:\n"), "{text}");
        assert!(text.contains("  src/lib.rs:4: `#[cfg("), "{text}");
        assert!(text.contains("1 finding(s)"), "{text}");

        let value: serde_json::Value =
            serde_json::from_str(&render_json(&analysis).unwrap()).unwrap();
        assert_eq!(value["findings"][0]["kind"], "unsatisfiable_cfg");
        assert_eq!(value["findings"][0]["name"], "win");
    }

    #[test]
    fn empty_analysis_reports_clean() {
        let clean = Analysis {
            workspace_root: PathBuf::from("/ws"),
            findings: vec![],
            warnings: vec![],
        };
        assert_eq!(render_text(&clean), "No issues found.\n");
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
