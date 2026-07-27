//! A library, where the same shape gets the opposite answer.

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
    }
}
