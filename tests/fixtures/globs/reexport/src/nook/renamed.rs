//! On the public surface because `lib.rs` writes `pub use nook::renamed as
//! api;` — the spelling with a rename, which changes the name a consumer
//! writes and nothing else.

/// Not reported: a consumer names it `reexport::api::only_dead_names_it_too`.
pub fn only_dead_names_it_too() -> u32 {
    3
}
