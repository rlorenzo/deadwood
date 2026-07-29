//! A plain dead file, beside a tree that is spliced in and alive. It has
//! nothing to do with where an included file's children resolve; it is here
//! so that "not dead" cannot leak from the tree to its neighbours unnoticed.

pub fn forgotten() {}
