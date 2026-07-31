//! A non-mod-rs file, so `child_base` here is `src/via_base/` while the
//! declaring directory is `src/`. The two probe starting points
//! `queue_speculative` uses are a directory apart in this file, which is the
//! point of it.
//!
//! The invocation below sits inside an inline `mod`, so what the macro emits
//! lands inside `inner` and its `#[path]` resolves from
//! `src/via_base/inner/` — the `base` probe. Neither probe covers the other;
//! `via_dir.rs` is the same declaration written where the other one answers.
pub mod inner {
    wrapper! {
        #[path = "UnderBase.rs"]
        pub mod under_base;
    }
}
