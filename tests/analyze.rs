//! End-to-end tests: run the full analysis on the fixture packages under
//! `tests/fixtures/` and check the findings. Requires `cargo` on PATH (used
//! for `cargo metadata`), which is a given anywhere `cargo test` runs.

use std::path::{Path, PathBuf};

use deadwood::{Analysis, FindingKind, analyze};

/// Analyze the named fixture, asserting the run was complete: any warning
/// makes a detector skip, which would make the assertions below vacuous.
fn analyze_fixture(name: &str) -> Analysis {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let analysis =
        analyze(&fixture.join(name), None).expect("analysis should succeed on the fixture");
    assert!(
        analysis.warnings.is_empty(),
        "unexpected warnings: {:?}",
        analysis.warnings
    );
    analysis
}

/// Findings of one kind, as `(file, name)` pairs in report order.
fn reported(analysis: &Analysis, kind: FindingKind) -> Vec<(String, &str)> {
    analysis
        .findings
        .iter()
        .filter(|f| f.kind == kind)
        .map(|f| {
            (
                f.file.display().to_string(),
                f.name.as_deref().unwrap_or_default(),
            )
        })
        .collect()
}

#[test]
fn detects_dead_file_and_unused_pub_item() {
    let analysis = analyze_fixture("simple");

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
    let analysis = analyze_fixture("pathmod");

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

/// When a module file cannot be parsed, both detectors must skip instead of
/// reporting findings from incomplete data: the broken file could declare
/// `mod`s (so no dead-file findings) and could use any item (so no
/// unused-pub findings).
#[test]
fn detectors_skip_when_module_resolution_is_incomplete() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/broken");
    let analysis = analyze(&fixture, None).expect("analysis should succeed on the fixture");

    assert!(
        analysis.findings.is_empty(),
        "incomplete data must produce no findings: {:?}",
        analysis.findings
    );
    assert!(
        analysis
            .warnings
            .iter()
            .any(|w| w.contains("could not parse")),
        "the parse failure must be surfaced: {:?}",
        analysis.warnings
    );
    assert!(
        analysis
            .warnings
            .iter()
            .any(|w| w.contains("dead-file check skipped")),
        "the dead-file skip must be surfaced: {:?}",
        analysis.warnings
    );
    assert!(
        analysis
            .warnings
            .iter()
            .any(|w| w.contains("unused-pub check skipped")),
        "the unused-pub skip must be surfaced: {:?}",
        analysis.warnings
    );
    // A file that failed to parse could hold the only reference to a
    // dependency, so that check skips the package too.
    assert!(
        analysis
            .warnings
            .iter()
            .any(|w| w.contains("unused-dependency check skipped")),
        "the unused-dependency skip must be surfaced: {:?}",
        analysis.warnings
    );
}

