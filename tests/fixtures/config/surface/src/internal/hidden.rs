/// Re-exported by the private `internal` module and reached by nothing. The
/// `pub use` names this on its own behalf, so a re-export nothing goes
/// through cannot keep its target alive: both are reported, because both have
/// to be deleted.
pub struct Buried;
