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