/// A file included by several workspace members via `#[path]` is a module of
/// each crate, so its dead pub items are still reported — once, not per
/// including package.
#[test]
fn file_shared_between_packages_is_counted_once() {
    let analysis = analyze_fixture("shared");
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

/// The three classes of dead code the old identifier census could not see:
/// an item sharing a name with a used one, a type mentioned only by its own
/// `impl` block, and a `pub use` re-export nothing goes through. Renamed and
/// nested imports and `crate::`/`self::`/`super::` paths all resolve, so the
/// items they reach stay unreported.
#[test]
fn resolution_sees_through_names_impls_and_reexports() {
    let analysis = analyze_fixture("paths");

    assert_eq!(
        reported(&analysis, FindingKind::UnusedPubItem),
        vec![
            // Only mentioned by `impl ImplOnly`, including inside its body.
            ("src/alpha.rs".to_string(), "ImplOnly"),
            // `alpha::collision` is the one every path resolves to; a census
            // of the name "collision" hid this one behind it.
            ("src/beta.rs".to_string(), "collision"),
            // Only mentioned by `impl crate::qualified::Selfish`, whose body
            // spells the same type bare.
            ("src/qualified.rs".to_string(), "Selfish"),
            // Same, one module down; the bare `Wrapper` at that impl is a
            // different type, so it is not mistaken for a self-reference.
            ("src/qualified.rs".to_string(), "Wrapper"),
        ]
    );

    // The bare `Wrapper` in that impl body is `inner::Other`, renamed by a
    // `use`. Treating it as the impl's self-reference would drop the only
    // path that reaches `Other` and report a live item as dead.
    assert!(
        !analysis
            .findings
            .iter()
            .any(|f| f.name.as_deref() == Some("Other")),
        "a namesake in scope is not the impl's own type: {:?}",
        analysis.findings
    );
    assert_eq!(
        reported(&analysis, FindingKind::UnusedReexport),
        vec![
            ("src/surface.rs".to_string(), "Ignored"),
            ("src/surface.rs".to_string(), "Alias"),
        ]
    );

    // The re-exported definitions themselves are *not* reported as well: a
    // dead re-export is one finding, and removing it is what surfaces the
    // item underneath on the next run.
    assert!(
        !analysis
            .findings
            .iter()
            .any(|f| f.file == Path::new("src/surface/hidden.rs")),
        "re-export targets must not cascade into a second finding: {:?}",
        analysis.findings
    );

    // `surface` is private, so its dead re-exports are dead with certainty.
    // `facade` is `pub` from the crate root of a library, where an unused
    // re-export is the public-API idiom rather than a finding.
    assert!(
        !analysis
            .findings
            .iter()
            .any(|f| f.file.starts_with("src/facade")),
        "a re-export on the public surface must be left alone: {:?}",
        analysis.findings
    );
}

/// Anything Deadwood cannot resolve counts as a use: identifiers in macro
/// input, and names in a module whose scope has a glob import that does not
/// lead into the workspace. A glob that *does* is expanded instead of giving
/// up, so it hides nothing.
#[test]
fn unresolvable_paths_keep_their_targets_alive() {
    let analysis = analyze_fixture("macros");

    assert_eq!(
        reported(&analysis, FindingKind::UnusedPubItem),
        vec![("src/glob_source.rs".to_string(), "never_named")],
        "only the item behind a followed glob that nobody names is dead"
    );
}

/// Paths that cross workspace members resolve by crate name, with cargo's
/// dash-to-underscore normalization (`engine-core` is `engine_core` in a
/// path). Items the other member never reaches are still reported.
#[test]
fn paths_resolve_across_workspace_members() {
    let analysis = analyze_fixture("crosscrate");

    assert_eq!(
        reported(&analysis, FindingKind::UnusedPubItem),
        vec![
            ("engine-core/src/api.rs".to_string(), "Unused"),
            ("engine-core/src/lib.rs".to_string(), "never_started"),
        ],
        "`start` and `Handle` are reached from `app`, and `aliased_only` from \
         `aliased` through the `motor` dependency rename; none may be reported"
    );
}

/// Dependencies a package declares but never names are reported, and every
/// channel through which one *can* be named keeps its entry alive: plain
/// paths, a rename, a re-export, `extern crate`, macro bodies, attribute
/// paths and attribute strings, a doc example, a test target, a build script,
/// a file only a macro expansion declares, and the `[features]` table.
/// The entries Deadwood cannot judge are skipped out loud.
#[test]
fn unused_dependencies_are_reported_and_every_reference_channel_counts() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/deps");
    let analysis = analyze(&fixture, None).expect("analysis should succeed on the fixture");

    assert_eq!(
        reported(&analysis, FindingKind::UnusedDependency),
        vec![
            // `[dependencies]`: named nowhere in the package.
            ("Cargo.toml".to_string(), "unused_crate"),
            // `[dev-dependencies]`: the other one is used by `tests/it.rs`.
            ("Cargo.toml".to_string(), "unused_dev_crate"),
            // `[build-dependencies]`: the other one is used by `build.rs`.
            ("Cargo.toml".to_string(), "unused_build_crate"),
        ],
        "every other entry is named somewhere Deadwood has to look"
    );

    // Two channels with no reachable code behind them at all: a file that
    // only `automod::dir!` declares, and a `[features]` entry forwarding to a
    // dependency. Both were false positives before they were closed.
    for entry in ["regression_only_crate", "feature_only_crate"] {
        assert!(
            !analysis
                .findings
                .iter()
                .any(|f| f.name.as_deref() == Some(entry)),
            "`{entry}` is named where module resolution cannot see it: {:?}",
            analysis.findings
        );
    }

    // The rename is reported (and matched) by the manifest key, not by the
    // package name behind it.
    assert!(
        !analysis
            .findings
            .iter()
            .any(|f| f.name.as_deref() == Some("engine-widget")),
        "a renamed entry is judged by its alias: {:?}",
        analysis.findings
    );

    // Feature- and platform-gated entries are not judgeable until `cfg`
    // evaluation lands, and are skipped out loud rather than guessed at.
    for entry in ["optional_crate", "platform_crate"] {
        assert!(
            analysis
                .warnings
                .iter()
                .any(|w| w.contains(entry) && w.contains("unused-dependency check skipped")),
            "`{entry}` must be skipped with a warning: {:?}",
            analysis.warnings
        );
        assert!(
            !analysis
                .findings
                .iter()
                .any(|f| f.name.as_deref() == Some(entry)),
            "`{entry}` must not be reported: {:?}",
            analysis.findings
        );
    }

    // A skipped dependency entry says nothing about the other detectors, so
    // they must still run: the fixture's unused pub items are still reported.
    assert!(
        !reported(&analysis, FindingKind::UnusedPubItem).is_empty(),
        "dependency warnings must not silence the unused-pub check: {:?}",
        analysis.warnings
    );
}

/// Path dependencies between workspace members are dependencies like any
/// other: `app` uses `engine_core::`, `aliased` uses the `motor` rename, and
/// neither entry may be reported.
#[test]
fn dependencies_between_workspace_members_are_seen() {
    let analysis = analyze_fixture("crosscrate");
    assert_eq!(
        reported(&analysis, FindingKind::UnusedDependency),
        Vec::new(),
        "both members name the crate they depend on"
    );
}

/// A dependency renamed in `Cargo.toml` (`motor = { package = "engine-core" }`)
/// is spelled by its alias in code. The alias is derivable from neither the
/// package name nor the lib target name, so without reading it out of the
/// manifest the path resolves to nothing and everything it reaches looks dead.
#[test]
fn paths_through_a_dependency_rename_resolve() {
    let analysis = analyze_fixture("crosscrate");

    let unused = reported(&analysis, FindingKind::UnusedPubItem);
    assert!(
        !unused.iter().any(|(_, name)| *name == "aliased_only"),
        "`aliased_only` is called as `motor::aliased_only()`: {unused:?}"
    );
    assert!(
        !unused.iter().any(|(_, name)| *name == "Handle"),
        "`Handle` is named through the alias as `motor::api::Handle`: {unused:?}"
    );
}
