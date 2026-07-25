mod alpha;
mod beta;
mod qualified;
mod surface;

// Reachable from outside the crate, unlike `surface` below.
pub mod facade;

// Renamed import: the item it points at is what counts as used.
use alpha::shared as alpha_shared;
// Nested use tree, mixing a plain name and a deeper path.
use beta::{Marker, inner::Deep};

// Nothing in this file is `pub`, so the fixture's findings all come from the
// modules below.
fn entry() -> u32 {
    alpha_shared() + Deep::VALUE
}

fn takes_marker<T: Marker>(_value: T) {}

fn exposed() -> surface::Exposed {
    surface::Exposed
}
