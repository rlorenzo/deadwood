// Each of these is named by a `pub use` in the parent module, which counts as
// a use of the definition: a dead re-export is reported as a re-export, not
// twice. Removing the re-export is what surfaces the item itself.
pub struct Exposed;

pub struct Ignored;

pub struct Renamed;
