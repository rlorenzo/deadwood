//! Fixture: which `cfg`-gated modules are followed, and which gate is dead.
//!
//! Every module below holds one unreferenced `pub fn`, so "was this analyzed?"
//! reads directly off the unused-pub findings: the item appears when the
//! module is part of the analyzed build and vanishes when it is not. None of
//! the files may ever be reported as a dead file.

// The manifest declares `extra`, so some build compiles this.
#[cfg(feature = "extra")]
mod declared_feature;

// Nothing declares `gone`, so no build ever compiles this — the finding. The
// module is still followed, because reporting a gate must not also move what
// every other detector sees.
#[cfg(feature = "gone")]
mod missing_feature;

// Followed under the default all-targets matrix; left out once the matrix
// says the build is Linux only.
#[cfg(windows)]
mod on_windows;

// Not a `cfg` we model, at any depth: always followed.
#[cfg(mystery_flag)]
mod unevaluable;

#[cfg(any(mystery_flag, feature = "gone"))]
mod partly_unevaluable;

/// Referenced only by the test module below, which is why it is not reported
/// under the default matrix and is under `test = false`.
pub fn used_by_tests_only() {}

#[cfg(test)]
mod tests {
    #[test]
    fn exercises_the_helper() {
        super::used_by_tests_only();
    }
}
