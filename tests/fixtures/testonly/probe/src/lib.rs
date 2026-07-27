//! A library, where the same shape gets the opposite answer.

pub mod facade;
pub mod surface;

mod hidden;

#[cfg(test)]
mod tests {
    #[test]
    fn covers_them() {
        if crate::hidden::declared() + crate::hidden::undeclared() != 13 {
            panic!("hidden is broken");
        }
        if crate::surface::exported() != 5 {
            panic!("exported is broken");
        }
        // Through the re-export, which is the only spelling that compiles from
        // here: `inner` is private to `facade`.
        if crate::facade::from_glob() + crate::facade::nested::deeper() != 17 {
            panic!("the glob re-export is broken");
        }
    }
}
