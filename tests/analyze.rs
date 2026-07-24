//! End-to-end test: run the full analysis on the fixture package in
//! `tests/fixtures/simple` and check the findings. Requires `cargo` on PATH
//! (used for `cargo metadata`), which is a given anywhere `cargo test` runs.

use std::path::{Path, PathBuf};

use deadwood::{FindingKind, analyze};

#[test]
fn detects_dead_file_and_unused_pub_item() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/simple");
    let analysis = analyze(&fixture).expect("analysis should succeed on the fixture");

    assert!(
        analysis.warnings.is_empty(),
        "unexpected warnings: {:?}",
        analysis.warnings
    );

    let dead_files: Vec<&PathBuf> = analysis
        .findings
        .iter()
        .filter(|f| f.kind == FindingKind::DeadFile)
        .map(|f| &f.file)
        .collect();
    assert_eq!(dead_files, vec![&PathBuf::from("src/orphan.rs")]);

    let unused: Vec<&str> = analysis
        .findings
        .iter()
        .filter(|f| f.kind == FindingKind::UnusedPubItem)
        .filter_map(|f| f.name.as_deref())
        .collect();
    // `entry` and `dead_fn` are pub and referenced nowhere else in the
    // fixture workspace; `helper` is pub but called from lib.rs, and
    // `lonely` lives in a dead file, which is reported as a whole instead.
    assert_eq!(unused, vec!["entry", "dead_fn"]);

    // Findings carry 1-based line numbers pointing at the item name.
    let dead_fn = analysis
        .findings
        .iter()
        .find(|f| f.name.as_deref() == Some("dead_fn"))
        .unwrap();
    assert_eq!(dead_fn.line, Some(7));
    assert_eq!(dead_fn.file, PathBuf::from("src/lib.rs"));
}

/// A module loaded via `#[path]` keeps the stem-based directory rule for its
/// own children: `#[path = "renamed_file.rs"] mod alias;` with `mod child;`
/// inside must find `src/renamed_file/child.rs`, not `src/child.rs`.
#[test]
fn path_attr_module_children_resolve_in_stem_directory() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pathmod");
    let analysis = analyze(&fixture).expect("analysis should succeed on the fixture");

    assert!(
        analysis.warnings.is_empty(),
        "unexpected warnings: {:?}",
        analysis.warnings
    );
    assert!(
        !analysis
            .findings
            .iter()
            .any(|f| f.kind == FindingKind::DeadFile),
        "no file in the fixture is dead: {:?}",
        analysis.findings
    );
    // `seven` is called through the aliased module, so it must not be flagged.
    assert!(
        !analysis
            .findings
            .iter()
            .any(|f| f.name.as_deref() == Some("seven")),
        "seven is referenced and must not be reported"
    );
}

/// A file included by several workspace members via `#[path]` enters the
/// identifier census exactly once, so its dead pub items are still reported
/// (and reported once, not per including package).
#[test]
fn file_shared_between_packages_is_counted_once() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/shared");
    let analysis = analyze(&fixture).expect("analysis should succeed on the fixture");

    assert!(
        analysis.warnings.is_empty(),
        "unexpected warnings: {:?}",
        analysis.warnings
    );
    let shared_dead_reports = analysis
        .findings
        .iter()
        .filter(|f| f.name.as_deref() == Some("shared_dead"))
        .count();
    assert_eq!(
        shared_dead_reports, 1,
        "shared_dead is unused and must be reported exactly once: {:?}",
        analysis.findings
    );
}
