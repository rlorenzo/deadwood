//! The macros whose token streams declare modules.

macro_rules! wrapper {
    ($($item:item)*) => {
        $($item)*
    };
}

macro_rules! emit_mods {
    ($($m:ident),*) => {
        mod grouped {
            $(pub mod $m;)*
        }
    };
}

macro_rules! tree {
    () => {
        mod tucked;
    };
}
