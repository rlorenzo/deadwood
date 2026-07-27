//! Reachability from a binary's entry point.
//!
//! A binary has no consumers outside the workspace, so nothing here is a root
//! by being `pub`: every item is judged on whether the walk from `fn main`
//! gets to it. That makes this the crate where the cascade is visible in its
//! pure form.

mod cascade;
mod cycle;
mod live;
mod opaque;

fn main() {
    println!("{}", live::start());
}
