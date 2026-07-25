//! End-to-end tests: run the full analysis on the fixture packages under
//! `tests/fixtures/` and check the findings. Requires `cargo` on PATH (used
//! for `cargo metadata`), which is a given anywhere `cargo test` runs.

use std::path::{Path, PathBuf};

use deadwood::config::Severity;
use deadwood::{Analysis, FindingKind, analyze};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Analyze the named fixture, asserting the run was complete: any warning
/// makes a detector skip, which would make the assertions below vacuous.
fn analyze_fixture(name: &str) -> Analysis {
    let analysis =
        analyze(&fixtures().join(name), None).expect("analysis should succeed on the fixture");
    assert!(
        analysis.warnings.is_empty(),
        "unexpected warnings: {:?}",
        analysis.warnings
    );
    analysis
}

/// Analyze a fixture under one of the config files stored beside it.
fn analyze_configured(name: &str, config: &str) -> Analysis {
    let fixture = fixtures().join(name);
    let analysis = analyze(&fixture, Some(&fixture.join(config)))
        .unwrap_or_else(|err| panic!("`{config}` should load: {err:#}"));
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
    // dependency, and could hold it in code from a target no other file
    // covers, so both dependency checks skip the package too.
    for check in ["unused-dependency", "misplaced-dependency"] {
        assert!(
            analysis
                .warnings
                .iter()
                .any(|w| w.contains(&format!("{check} check skipped"))),
            "the {check} skip must be surfaced: {:?}",
            analysis.warnings
        );
    }
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

    // The same file is compiled by two packages with different feature
    // tables, so `#[cfg(feature = "wide")]` in it is impossible for the member
    // that does not declare `wide` and ordinary for the one that does. A gate
    // is only dead by construction when *no* package that compiles it can
    // satisfy it — otherwise every shared file would report against whichever
    // member happened to be judged first.
    assert!(
        reported(&analysis, FindingKind::UnsatisfiableCfg).is_empty(),
        "one member declares the feature, so the gate can hold: {:?}",
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
/// Feature- and platform-gated entries are judged like any other under the
/// default `cfg` matrix, which analyzes the code behind those gates.
#[test]
fn unused_dependencies_are_reported_and_every_reference_channel_counts() {
    let analysis = analyze_fixture("deps");

    assert_eq!(
        reported(&analysis, FindingKind::UnusedDependency),
        vec![
            // Optional, and named by nothing: judgeable since the default
            // matrix compiles every `#[cfg(feature = ...)]` branch.
            ("Cargo.toml".to_string(), "optional_crate"),
            // `[dependencies]`: named nowhere in the package.
            ("Cargo.toml".to_string(), "unused_crate"),
            // `[dev-dependencies]`: the other one is used by `tests/it.rs`.
            ("Cargo.toml".to_string(), "unused_dev_crate"),
            // `[build-dependencies]`: the other one is used by `build.rs`.
            ("Cargo.toml".to_string(), "unused_build_crate"),
            // `[target.'cfg(unix)'.dependencies]`, judged for the same reason.
            ("Cargo.toml".to_string(), "platform_crate"),
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

    // Gating an entry is not what keeps it alive; being named is. Both of
    // these are used only from behind the very `cfg` that gates them.
    for entry in ["gated_used_crate", "platform_used_crate"] {
        assert!(
            !analysis
                .findings
                .iter()
                .any(|f| f.name.as_deref() == Some(entry)),
            "`{entry}` is named behind its own gate, which the default matrix \
             compiles: {:?}",
            analysis.findings
        );
    }

    assert!(
        !reported(&analysis, FindingKind::UnusedPubItem).is_empty(),
        "the unused-pub check still runs alongside: {:?}",
        analysis.warnings
    );
}

/// `[target.'cfg(any())'.dependencies]` is the idiom for an entry that exists
/// to constrain version resolution and is deliberately compiled by no target
/// (serde pins `serde_derive` this way). No code can name it, and reporting it
/// would be a false positive, so it is skipped with a warning that says which
/// of the two it is.
#[test]
fn a_dependency_no_target_ever_builds_is_skipped_rather_than_reported() {
    let fixture = fixtures().join("cfgdeps");
    let analysis = analyze(&fixture, None).expect("analysis should succeed on the fixture");

    assert!(
        !analysis
            .findings
            .iter()
            .any(|f| f.name.as_deref() == Some("pinned_crate")),
        "a never-built entry is not a finding: {:?}",
        analysis.findings
    );
    assert!(
        analysis
            .warnings
            .iter()
            .any(|w| w.contains("`pinned_crate`") && w.contains("holds on no target at all")),
        "the skip must name the reason: {:?}",
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

// -- cfg evaluation --------------------------------------------------------
//
// The `cfggates` fixture gives every module one unreferenced `pub fn`, so
// "was this analyzed?" reads straight off the unused-pub findings: the item is
// there when the module is part of the analyzed build and gone when it is not.

/// Under the default matrix — every feature, every target, tests included —
/// every gate that can hold anywhere is followed, exactly as before this phase.
/// The one thing that is new is the finding about the gate that cannot.
#[test]
fn the_default_matrix_follows_every_gate_and_reports_the_ones_that_can_never_hold() {
    let analysis = analyze_fixture("cfggates");

    assert_eq!(
        reported(&analysis, FindingKind::UnsatisfiableCfg),
        vec![
            // An item-level gate, `all` with one arm no manifest can satisfy.
            ("src/declared_feature.rs".to_string(), "never_built"),
            // A file-level `#![cfg(...)]`, which gates every item below it.
            ("src/inner_impossible.rs".to_string(), ""),
            // A `mod` behind a feature that does not exist.
            ("src/lib.rs".to_string(), "missing_feature"),
        ],
        "only gates no build can satisfy are reported: {:?}",
        analysis.findings
    );
    let gate = analysis
        .findings
        .iter()
        .find(|f| {
            f.kind == FindingKind::UnsatisfiableCfg && f.name.as_deref() == Some("missing_feature")
        })
        .expect("the mod gate is reported");
    assert_eq!(gate.line, Some(15), "the line is the `#[cfg]` attribute's");
    assert!(
        gate.message
            .contains("`#[cfg(feature = \"gone\")]` can never hold")
            && gate.message.contains("declares no feature `gone`"),
        "the message must name the gate and the missing feature: {}",
        gate.message
    );

    assert_eq!(
        reported(&analysis, FindingKind::UnusedPubItem),
        vec![
            // Behind a feature the manifest declares: followed.
            (
                "src/declared_feature.rs".to_string(),
                "from_declared_feature"
            ),
            // Gated by an inner `#![cfg(...)]` rather than one on the `mod`.
            ("src/inner_gated.rs".to_string(), "from_inner_gate"),
            // Behind the impossible gates: reported *and* still followed, so
            // that adding a finding kind never moves the other detectors.
            (
                "src/inner_impossible.rs".to_string(),
                "from_inner_impossible",
            ),
            ("src/missing_feature.rs".to_string(), "from_missing_feature"),
            // Platform-gated, and every platform is possible by default.
            ("src/on_windows.rs".to_string(), "from_windows"),
            // `any(<unevaluable>, <impossible>)` is not impossible.
            (
                "src/partly_unevaluable.rs".to_string(),
                "from_partly_unevaluable"
            ),
            // A `cfg` we do not model at all.
            ("src/unevaluable.rs".to_string(), "from_unevaluable"),
        ],
        "every module was analyzed: {:?}",
        analysis.findings
    );
    assert!(
        reported(&analysis, FindingKind::DeadFile).is_empty(),
        "a gated file is never a dead file: {:?}",
        analysis.findings
    );
    assert!(
        !analysis
            .findings
            .iter()
            .any(|f| f.name.as_deref() == Some("used_by_tests_only")),
        "`#[cfg(test)]` code counts as a use by default: {:?}",
        analysis.findings
    );
}

/// Narrowing the targets takes the platform modules out of the analyzed build
/// — the one gated on its `mod` declaration and the one gated by an inner
/// `#![cfg(...)]` alike. The load-bearing half is the first assertion: code
/// that is not in this build must not turn into a dead file, or narrowing the
/// matrix would trade one false positive for another.
#[test]
fn a_narrowed_target_matrix_excludes_a_platform_module_without_calling_it_dead() {
    let analysis = analyze_configured("cfggates", "linux-only.toml");

    for excluded in ["src/on_windows.rs", "src/inner_gated.rs"] {
        assert!(
            !analysis
                .findings
                .iter()
                .any(|f| f.file.starts_with(excluded)),
            "`{excluded}` is neither analyzed nor reported: {:?}",
            analysis.findings
        );
    }
    // Everything else is untouched, including the gates we cannot evaluate.
    assert_eq!(
        reported(&analysis, FindingKind::UnusedPubItem),
        vec![
            (
                "src/declared_feature.rs".to_string(),
                "from_declared_feature"
            ),
            (
                "src/inner_impossible.rs".to_string(),
                "from_inner_impossible",
            ),
            ("src/missing_feature.rs".to_string(), "from_missing_feature"),
            (
                "src/partly_unevaluable.rs".to_string(),
                "from_partly_unevaluable"
            ),
            ("src/unevaluable.rs".to_string(), "from_unevaluable"),
        ]
    );
}

/// The `cfg(test)` decision, pinned from both sides. Test code counts as a use
/// by default — the quiet answer, and the one that keeps an absent config a
/// no-op — and `test = false` is how a project asks the other question.
#[test]
fn test_code_counts_as_a_use_until_the_matrix_says_otherwise() {
    let baseline = analyze_fixture("cfggates");
    assert!(
        !baseline
            .findings
            .iter()
            .any(|f| f.name.as_deref() == Some("used_by_tests_only")),
        "the default must be the quiet one: {:?}",
        baseline.findings
    );

    let analysis = analyze_configured("cfggates", "no-tests.toml");
    assert!(
        analysis
            .findings
            .iter()
            .any(|f| f.kind == FindingKind::UnusedPubItem
                && f.name.as_deref() == Some("used_by_tests_only")),
        "with the test build out of the matrix, a test-only helper is unused: {:?}",
        analysis.findings
    );
}

/// Narrowing the features leaves a declared-feature module out of the build,
/// and takes the optional dependencies nothing can enable with it — those go
/// back to being unjudgeable, out loud.
#[test]
fn a_narrowed_feature_matrix_excludes_gated_code_and_its_optional_dependencies() {
    let fixture = fixtures().join("cfggates");
    let analysis = analyze(&fixture, Some(&fixture.join("features-off.toml")))
        .expect("`features-off.toml` should load");

    assert!(
        !analysis
            .findings
            .iter()
            .any(|f| f.file.starts_with("src/declared_feature.rs")),
        "`extra` is off, so nothing in that module is in the build: {:?}",
        analysis.findings
    );
    assert!(
        reported(&analysis, FindingKind::UnusedDependency).is_empty(),
        "an optional entry no feature can enable is not judgeable: {:?}",
        analysis.findings
    );
    assert!(
        analysis
            .warnings
            .iter()
            .any(|w| w.contains("`optional_helper`") && w.contains("[cfg] features")),
        "the skip must name the matrix as the reason: {:?}",
        analysis.warnings
    );
    // The gate that can never hold is judged against every build there could
    // be, not against the configured one, so narrowing does not silence it.
    assert_eq!(
        reported(&analysis, FindingKind::UnsatisfiableCfg),
        vec![
            ("src/inner_impossible.rs".to_string(), ""),
            ("src/lib.rs".to_string(), "missing_feature")
        ]
    );
}

/// The new kind is a first-class one: `[severity]` reaches it by its serde tag
/// with no plumbing of its own, and it carries that tag into the JSON.
#[test]
fn the_new_finding_kind_is_configurable_and_typed() {
    let fixture = fixtures().join("cfggates");
    let analysis =
        analyze(&fixture, Some(&fixture.join("cfg-off.toml"))).expect("`cfg-off.toml` should load");
    assert!(
        reported(&analysis, FindingKind::UnsatisfiableCfg).is_empty(),
        "`unsatisfiable_cfg = \"off\"` removes the finding entirely: {:?}",
        analysis.findings
    );

    let reported = analyze_fixture("cfggates");
    let json = deadwood::report::render_json(&reported).expect("the report should render");
    assert!(
        json.contains("\"kind\": \"unsatisfiable_cfg\""),
        "the JSON carries the same tag the config file keys off:\n{json}"
    );
    let text = deadwood::report::render_text(&reported);
    assert!(
        text.contains("Unsatisfiable cfg gates:\n"),
        "and the text report gives it a group of its own:\n{text}"
    );
}

// -- configuration ---------------------------------------------------------
//
// The `config` fixture produces at least one finding of every kind with no
// configuration at all. Each test below applies one setting and asserts what
// it removed from that baseline, and — just as important — what it left.

/// Names of every finding, kind by kind, so a config's effect can be stated as
/// a difference from the unconfigured run.
fn all_reported(analysis: &Analysis) -> Vec<(FindingKind, String, String)> {
    analysis
        .findings
        .iter()
        .map(|f| {
            (
                f.kind,
                f.file.display().to_string(),
                f.name.clone().unwrap_or_default(),
            )
        })
        .collect()
}

/// The baseline the rest of this section is measured against, and the property
/// the whole feature is built around: with no config file, nothing changes.
/// Every finding is `deny`, exactly as before severity existed.
#[test]
fn a_workspace_with_no_config_file_behaves_exactly_as_before() {
    let fixture = fixtures().join("config");
    assert!(
        !fixture.join("deadwood.toml").exists(),
        "the baseline fixture must have no discoverable config"
    );

    let analysis = analyze_fixture("config");
    assert_eq!(
        all_reported(&analysis)
            .into_iter()
            .map(|(kind, file, name)| (kind, format!("{file}:{name}")))
            .collect::<Vec<_>>(),
        vec![
            (
                FindingKind::UnusedDependency,
                "app/Cargo.toml:stale_crate".into()
            ),
            (
                FindingKind::UnusedDependency,
                "app/Cargo.toml:vendored_native".into()
            ),
            (
                FindingKind::UnusedDependency,
                "surface/Cargo.toml:sidecar".into()
            ),
            (
                FindingKind::UnusedPubItem,
                "surface/src/api.rs:public_entry".into()
            ),
            (
                FindingKind::UnusedPubItem,
                "surface/src/generated.rs:generated_thing".into()
            ),
            (
                FindingKind::UnusedPubItem,
                "surface/src/generated.rs:call_helper".into()
            ),
            (
                FindingKind::UnusedReexport,
                "surface/src/internal.rs:Buried".into()
            ),
            (
                FindingKind::UnusedPubItem,
                "surface/src/internal.rs:internal_leftover".into()
            ),
            (
                FindingKind::UnusedPubItem,
                "surface/src/lib.rs:another_entry".into()
            ),
            (FindingKind::DeadFile, "surface/src/orphan.rs:".into()),
        ]
    );
    assert!(
        analysis
            .findings
            .iter()
            .all(|f| f.severity == Severity::Deny),
        "every kind defaults to deny: {:?}",
        analysis.findings
    );
    assert!(
        analysis.has_denied(),
        "an unconfigured run with findings fails"
    );
}

/// `ignore` removes findings *about* the matched files, and nothing else. The
/// second assertion is the load-bearing one: `generated.rs` holds the only
/// call to `api::helper`, so dropping its references along with its findings
/// would invent a false positive next door — turning every `ignore` entry into
/// a liability.
#[test]
fn ignore_patterns_suppress_findings_without_dropping_references() {
    let analysis = analyze_configured("config", "ignore.toml");

    assert!(
        !analysis
            .findings
            .iter()
            .any(|f| f.file.starts_with("surface/src/generated.rs")
                || f.file.starts_with("surface/src/orphan.rs")),
        "no finding may be reported about an ignored file: {:?}",
        analysis.findings
    );
    assert!(
        !analysis
            .findings
            .iter()
            .any(|f| f.name.as_deref() == Some("helper")),
        "an ignored file's references still count: {:?}",
        analysis.findings
    );
    // Everything outside the ignored files is untouched.
    assert_eq!(
        reported(&analysis, FindingKind::UnusedPubItem),
        vec![
            ("surface/src/api.rs".to_string(), "public_entry"),
            ("surface/src/internal.rs".to_string(), "internal_leftover"),
            ("surface/src/lib.rs".to_string(), "another_entry"),
        ]
    );
}

/// The one place `ignore` has to reach inside module resolution. A live file
/// declares `mod generated;` for a file a build step would have written; it is
/// not there. Unignored, that is an unresolved module, which skips every check
/// for the package — so an ignored file would silence the code around it.
///
/// This fixture also pins config *discovery*: its `deadwood.toml` is found by
/// walking up, not passed with `--config`.
#[test]
fn an_ignored_module_declaration_does_not_skip_the_package() {
    let discovered = analyze_fixture("ignoremod");
    assert!(
        discovered.warnings.is_empty(),
        "an ignored module is not an unresolved one: {:?}",
        discovered.warnings
    );
    assert_eq!(
        reported(&discovered, FindingKind::UnusedPubItem),
        vec![("src/lib.rs".to_string(), "leftover")],
        "the rest of the package must still be checked"
    );

    // The contrast, with a config that sets nothing: the declaration is an
    // unresolved module again, and every check for the package stands down.
    let fixture = fixtures().join("ignoremod");
    let unconfigured = analyze(&fixture, Some(&fixture.join("empty.toml"))).unwrap();
    assert!(
        unconfigured.findings.is_empty(),
        "incomplete resolution must report nothing: {:?}",
        unconfigured.findings
    );
    assert!(
        unconfigured
            .warnings
            .iter()
            .any(|w| w.contains("has no file at")),
        "the unresolved module must be surfaced: {:?}",
        unconfigured.warnings
    );
}

/// `warn` keeps every finding visible and stops it failing the run. That split
/// is the point: a project can adopt a check before it is clean.
#[test]
fn warn_severity_reports_everything_and_fails_nothing() {
    let baseline = analyze_fixture("config");
    let analysis = analyze_configured("config", "severity-warn.toml");

    assert_eq!(
        all_reported(&analysis),
        all_reported(&baseline),
        "`warn` changes no finding, only what it costs"
    );
    assert!(
        analysis
            .findings
            .iter()
            .all(|f| f.severity == Severity::Warn),
        "{:?}",
        analysis.findings
    );
    assert!(
        !analysis.has_denied(),
        "a run with only `warn` findings succeeds"
    );

    let text = deadwood::report::render_text(&analysis);
    assert!(text.contains("Dead files (warn):"), "{text}");
    assert!(text.contains("0 deny, 10 warn"), "{text}");
}

/// `off` is stronger than `warn`: the finding never exists, so it is absent
/// from the text, the JSON, and the count.
#[test]
fn off_severity_removes_findings_entirely() {
    let analysis = analyze_configured("config", "severity-off.toml");

    assert!(
        reported(&analysis, FindingKind::DeadFile).is_empty(),
        "{:?}",
        analysis.findings
    );
    assert!(
        reported(&analysis, FindingKind::UnusedDependency).is_empty(),
        "{:?}",
        analysis.findings
    );
    // Kinds the file does not mention keep the default.
    assert!(
        analysis
            .findings
            .iter()
            .all(|f| f.severity == Severity::Deny),
        "{:?}",
        analysis.findings
    );
    assert!(analysis.has_denied());

    let text = deadwood::report::render_text(&analysis);
    assert!(!text.contains("Dead files"), "{text}");
    assert!(
        !text.contains("deny,"),
        "with nothing downgraded the summary keeps its original shape:\n{text}"
    );
}

/// Severity is per kind, so a run can carry both at once — and one surviving
/// `deny` finding is enough to fail it.
#[test]
fn a_single_deny_finding_fails_a_run_full_of_warnings() {
    let analysis = analyze_configured("config", "severity-mixed.toml");

    let dead_file = analysis
        .findings
        .iter()
        .find(|f| f.kind == FindingKind::DeadFile)
        .expect("the dead file is downgraded, not removed");
    assert_eq!(dead_file.severity, Severity::Warn);
    assert!(
        reported(&analysis, FindingKind::UnusedPubItem).is_empty(),
        "{:?}",
        analysis.findings
    );
    assert!(
        analysis.has_denied(),
        "the re-export and dependency findings are still deny: {:?}",
        analysis.findings
    );
}

/// The noise lever the whole setting exists for: one line saying "this crate's
/// surface is the API" instead of an entry per item.
#[test]
fn a_public_api_crate_listing_silences_that_crates_surface() {
    let analysis = analyze_configured("config", "public-api-crate.toml");

    assert!(
        reported(&analysis, FindingKind::UnusedPubItem).is_empty(),
        "the listed crate's pub items are declared API: {:?}",
        analysis.findings
    );
    assert!(
        reported(&analysis, FindingKind::UnusedReexport).is_empty(),
        "a re-export is surface too: {:?}",
        analysis.findings
    );
    // The other detectors are untouched — this is not a blanket mute.
    assert_eq!(
        reported(&analysis, FindingKind::DeadFile),
        vec![("surface/src/orphan.rs".to_string(), "")]
    );
    assert_eq!(reported(&analysis, FindingKind::UnusedDependency).len(), 3);
}

/// An `items` glob covers what it names and no more, so a project can declare
/// one module its API without losing the check everywhere else.
#[test]
fn public_api_item_globs_cover_only_what_they_name() {
    let analysis = analyze_configured("config", "public-api-items.toml");

    assert_eq!(
        reported(&analysis, FindingKind::UnusedPubItem),
        vec![
            // `surface::api::*` covers `public_entry`, and nothing below.
            ("surface/src/generated.rs".to_string(), "generated_thing"),
            ("surface/src/generated.rs".to_string(), "call_helper"),
            ("surface/src/internal.rs".to_string(), "internal_leftover"),
            ("surface/src/lib.rs".to_string(), "another_entry"),
        ],
        "a single-segment `*` must not swallow other modules or the crate root"
    );
}

/// Closes #9: entries that are load bearing without being named by any code.
/// Workspace-wide and per-package lists both apply, and neither reaches an
/// entry it does not name.
#[test]
fn allowlisted_dependency_entries_are_never_judged() {
    let analysis = analyze_configured("config", "deps-allow.toml");

    assert_eq!(
        reported(&analysis, FindingKind::UnusedDependency),
        vec![("app/Cargo.toml".to_string(), "stale_crate")],
        "`sidecar` is allowed workspace-wide and `vendored_native` in `app`; \
         the unlisted entry in the same manifest stays reported"
    );
}

/// A configuration that does not do what it says is worse than none, so a
/// misspelling stops the run with a message naming the file, the key, and the
/// keys that do exist.
#[test]
fn a_malformed_config_fails_the_run_with_an_actionable_message() {
    let fixture = fixtures().join("config");
    let err = analyze(&fixture, Some(&fixture.join("broken.toml")))
        .expect_err("a config error must not be survivable");
    let message = format!("{err:#}");

    assert!(message.contains("broken.toml"), "{message}");
    assert!(message.contains("unknown field `ignor`"), "{message}");
    assert!(message.contains("`ignore`"), "{message}");
}

/// A `--config` naming a file that is not there is an error too: silently
/// falling back to the defaults would apply the wrong settings to a whole CI
/// run, and the run would pass while checking something else.
#[test]
fn a_missing_config_file_is_an_error_rather_than_a_silent_fallback() {
    let fixture = fixtures().join("config");
    let err = analyze(&fixture, Some(&fixture.join("no-such-file.toml")))
        .expect_err("a named config file must exist");
    assert!(
        format!("{err:#}").contains("could not read config file"),
        "{err:#}"
    );
}

/// The exit code is the whole interface for CI, so it is pinned on the binary
/// rather than inferred from the library: 0 clean or advisory, 1 denied,
/// 2 configuration error.
#[test]
fn exit_codes_follow_severity() {
    let run = |config: Option<&str>| {
        let fixture = fixtures().join("config");
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_deadwood"));
        command.arg("check").arg(&fixture);
        if let Some(config) = config {
            command.arg("--config").arg(fixture.join(config));
        }
        let output = command.output().expect("the binary should run");
        (
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
        )
    };

    let (code, stdout) = run(None);
    assert_eq!(code, Some(1), "unconfigured findings fail the run");
    assert!(stdout.contains("10 finding(s)"), "{stdout}");

    let (code, stdout) = run(Some("severity-warn.toml"));
    assert_eq!(code, Some(0), "advisory findings do not fail the run");
    assert!(
        stdout.contains("Unused public items (warn):"),
        "and are still printed, marked:\n{stdout}"
    );

    let (code, _) = run(Some("severity-mixed.toml"));
    assert_eq!(code, Some(1), "one surviving deny finding fails the run");

    let (code, stdout) = run(Some("broken.toml"));
    assert_eq!(code, Some(2), "a config error is an error, not a finding");
    assert!(stdout.is_empty(), "no report is produced: {stdout}");
}

/// The five cases that decide whether the misplaced-dependency check is worth
/// shipping, each pinned by one entry of the `depkinds` fixture. Two of them
/// are findings; the other three are the false positives a naive per-target
/// split would produce.
#[test]
fn dependencies_declared_in_the_wrong_table_are_reported() {
    let analysis = analyze_fixture("depkinds");

    let mut names: Vec<&str> = reported(&analysis, FindingKind::MisplacedDependency)
        .into_iter()
        .map(|(_, name)| name)
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        // A normal entry only `tests/it.rs` names, one only `examples/demo.rs`
        // names — both link `[dev-dependencies]` — and a build-dependency the
        // build script never touches.
        vec!["example_only_crate", "stale_build_crate", "test_only_crate"],
        "{:?}",
        analysis.findings
    );

    for entry in [
        // Used by the lib *and* the test: `[dependencies]` is where it belongs.
        "shared_crate",
        // A dev-dependency used by a `#[cfg(test)]` module inside the lib. The
        // module is part of the lib target and links dev-dependencies anyway,
        // and this is the largest false-positive source the check has.
        "cfg_test_crate",
        // A dev-dependency named only by a doc example. Doctests link
        // dev-dependencies, and a word in a doc comment names no target at all.
        "doc_only_crate",
        // A build-dependency the build script names.
        "build_crate",
    ] {
        assert!(
            !analysis
                .findings
                .iter()
                .any(|f| f.name.as_deref() == Some(entry)),
            "`{entry}` is declared in the table its references can see: {:?}",
            analysis.findings
        );
    }

    // The other dependency check answers a different question, and this
    // fixture gives it nothing to say: every entry is named by something.
    assert!(
        reported(&analysis, FindingKind::UnusedDependency).is_empty(),
        "no entry here is unreferenced: {:?}",
        analysis.findings
    );
}

/// Both halves of the finding have to be actionable: the table the entry sits
/// in and the table it belongs in, in text and in JSON, under a kind
/// `[severity]` reaches by its own serde tag.
#[test]
fn a_misplaced_dependency_names_both_tables_and_is_configurable() {
    let analysis = analyze_fixture("depkinds");

    let moved_down = analysis
        .findings
        .iter()
        .find(|f| f.name.as_deref() == Some("test_only_crate"))
        .expect("the test-only entry is reported");
    assert!(
        moved_down
            .message
            .contains("belongs in `[dev-dependencies]`")
            && moved_down.message.contains("rather than `[dependencies]`"),
        "{}",
        moved_down.message
    );
    assert_eq!(moved_down.file, PathBuf::from("Cargo.toml"));

    let stale_build = analysis
        .findings
        .iter()
        .find(|f| f.name.as_deref() == Some("stale_build_crate"))
        .expect("the stale build entry is reported");
    assert!(
        stale_build.message.contains("belongs in `[dependencies]`")
            && stale_build
                .message
                .contains("rather than `[build-dependencies]`"),
        "{}",
        stale_build.message
    );

    let text = deadwood::report::render_text(&analysis);
    assert!(text.contains("Misplaced dependencies:\n"), "{text}");

    let json: serde_json::Value =
        serde_json::from_str(&deadwood::report::render_json(&analysis).unwrap()).unwrap();
    assert!(
        json["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["kind"] == "misplaced_dependency" && f["name"] == "test_only_crate"),
        "{json}"
    );

    let silenced = analyze_configured("depkinds", "off.toml");
    assert!(
        reported(&silenced, FindingKind::MisplacedDependency).is_empty(),
        "`misplaced_dependency = \"off\"` removes the finding entirely: {:?}",
        silenced.findings
    );
}
