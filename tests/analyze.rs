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
