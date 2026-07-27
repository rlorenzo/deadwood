// Each of these is named by a `pub use` in the parent module — on the
// re-export's behalf, so the definition is alive exactly while the re-export
// is. `Exposed` is reached through its re-export and stays quiet; the other
// two are reported alongside theirs, because a dead re-export and the
// definition under it are two deletions in two places rather than one.
pub struct Exposed;

pub struct Ignored;

pub struct Renamed;
