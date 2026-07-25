//! Rendering of analysis results for humans and machines.

use anyhow::Result;

use crate::{Analysis, FindingKind};

/// Human-readable report, grouped by finding kind.
pub fn render_text(analysis: &Analysis) -> String {
    if analysis.findings.is_empty() {
        return "No issues found.\n".to_string();
    }

    let mut out = String::new();
    for (kind, header) in [
        (FindingKind::DeadFile, "Dead files"),
        (FindingKind::UnusedPubItem, "Unused public items"),
        (FindingKind::UnusedReexport, "Unused re-exports"),
    ] {
        let group: Vec<_> = analysis
            .findings
            .iter()
            .filter(|f| f.kind == kind)
            .collect();
        if group.is_empty() {
            continue;
        }
        out.push_str(header);
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
    out.push_str(&format!(
        "{} finding(s) in workspace `{}`.\n",
        analysis.findings.len(),
        analysis.workspace_root.display()
    ));
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
                    file: PathBuf::from("src/lib.rs"),
                    line: Some(3),
                    name: Some("dead".into()),
                    message: "pub fn `dead` is never referenced by any resolved path in this \
                              workspace"
                        .into(),
                },
                Finding {
                    kind: FindingKind::UnusedReexport,
                    file: PathBuf::from("src/lib.rs"),
                    line: Some(9),
                    name: Some("Stale".into()),
                    message: "`pub use` re-export of `Stale` is never referenced through this \
                              module"
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
        assert!(text.contains("2 finding(s)"));
    }

    #[test]
    fn text_report_groups_re_exports_separately() {
        let text = render_text(&sample());
        let items = text.find("Unused public items:").expect("items group");
        let reexports = text.find("Unused re-exports:").expect("re-exports group");
        assert!(items < reexports, "groups render in kind order:\n{text}");
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
    }
}
