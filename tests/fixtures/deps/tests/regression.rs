//! Fixture: the shape serde_json uses for its regression tests. `automod`
//! expands to one `mod` declaration per file in the directory, so nothing in
//! the module tree ever names `tests/regression/issue1.rs` — and the only use
//! of one dev-dependency lives there.

mod regression {
    automod::dir!("tests/regression");
}
