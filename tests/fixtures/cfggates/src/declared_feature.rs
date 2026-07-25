pub fn from_declared_feature() {}

/// `all` with one arm that can never hold can never hold either — an
/// item-level gate rather than a `mod` one. Private, so the only finding it
/// produces is the one about the gate.
#[cfg(all(feature = "extra", feature = "vanished"))]
fn never_built() {}
