//! The collision the phase is about, on the one finding kind that has it.
//!
//! A re-export is an `unused_reexport` finding and a definition is an
//! `unused_pub_item` one, and the kind is part of the match key — so those two
//! never share an entry however broad the namespace is. `test_only_item` is the
//! single kind both a re-export and a definition are reported under, so it is
//! the one place where the namespace is the whole of what tells them apart.

mod inner;

/// A *braced* struct is in the type namespace alone, so this alias is too. The
/// function below binds the same name in the value namespace, which is why the
/// two compile side by side — and why one baseline entry covering both was one
/// entry too few.
pub use inner::Braced;

/// The value half. Under the old rule the re-export above claimed `both`, which
/// overlaps `value`, so an entry recording the re-export suppressed this as
/// well and a third `Braced` added here later would have been suppressed before
/// it existed.
#[allow(non_snake_case)]
pub fn Braced() -> usize {
    1
}

/// A *unit* struct binds a constructor value of its own name, so this alias
/// really does bind both namespaces and keeps recording `both`. It is the shape
/// four of the corpus's five re-export findings have, so it is the one a
/// regression would hit first.
#[cfg(feature = "wide")]
pub use inner::Sole;

/// Which is why this is a `cfg` alternative and not a neighbour: two things
/// binding `Sole` in the value namespace do not compile (E0428). That also
/// makes the pair two spellings of one item, which is the case one entry is
/// right for — and the `both` above is what keeps it covered.
#[cfg(not(feature = "wide"))]
#[allow(non_snake_case)]
pub fn Sole() -> usize {
    0
}
