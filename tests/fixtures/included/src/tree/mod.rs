// Spliced into `src/lib.rs`, so this declares `crate::branch` and not
// `crate::tree::branch` — the module path is the *including* module's.
//
// The file it resolves to is `src/tree/branch.rs`, beside this file, and not
// `src/branch.rs` beside the includer. With only the latter on disk rustc
// says:
//
//   error[E0583]: file not found for module `branch`
//    --> src/tree/mod.rs:1:1
//     = help: to create the module `branch`, create file "src/tree/branch.rs"
//
// Both files exist here; the one this compiles against is the one that is not
// reported dead.
pub mod branch;

// `#[path]` inside a spliced file resolves from *this* file's directory too,
// so `../dual.rs` is `src/dual.rs` — the file `src/lib.rs` already declares.
#[path = "../dual.rs"]
pub mod dual_again;
