//! The path is assembled at build time, so nothing that does not build the
//! crate can say which file it names.

include!(concat!(env!("OUT_DIR"), "/generated.rs"));

pub mod live;
