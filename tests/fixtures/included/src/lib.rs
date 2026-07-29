//! Almost the whole module tree is spliced in.
//!
//! `src/branch.rs` sits beside this file and is *not* what `pub mod branch;`
//! in `src/tree/mod.rs` resolves to — see that file.

/// The one file both routes reach: declared here as an ordinary module, and
/// again from inside the spliced file under another name. The `mod` walk
/// drains first, so it is analyzed rather than merely counted reachable —
/// which shows up as the one unused-public-item finding this crate has.
pub mod dual;

include!("tree/mod.rs");

/// An `include!` written inside an inline module. Its path is relative to
/// *this file's* directory, so it is `src/tree/twiglet.rs` and not
/// `src/inner/tree/twiglet.rs`; the items it splices in are `crate::inner`'s.
pub mod inner {
    include!("tree/twiglet.rs");
}

// Followed under the default matrix, which analyzes every target — so
// `src/winonly/mod.rs` is not a dead file on any platform. Under a matrix that
// rules `windows` out it is not followed, and the file falls back to the
// answer it gets today; `linux-only.toml` beside this crate is that matrix.
#[cfg(windows)]
include!("winonly/mod.rs");
