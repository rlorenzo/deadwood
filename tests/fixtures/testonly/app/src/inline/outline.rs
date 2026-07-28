//! The out-of-line half of the pair. `#[cfg(test)] mod outline;` in
//! `inline.rs` is the same construct as the `#[cfg(test)] mod gated { ... }`
//! beside it, and phase 7 already made this spelling arrive here marked as
//! test code ([`ParsedFile::test_only`]). Phase 14 made the other one agree.

/// The same entry point as `gated::kept`, written in a file instead of a
/// block. `only_an_outline_gate` is reported for the same reason
/// `only_an_inline_gate` is.
#[allow(dead_code)]
fn kept() -> u32 {
    super::only_an_outline_gate()
}
