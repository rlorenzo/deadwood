//! A glob re-exports the module's `pub` modules as well as its functions, so
//! this is `facade::nested` to a consumer and the rule has to descend as well
//! as follow the glob.

/// Not reported: nameable as `facade::nested::deeper`.
pub fn deeper() -> u32 {
    3
}
