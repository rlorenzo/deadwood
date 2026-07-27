//! A binary, so nothing here is on a public surface: every item is judged on
//! which entry points reach it, and there are only two kinds of those — this
//! `fn main`, and the `#[test]` functions in the modules below.

mod inline;
mod opaque;
mod shared;

fn main() {
    println!("{}", shared::both());
}
