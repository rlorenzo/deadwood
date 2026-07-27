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

// The three functions above are where this fixture's `crate::`, `super::` and
// `self::` paths are written, so something has to reach them for those paths
// to count. A `#[test]` is a root under the default `cfg` matrix.
#[cfg(test)]
mod tests {
    #[test]
    fn the_qualified_paths_above_are_reached() {
        assert_eq!(super::through_crate_path(), 2);
        assert_eq!(super::through_super_path(), 1);
        let _ = super::through_self_path();
    }
}
