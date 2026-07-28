//! The attribute half of the split: a `#[test]` function is a test entry
//! point wherever it is written, including in an inline `#[cfg(test)] mod`
//! inside ordinary library code.
//!
//! And the half the attribute cannot answer for. An entry point that is
//! neither `#[test]` nor `#[bench]` — an `#[allow(dead_code)]`, an
//! `#[allow(unused)] use` — is test code when the `mod` it sits in is gated to
//! a test build, whichever of the two ways that gate is written. The pair
//! below is what the file exists for: `only_an_inline_gate` and
//! `only_an_outline_gate` are one construct spelled two ways, and they have to
//! come out of the report together or not at all.

/// Reported. `main` does not reach it, the `#[test]` below does, and dropping
/// the test entry points from the root set is exactly what makes it
/// unreachable. Note what the finding does *not* say: this function is
/// referenced and alive, so "delete it" would be wrong — `pub(crate)` is the
/// answer, or a move behind `#[cfg(test)]`.
pub fn only_tests() -> u32 {
    1
}

/// Reported, by the inline spelling of the gate alone: nothing in `gated` is
/// `#[test]`, so the attribute has no answer and the `mod`'s own
/// `#[cfg(test)]` is the only thing that says this is test code.
pub fn only_an_inline_gate() -> u32 {
    11
}

/// Reported, and by the out-of-line spelling of the very same shape: the gate
/// is on a `mod` declaration whose body is a file. Phase 7 made this half
/// work; the point of the pair is that the report cannot tell them apart.
pub fn only_an_outline_gate() -> u32 {
    17
}

/// Reported, one level further down: `deeper` carries no gate of its own and
/// is test code because of where it sits. Confinement accumulates downward and
/// never lifts.
pub fn only_a_nested_inline_gate() -> u32 {
    12
}

/// Reported, through the consumer of the rule that is easiest to forget: a
/// `use` is an entry point by the same attribute test, and the edge to what it
/// names is recorded from the import.
pub fn only_an_inline_gate_use() -> u32 {
    13
}

/// Not reported, and not because of anything the gate says: the kind is about
/// `pub` items, and this is already the fix a reported one is told to make. It
/// is here so that "reached only from a test-gated entry point" cannot on its
/// own be what produces a finding.
pub(crate) fn already_crate_private() -> u32 {
    14
}

/// Reported: `all(test, feature = "extra")` holds in no build without the
/// tests, so it confines its module exactly as a bare `test` does. Only a
/// predicate that reads the whole gate gets this one and `any` below both
/// right.
pub fn behind_an_all_gate() -> u32 {
    18
}

/// Not reported: `any(test, unix)` holds in a build with no tests in it, so
/// the `mod` is not confined to one and the entry point inside it is an
/// ordinary root. This is what `Gates::test_only` buys over matching
/// `#[cfg(test)]` by shape.
pub fn behind_an_any_gate() -> u32 {
    15
}

/// Not reported, for the opposite reason: `not(test)` is the gate that holds
/// only *outside* a test build.
pub fn behind_a_not_test_gate() -> u32 {
    16
}

/// Not reported, and this is the alternatives rule rather than a gate shape:
/// `alt` below is declared twice under disjoint gates, one confining it to a
/// test build and one — `all(not(test), unix)` — carrying a gate of its own
/// and compiled without the tests. The symbol table merges the two into one
/// module, so the path cannot be answered two ways, and the declaration that
/// is not confined clears it. A rule that cleared the path only for an
/// *ungated* alternative would call this module test code and report this
/// function.
pub fn behind_one_of_two_alternatives() -> u32 {
    19
}

#[cfg(test)]
mod tests {
    /// Written without `assert_eq!` on purpose. A name in macro input is a
    /// *root*, so an assertion naming `only_tests` would keep it out of the
    /// kind entirely — which is the point `opaque.rs` makes.
    #[test]
    fn covers_it() {
        if super::only_tests() != 1 {
            panic!("only_tests is broken");
        }
    }
}

/// The inline spelling. Nothing in here is `#[test]` or `#[bench]`, so the
/// gate on the `mod` is the only thing that can answer.
#[cfg(test)]
mod gated {
    /// The `use` path, through `add_use`, which is a second place the same
    /// answer has to arrive at. Reaching an import reaches what it names.
    #[allow(unused)]
    use super::only_an_inline_gate_use;

    /// An entry point by `#[allow(dead_code)]` and nothing else — the author
    /// saying the item is kept on purpose, which makes it a root.
    #[allow(dead_code)]
    fn kept() -> u32 {
        super::only_an_inline_gate() + super::already_crate_private()
    }

    /// Ungated, and test code all the same, because of the module it is in.
    mod deeper {
        #[allow(dead_code)]
        fn also_kept() -> u32 {
            crate::inline::only_a_nested_inline_gate()
        }
    }
}

/// The out-of-line spelling of `gated`, in `inline/outline.rs`, so that the
/// two are pinned to agree by a test that fails if either moves alone.
#[cfg(test)]
mod outline;

/// A gate that narrows a test build further is still a test build.
#[cfg(all(test, feature = "extra"))]
mod narrow {
    #[allow(dead_code)]
    fn kept() -> u32 {
        super::behind_an_all_gate()
    }
}

/// A gate that can hold without the tests roots what is under it normally.
#[cfg(any(test, unix))]
mod either_way {
    #[allow(dead_code)]
    fn kept() -> u32 {
        super::behind_an_any_gate()
    }
}

/// And the inverse gate is not touched at all.
#[cfg(not(test))]
mod never_in_tests {
    #[allow(dead_code)]
    fn kept() -> u32 {
        super::behind_a_not_test_gate()
    }
}

/// Two declarations of one module path under disjoint gates. This half is
/// confined to a test build.
#[cfg(test)]
mod alt {
    #[allow(dead_code)]
    fn kept() -> u32 {
        super::behind_one_of_two_alternatives()
    }
}

/// And this half is *gated* — it is not the ungated spelling — and compiled by
/// a build with no tests in it, so it clears the path for both.
#[cfg(all(not(test), unix))]
mod alt {
    #[allow(dead_code)]
    fn kept() -> u32 {
        super::behind_one_of_two_alternatives()
    }
}
