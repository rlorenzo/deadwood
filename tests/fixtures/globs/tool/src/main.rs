//! A binary has no public surface, so a `pub use` glob in it puts nothing
//! anywhere and the cascade is untouched.

mod dead_end;
mod hidden;

pub use hidden::*;

fn main() {
    println!("{}", used_by_main());
}
