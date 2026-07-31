// C++ calls the shim the attribute may have emitted; no Rust path names
// `entry`.
#[shim::host_fn]
pub fn entry() {
    helper();
}

// Named only inside `entry`, whose body the macro owns: opaque, so alive.
pub fn helper() {
    deeper();
}

pub fn deeper() {}

// A tool attribute rewrites nothing: still a finding.
#[rustfmt::skip]
pub fn inert_orphan() {}

// No attribute at all: still a finding.
pub fn plain_orphan() {}
