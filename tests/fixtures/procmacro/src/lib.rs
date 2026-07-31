use proc_macro::TokenStream;

// Consumers write `#[derive(Linked)]`, never a path to the function.
#[proc_macro_derive(Linked, attributes(linked))]
pub fn derive_linked(input: TokenStream) -> TokenStream {
    expand(input)
}

// Consumers write `#[host_fn]` on their own items.
#[proc_macro_attribute]
pub fn host_fn(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

// Consumers write `make_ident!(..)`.
#[proc_macro]
pub fn make_ident(input: TokenStream) -> TokenStream {
    input
}

// Reached only through `derive_linked`, so alive by the same fact.
fn expand(input: TokenStream) -> TokenStream {
    input
}

// Registered as nothing and named by nothing: the finding that stays.
pub fn orphan() -> u32 {
    2
}
