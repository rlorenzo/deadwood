//! No `mod` declaration names this file: a `dead_file` finding, and the kind
//! this phase deliberately does not help. It carries no name and no module, so
//! nothing survives a move for a matcher to compare — two unrelated dead files
//! are indistinguishable without a content signal Deadwood does not compute.

pub fn lonely() {}
