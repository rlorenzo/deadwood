//! A binary has no public surface, so neither a `pub use` glob in it nor a
//! named `pub use` of a module puts anything anywhere, and the cascade is
//! untouched by both.

mod dead_end;
mod hidden;
mod tucked;

pub use hidden::*;

/// Reported as an unused re-export, and it stays reported: the crate root of a
/// binary never enters the surface set, so the filter that excuses a `pub use`
/// on a library's surface has nothing to excuse this with.
pub use tucked::inner;

fn main() {
    println!("{}", used_by_main());
}
