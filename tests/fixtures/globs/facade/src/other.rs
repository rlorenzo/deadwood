//! A private module nothing reaches, which is what makes every claim above a
//! claim about *reachability* rather than about reference counting.

use crate::imported::*;

/// Reported: nothing in the workspace names it, and no consumer can name
/// `facade::other` either.
pub fn helper() -> u32 {
    crate::inner::thing()
        + crate::inner::nested::deeper()
        + crate::inner::deep::buried()
        + from_import()
}
