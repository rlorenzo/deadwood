//! Private, and every item in it is `probe::facade::*` to a consumer.

/// Not reported: nameable as `probe::facade::from_glob`.
pub fn from_glob() -> u32 {
    8
}

/// A glob re-export carries `pub` modules with it, not only functions, so
/// `nested` is `probe::facade::nested` and the rule has to descend as well as
/// follow the glob.
pub mod nested;
