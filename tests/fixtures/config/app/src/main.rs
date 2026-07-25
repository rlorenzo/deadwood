//! A binary target: nothing outside the workspace can name its items, so a
//! `public-api` listing never applies to it.

fn main() {
    let _handle = surface::api::Handle;
}
