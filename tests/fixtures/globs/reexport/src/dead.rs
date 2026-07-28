//! A private module nothing reaches, which is what makes every claim in this
//! package a claim about *reachability* rather than about reference counting.
//! Without it the items above would be unreferenced, and unreferenced items are
//! reported however rooted they are.

/// Reported: nothing in the workspace names it, and no consumer can name
/// `reexport::dead` either. Every item above is judged against it.
pub fn unreached_referrer() -> u32 {
    crate::nook::plain::only_dead_names_it()
        + crate::nook::renamed::only_dead_names_it_too()
        + crate::chain::second::two_hops_from_the_root()
        + crate::locked::still_reported()
        + crate::item::beside_the_lifted_one()
}
