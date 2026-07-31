//! The same shape with the invocation at the top level of the file instead.
//! What the macro emits lands outside every inline block, so its `#[path]`
//! resolves from the *declaring file's* directory, `src/` — the second probe.
//! Written under `src/via_dir/` it would not compile.
wrapper! {
    #[path = "BesideDeclarer.rs"]
    pub mod beside;
}
