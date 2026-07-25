// A glob into a crate outside the workspace. Deadwood cannot enumerate what
// it brings into scope, so any name in this module that is not otherwise in
// scope might be a workspace item arriving through it.
use outside_crate::prelude::*;

pub fn run() -> u32 {
    // Whatever this is, Deadwood must assume it could be
    // `crate::exported::shadowed` and keep that alive.
    shadowed()
}
