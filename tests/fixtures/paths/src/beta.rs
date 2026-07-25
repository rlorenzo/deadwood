pub mod inner;

/// Dead: `alpha::collision` is the one every path resolves to. A name census
/// counts the mentions of "collision" over there and stays quiet about this.
pub fn collision() -> u32 {
    3
}

/// Used as a trait bound in the crate root.
pub trait Marker {}

fn through_crate_path() -> u32 {
    crate::alpha::collision()
}

fn through_super_path() -> u32 {
    super::alpha::shared()
}

fn through_self_path() -> inner::Deep {
    self::inner::Deep
}
