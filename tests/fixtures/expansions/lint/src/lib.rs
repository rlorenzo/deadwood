#[macro_export]
macro_rules! rule_module {
    ($module:ident) => {
        pub mod $module;
    };
}

#[macro_export]
macro_rules! rules {
    ($($module:ident),*) => {
        pub mod rules {
            $($crate::rule_module! { $module })*
        }
    };
}

mod logging {
    #[macro_export]
    macro_rules! warn_once {
        () => {{
            // A local, not a crate: it names no dependency.
            let cache = 1;
            $crate::paint_it!();
            cache
        }};
    }

    #[macro_export]
    macro_rules! paint_it {
        () => {
            paint::paint()
        };
    }
}
