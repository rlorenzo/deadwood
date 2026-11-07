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

#[macro_export]
macro_rules! warn_once {
    () => {
        paint::paint()
    };
}
