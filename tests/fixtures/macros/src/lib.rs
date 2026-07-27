mod exported;
mod glob_source;
mod opaque_scope;

// A glob into a module of this workspace is expanded: the names it brings
// into scope resolve to their definitions, and the ones nobody names stay
// reportable.
use glob_source::*;

macro_rules! wrap {
    () => {
        $crate::exported::only_in_macro()
    };
}

fn go() -> u32 {
    from_glob() + opaque_scope::run() + wrap!()
}

fn in_macro_arguments() {
    // The only mention of `only_in_macro` outside the macro definition above
    // is inside macro input, which Deadwood does not expand.
    println!("{}", only_in_macro as usize);
}

// `go` is where the resolvable paths in this fixture are written, and a path
// written inside something nothing reaches is not evidence of anything. A
// `#[test]` is a root under the default `cfg` matrix, which is how library
// code with no other caller yet stays alive.
#[cfg(test)]
mod tests {
    #[test]
    fn the_driver_above_is_reached() {
        let _ = super::go();
    }
}
