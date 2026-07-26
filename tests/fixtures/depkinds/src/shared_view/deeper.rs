//! A child of `shared_view.rs`, which inherits whatever that file is.
//!
//! Its own declaration carries no gate, so it is test code only if its parent
//! is — which is the case the ungated declaration has to clear on the way
//! down, not just for the file it names.

pub(crate) fn describe() {
    shared_view_child_crate::describe();
}
