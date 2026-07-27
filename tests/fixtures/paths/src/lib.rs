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

// Every resolvable path in this fixture is written inside one of the three
// functions above, and a path written inside something nothing reaches is not
// evidence of anything. The unit tests are what reaches them, exactly as in a
// library whose internals have no other caller yet: a `#[test]` is a root
// under the default `cfg` matrix.
#[cfg(test)]
mod tests {
    use super::{Marker, entry, exposed, takes_marker};

    struct Marked;

    impl Marker for Marked {}

    #[test]
    fn the_drivers_above_are_reached() {
        assert_eq!(entry(), 5);
        takes_marker(Marked);
        let _ = exposed();
    }
}
