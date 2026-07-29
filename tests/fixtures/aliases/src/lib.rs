//! What a `use` alias binds is what its target binds.
//!
//! Both modules below are private, so nothing in either is public surface and
//! every `pub` item in one is reportable — which is what makes the namespace
//! each alias records visible in the report at all.

mod hidden;
mod shared;

#[cfg(test)]
mod tests {
    /// The only code that names anything in `shared`, which is what makes its
    /// items `test_only_item` findings rather than dead ones. `hidden` is
    /// deliberately left unnamed: its aliases are dead, and a dead re-export
    /// prints the namespace it recorded just as well.
    ///
    /// Written without a single macro, deliberately. An identifier in macro
    /// input counts as a use of every workspace item of that name *and* roots
    /// it, so one `assert_eq!` naming `Braced` would put both halves of the
    /// collision out of every finding kind there is.
    #[test]
    fn names_both_halves_of_each_collision() {
        let braced = crate::shared::Braced {
            field: crate::shared::Braced(),
        };
        // Written this way because it has to resolve under either alternative:
        // with `wide` it names a unit struct's constructor, without it a
        // function. Either way it names both definitions Deadwood indexed.
        let sole = crate::shared::Sole;
        drop((braced, sole));
    }
}
